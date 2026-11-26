//! Leitura de `/proc/cpuinfo` em x86 e ARM.
//!
//! O arquivo tem dois dialetos incompatíveis: em x86 cada bloco traz
//! `model name`, `physical id`, `core id` e `flags`; em ARM traz `Features`,
//! `CPU implementer` e `CPU part`, e o nome do produto costuma aparecer em um
//! bloco final sem `processor` (`Hardware`, `Model`). Os dois são tratados
//! aqui para que as sondas não precisem saber em qual máquina estão.

use std::collections::BTreeSet;

use super::{ParseError, split_key_value};
use crate::types::{CpuFeatures, CpuVendor};

/// Um bloco `processor:` do `/proc/cpuinfo`, ou seja, uma CPU lógica online.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CpuinfoProcessor {
    /// Índice lógico como o kernel o numera.
    pub id: u32,
    /// `model name` em x86; ausente na maioria dos ARM.
    pub model_name: Option<String>,
    /// `vendor_id` em x86, ex.: `GenuineIntel`.
    pub vendor_id: Option<String>,
    /// `physical id`: identifica o socket.
    pub physical_id: Option<u32>,
    /// `core id`: identifica o núcleo físico dentro do socket.
    pub core_id: Option<u32>,
    /// `cpu cores`: núcleos físicos declarados para este socket.
    pub cpu_cores: Option<u32>,
    /// `siblings`: threads por socket.
    pub siblings: Option<u32>,
    /// `cpu MHz`: frequência **instantânea**, não a base — usada só como
    /// último recurso.
    pub mhz: Option<f64>,
    /// `cache size` em KiB. O nível a que se refere varia por fabricante, por
    /// isso não vira automaticamente L2 nem L3.
    pub cache_size_kb: Option<u32>,
    /// `flags` (x86) ou `Features` (ARM), na ordem original.
    pub flags: Vec<String>,
    /// `CPU implementer` do ARM, ex.: `0x41` para a própria ARM Ltd.
    pub implementer: Option<u32>,
    /// `CPU part` do ARM, ex.: `0xd08` para Cortex-A72.
    pub part: Option<u32>,
    /// `CPU architecture` do ARM, ex.: `8`.
    pub arm_architecture: Option<u32>,
}

impl CpuinfoProcessor {
    /// `true` se a CPU anuncia a flag (comparação exata, minúscula).
    pub fn has_flag(&self, flag: &str) -> bool {
        self.flags.iter().any(|f| f == flag)
    }
}

/// Conteúdo completo do `/proc/cpuinfo`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Cpuinfo {
    /// Um item por CPU lógica **online** — o kernel não lista as offline.
    pub processors: Vec<CpuinfoProcessor>,
    /// Campo `Hardware` das plataformas ARM, ex.: `BCM2711`.
    pub hardware: Option<String>,
    /// Campo `Model` das plataformas ARM, ex.: `Raspberry Pi 4 Model B`.
    pub model: Option<String>,
}

/// Interpreta o conteúdo de `/proc/cpuinfo`.
///
/// Falha apenas quando não há nenhum bloco `processor`: sem isso não dá para
/// afirmar nada sobre a CPU, e chutar seria pior que erro.
pub fn parse_cpuinfo(text: &str) -> Result<Cpuinfo, ParseError> {
    let mut info = Cpuinfo::default();
    let mut current: Option<CpuinfoProcessor> = None;

    for line in text.lines() {
        let Some((key, value)) = split_key_value(line, ':') else {
            // Linha em branco separa blocos; qualquer outra coisa é ruído.
            if line.trim().is_empty()
                && let Some(done) = current.take()
            {
                info.processors.push(done);
            }
            continue;
        };

        if key.eq_ignore_ascii_case("processor") {
            if let Some(done) = current.take() {
                info.processors.push(done);
            }
            let id = value.parse::<u32>().map_err(|_| ParseError::invalid("processor", value))?;
            current = Some(CpuinfoProcessor { id, ..CpuinfoProcessor::default() });
            continue;
        }

        match current.as_mut() {
            Some(processor) => apply_processor_field(processor, key, value),
            None => apply_global_field(&mut info, key, value),
        }
    }

    if let Some(done) = current.take() {
        info.processors.push(done);
    }

    if info.processors.is_empty() { Err(ParseError::Empty) } else { Ok(info) }
}

/// Campos que pertencem a uma CPU específica.
fn apply_processor_field(processor: &mut CpuinfoProcessor, key: &str, value: &str) {
    match key {
        "model name" => processor.model_name = Some(value.to_owned()),
        "vendor_id" => processor.vendor_id = Some(value.to_owned()),
        "physical id" => processor.physical_id = value.parse().ok(),
        "core id" => processor.core_id = value.parse().ok(),
        "cpu cores" => processor.cpu_cores = value.parse().ok(),
        "siblings" => processor.siblings = value.parse().ok(),
        "cpu MHz" => processor.mhz = value.parse().ok(),
        "cache size" => processor.cache_size_kb = super::size::parse_cache_size_kib(value).ok(),
        "flags" | "Features" => {
            processor.flags = value.split_ascii_whitespace().map(str::to_owned).collect();
        }
        "CPU implementer" => processor.implementer = parse_hex(value),
        "CPU part" => processor.part = parse_hex(value),
        "CPU architecture" => processor.arm_architecture = value.parse().ok(),
        _ => {}
    }
}

/// Campos que aparecem fora de qualquer bloco `processor` (típico de ARM).
fn apply_global_field(info: &mut Cpuinfo, key: &str, value: &str) {
    match key {
        "Hardware" => info.hardware = Some(value.to_owned()),
        "Model" => info.model = Some(value.to_owned()),
        _ => {}
    }
}

/// Aceita `0x41` e `41` — o kernel usa a primeira forma, mas emuladores
/// aparecem com a segunda.
fn parse_hex(value: &str) -> Option<u32> {
    let digits = value.strip_prefix("0x").or_else(|| value.strip_prefix("0X")).unwrap_or(value);
    u32::from_str_radix(digits, 16).ok()
}

impl Cpuinfo {
    /// Núcleos físicos distintos, contados por par `(physical id, core id)`.
    ///
    /// `None` quando o kernel não expõe esses campos (comum em ARM e em VMs),
    /// caso em que a sonda cai para a topologia do sysfs em vez de assumir que
    /// threads são núcleos.
    pub fn physical_core_count(&self) -> Option<u32> {
        let pairs: BTreeSet<(u32, u32)> =
            self.processors.iter().filter_map(|p| Some((p.physical_id?, p.core_id?))).collect();
        u32::try_from(pairs.len()).ok().filter(|count| *count > 0)
    }

    /// Sockets distintos, pelo campo `physical id`.
    pub fn socket_count(&self) -> Option<u32> {
        let sockets: BTreeSet<u32> = self.processors.iter().filter_map(|p| p.physical_id).collect();
        u32::try_from(sockets.len()).ok().filter(|count| *count > 0)
    }

    /// Nome comercial da CPU, na melhor fonte disponível.
    ///
    /// Ordem: `model name` (x86) → `Model`/`Hardware` (placas ARM) → decodificação
    /// de `CPU implementer`/`CPU part`. Devolve `None` quando nem isso existe,
    /// para que a sonda registre um aviso em vez de gravar um nome inventado.
    pub fn model_label(&self) -> Option<String> {
        if let Some(name) = self.processors.iter().find_map(|p| p.model_name.clone()) {
            return Some(name);
        }
        if let Some(model) = self.model.clone().or_else(|| self.hardware.clone()) {
            return Some(model);
        }
        let first = self.processors.first()?;
        arm_part_label(first.implementer?, first.part?)
    }

    /// Fabricante da CPU a partir de `vendor_id` (x86) ou `CPU implementer` (ARM).
    pub fn vendor(&self) -> CpuVendor {
        let vendor_id = self.processors.iter().find_map(|p| p.vendor_id.as_deref());
        if let Some(id) = vendor_id {
            return match id {
                "GenuineIntel" => CpuVendor::Intel,
                "AuthenticAMD" | "AMDisbetter!" => CpuVendor::Amd,
                _ => CpuVendor::Other,
            };
        }
        match self.processors.iter().find_map(|p| p.implementer) {
            Some(0x41 | 0x42 | 0x43 | 0x4e | 0x51 | 0xc0) => CpuVendor::Arm,
            Some(0x61) => CpuVendor::Apple,
            Some(_) | None => CpuVendor::Other,
        }
    }

    /// Extensões de instrução relevantes para inferência, agregadas.
    ///
    /// Usa a primeira CPU listada: em máquinas híbridas o kernel só expõe em
    /// `/proc/cpuinfo` flags comuns a todos os núcleos, então não há ganho em
    /// unir os conjuntos.
    pub fn features(&self) -> CpuFeatures {
        self.processors.first().map(|p| features_from_flags(&p.flags)).unwrap_or_default()
    }

    /// Arquitetura inferida do próprio conteúdo do arquivo.
    ///
    /// Existe porque `uname(2)` ignora a raiz injetada nos testes: um
    /// `/proc/cpuinfo` de fixture ARM lido em uma máquina x86 precisa continuar
    /// dizendo `aarch64`.
    pub fn architecture(&self) -> Option<&'static str> {
        let first = self.processors.first()?;
        if first.vendor_id.is_some() || first.has_flag("fpu") {
            return Some(if first.has_flag("lm") { "x86_64" } else { "i686" });
        }
        match first.arm_architecture {
            Some(8..) => Some("aarch64"),
            Some(7) => Some("armv7l"),
            _ => first.implementer.map(|_| "aarch64"),
        }
    }

    /// Frequência base declarada no nome do modelo, ex.: `@ 2.70GHz` → `2700`.
    ///
    /// É a única fonte de frequência **base** em CPUs sem `base_frequency` no
    /// sysfs; `cpu MHz` não serve porque é a frequência do instante da leitura.
    pub fn base_frequency_mhz(&self) -> Option<u32> {
        let model = self.processors.iter().find_map(|p| p.model_name.as_deref())?;
        base_frequency_from_model(model)
    }
}

/// Traduz o conjunto de flags do kernel para [`CpuFeatures`].
///
/// Aceita os dois vocabulários: `pni`/`asimd` são os nomes que o Linux usa para
/// SSE3 e NEON, e ignorar isso faria uma máquina perfeitamente capaz parecer
/// sem suporte.
pub fn features_from_flags(flags: &[String]) -> CpuFeatures {
    let set: BTreeSet<&str> = flags.iter().map(String::as_str).collect();
    let has = |name: &str| set.contains(name);

    CpuFeatures {
        sse3: has("pni") || has("sse3"),
        ssse3: has("ssse3"),
        sse4_2: has("sse4_2"),
        avx: has("avx"),
        avx2: has("avx2"),
        fma: has("fma"),
        f16c: has("f16c"),
        avx512f: has("avx512f"),
        avx512_vnni: has("avx512_vnni"),
        avx512_bf16: has("avx512_bf16"),
        amx_int8: has("amx_int8"),
        amx_bf16: has("amx_bf16"),
        neon: has("neon") || has("asimd"),
        dotprod: has("asimddp") || has("dotprod"),
        i8mm: has("i8mm"),
        sve: has("sve"),
    }
}

/// Extrai a frequência base de um `model name` no formato `... @ 2.70GHz`.
pub fn base_frequency_from_model(model: &str) -> Option<u32> {
    let (_, tail) = model.rsplit_once('@')?;
    let tail = tail.trim();
    let (number, unit) = tail.split_at(tail.find(|c: char| c.is_ascii_alphabetic())?);
    let value: f64 = number.trim().parse().ok()?;
    let mhz = match unit.trim().to_ascii_lowercase().as_str() {
        "ghz" => value * 1000.0,
        "mhz" => value,
        _ => return None,
    };
    if mhz.is_finite() && (1.0..=100_000.0).contains(&mhz) {
        // Truncamento aceitável: a granularidade útil aqui é 1 MHz.
        #[allow(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "faixa validada logo acima, entre 1 e 100000"
        )]
        Some(mhz.round() as u32)
    } else {
        None
    }
}

/// Nome legível de um núcleo ARM a partir de `implementer`/`part`.
///
/// A tabela cobre os núcleos que aparecem em máquinas onde faz sentido rodar
/// inferência local. Fora dela devolvemos os códigos crus — identificar errado
/// é pior do que mostrar o número que o kernel deu.
pub fn arm_part_label(implementer: u32, part: u32) -> Option<String> {
    let vendor = match implementer {
        0x41 => "ARM",
        0x42 => "Broadcom",
        0x43 => "Cavium",
        0x4e => "NVIDIA",
        0x51 => "Qualcomm",
        0x61 => "Apple",
        0xc0 => "Ampere",
        _ => return Some(format!("ARM implementer {implementer:#04x} part {part:#05x}")),
    };

    let core = match (implementer, part) {
        (0x41, 0xd03) => "Cortex-A53",
        (0x41, 0xd04) => "Cortex-A35",
        (0x41, 0xd05) => "Cortex-A55",
        (0x41, 0xd07) => "Cortex-A57",
        (0x41, 0xd08) => "Cortex-A72",
        (0x41, 0xd09) => "Cortex-A73",
        (0x41, 0xd0a) => "Cortex-A75",
        (0x41, 0xd0b) => "Cortex-A76",
        (0x41, 0xd0c) => "Neoverse-N1",
        (0x41, 0xd0d) => "Cortex-A77",
        (0x41, 0xd40) => "Neoverse-V1",
        (0x41, 0xd41) => "Cortex-A78",
        (0x41, 0xd44) => "Cortex-X1",
        (0x41, 0xd49) => "Neoverse-N2",
        _ => return Some(format!("{vendor} part {part:#05x}")),
    };
    Some(format!("{vendor} {core}"))
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]
mod tests {
    use super::*;

    const X86: &str = "\
processor\t: 0
vendor_id\t: GenuineIntel
cpu family\t: 6
model\t\t: 94
model name\t: Intel(R) Core(TM) i5-6400 CPU @ 2.70GHz
stepping\t: 3
cpu MHz\t\t: 3167.833
cache size\t: 6144 KB
physical id\t: 0
siblings\t: 4
core id\t\t: 0
cpu cores\t: 4
flags\t\t: fpu vme de pse tsc msr lm pni ssse3 sse4_2 avx f16c fma avx2
bugs\t\t: cpu_meltdown spectre_v1

processor\t: 1
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i5-6400 CPU @ 2.70GHz
physical id\t: 0
siblings\t: 4
core id\t\t: 1
cpu cores\t: 4
flags\t\t: fpu vme de pse tsc msr lm pni ssse3 sse4_2 avx f16c fma avx2
";

    const ARM: &str = "\
processor\t: 0
BogoMIPS\t: 108.00
Features\t: fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics fphp asimdhp asimddp i8mm sve
CPU implementer\t: 0x41
CPU architecture: 8
CPU variant\t: 0x3
CPU part\t: 0xd0b
CPU revision\t: 1

processor\t: 1
BogoMIPS\t: 108.00
Features\t: fp asimd evtstrm aes pmull sha1 sha2 crc32 atomics fphp asimdhp asimddp i8mm sve
CPU implementer\t: 0x41
CPU architecture: 8
CPU variant\t: 0x3
CPU part\t: 0xd0b
CPU revision\t: 1

Hardware\t: BCM2711
Revision\t: c03111
Model\t\t: Raspberry Pi 4 Model B Rev 1.1
";

    #[test]
    fn le_blocos_x86() {
        let info = parse_cpuinfo(X86).unwrap();
        assert_eq!(info.processors.len(), 2);
        assert_eq!(info.processors[0].id, 0);
        assert_eq!(
            info.processors[0].model_name.as_deref(),
            Some("Intel(R) Core(TM) i5-6400 CPU @ 2.70GHz")
        );
        assert_eq!(info.processors[0].cache_size_kb, Some(6144));
        assert_eq!(info.processors[1].core_id, Some(1));
        assert_eq!(info.vendor(), CpuVendor::Intel);
        assert_eq!(info.architecture(), Some("x86_64"));
        assert_eq!(info.base_frequency_mhz(), Some(2700));
    }

    #[test]
    fn conta_nucleos_e_sockets_x86() {
        let info = parse_cpuinfo(X86).unwrap();
        assert_eq!(info.physical_core_count(), Some(2));
        assert_eq!(info.socket_count(), Some(1));
    }

    #[test]
    fn nucleos_repetidos_sao_deduplicados() {
        let hyperthreaded = X86.replace("core id\t\t: 1", "core id\t\t: 0");
        let info = parse_cpuinfo(&hyperthreaded).unwrap();
        assert_eq!(info.processors.len(), 2);
        assert_eq!(info.physical_core_count(), Some(1));
    }

    #[test]
    fn le_blocos_arm_e_bloco_global() {
        let info = parse_cpuinfo(ARM).unwrap();
        assert_eq!(info.processors.len(), 2);
        assert_eq!(info.hardware.as_deref(), Some("BCM2711"));
        assert_eq!(info.model.as_deref(), Some("Raspberry Pi 4 Model B Rev 1.1"));
        assert_eq!(info.model_label().as_deref(), Some("Raspberry Pi 4 Model B Rev 1.1"));
        assert_eq!(info.vendor(), CpuVendor::Arm);
        assert_eq!(info.architecture(), Some("aarch64"));
        assert_eq!(info.processors[0].implementer, Some(0x41));
        assert_eq!(info.processors[0].part, Some(0xd0b));
    }

    #[test]
    fn arm_sem_topologia_nao_finge_saber_nucleos() {
        let info = parse_cpuinfo(ARM).unwrap();
        assert_eq!(info.physical_core_count(), None);
        assert_eq!(info.socket_count(), None);
    }

    #[test]
    fn flags_x86_viram_features() {
        let features = parse_cpuinfo(X86).unwrap().features();
        assert!(features.avx2 && features.fma && features.f16c && features.sse3);
        assert!(!features.avx512f);
        assert!(!features.neon);
    }

    #[test]
    fn features_arm_reconhecem_asimd() {
        let features = parse_cpuinfo(ARM).unwrap().features();
        assert!(features.neon, "asimd é o nome de NEON em aarch64");
        assert!(features.dotprod && features.i8mm && features.sve);
        assert!(!features.avx2);
    }

    #[test]
    fn cpuinfo_vazio_e_erro() {
        assert_eq!(parse_cpuinfo("").unwrap_err(), ParseError::Empty);
        assert_eq!(parse_cpuinfo("lixo sem dois pontos\n").unwrap_err(), ParseError::Empty);
    }

    #[test]
    fn processador_com_id_invalido_e_erro() {
        let err = parse_cpuinfo("processor\t: x\n").unwrap_err();
        assert_eq!(err, ParseError::invalid("processor", "x"));
    }

    #[test]
    fn frequencia_base_apenas_quando_declarada() {
        assert_eq!(base_frequency_from_model("Intel(R) Xeon(R) @ 3.00GHz"), Some(3000));
        assert_eq!(base_frequency_from_model("Some CPU @ 800MHz"), Some(800));
        assert_eq!(base_frequency_from_model("AMD Ryzen 7 5800X 8-Core Processor"), None);
        assert_eq!(base_frequency_from_model("CPU @ turbo"), None);
    }

    #[test]
    fn rotulo_arm_desconhecido_mostra_codigos() {
        assert_eq!(arm_part_label(0x41, 0xd08).as_deref(), Some("ARM Cortex-A72"));
        assert_eq!(arm_part_label(0x61, 0x022).as_deref(), Some("Apple part 0x022"));
        assert_eq!(arm_part_label(0x99, 0x001).as_deref(), Some("ARM implementer 0x99 part 0x001"));
    }
}
