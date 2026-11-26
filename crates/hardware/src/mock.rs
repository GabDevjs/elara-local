//! Provider de teste.
//!
//! Todo teste dos outros crates depende deste tipo, e não do provider real:
//! é o que permite que a suíte inteira rode em uma máquina sem GPU, sem root e
//! sem NVML. O perfil devolvido por [`MockHardwareProvider::new`] é o
//! perfil-alvo da especificação — 8 núcleos, 32 GiB e uma RTX de 8 GiB — para
//! que os testes de planejamento raciocinem sobre uma máquina plausível.
//!
//! As amostras são **determinísticas**: a n-ésima chamada a `sample()` devolve
//! sempre os mesmos contadores, avançados exatamente 100 jiffies por CPU. Assim
//! um teste de cálculo de percentual tem uma resposta certa em vez de uma faixa.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::types::{
    BandwidthSource, CgroupLimits, CgroupVersion, CpuCache, CpuFeatures, CpuProfile, CpuTimes,
    CpuVendor, EffectiveResources, GpuBackend, GpuProbeSource, GpuProfile, GpuSample, GpuVendor,
    HARDWARE_SCHEMA_VERSION, HardwareProfile, HardwareTopology, HostProfile, LiveSample,
    MemorySample, MountPoint, NumaNode, RamProfile, StorageKind, StorageProfile,
};
use crate::{HardwareError, HardwareProvider, fingerprint};

/// Instante base das amostras: `2026-01-01T00:00:00Z`.
///
/// Fixo para que dois testes que comparam timestamps não dependam do relógio da
/// máquina que os roda.
const SAMPLE_EPOCH: jiff::Timestamp = jiff::Timestamp::constant(1_767_225_600, 0);

/// Jiffies de trabalho útil somados por CPU a cada amostra.
const BUSY_JIFFIES_PER_TICK: u64 = 50;

/// Jiffies ociosos somados por CPU a cada amostra.
///
/// Com 50 ocupados e 50 ociosos, o uso entre duas amostras é exatamente 50% —
/// um número redondo que deixa óbvio quando o cálculo do consumidor errou.
const IDLE_JIFFIES_PER_TICK: u64 = 50;

/// Threads lógicas do perfil de referência.
const REFERENCE_THREADS: u32 = 16;

/// Provider que devolve um perfil fixo e amostras determinísticas.
#[derive(Debug)]
pub struct MockHardwareProvider {
    outcome: Outcome,
    ticks: AtomicU64,
}

/// O que este mock faz quando é consultado.
#[derive(Debug)]
enum Outcome {
    /// Devolve este perfil.
    Profile(Box<HardwareProfile>),
    /// Falha com este erro, em `detect()` e em `sample()`.
    Failure(FailureSpec),
}

/// Erro reproduzível.
///
/// [`HardwareError`] não é `Clone` (contém um [`std::io::Error`]), e um mock
/// precisa poder falhar quantas vezes for chamado. Guardamos a descrição e
/// reconstruímos o erro a cada chamada — a mensagem e a variante são
/// preservadas; a causa original do `io::Error` é recriada a partir do `kind`.
#[derive(Debug, Clone)]
enum FailureSpec {
    Io { path: String, kind: std::io::ErrorKind, message: String },
    Parse { path: String, detail: String },
    UnsupportedPlatform(String),
}

impl FailureSpec {
    fn from_error(error: HardwareError) -> Self {
        match error {
            HardwareError::Io { path, source } => {
                Self::Io { path, kind: source.kind(), message: source.to_string() }
            }
            HardwareError::Parse { path, detail } => Self::Parse { path, detail },
            HardwareError::UnsupportedPlatform(detail) => Self::UnsupportedPlatform(detail),
        }
    }

    fn to_error(&self) -> HardwareError {
        match self {
            Self::Io { path, kind, message } => HardwareError::Io {
                path: path.clone(),
                source: std::io::Error::new(*kind, message.clone()),
            },
            Self::Parse { path, detail } => {
                HardwareError::Parse { path: path.clone(), detail: detail.clone() }
            }
            Self::UnsupportedPlatform(detail) => HardwareError::UnsupportedPlatform(detail.clone()),
        }
    }
}

impl MockHardwareProvider {
    /// Provider com o perfil de referência.
    pub fn new() -> Self {
        Self::with_profile(reference_profile())
    }

    /// Provider que devolve o perfil informado.
    pub fn with_profile(profile: HardwareProfile) -> Self {
        Self { outcome: Outcome::Profile(Box::new(profile)), ticks: AtomicU64::new(0) }
    }

    /// Provider que falha em toda consulta.
    ///
    /// Serve para exercitar o caminho degradado do daemon: sem hardware, a API
    /// precisa responder um erro claro em vez de um perfil vazio.
    pub fn failing(error: HardwareError) -> Self {
        Self { outcome: Outcome::Failure(FailureSpec::from_error(error)), ticks: AtomicU64::new(0) }
    }

    /// Perfil de referência: 8 núcleos, 32 GiB, uma RTX de 8 GiB, um `NVMe`.
    pub fn reference_profile() -> HardwareProfile {
        reference_profile()
    }

    /// Quantas amostras já foram servidas.
    ///
    /// Deixa um teste afirmar que o sampler chamou o provider o número esperado
    /// de vezes.
    pub fn ticks(&self) -> u64 {
        self.ticks.load(Ordering::Relaxed)
    }
}

impl Default for MockHardwareProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareProvider for MockHardwareProvider {
    fn detect(&self) -> Result<HardwareProfile, HardwareError> {
        match &self.outcome {
            Outcome::Profile(profile) => Ok((**profile).clone()),
            Outcome::Failure(spec) => Err(spec.to_error()),
        }
    }

    fn sample(&self) -> Result<LiveSample, HardwareError> {
        let profile = match &self.outcome {
            Outcome::Profile(profile) => profile,
            Outcome::Failure(spec) => return Err(spec.to_error()),
        };

        let tick = self.ticks.fetch_add(1, Ordering::Relaxed);
        Ok(deterministic_sample(profile, tick))
    }
}

/// Amostra da n-ésima chamada.
///
/// Depende apenas de `tick`: chamar duas vezes com o mesmo número devolve
/// exatamente a mesma amostra.
fn deterministic_sample(profile: &HardwareProfile, tick: u64) -> LiveSample {
    let threads = u64::from(profile.cpu.online_threads.max(1));
    let per_core: Vec<CpuTimes> = (0..threads).map(|_| core_times(tick)).collect();

    let total = CpuTimes {
        user: core_times(tick).user * threads,
        nice: 0,
        system: core_times(tick).system * threads,
        idle: core_times(tick).idle * threads,
        iowait: 0,
        irq: 0,
        softirq: 0,
        steal: 0,
    };

    // A memória disponível cai 128 MiB por amostra até um piso, para que um
    // teste consiga observar variação sem nunca chegar a números impossíveis.
    let floor = profile.ram.total_mb / 8;
    let available = profile.ram.available_mb.saturating_sub(tick * 128).max(floor);

    LiveSample {
        taken_at: SAMPLE_EPOCH
            + jiff::SignedDuration::from_secs(i64::try_from(tick).unwrap_or(i64::MAX)),
        cpu_total: total,
        cpu_per_core: per_core,
        load_average: [1.5, 1.25, 1.0],
        memory: MemorySample {
            total_mb: profile.ram.total_mb,
            available_mb: available,
            free_mb: available / 2,
            cached_mb: profile.ram.total_mb / 4,
            swap_total_mb: profile.ram.swap_total_mb,
            swap_free_mb: profile.ram.swap_free_mb,
        },
        gpus: profile.gpus.iter().map(|gpu| gpu_sample(gpu, tick)).collect(),
    }
}

/// Contadores de uma CPU depois de `tick` amostras.
fn core_times(tick: u64) -> CpuTimes {
    let busy = BUSY_JIFFIES_PER_TICK * tick;
    CpuTimes {
        user: busy * 4 / 5,
        nice: 0,
        system: busy / 5,
        idle: IDLE_JIFFIES_PER_TICK * tick,
        iowait: 0,
        irq: 0,
        softirq: 0,
        steal: 0,
    }
}

/// Amostra de GPU que percorre a faixa de utilização de forma previsível.
fn gpu_sample(gpu: &GpuProfile, tick: u64) -> GpuSample {
    let utilization = (tick % 11) * 10;
    let used = gpu.vram_total_mb.map(|total| (total / 8).saturating_mul(1 + tick % 4));

    #[allow(clippy::cast_precision_loss, reason = "valores abaixo de 100; a conversão é exata")]
    let utilization_percent = utilization as f32;

    GpuSample {
        index: gpu.index,
        utilization_percent: Some(utilization_percent),
        memory_utilization_percent: Some(utilization_percent / 2.0),
        vram_used_mb: used,
        vram_total_mb: gpu.vram_total_mb,
        temperature_c: Some(45.0 + utilization_percent / 4.0),
        power_watts: Some(30.0 + utilization_percent),
        clock_mhz: Some(1_800),
    }
}

/// Monta o perfil de referência.
fn reference_profile() -> HardwareProfile {
    let cpu = reference_cpu();
    let ram = reference_ram();
    let gpus = vec![reference_gpu()];

    HardwareProfile {
        schema_version: HARDWARE_SCHEMA_VERSION,
        fingerprint: fingerprint::compute(&cpu, &ram, &gpus),
        collected_at: SAMPLE_EPOCH,
        probe_duration_ms: 42,
        host: HostProfile {
            hostname: Some("elara-referencia".to_owned()),
            os: "linux".to_owned(),
            distribution: Some("Pop!_OS 24.04 LTS".to_owned()),
            kernel_version: Some("6.12.0-generic".to_owned()),
            architecture: "x86_64".to_owned(),
            virtualization: None,
        },
        cpu,
        ram,
        gpus,
        storage: vec![reference_storage()],
        topology: HardwareTopology {
            numa_nodes: vec![NumaNode {
                id: 0,
                cpus: (0..REFERENCE_THREADS).collect(),
                memory_mb: Some(32_768),
            }],
            simultaneous_multithreading: true,
            cgroup: CgroupLimits {
                version: CgroupVersion::V2,
                cpu_quota_cores: None,
                memory_limit_mb: None,
                memory_usage_mb: None,
            },
        },
        effective: EffectiveResources {
            cpu_threads: 15,
            ram_mb: 20_400,
            vram_mb: 7_200,
            vram_headroom_ratio: 0.10,
            rationale: vec![
                "CPU: 16 threads online, menos 1 reservada ao sistema → 15".to_owned(),
                "RAM: 24000 MiB disponíveis, menos 3600 MiB de folga (15%) → 20400 MiB".to_owned(),
                "GPU 0 (NVIDIA GeForce RTX 4060 Ti): 8000 MiB livres, menos 10% de folga \
                 → 7200 MiB"
                    .to_owned(),
            ],
        },
        warnings: Vec::new(),
    }
}

fn reference_cpu() -> CpuProfile {
    CpuProfile {
        model: "AMD Ryzen 7 5800X 8-Core Processor".to_owned(),
        vendor: CpuVendor::Amd,
        architecture: "x86_64".to_owned(),
        physical_cores: 8,
        logical_threads: REFERENCE_THREADS,
        online_threads: REFERENCE_THREADS,
        sockets: 1,
        base_frequency_mhz: Some(3_800),
        max_frequency_mhz: Some(4_700),
        cache: CpuCache {
            l1d_kb: Some(256),
            l1i_kb: Some(256),
            l2_kb: Some(4_096),
            l3_kb: Some(32_768),
        },
        features: CpuFeatures {
            sse3: true,
            ssse3: true,
            sse4_2: true,
            avx: true,
            avx2: true,
            fma: true,
            f16c: true,
            ..CpuFeatures::default()
        },
        hybrid: None,
    }
}

fn reference_ram() -> RamProfile {
    RamProfile {
        total_mb: 32_768,
        available_mb: 24_000,
        free_mb: 12_000,
        swap_total_mb: 8_192,
        swap_free_mb: 8_192,
        measured_bandwidth_gbps: None,
        bandwidth_source: BandwidthSource::NotMeasured,
    }
}

fn reference_gpu() -> GpuProfile {
    GpuProfile {
        index: 0,
        vendor: GpuVendor::Nvidia,
        model: "NVIDIA GeForce RTX 4060 Ti".to_owned(),
        pci_address: Some("0000:01:00.0".to_owned()),
        vram_total_mb: Some(8_192),
        vram_available_mb: Some(8_000),
        driver_version: Some("550.144.03".to_owned()),
        runtime_version: Some("12.4".to_owned()),
        compute_capability: Some("8.9".to_owned()),
        backends: vec![GpuBackend::Cuda, GpuBackend::Vulkan],
        is_integrated: false,
        power_limit_watts: Some(160.0),
        source: GpuProbeSource::Nvml,
    }
}

fn reference_storage() -> StorageProfile {
    StorageProfile {
        device: "nvme0n1".to_owned(),
        model: Some("Samsung SSD 990 PRO 1TB".to_owned()),
        kind: StorageKind::Nvme,
        size_mb: 953_869,
        rotational: Some(false),
        mounts: vec![MountPoint {
            path: "/".to_owned(),
            filesystem: "ext4".to_owned(),
            total_mb: 953_000,
            available_mb: 700_000,
            read_only: false,
        }],
        measured_read_mbps: None,
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]
mod tests {
    use super::*;

    #[test]
    fn perfil_de_referencia_e_a_maquina_alvo() {
        let profile = MockHardwareProvider::new().detect().unwrap();
        assert_eq!(profile.cpu.physical_cores, 8);
        assert_eq!(profile.cpu.logical_threads, 16);
        assert!(profile.cpu.features.avx2);
        assert_eq!(profile.ram.total_mb, 32_768);
        assert_eq!(profile.gpus.len(), 1);
        assert_eq!(profile.gpus[0].vram_total_mb, Some(8_192));
        assert!(profile.gpus[0].backends.contains(&GpuBackend::Cuda));
        assert_eq!(profile.storage.len(), 1);
        assert_eq!(profile.storage[0].kind, StorageKind::Nvme);
        assert_eq!(profile.fingerprint.len(), fingerprint::FINGERPRINT_LEN);
    }

    #[test]
    fn detect_e_estavel_entre_chamadas() {
        let provider = MockHardwareProvider::new();
        assert_eq!(provider.detect().unwrap(), provider.detect().unwrap());
    }

    #[test]
    fn perfil_customizado_e_devolvido_como_esta() {
        let mut profile = MockHardwareProvider::reference_profile();
        profile.cpu.model = "CPU inventada".to_owned();
        let provider = MockHardwareProvider::with_profile(profile.clone());
        assert_eq!(provider.detect().unwrap(), profile);
    }

    #[test]
    fn amostras_avancam_de_forma_determinista() {
        let provider = MockHardwareProvider::new();
        let primeira = provider.sample().unwrap();
        let segunda = provider.sample().unwrap();

        assert_eq!(provider.ticks(), 2);
        let delta_total = segunda.cpu_total.total() - primeira.cpu_total.total();
        let delta_idle = segunda.cpu_total.idle_total() - primeira.cpu_total.idle_total();
        assert_eq!(delta_total, 100 * 16, "100 jiffies por thread");
        assert_eq!(delta_idle * 2, delta_total, "exatamente 50% de uso");
    }

    #[test]
    fn duas_instancias_produzem_a_mesma_sequencia() {
        let a = MockHardwareProvider::new();
        let b = MockHardwareProvider::new();
        for _ in 0..5 {
            assert_eq!(a.sample().unwrap(), b.sample().unwrap());
        }
    }

    #[test]
    fn amostra_tem_uma_entrada_por_thread_online() {
        let sample = MockHardwareProvider::new().sample().unwrap();
        assert_eq!(sample.cpu_per_core.len(), 16);
        assert_eq!(sample.gpus.len(), 1);
        assert_eq!(sample.gpus[0].vram_total_mb, Some(8_192));
    }

    #[test]
    fn tempo_avanca_um_segundo_por_amostra() {
        let provider = MockHardwareProvider::new();
        let primeira = provider.sample().unwrap();
        let segunda = provider.sample().unwrap();
        assert_eq!(segunda.taken_at.as_second() - primeira.taken_at.as_second(), 1);
    }

    #[test]
    fn memoria_disponivel_nunca_fica_absurda() {
        let provider = MockHardwareProvider::new();
        let mut ultima = 0;
        for _ in 0..500 {
            ultima = provider.sample().unwrap().memory.available_mb;
        }
        assert!(ultima >= 32_768 / 8, "o piso protege contra valores impossíveis");
    }

    #[test]
    fn mock_que_falha_falha_sempre_e_nas_duas_operacoes() {
        let provider = MockHardwareProvider::failing(HardwareError::Parse {
            path: "/proc/cpuinfo".to_owned(),
            detail: "conteúdo vazio".to_owned(),
        });

        for _ in 0..3 {
            let error = provider.detect().unwrap_err();
            assert!(
                matches!(error, HardwareError::Parse { ref path, .. } if path == "/proc/cpuinfo")
            );
        }
        assert!(provider.sample().is_err());
    }

    #[test]
    fn erro_de_io_preserva_caminho_e_mensagem() {
        let provider = MockHardwareProvider::failing(HardwareError::Io {
            path: "/proc/meminfo".to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "sem permissão"),
        });
        let error = provider.detect().unwrap_err();
        let texto = error.to_string();
        assert!(texto.contains("/proc/meminfo"));
        assert!(texto.contains("sem permissão"));
    }
}
