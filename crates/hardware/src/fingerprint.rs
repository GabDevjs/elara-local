//! Identidade estável da máquina.
//!
//! O fingerprint existe para responder a uma pergunta: *os benchmarks que eu
//! gravei ontem ainda valem?* Ele só pode mudar quando o hardware muda. Por
//! isso entra apenas o que é invariante entre boots — modelo e contagem de CPU,
//! extensões, RAM instalada, GPUs e sua VRAM total.
//!
//! Ficam **de fora**, deliberadamente: memória disponível, VRAM livre,
//! utilização, temperatura, frequência instantânea e qualquer timestamp. Se um
//! desses entrasse, cada coleta produziria uma máquina "nova" e a base de
//! calibração seria descartada a cada segundo.

use crate::types::{CpuFeatures, CpuProfile, GpuProfile, HardwareProfile, RamProfile};

/// Número de caracteres hexadecimais do fingerprint.
///
/// 32 caracteres são 128 bits de blake3: colisão acidental entre máquinas é
/// inconcebível, e o valor ainda cabe em um log ou em uma coluna de tabela.
pub const FINGERPRINT_LEN: usize = 32;

/// Calcula o fingerprint a partir das partes invariantes de um perfil.
pub fn compute(cpu: &CpuProfile, ram: &RamProfile, gpus: &[GpuProfile]) -> String {
    let input = canonical_input(cpu, ram, gpus);
    blake3::hash(input.as_bytes()).to_hex().chars().take(FINGERPRINT_LEN).collect()
}

/// Calcula o fingerprint de um perfil já montado.
pub fn of_profile(profile: &HardwareProfile) -> String {
    compute(&profile.cpu, &profile.ram, &profile.gpus)
}

/// Texto canônico que é efetivamente hasheado.
///
/// Público porque, quando um fingerprint muda sem explicação, comparar os dois
/// textos mostra em uma linha o que mudou — coisa que comparar dois hashes
/// jamais mostraria.
pub fn canonical_input(cpu: &CpuProfile, ram: &RamProfile, gpus: &[GpuProfile]) -> String {
    let mut lines = vec![
        format!("cpu.model={}", normalize_whitespace(&cpu.model)),
        format!("cpu.physical_cores={}", cpu.physical_cores),
        format!("cpu.logical_threads={}", cpu.logical_threads),
        format!("cpu.sockets={}", cpu.sockets),
        format!("cpu.features={}", feature_signature(&cpu.features)),
        format!("ram.total_gib={}", round_to_gib(ram.total_mb)),
    ];

    // Ordenar por endereço PCI mantém o hash estável mesmo que o driver enumere
    // as GPUs em outra ordem entre boots.
    let mut sorted: Vec<&GpuProfile> = gpus.iter().collect();
    sorted.sort_by(|a, b| a.pci_address.cmp(&b.pci_address).then_with(|| a.model.cmp(&b.model)));
    for gpu in sorted {
        let vram = gpu.vram_total_mb.map_or_else(|| "desconhecida".to_owned(), |mb| mb.to_string());
        lines.push(format!("gpu={}|{}", normalize_whitespace(&gpu.model), vram));
    }

    lines.join("\n")
}

/// Lista ordenada das extensões ativas.
///
/// Uma lista de nomes, e não os bits da struct, para que acrescentar um campo
/// novo a [`CpuFeatures`] no futuro não invalide os fingerprints já gravados
/// das máquinas que não têm a extensão.
fn feature_signature(features: &CpuFeatures) -> String {
    let flags = [
        ("amx_bf16", features.amx_bf16),
        ("amx_int8", features.amx_int8),
        ("avx", features.avx),
        ("avx2", features.avx2),
        ("avx512_bf16", features.avx512_bf16),
        ("avx512_vnni", features.avx512_vnni),
        ("avx512f", features.avx512f),
        ("dotprod", features.dotprod),
        ("f16c", features.f16c),
        ("fma", features.fma),
        ("i8mm", features.i8mm),
        ("neon", features.neon),
        ("sse3", features.sse3),
        ("sse4_2", features.sse4_2),
        ("ssse3", features.ssse3),
        ("sve", features.sve),
    ];
    flags.iter().filter(|(_, active)| *active).map(|(name, _)| *name).collect::<Vec<_>>().join(",")
}

/// Arredonda a RAM para o GiB mais próximo.
///
/// O kernel reserva quantidades ligeiramente diferentes a cada boot (firmware,
/// carve-out de vídeo), então `MemTotal` oscila alguns MiB na mesma máquina.
/// Sem o arredondamento, o fingerprint mudaria sem que nada tivesse mudado.
fn round_to_gib(total_mb: u64) -> u64 {
    (total_mb + 512) / 1024
}

/// Colapsa espaços repetidos: alguns modelos de CPU vêm com espaçamento duplo.
fn normalize_whitespace(value: &str) -> String {
    value.split_ascii_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;
    use crate::types::{BandwidthSource, CpuCache, CpuVendor, GpuProbeSource, GpuVendor};

    fn cpu() -> CpuProfile {
        CpuProfile {
            model: "AMD Ryzen 7 5800X 8-Core Processor".to_owned(),
            vendor: CpuVendor::Amd,
            architecture: "x86_64".to_owned(),
            physical_cores: 8,
            logical_threads: 16,
            online_threads: 16,
            sockets: 1,
            base_frequency_mhz: Some(3800),
            max_frequency_mhz: Some(4700),
            cache: CpuCache {
                l1d_kb: Some(256),
                l1i_kb: Some(256),
                l2_kb: Some(4096),
                l3_kb: Some(32768),
            },
            features: CpuFeatures { avx: true, avx2: true, fma: true, ..CpuFeatures::default() },
            hybrid: None,
        }
    }

    fn ram() -> RamProfile {
        RamProfile {
            total_mb: 32_046,
            available_mb: 24_000,
            free_mb: 12_000,
            swap_total_mb: 8_192,
            swap_free_mb: 8_192,
            measured_bandwidth_gbps: None,
            bandwidth_source: BandwidthSource::NotMeasured,
        }
    }

    fn gpu(pci: &str, model: &str, vram: u64) -> GpuProfile {
        GpuProfile {
            index: 0,
            vendor: GpuVendor::Nvidia,
            model: model.to_owned(),
            pci_address: Some(pci.to_owned()),
            vram_total_mb: Some(vram),
            vram_available_mb: Some(vram - 500),
            driver_version: Some("580.173.02".to_owned()),
            runtime_version: Some("12.0".to_owned()),
            compute_capability: Some("12.0".to_owned()),
            backends: Vec::new(),
            is_integrated: false,
            power_limit_watts: Some(180.0),
            source: GpuProbeSource::Nvml,
        }
    }

    #[test]
    fn tem_32_caracteres_hexadecimais() {
        let fingerprint = compute(&cpu(), &ram(), &[gpu("0000:01:00.0", "RTX 5060 Ti", 8151)]);
        assert_eq!(fingerprint.len(), FINGERPRINT_LEN);
        assert!(fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn memoria_disponivel_nao_muda_o_fingerprint() {
        let base = compute(&cpu(), &ram(), &[]);

        let mut ocupada = ram();
        ocupada.available_mb = 1_024;
        ocupada.free_mb = 512;
        assert_eq!(
            compute(&cpu(), &ocupada, &[]),
            base,
            "a mesma máquina sob carga é a mesma máquina"
        );
    }

    #[test]
    fn vram_livre_e_metricas_nao_mudam_o_fingerprint() {
        let gpus = vec![gpu("0000:01:00.0", "RTX 5060 Ti", 8151)];
        let base = compute(&cpu(), &ram(), &gpus);

        let mut ocupada = gpus.clone();
        if let Some(first) = ocupada.first_mut() {
            first.vram_available_mb = Some(12);
            first.power_limit_watts = Some(90.0);
            first.index = 3;
        }
        assert_eq!(compute(&cpu(), &ram(), &ocupada), base);
    }

    #[test]
    fn oscilacao_de_alguns_mib_na_ram_nao_muda_o_fingerprint() {
        let base = compute(&cpu(), &ram(), &[]);
        let mut outro_boot = ram();
        outro_boot.total_mb = 32_040;
        assert_eq!(compute(&cpu(), &outro_boot, &[]), base);
    }

    #[test]
    fn trocar_de_cpu_muda_o_fingerprint() {
        let base = compute(&cpu(), &ram(), &[]);
        let mut outra = cpu();
        outra.model = "AMD Ryzen 9 5900X 12-Core Processor".to_owned();
        assert_ne!(compute(&outra, &ram(), &[]), base);
    }

    #[test]
    fn perder_uma_extensao_muda_o_fingerprint() {
        let base = compute(&cpu(), &ram(), &[]);
        let mut sem_avx2 = cpu();
        sem_avx2.features.avx2 = false;
        assert_ne!(compute(&sem_avx2, &ram(), &[]), base);
    }

    #[test]
    fn trocar_a_gpu_muda_o_fingerprint() {
        let base = compute(&cpu(), &ram(), &[gpu("0000:01:00.0", "RTX 5060 Ti", 8151)]);
        let outra = compute(&cpu(), &ram(), &[gpu("0000:01:00.0", "RTX 4090", 24564)]);
        assert_ne!(outra, base);
    }

    #[test]
    fn ordem_de_enumeracao_das_gpus_nao_importa() {
        let a = gpu("0000:01:00.0", "RTX 5060 Ti", 8151);
        let b = gpu("0000:04:00.0", "RTX 3060", 12288);
        assert_eq!(
            compute(&cpu(), &ram(), &[a.clone(), b.clone()]),
            compute(&cpu(), &ram(), &[b, a]),
            "trocar a ordem do driver não é trocar de máquina"
        );
    }

    #[test]
    fn dobrar_a_ram_muda_o_fingerprint() {
        let base = compute(&cpu(), &ram(), &[]);
        let mut maior = ram();
        maior.total_mb = 64_092;
        assert_ne!(compute(&cpu(), &maior, &[]), base);
    }

    #[test]
    fn entrada_canonica_nao_contem_dado_volatil() {
        let input = canonical_input(&cpu(), &ram(), &[gpu("0000:01:00.0", "RTX 5060 Ti", 8151)]);
        assert!(!input.contains("24000"), "available_mb não pode aparecer");
        assert!(!input.contains("7651"), "vram livre não pode aparecer");
        assert!(input.contains("cpu.model=AMD Ryzen 7 5800X 8-Core Processor"));
        assert!(input.contains("cpu.features=avx,avx2,fma"));
        assert!(input.contains("ram.total_gib=31"));
        assert!(input.contains("gpu=RTX 5060 Ti|8151"));
    }

    #[test]
    fn gpu_sem_vram_conhecida_e_declarada_como_tal() {
        let mut sem_vram = gpu("0000:01:00.0", "NVIDIA 10de:2d04", 8151);
        sem_vram.vram_total_mb = None;
        let input = canonical_input(&cpu(), &ram(), &[sem_vram]);
        assert!(input.contains("gpu=NVIDIA 10de:2d04|desconhecida"));
    }
}
