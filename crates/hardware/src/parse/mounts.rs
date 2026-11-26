//! Leitura de `/proc/mounts` e correlação de partição com disco.

/// Uma linha do `/proc/mounts`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MountEntry {
    /// Origem: `/dev/sda3`, `tmpfs`, `cgroup2`…
    pub source: String,
    /// Onde está montado, já com os escapes resolvidos.
    pub target: String,
    /// Tipo do sistema de arquivos, ex.: `ext4`.
    pub filesystem: String,
    /// Opções separadas, na ordem original.
    pub options: Vec<String>,
}

impl MountEntry {
    /// `true` quando o kernel marcou a montagem como somente leitura.
    pub fn is_read_only(&self) -> bool {
        self.options.iter().any(|o| o == "ro")
    }

    /// `true` quando a origem é um dispositivo de bloco em `/dev`.
    ///
    /// Filtra `tmpfs`, `proc`, `overlay` e companhia, que não pertencem a
    /// nenhum disco físico e poluiriam o perfil de armazenamento.
    pub fn is_block_device(&self) -> bool {
        self.source.starts_with("/dev/")
    }
}

/// Interpreta o conteúdo de `/proc/mounts`.
///
/// Linhas com menos de quatro campos são descartadas em silêncio: o arquivo é
/// gerado pelo kernel e uma linha truncada só acontece se a leitura pegou o
/// arquivo no meio de uma alteração.
pub fn parse_mounts(text: &str) -> Vec<MountEntry> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_ascii_whitespace();
            let source = unescape(fields.next()?);
            let target = unescape(fields.next()?);
            let filesystem = fields.next()?.to_owned();
            let options = fields.next()?.split(',').map(str::to_owned).collect();
            Some(MountEntry { source, target, filesystem, options })
        })
        .collect()
}

/// Resolve os escapes octais que o kernel usa nos caminhos.
///
/// Um ponto de montagem com espaço vira `\040`; comparar o caminho cru com o
/// que o usuário vê falharia justamente nos diretórios com espaço.
pub fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut digits = String::with_capacity(3);
        for _ in 0..3 {
            match chars.peek() {
                Some(d) if d.is_digit(8) => {
                    digits.push(*d);
                    chars.next();
                }
                _ => break,
            }
        }
        match u8::from_str_radix(&digits, 8) {
            Ok(byte) if digits.len() == 3 => out.push(char::from(byte)),
            _ => {
                out.push('\\');
                out.push_str(&digits);
            }
        }
    }

    out
}

/// Nome do disco a que uma partição pertence, ex.: `/dev/nvme0n1p2` → `nvme0n1`.
///
/// Aceita tanto `/dev/sda3` quanto `sda3`. Devolve `None` para nomes com
/// subdiretório (`/dev/mapper/vg-root`), porque mapear LVM ou LUKS ao disco
/// físico exige caminhar o grafo de `holders` do sysfs — a sonda de
/// armazenamento trata esse caso separadamente, e adivinhar aqui produziria
/// associação errada.
pub fn parent_block_device(source: &str) -> Option<String> {
    let name = source.strip_prefix("/dev/").unwrap_or(source);
    if name.is_empty() || name.contains('/') {
        return None;
    }

    // Dispositivos cujo nome já termina em dígito separam a partição com `p`.
    if let Some((head, tail)) = name.rsplit_once('p') {
        let digits_only = !tail.is_empty() && tail.chars().all(|c| c.is_ascii_digit());
        let head_is_disk = head.ends_with(|c: char| c.is_ascii_digit());
        if digits_only && head_is_disk {
            return Some(head.to_owned());
        }
    }
    if name.starts_with("nvme") || name.starts_with("mmcblk") {
        return Some(name.to_owned());
    }

    let trimmed = name.trim_end_matches(|c: char| c.is_ascii_digit());
    if trimmed.is_empty() || trimmed.ends_with('-') {
        // `dm-0`, `md0`: o número faz parte do nome do próprio dispositivo.
        return Some(name.to_owned());
    }
    Some(trimmed.to_owned())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
sysfs /sys sysfs rw,nosuid,nodev,noexec,relatime 0 0
/dev/sda3 / ext4 rw,noatime,errors=remount-ro 0 0
/dev/nvme0n1p1 /boot/efi vfat ro,relatime,fmask=0077 0 0
/dev/sdb1 /mnt/disco\\040externo ext4 rw,relatime 0 0
tmpfs /run tmpfs rw,nosuid,nodev,size=3281572k 0 0
";

    #[test]
    fn le_campos_e_opcoes() {
        let mounts = parse_mounts(SAMPLE);
        assert_eq!(mounts.len(), 5);
        assert_eq!(mounts[1].source, "/dev/sda3");
        assert_eq!(mounts[1].target, "/");
        assert_eq!(mounts[1].filesystem, "ext4");
        assert!(!mounts[1].is_read_only());
        assert!(mounts[2].is_read_only());
    }

    #[test]
    fn resolve_escape_octal_no_caminho() {
        let mounts = parse_mounts(SAMPLE);
        assert_eq!(mounts[3].target, "/mnt/disco externo");
    }

    #[test]
    fn distingue_dispositivo_de_bloco() {
        let mounts = parse_mounts(SAMPLE);
        assert!(!mounts[0].is_block_device());
        assert!(mounts[1].is_block_device());
        assert!(!mounts[4].is_block_device());
    }

    #[test]
    fn linha_truncada_e_descartada() {
        assert!(parse_mounts("/dev/sda1 /\n").is_empty());
    }

    #[test]
    fn particao_para_disco_pai() {
        assert_eq!(parent_block_device("/dev/sda3").as_deref(), Some("sda"));
        assert_eq!(parent_block_device("/dev/sdb").as_deref(), Some("sdb"));
        assert_eq!(parent_block_device("/dev/nvme0n1p2").as_deref(), Some("nvme0n1"));
        assert_eq!(parent_block_device("/dev/nvme0n1").as_deref(), Some("nvme0n1"));
        assert_eq!(parent_block_device("/dev/mmcblk0p1").as_deref(), Some("mmcblk0"));
        assert_eq!(parent_block_device("/dev/vda1").as_deref(), Some("vda"));
        assert_eq!(parent_block_device("/dev/dm-0").as_deref(), Some("dm-0"));
    }

    #[test]
    fn aceita_nome_sem_prefixo_dev() {
        assert_eq!(parent_block_device("sda3").as_deref(), Some("sda"));
    }

    #[test]
    fn origem_que_nao_e_disco_simples() {
        assert_eq!(parent_block_device("/dev/mapper/vg-root"), None);
        assert_eq!(parent_block_device("/dev/"), None);
    }

    #[test]
    fn escape_de_barra_invertida_e_de_sequencia_incompleta() {
        assert_eq!(unescape("/mnt/a\\134b"), "/mnt/a\\b");
        assert_eq!(unescape("/mnt/a\\04b"), "/mnt/a\\04b");
    }
}
