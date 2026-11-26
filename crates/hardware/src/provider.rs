//! Implementação real do [`HardwareProvider`] para Linux.
//!
//! Duas operações com custos muito diferentes: [`LinuxHardwareProvider::detect`]
//! roda todas as sondas (dezenas de milissegundos, uma vez no startup) e
//! [`LinuxHardwareProvider::sample`] lê três arquivos e o NVML (microssegundos,
//! uma vez por segundo). Misturar as duas seria pagar o custo da primeira na
//! frequência da segunda.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;

use nvml_wrapper::Nvml;

use crate::types::{
    CgroupLimits, CpuProfile, EffectiveResources, GpuProfile, HARDWARE_SCHEMA_VERSION,
    HardwareProfile, LiveSample, ProbeWarning, RamProfile,
};
use crate::{HardwareError, HardwareProvider, fingerprint, probes};

/// Threads deixadas para o sistema operacional quando há mais de duas.
///
/// Sem essa reserva, uma inferência a todo vapor trava o compositor do desktop
/// e o próprio daemon deixa de responder ao health check.
const SYSTEM_RESERVED_THREADS: u32 = 1;

/// A partir de quantas threads vale a pena reservar uma.
const MIN_THREADS_TO_RESERVE: u32 = 2;

/// Fração da memória utilizável deixada de lado, em porcentagem.
///
/// `MemAvailable` inclui cache que o kernel *acredita* poder liberar; sob
/// pressão real nem sempre libera. A folga é o que separa "apertado" de
/// "morto pelo OOM killer".
const RAM_HEADROOM_PERCENT: u64 = 15;

/// Piso da reserva de memória, em MiB.
///
/// Em orçamentos pequenos, 15% é pouco demais: 15% de 2 GiB são 300 MiB, menos
/// que o próprio daemon e a UI consomem.
const MIN_SYSTEM_RESERVE_MB: u64 = 512;

/// Folga de VRAM, em porcentagem, para o compositor e o driver.
const VRAM_HEADROOM_PERCENT: u64 = 10;

/// A mesma folga em razão, como o contrato de [`EffectiveResources`] a expõe.
const VRAM_HEADROOM_RATIO: f64 = 0.10;

/// Detecção de hardware lendo o sistema de arquivos real.
#[derive(Debug)]
pub struct LinuxHardwareProvider {
    root: PathBuf,
    nvml: OnceLock<NvmlState>,
}

/// Resultado — bem-sucedido ou não — da inicialização do NVML.
///
/// Guardado em [`OnceLock`] porque inicializar o NVML custa dezenas de
/// milissegundos: fazer isso a cada `sample()` consumiria mais tempo que todo o
/// resto da amostragem.
#[derive(Debug)]
struct NvmlState {
    handle: Option<Nvml>,
    error: Option<String>,
}

impl LinuxHardwareProvider {
    /// Provider para a máquina real (raiz `/`).
    pub fn new() -> Self {
        Self::with_root(PathBuf::from("/"))
    }

    /// Provider que lê uma árvore alternativa de `/proc` e `/sys`.
    ///
    /// Existe para os testes: com uma raiz de fixture dá para verificar a
    /// detecção de um contêiner limitado a 2 núcleos sem contêiner nenhum. Sob
    /// uma raiz que não seja `/`, o NVML e o `nvidia-smi` ficam desligados —
    /// eles descreveriam o host, não a fixture.
    pub fn with_root(root: PathBuf) -> Self {
        Self { root, nvml: OnceLock::new() }
    }

    /// Raiz que este provider está lendo.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `true` quando estamos lendo a máquina real.
    fn reads_real_machine(&self) -> bool {
        self.root == Path::new("/")
    }

    /// Handle NVML, inicializado no primeiro uso.
    fn nvml(&self) -> Option<&Nvml> {
        if !self.reads_real_machine() {
            return None;
        }
        self.nvml_state().handle.as_ref()
    }

    fn nvml_state(&self) -> &NvmlState {
        self.nvml.get_or_init(|| match Nvml::init() {
            Ok(handle) => NvmlState { handle: Some(handle), error: None },
            Err(error) => NvmlState { handle: None, error: Some(error.to_string()) },
        })
    }
}

impl Default for LinuxHardwareProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl HardwareProvider for LinuxHardwareProvider {
    fn detect(&self) -> Result<HardwareProfile, HardwareError> {
        let started = Instant::now();
        let mut warnings = Vec::new();
        let root = self.root.as_path();

        let host = probes::host::probe_host(root, &mut warnings);
        let cpu = probes::cpu::probe_cpu(root, &mut warnings)?;
        let ram = probes::ram::probe_ram(root, &mut warnings)?;

        let nvml = self.nvml();
        if self.reads_real_machine()
            && nvml.is_none()
            && let Some(error) = self.nvml_state().error.as_deref()
        {
            warnings.push(ProbeWarning::info(
                "gpu.nvml",
                format!("NVML não carregou ({error}); tentando nvidia-smi e sysfs"),
            ));
        }

        let options =
            probes::gpu::GpuProbeOptions { nvml, allow_host_tools: self.reads_real_machine() };
        let gpus = probes::gpu::probe_gpus(root, &options, &mut warnings);
        let storage = probes::storage::probe_storage(root, &mut warnings);
        let topology = probes::topology::probe_topology(root, &mut warnings);

        let effective = effective_resources(&cpu, &ram, &gpus, &topology.cgroup);

        Ok(HardwareProfile {
            schema_version: HARDWARE_SCHEMA_VERSION,
            fingerprint: fingerprint::compute(&cpu, &ram, &gpus),
            collected_at: jiff::Timestamp::now(),
            probe_duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            host,
            cpu,
            ram,
            gpus,
            storage,
            topology,
            effective,
            warnings,
        })
    }

    fn sample(&self) -> Result<LiveSample, HardwareError> {
        let root = self.root.as_path();
        let stat = probes::cpu::sample_cpu_times(root)?;

        Ok(LiveSample {
            taken_at: jiff::Timestamp::now(),
            cpu_total: stat.total,
            cpu_per_core: stat.per_cpu,
            load_average: probes::cpu::sample_load_average(root)?,
            memory: probes::ram::sample_memory(root)?,
            gpus: probes::gpu::sample_gpus(self.nvml()),
        })
    }
}

/// Calcula o orçamento realmente utilizável.
///
/// Cada número vem acompanhado de uma linha em `rationale`: quando o usuário
/// perguntar "por que só 12 GiB se eu tenho 32?", a resposta já está no perfil.
fn effective_resources(
    cpu: &CpuProfile,
    ram: &RamProfile,
    gpus: &[GpuProfile],
    cgroup: &CgroupLimits,
) -> EffectiveResources {
    let mut rationale = Vec::new();
    let cpu_threads = effective_threads(cpu, cgroup, &mut rationale);
    let ram_mb = effective_ram(ram, cgroup, &mut rationale);
    let vram_mb = effective_vram(gpus, &mut rationale);

    EffectiveResources {
        cpu_threads,
        ram_mb,
        vram_mb,
        vram_headroom_ratio: VRAM_HEADROOM_RATIO,
        rationale,
    }
}

/// Threads utilizáveis: o menor entre as online e a cota do cgroup, menos a
/// reserva do sistema.
fn effective_threads(cpu: &CpuProfile, cgroup: &CgroupLimits, rationale: &mut Vec<String>) -> u32 {
    let online = cpu.online_threads.max(1);
    let mut threads = online;
    let mut parts = vec![format!("CPU: {online} threads online")];

    if let Some(quota) = cgroup.cpu_quota_cores {
        let allowed = quota_to_threads(quota);
        if allowed < threads {
            threads = allowed;
            parts.push(format!("limitadas a {allowed} pela cota do cgroup ({quota:.2} núcleos)"));
        } else {
            parts.push(format!("cota do cgroup de {quota:.2} núcleos não restringe"));
        }
    }

    if threads > MIN_THREADS_TO_RESERVE {
        threads -= SYSTEM_RESERVED_THREADS;
        parts.push("menos 1 reservada ao sistema".to_owned());
    } else {
        parts.push("sem reserva (menos de 3 threads disponíveis)".to_owned());
    }

    let threads = threads.max(1);
    rationale.push(format!("{} → {threads}", parts.join(", ")));
    threads
}

/// Converte a cota do cgroup em threads inteiras.
///
/// Arredonda para baixo: meio núcleo de cota não executa uma thread inteira, e
/// prometer que executa é o que causa throttling constante.
fn quota_to_threads(quota: f64) -> u32 {
    if !quota.is_finite() || quota < 1.0 {
        return 1;
    }
    let floored = quota.floor();
    if floored >= f64::from(u32::MAX) {
        return u32::MAX;
    }
    // Truncamento seguro: o valor está entre 1 e u32::MAX e já foi arredondado.
    #[allow(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "faixa validada nas duas linhas acima"
    )]
    let threads = floored as u32;
    threads
}

/// Memória utilizável, respeitando o cgroup e guardando uma folga.
fn effective_ram(ram: &RamProfile, cgroup: &CgroupLimits, rationale: &mut Vec<String>) -> u64 {
    let mut budget = ram.available_mb;
    let mut parts = vec![format!("RAM: {budget} MiB disponíveis segundo o kernel")];

    if let Some(limit) = cgroup.memory_limit_mb {
        let usage = cgroup.memory_usage_mb.unwrap_or(0);
        let allowed = limit.saturating_sub(usage);
        parts.push(format!(
            "cgroup permite {limit} MiB e já usa {usage} MiB (sobram {allowed} MiB)"
        ));
        budget = budget.min(allowed);
    }

    let reserve = (budget * RAM_HEADROOM_PERCENT / 100).max(MIN_SYSTEM_RESERVE_MB).min(budget / 2);
    let usable = budget.saturating_sub(reserve);
    parts.push(format!(
        "menos {reserve} MiB de folga ({RAM_HEADROOM_PERCENT}%, piso de {MIN_SYSTEM_RESERVE_MB} \
         MiB, no máximo metade)"
    ));
    rationale.push(format!("{} → {usable} MiB", parts.join(", ")));
    usable
}

/// VRAM utilizável somada, apenas de GPUs dedicadas e com backend usável.
fn effective_vram(gpus: &[GpuProfile], rationale: &mut Vec<String>) -> u64 {
    let mut total = 0;

    for gpu in gpus {
        let label = format!("GPU {} ({})", gpu.index, gpu.model);
        if gpu.is_integrated {
            rationale.push(format!("{label}: integrada, compartilha a RAM do sistema — não conta"));
            continue;
        }
        if gpu.backends.is_empty() {
            rationale.push(format!("{label}: sem backend de inferência utilizável — não conta"));
            continue;
        }
        let Some(free) = gpu.vram_available_mb else {
            rationale.push(format!("{label}: VRAM livre desconhecida — não conta"));
            continue;
        };
        let usable = free * (100 - VRAM_HEADROOM_PERCENT) / 100;
        rationale.push(format!(
            "{label}: {free} MiB livres, menos {VRAM_HEADROOM_PERCENT}% de folga → {usable} MiB"
        ));
        total += usable;
    }

    if gpus.is_empty() {
        rationale.push("VRAM: nenhuma GPU detectada → 0 MiB".to_owned());
    }
    total
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]
mod tests {
    use super::*;
    use crate::types::{
        BandwidthSource, CgroupVersion, CpuCache, CpuFeatures, CpuVendor, GpuBackend,
        GpuProbeSource, GpuVendor,
    };

    fn cpu(online: u32) -> CpuProfile {
        CpuProfile {
            model: "CPU de teste".to_owned(),
            vendor: CpuVendor::Amd,
            architecture: "x86_64".to_owned(),
            physical_cores: online / 2,
            logical_threads: online,
            online_threads: online,
            sockets: 1,
            base_frequency_mhz: None,
            max_frequency_mhz: None,
            cache: CpuCache::default(),
            features: CpuFeatures::default(),
            hybrid: None,
        }
    }

    fn ram(available_mb: u64) -> RamProfile {
        RamProfile {
            total_mb: 32_768,
            available_mb,
            free_mb: available_mb / 2,
            swap_total_mb: 0,
            swap_free_mb: 0,
            measured_bandwidth_gbps: None,
            bandwidth_source: BandwidthSource::NotMeasured,
        }
    }

    fn sem_cgroup() -> CgroupLimits {
        CgroupLimits {
            version: CgroupVersion::None,
            cpu_quota_cores: None,
            memory_limit_mb: None,
            memory_usage_mb: None,
        }
    }

    fn gpu(free_mb: Option<u64>, integrated: bool, backends: Vec<GpuBackend>) -> GpuProfile {
        GpuProfile {
            index: 0,
            vendor: GpuVendor::Nvidia,
            model: "GPU de teste".to_owned(),
            pci_address: Some("0000:01:00.0".to_owned()),
            vram_total_mb: free_mb.map(|free| free + 500),
            vram_available_mb: free_mb,
            driver_version: None,
            runtime_version: None,
            compute_capability: None,
            backends,
            is_integrated: integrated,
            power_limit_watts: None,
            source: GpuProbeSource::Nvml,
        }
    }

    #[test]
    fn reserva_uma_thread_para_o_sistema() {
        let effective = effective_resources(&cpu(16), &ram(24_000), &[], &sem_cgroup());
        assert_eq!(effective.cpu_threads, 15);
        assert!(effective.rationale.iter().any(|line| line.contains("reservada ao sistema")));
    }

    #[test]
    fn maquina_minuscula_nunca_fica_sem_thread() {
        assert_eq!(effective_resources(&cpu(1), &ram(2_000), &[], &sem_cgroup()).cpu_threads, 1);
        assert_eq!(effective_resources(&cpu(2), &ram(2_000), &[], &sem_cgroup()).cpu_threads, 2);
        assert_eq!(effective_resources(&cpu(3), &ram(2_000), &[], &sem_cgroup()).cpu_threads, 2);
    }

    #[test]
    fn cota_de_cgroup_limita_as_threads() {
        let cgroup = CgroupLimits {
            version: CgroupVersion::V2,
            cpu_quota_cores: Some(2.5),
            memory_limit_mb: None,
            memory_usage_mb: None,
        };
        let effective = effective_resources(&cpu(16), &ram(24_000), &[], &cgroup);
        assert_eq!(effective.cpu_threads, 2, "2.5 núcleos viram 2, e 2 não sofre reserva");
    }

    #[test]
    fn cota_generosa_nao_restringe() {
        let cgroup = CgroupLimits {
            version: CgroupVersion::V2,
            cpu_quota_cores: Some(32.0),
            memory_limit_mb: None,
            memory_usage_mb: None,
        };
        assert_eq!(effective_resources(&cpu(16), &ram(24_000), &[], &cgroup).cpu_threads, 15);
    }

    #[test]
    fn ram_guarda_folga_percentual() {
        let effective = effective_resources(&cpu(8), &ram(20_000), &[], &sem_cgroup());
        assert_eq!(effective.ram_mb, 20_000 - 3_000);
    }

    #[test]
    fn ram_pequena_usa_o_piso_de_reserva() {
        let effective = effective_resources(&cpu(2), &ram(2_000), &[], &sem_cgroup());
        assert_eq!(effective.ram_mb, 2_000 - 512, "15% seriam 300 MiB, menos que o piso");
    }

    #[test]
    fn reserva_nunca_passa_de_metade_do_orcamento() {
        let effective = effective_resources(&cpu(2), &ram(600), &[], &sem_cgroup());
        assert_eq!(effective.ram_mb, 300);
    }

    #[test]
    fn cgroup_manda_na_ram_e_nao_o_host() {
        let cgroup = CgroupLimits {
            version: CgroupVersion::V2,
            cpu_quota_cores: None,
            memory_limit_mb: Some(4_096),
            memory_usage_mb: Some(1_024),
        };
        let effective = effective_resources(&cpu(8), &ram(24_000), &[], &cgroup);
        assert_eq!(effective.ram_mb, 3_072 - 512, "sobram 3072 MiB no cgroup, menos o piso");
    }

    #[test]
    fn vram_desconta_a_folga_do_compositor() {
        let gpus = [gpu(Some(8_000), false, vec![GpuBackend::Cuda])];
        let effective = effective_resources(&cpu(8), &ram(24_000), &gpus, &sem_cgroup());
        assert_eq!(effective.vram_mb, 7_200);
        assert!((effective.vram_headroom_ratio - 0.10).abs() < f64::EPSILON);
    }

    #[test]
    fn gpu_integrada_nao_entra_no_orcamento() {
        let gpus = [gpu(Some(8_000), true, vec![GpuBackend::Vulkan])];
        let effective = effective_resources(&cpu(8), &ram(24_000), &gpus, &sem_cgroup());
        assert_eq!(effective.vram_mb, 0);
        assert!(effective.rationale.iter().any(|line| line.contains("integrada")));
    }

    #[test]
    fn gpu_sem_backend_nao_entra_no_orcamento() {
        let gpus = [gpu(Some(8_000), false, Vec::new())];
        assert_eq!(effective_resources(&cpu(8), &ram(24_000), &gpus, &sem_cgroup()).vram_mb, 0);
    }

    #[test]
    fn gpu_com_vram_desconhecida_nao_entra_no_orcamento() {
        let gpus = [gpu(None, false, vec![GpuBackend::Cuda])];
        let effective = effective_resources(&cpu(8), &ram(24_000), &gpus, &sem_cgroup());
        assert_eq!(effective.vram_mb, 0);
        assert!(effective.rationale.iter().any(|line| line.contains("desconhecida")));
    }

    #[test]
    fn duas_gpus_dedicadas_somam() {
        let gpus = [
            gpu(Some(8_000), false, vec![GpuBackend::Cuda]),
            gpu(Some(12_000), false, vec![GpuBackend::Cuda]),
        ];
        assert_eq!(
            effective_resources(&cpu(8), &ram(24_000), &gpus, &sem_cgroup()).vram_mb,
            7_200 + 10_800
        );
    }

    #[test]
    fn cota_fracionaria_arredonda_para_baixo() {
        assert_eq!(quota_to_threads(0.5), 1);
        assert_eq!(quota_to_threads(1.0), 1);
        assert_eq!(quota_to_threads(3.9), 3);
        assert_eq!(quota_to_threads(f64::INFINITY), 1);
    }

    #[test]
    fn folga_de_vram_em_porcentagem_e_em_razao_concordam() {
        #[allow(clippy::cast_precision_loss, reason = "a constante é 10; a conversão é exata")]
        let ratio = VRAM_HEADROOM_PERCENT as f64 / 100.0;
        assert!((ratio - VRAM_HEADROOM_RATIO).abs() < f64::EPSILON);
    }

    #[test]
    fn rationale_explica_todos_os_numeros() {
        let gpus = [gpu(Some(8_000), false, vec![GpuBackend::Cuda])];
        let effective = effective_resources(&cpu(16), &ram(24_000), &gpus, &sem_cgroup());
        assert!(effective.rationale.iter().any(|line| line.starts_with("CPU:")));
        assert!(effective.rationale.iter().any(|line| line.starts_with("RAM:")));
        assert!(effective.rationale.iter().any(|line| line.starts_with("GPU 0")));
    }

    #[test]
    fn provider_com_raiz_alternativa_nao_toca_no_nvml() {
        let provider = LinuxHardwareProvider::with_root(PathBuf::from("/tmp"));
        assert!(provider.nvml().is_none());
        assert_eq!(provider.root(), Path::new("/tmp"));
    }
}
