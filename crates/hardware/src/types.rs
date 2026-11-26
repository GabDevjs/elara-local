//! Contrato de dados do perfil de hardware.
//!
//! Estes tipos são a fronteira entre o daemon (Rust), o desktop (TypeScript) e o
//! agent-runtime (TypeScript). As definições TypeScript em
//! `packages/schemas/src/generated/` são geradas a partir daqui via `ts-rs`
//! (`pnpm bindings`), portanto **este arquivo é a fonte da verdade**.
//!
//! Princípio de honestidade: todo campo que pode não ser detectável em uma
//! máquina qualquer é `Option`. O sistema nunca inventa um número — quando uma
//! sonda falha, o valor fica `None` e um [`ProbeWarning`] explica o porquê.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Versão do esquema do [`HardwareProfile`] persistido.
///
/// Incrementar sempre que a forma serializada mudar de modo incompatível: o
/// daemon descarta perfis persistidos com versão diferente e re-detecta.
pub const HARDWARE_SCHEMA_VERSION: u32 = 1;

/// Diretório de saída das definições TypeScript geradas.
///
/// O caminho é relativo a `<crate>/bindings/` (o `TS_RS_EXPORT_DIR` padrão do
/// `ts-rs`), por isso os três níveis: `crates/hardware/bindings/../../..` é a
/// raiz do repositório.
macro_rules! ts_export {
    () => {
        "../../../packages/schemas/src/generated/"
    };
}

// ---------------------------------------------------------------------------
// Raiz
// ---------------------------------------------------------------------------

/// Retrato completo da máquina em um instante.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct HardwareProfile {
    /// Versão do esquema (ver [`HARDWARE_SCHEMA_VERSION`]).
    pub schema_version: u32,
    /// Identidade estável da máquina, derivada apenas de atributos invariantes.
    pub fingerprint: String,
    /// Instante da coleta, RFC 3339.
    #[ts(type = "string")]
    pub collected_at: jiff::Timestamp,
    /// Quanto tempo a detecção levou — usado para orçar o startup.
    pub probe_duration_ms: u64,
    pub host: HostProfile,
    pub cpu: CpuProfile,
    pub ram: RamProfile,
    pub gpus: Vec<GpuProfile>,
    pub storage: Vec<StorageProfile>,
    pub topology: HardwareTopology,
    /// Orçamento realmente utilizável, já descontando cgroups e reservas.
    pub effective: EffectiveResources,
    /// Sondas que falharam ou entregaram dados parciais.
    pub warnings: Vec<ProbeWarning>,
}

/// Identificação do sistema operacional hospedeiro.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct HostProfile {
    pub hostname: Option<String>,
    /// Ex.: `linux`.
    pub os: String,
    /// Nome amigável da distribuição, de `/etc/os-release`.
    pub distribution: Option<String>,
    /// Versão do kernel, de `/proc/sys/kernel/osrelease`.
    pub kernel_version: Option<String>,
    /// Ex.: `x86_64`.
    pub architecture: String,
    /// Ambiente de virtualização detectado, quando houver.
    pub virtualization: Option<Virtualization>,
}

/// Ambiente virtualizado ou conteinerizado em que o processo roda.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export, export_to = ts_export!())]
pub enum Virtualization {
    Kvm,
    Xen,
    VmWare,
    HyperV,
    VirtualBox,
    Wsl,
    Docker,
    Podman,
    Lxc,
    Unknown,
}

// ---------------------------------------------------------------------------
// CPU
// ---------------------------------------------------------------------------

/// Descrição da CPU e das extensões relevantes para inferência.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct CpuProfile {
    /// Nome comercial, ex.: `AMD Ryzen 7 5800X`.
    pub model: String,
    pub vendor: CpuVendor,
    pub architecture: String,
    /// Núcleos físicos distintos somando todos os sockets.
    pub physical_cores: u32,
    /// Threads lógicas visíveis ao SO.
    pub logical_threads: u32,
    /// Threads atualmente online (podem ser menos que `logical_threads`).
    pub online_threads: u32,
    pub sockets: u32,
    pub base_frequency_mhz: Option<u32>,
    pub max_frequency_mhz: Option<u32>,
    pub cache: CpuCache,
    pub features: CpuFeatures,
    /// Em CPUs híbridas (P/E cores), contagem por classe.
    pub hybrid: Option<HybridTopology>,
}

/// Fabricante da CPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = ts_export!())]
pub enum CpuVendor {
    Amd,
    Intel,
    Arm,
    Apple,
    Other,
}

/// Tamanhos de cache por nível, em KiB, agregados por socket.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct CpuCache {
    pub l1d_kb: Option<u32>,
    pub l1i_kb: Option<u32>,
    pub l2_kb: Option<u32>,
    pub l3_kb: Option<u32>,
}

/// Extensões de instrução que mudam materialmente a velocidade de inferência
/// em CPU. Só entram aqui flags que o llama.cpp realmente explora.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
#[allow(
    clippy::struct_excessive_bools,
    reason = "é um conjunto de flags de CPU vindo de /proc/cpuinfo; \
              transformá-las em enums de duas variantes só esconderia o dado bruto"
)]
pub struct CpuFeatures {
    // x86
    pub sse3: bool,
    pub ssse3: bool,
    pub sse4_2: bool,
    pub avx: bool,
    pub avx2: bool,
    pub fma: bool,
    pub f16c: bool,
    pub avx512f: bool,
    pub avx512_vnni: bool,
    pub avx512_bf16: bool,
    pub amx_int8: bool,
    pub amx_bf16: bool,
    // ARM
    pub neon: bool,
    pub dotprod: bool,
    pub i8mm: bool,
    pub sve: bool,
}

/// Distribuição de núcleos em CPUs híbridas (performance vs. eficiência).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct HybridTopology {
    pub performance_cores: u32,
    pub efficiency_cores: u32,
}

// ---------------------------------------------------------------------------
// Memória
// ---------------------------------------------------------------------------

/// Memória do sistema. `total` é físico; `available` é o que dá para usar de
/// fato agora — a distinção é obrigatória para o Resource Broker.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct RamProfile {
    pub total_mb: u64,
    /// `MemAvailable` do kernel: inclui cache recuperável.
    pub available_mb: u64,
    /// `MemFree`: memória sem nenhum uso.
    pub free_mb: u64,
    pub swap_total_mb: u64,
    pub swap_free_mb: u64,
    /// Largura de banda medida. `None` enquanto o Calibration Engine (Fase 7)
    /// não tiver rodado — jamais preenchido com estimativa teórica.
    pub measured_bandwidth_gbps: Option<f64>,
    pub bandwidth_source: BandwidthSource,
}

/// Origem do número de largura de banda de memória.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export, export_to = ts_export!())]
pub enum BandwidthSource {
    /// Ainda não medido — o campo correspondente é `None`.
    NotMeasured,
    /// Benchmark local do tipo STREAM triad.
    Measured,
}

// ---------------------------------------------------------------------------
// GPU
// ---------------------------------------------------------------------------

/// Uma GPU detectada. Campos opcionais refletem sondas que podem não existir:
/// sem NVML/ROCm só conseguimos identidade PCI, não VRAM livre.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct GpuProfile {
    /// Índice estável dentro desta coleta.
    pub index: u32,
    pub vendor: GpuVendor,
    pub model: String,
    /// Endereço PCI, ex.: `0000:01:00.0`. Chave de correlação entre sondas.
    pub pci_address: Option<String>,
    pub vram_total_mb: Option<u64>,
    /// VRAM livre no momento da coleta — pode ser bem menor que o total por
    /// causa do compositor do desktop.
    pub vram_available_mb: Option<u64>,
    pub driver_version: Option<String>,
    /// Ex.: `12.4` para CUDA.
    pub runtime_version: Option<String>,
    /// Compute capability NVIDIA, ex.: `8.9`.
    pub compute_capability: Option<String>,
    /// Backends de inferência que esta GPU pode servir, em ordem de preferência.
    pub backends: Vec<GpuBackend>,
    /// GPU integrada compartilha RAM do sistema — não conta como VRAM dedicada.
    pub is_integrated: bool,
    pub power_limit_watts: Option<f32>,
    /// Como esta entrada foi obtida.
    pub source: GpuProbeSource,
}

/// Fabricante da GPU.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = ts_export!())]
pub enum GpuVendor {
    Nvidia,
    Amd,
    Intel,
    Apple,
    Unknown,
}

/// Backend de aceleração utilizável.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = ts_export!())]
pub enum GpuBackend {
    Cuda,
    Rocm,
    Vulkan,
    Metal,
    Sycl,
    OpenCl,
}

/// Sonda que produziu os dados da GPU, do mais confiável ao menos.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export, export_to = ts_export!())]
pub enum GpuProbeSource {
    /// Biblioteca NVML da NVIDIA — dados completos e ao vivo.
    Nvml,
    /// Binário `nvidia-smi` — fallback quando NVML não carrega.
    NvidiaSmi,
    /// `/sys/class/drm` + PCI — identidade e VRAM total, sem métricas ao vivo.
    Sysfs,
}

// ---------------------------------------------------------------------------
// Armazenamento
// ---------------------------------------------------------------------------

/// Um dispositivo de bloco e onde ele está montado.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct StorageProfile {
    /// Nome do dispositivo em `/sys/block`, ex.: `nvme0n1`.
    pub device: String,
    pub model: Option<String>,
    pub kind: StorageKind,
    pub size_mb: u64,
    pub rotational: Option<bool>,
    pub mounts: Vec<MountPoint>,
    /// Throughput sequencial medido. `None` até o Calibration Engine rodar.
    pub measured_read_mbps: Option<f64>,
}

/// Classe física do dispositivo de armazenamento.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = ts_export!())]
pub enum StorageKind {
    Nvme,
    Ssd,
    Hdd,
    Unknown,
}

/// Um ponto de montagem e seu espaço livre.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct MountPoint {
    pub path: String,
    pub filesystem: String,
    pub total_mb: u64,
    pub available_mb: u64,
    pub read_only: bool,
}

// ---------------------------------------------------------------------------
// Topologia e limites
// ---------------------------------------------------------------------------

/// Layout NUMA, SMT e limites impostos por cgroup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct HardwareTopology {
    pub numa_nodes: Vec<NumaNode>,
    /// SMT / Hyper-Threading ativo.
    pub simultaneous_multithreading: bool,
    pub cgroup: CgroupLimits,
}

/// Um nó NUMA e os recursos ligados a ele.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct NumaNode {
    pub id: u32,
    /// IDs de CPU lógica pertencentes a este nó.
    pub cpus: Vec<u32>,
    pub memory_mb: Option<u64>,
}

/// Restrições do cgroup que contém este processo.
///
/// Ignorar isto é o erro clássico: dentro de um contêiner, `/proc/meminfo`
/// mostra a RAM do host e o processo é morto por OOM ao usá-la.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct CgroupLimits {
    pub version: CgroupVersion,
    /// Cota de CPU em núcleos equivalentes, ex.: `2.5`.
    pub cpu_quota_cores: Option<f64>,
    pub memory_limit_mb: Option<u64>,
    pub memory_usage_mb: Option<u64>,
}

/// Versão da hierarquia de cgroups em uso.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export, export_to = ts_export!())]
pub enum CgroupVersion {
    None,
    V1,
    V2,
}

/// Orçamento efetivamente utilizável pelo Elara, já descontando cgroups e as
/// reservas para o desktop e o driver.
///
/// É este valor — não o total físico — que o Resource Broker deve consumir.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct EffectiveResources {
    /// Threads que podemos usar sem sufocar o sistema.
    pub cpu_threads: u32,
    /// RAM utilizável, em MiB.
    pub ram_mb: u64,
    /// VRAM utilizável somada. `0` quando não há GPU dedicada usável.
    pub vram_mb: u64,
    /// Fração da VRAM total que deixamos livre para o compositor e o driver.
    pub vram_headroom_ratio: f64,
    /// Explicação legível de como cada número acima foi obtido.
    pub rationale: Vec<String>,
}

// ---------------------------------------------------------------------------
// Diagnóstico
// ---------------------------------------------------------------------------

/// Registro de uma sonda que não entregou dados completos.
///
/// Existe para que a UI possa dizer "VRAM desconhecida porque NVML não carregou"
/// em vez de mostrar zero e mentir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct ProbeWarning {
    /// Sonda de origem, ex.: `gpu.nvml`, `storage.mounts`.
    pub source: String,
    pub severity: WarningSeverity,
    pub message: String,
}

/// Gravidade de um [`ProbeWarning`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = ts_export!())]
pub enum WarningSeverity {
    /// Informação secundária ausente; decisões não são afetadas.
    Info,
    /// Dado relevante ausente; decisões ficam mais conservadoras.
    Warning,
    /// Sonda essencial falhou; o perfil não é confiável.
    Error,
}

impl ProbeWarning {
    pub fn info(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self { source: source.into(), severity: WarningSeverity::Info, message: message.into() }
    }

    pub fn warning(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self { source: source.into(), severity: WarningSeverity::Warning, message: message.into() }
    }

    pub fn error(source: impl Into<String>, message: impl Into<String>) -> Self {
        Self { source: source.into(), severity: WarningSeverity::Error, message: message.into() }
    }
}

// ---------------------------------------------------------------------------
// Amostragem ao vivo
// ---------------------------------------------------------------------------

/// Contadores brutos de um instante, usados pelo sampler de telemetria.
///
/// Contém valores acumulados (jiffies), não percentuais: quem calcula taxa é o
/// consumidor, comparando duas amostras.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct LiveSample {
    #[ts(type = "string")]
    pub taken_at: jiff::Timestamp,
    /// Contadores agregados de todas as CPUs.
    pub cpu_total: CpuTimes,
    /// Contadores por CPU lógica, na ordem de `/proc/stat`.
    pub cpu_per_core: Vec<CpuTimes>,
    /// Médias de carga de 1, 5 e 15 minutos.
    pub load_average: [f64; 3],
    pub memory: MemorySample,
    pub gpus: Vec<GpuSample>,
}

/// Tempo acumulado de CPU, em jiffies, como exposto por `/proc/stat`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    /// Jiffies em que a CPU não estava executando trabalho útil.
    pub const fn idle_total(&self) -> u64 {
        self.idle.saturating_add(self.iowait)
    }

    /// Jiffies totais desde o boot.
    pub const fn total(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }
}

/// Estado instantâneo da memória do sistema.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct MemorySample {
    pub total_mb: u64,
    pub available_mb: u64,
    pub free_mb: u64,
    pub cached_mb: u64,
    pub swap_total_mb: u64,
    pub swap_free_mb: u64,
}

/// Leitura instantânea de uma GPU. Tudo opcional: sem NVML só há identidade.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct GpuSample {
    pub index: u32,
    pub utilization_percent: Option<f32>,
    pub memory_utilization_percent: Option<f32>,
    pub vram_used_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,
    pub temperature_c: Option<f32>,
    pub power_watts: Option<f32>,
    /// Clock do SM, em MHz.
    pub clock_mhz: Option<u32>,
}
