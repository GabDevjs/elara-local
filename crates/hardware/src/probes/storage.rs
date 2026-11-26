//! Sonda de armazenamento: discos, classe física e espaço livre real.
//!
//! O espaço livre não sai do `/proc`: sai de `statvfs(3)`, porque é a única
//! fonte que conhece as reservas do sistema de arquivos (os 5% do `ext4`, por
//! exemplo). Prometer ao usuário o espaço "livre" do `df` sem essa reserva é
//! prometer disco que ele não pode usar.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use super::{list_dir, path_under, read_file, read_trimmed, read_u64};
use crate::parse::mounts;
use crate::types::{MountPoint, ProbeWarning, StorageKind, StorageProfile};

/// Nome desta sonda nos avisos.
const SOURCE: &str = "storage";

/// Prefixos de dispositivos virtuais que não descrevem mídia física.
///
/// `loop` são imagens montadas (todo Snap cria uma), `zram` é RAM comprimida e
/// `dm-` é o mapeador de LVM/LUKS, que aponta para um disco já listado.
const VIRTUAL_PREFIXES: [&str; 4] = ["loop", "ram", "zram", "dm-"];

/// Tamanho do setor que o kernel usa em `/sys/block/<dev>/size`.
///
/// É sempre 512 bytes, independentemente do setor físico do dispositivo.
const SECTOR_BYTES: u64 = 512;

/// Enumera discos físicos e suas montagens.
pub fn probe_storage(root: &Path, warnings: &mut Vec<ProbeWarning>) -> Vec<StorageProfile> {
    let devices = list_dir(root, "/sys/block")
        .into_iter()
        .filter(|name| !is_virtual(name))
        .collect::<Vec<_>>();

    if devices.is_empty() {
        warnings.push(ProbeWarning::warning(
            SOURCE,
            "nenhum dispositivo de bloco físico em /sys/block",
        ));
        return Vec::new();
    }

    let mut profiles: Vec<StorageProfile> =
        devices.iter().map(|device| read_device(root, device)).collect();

    attach_mounts(root, &mut profiles, warnings);
    profiles
}

/// `true` para dispositivos virtuais que não devem entrar no perfil.
fn is_virtual(name: &str) -> bool {
    VIRTUAL_PREFIXES.iter().any(|prefix| name.starts_with(prefix))
}

/// Lê os atributos de um disco em `/sys/block/<dev>`.
fn read_device(root: &Path, device: &str) -> StorageProfile {
    let base = format!("/sys/block/{device}");
    let sectors = read_u64(root, &format!("{base}/size")).unwrap_or(0);
    let rotational = read_u64(root, &format!("{base}/queue/rotational")).map(|value| value == 1);

    StorageProfile {
        device: device.to_owned(),
        model: read_trimmed(root, &format!("{base}/device/model")),
        kind: classify(device, rotational),
        size_mb: sectors.saturating_mul(SECTOR_BYTES) / (1024 * 1024),
        rotational,
        mounts: Vec::new(),
        measured_read_mbps: None,
    }
}

/// Classifica o dispositivo pela interface e pelo atributo `rotational`.
///
/// `rotational` desconhecido não vira SSD por otimismo: vira [`StorageKind::Unknown`].
fn classify(device: &str, rotational: Option<bool>) -> StorageKind {
    if device.starts_with("nvme") {
        return StorageKind::Nvme;
    }
    match rotational {
        Some(true) => StorageKind::Hdd,
        Some(false) => StorageKind::Ssd,
        None => StorageKind::Unknown,
    }
}

/// Liga cada montagem de `/proc/mounts` ao disco a que pertence.
fn attach_mounts(root: &Path, profiles: &mut [StorageProfile], warnings: &mut Vec<ProbeWarning>) {
    let Ok(text) = read_file(root, "/proc/mounts") else {
        warnings.push(ProbeWarning::warning(
            SOURCE,
            "/proc/mounts ilegível; pontos de montagem e espaço livre desconhecidos",
        ));
        return;
    };

    for entry in mounts::parse_mounts(&text).into_iter().filter(mounts::MountEntry::is_block_device)
    {
        let Some(parent) = mounts::parent_block_device(&entry.source) else {
            warnings.push(ProbeWarning::info(
                SOURCE,
                format!("montagem {} não é atribuível a um disco físico", entry.source),
            ));
            continue;
        };
        let Some(profile) = profiles.iter_mut().find(|profile| profile.device == parent) else {
            continue;
        };

        let (total_mb, available_mb) =
            if let Some(space) = filesystem_space(&path_under(root, &entry.target)) {
                space
            } else {
                // Acontece com raiz de fixture (o caminho não existe sob ela) e
                // com montagens que sumiram entre a leitura e a chamada.
                warnings.push(ProbeWarning::info(
                    SOURCE,
                    format!("espaço livre de {} desconhecido: statvfs não respondeu", entry.target),
                ));
                (0, 0)
            };

        profile.mounts.push(MountPoint {
            path: entry.target.clone(),
            filesystem: entry.filesystem.clone(),
            total_mb,
            available_mb,
            read_only: entry.is_read_only(),
        });
    }
}

/// Espaço total e disponível de um sistema de arquivos, em MiB.
///
/// Usa `f_bavail` (blocos livres para usuários comuns), não `f_bfree`: a
/// diferença entre os dois é a reserva do superusuário, que o Elara não pode
/// gastar.
pub fn filesystem_space(path: &Path) -> Option<(u64, u64)> {
    let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut raw = std::mem::MaybeUninit::<libc::statvfs>::uninit();

    // SAFETY: `c_path` é um ponteiro válido terminado em NUL e vivo durante toda
    // a chamada; `raw` é uma alocação com tamanho e alinhamento de `statvfs`, e
    // só é lida depois de a syscall retornar 0, que é quando o kernel garante
    // ter preenchido a estrutura inteira.
    #[allow(
        unsafe_code,
        reason = "statvfs(3) é a única fonte do espaço livre real; a libc não expõe alternativa segura"
    )]
    let status = unsafe { libc::statvfs(c_path.as_ptr(), raw.as_mut_ptr()) };
    if status != 0 {
        return None;
    }
    // SAFETY: `statvfs` retornou 0, portanto a estrutura está inicializada.
    #[allow(unsafe_code, reason = "leitura de estrutura já inicializada pela syscall acima")]
    let stat = unsafe { raw.assume_init() };

    // Em Linux 64 bits todos estes campos são `u64`; a aritmética é direta.
    // `f_frsize` é o tamanho de bloco "de fragmento" e é o que multiplica os
    // contadores; alguns sistemas de arquivos deixam-no zerado e aí vale
    // `f_bsize`.
    let block = if stat.f_frsize > 0 { stat.f_frsize } else { stat.f_bsize };
    if block == 0 {
        return None;
    }

    Some((
        stat.f_blocks.saturating_mul(block) / (1024 * 1024),
        stat.f_bavail.saturating_mul(block) / (1024 * 1024),
    ))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let full = dir.path().join(path.trim_start_matches('/'));
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(full, content).unwrap();
        }
        dir
    }

    #[test]
    fn classifica_nvme_ssd_e_hdd() {
        let dir = tree(&[
            ("/sys/block/nvme0n1/size", "2000409264\n"),
            ("/sys/block/nvme0n1/queue/rotational", "0\n"),
            ("/sys/block/nvme0n1/device/model", "Samsung SSD 990 PRO 1TB\n"),
            ("/sys/block/sda/size", "468862128\n"),
            ("/sys/block/sda/queue/rotational", "0\n"),
            ("/sys/block/sda/device/model", "KINGSTON SA400S3\n"),
            ("/sys/block/sdb/size", "625142448\n"),
            ("/sys/block/sdb/queue/rotational", "1\n"),
            ("/sys/block/sdb/device/model", "ST9320325AS\n"),
            ("/proc/mounts", ""),
        ]);
        let mut warnings = Vec::new();
        let disks = probe_storage(dir.path(), &mut warnings);

        assert_eq!(disks.len(), 3);
        assert_eq!(disks[0].device, "nvme0n1");
        assert_eq!(disks[0].kind, StorageKind::Nvme);
        assert_eq!(disks[0].size_mb, 976_762);
        assert_eq!(disks[0].model.as_deref(), Some("Samsung SSD 990 PRO 1TB"));
        assert_eq!(disks[1].kind, StorageKind::Ssd);
        assert_eq!(disks[2].kind, StorageKind::Hdd);
    }

    #[test]
    fn dispositivos_virtuais_sao_ignorados() {
        let dir = tree(&[
            ("/sys/block/loop0/size", "1024\n"),
            ("/sys/block/zram0/size", "1024\n"),
            ("/sys/block/dm-0/size", "1024\n"),
            ("/sys/block/ram0/size", "1024\n"),
            ("/sys/block/sda/size", "1024\n"),
            ("/proc/mounts", ""),
        ]);
        let mut warnings = Vec::new();
        let disks = probe_storage(dir.path(), &mut warnings);
        assert_eq!(disks.len(), 1);
        assert_eq!(disks[0].device, "sda");
    }

    #[test]
    fn rotational_desconhecido_nao_vira_ssd() {
        let dir = tree(&[("/sys/block/sda/size", "1024\n"), ("/proc/mounts", "")]);
        let mut warnings = Vec::new();
        let disks = probe_storage(dir.path(), &mut warnings);
        assert_eq!(disks[0].kind, StorageKind::Unknown);
        assert_eq!(disks[0].rotational, None);
    }

    #[test]
    fn particoes_sao_associadas_ao_disco_pai() {
        let dir = tree(&[
            ("/sys/block/nvme0n1/size", "2000409264\n"),
            ("/sys/block/nvme0n1/queue/rotational", "0\n"),
            (
                "/proc/mounts",
                "/dev/nvme0n1p2 / ext4 rw,relatime 0 0\n\
                 /dev/nvme0n1p1 /boot/efi vfat ro,relatime 0 0\n\
                 tmpfs /run tmpfs rw 0 0\n",
            ),
        ]);
        let mut warnings = Vec::new();
        let disks = probe_storage(dir.path(), &mut warnings);

        assert_eq!(disks[0].mounts.len(), 2);
        assert_eq!(disks[0].mounts[0].path, "/");
        assert_eq!(disks[0].mounts[0].filesystem, "ext4");
        assert!(!disks[0].mounts[0].read_only);
        assert!(disks[0].mounts[1].read_only);
    }

    #[test]
    fn montagem_de_disco_desconhecido_e_descartada() {
        let dir = tree(&[
            ("/sys/block/sda/size", "1024\n"),
            ("/proc/mounts", "/dev/mapper/vg-root / ext4 rw 0 0\n"),
        ]);
        let mut warnings = Vec::new();
        let disks = probe_storage(dir.path(), &mut warnings);
        assert!(disks[0].mounts.is_empty());
        assert!(warnings.iter().any(|w| w.message.contains("não é atribuível")));
    }

    #[test]
    fn statvfs_responde_para_um_caminho_real() {
        let dir = tempfile::tempdir().unwrap();
        let (total, available) = filesystem_space(dir.path()).unwrap();
        assert!(total > 0, "o diretório temporário está em algum sistema de arquivos");
        assert!(available <= total);
    }

    #[test]
    fn statvfs_em_caminho_inexistente_e_none() {
        assert_eq!(filesystem_space(Path::new("/caminho/que/nao/existe/mesmo")), None);
    }
}
