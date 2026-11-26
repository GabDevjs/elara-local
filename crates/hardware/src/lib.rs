//! Detecção de hardware do Elara.
//!
//! O crate expõe um [`HardwareProvider`]: a implementação real lê `/proc`,
//! `/sys`, NVML e `nvidia-smi`; a implementação de teste devolve fixtures. Todo
//! o resto do sistema depende apenas do trait, de modo que **nenhum teste do
//! projeto exige GPU, root ou uma máquina específica**.
//!
//! ```no_run
//! use elara_hardware::{HardwareProvider, LinuxHardwareProvider};
//!
//! # fn main() -> Result<(), elara_hardware::HardwareError> {
//! let provider = LinuxHardwareProvider::new();
//! let profile = provider.detect()?;
//! println!("{} · {} MiB RAM", profile.cpu.model, profile.ram.total_mb);
//! # Ok(())
//! # }
//! ```

pub mod fingerprint;
pub mod mock;
pub mod parse;
pub mod probes;
pub mod types;

mod provider;

pub use mock::MockHardwareProvider;
pub use provider::LinuxHardwareProvider;
pub use types::{
    BandwidthSource, CgroupLimits, CgroupVersion, CpuCache, CpuFeatures, CpuProfile, CpuTimes,
    CpuVendor, EffectiveResources, GpuBackend, GpuProbeSource, GpuProfile, GpuSample, GpuVendor,
    HARDWARE_SCHEMA_VERSION, HardwareProfile, HardwareTopology, HostProfile, HybridTopology,
    LiveSample, MemorySample, MountPoint, NumaNode, ProbeWarning, RamProfile, StorageKind,
    StorageProfile, Virtualization, WarningSeverity,
};

/// Falhas na detecção de hardware.
///
/// Só chega aqui o que impede produzir um perfil. Sondas parciais viram
/// [`ProbeWarning`] dentro do perfil, não erro.
#[derive(Debug, thiserror::Error)]
pub enum HardwareError {
    /// Um arquivo essencial de `/proc` ou `/sys` não pôde ser lido.
    #[error("não foi possível ler {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// Um arquivo essencial existe mas tem formato inesperado.
    #[error("formato inesperado em {path}: {detail}")]
    Parse { path: String, detail: String },

    /// A plataforma atual não é suportada por este provider.
    #[error("plataforma não suportada: {0}")]
    UnsupportedPlatform(String),
}

/// Fonte de informação de hardware.
///
/// Implementado por [`LinuxHardwareProvider`] (real) e
/// [`MockHardwareProvider`] (testes).
pub trait HardwareProvider: Send + Sync + std::fmt::Debug {
    /// Coleta o perfil completo. Operação cara (dezenas de ms): o daemon faz
    /// isso no startup e em refresh explícito, não a cada requisição.
    fn detect(&self) -> Result<HardwareProfile, HardwareError>;

    /// Lê os contadores instantâneos. Precisa ser barato o bastante para rodar
    /// a cada segundo.
    fn sample(&self) -> Result<LiveSample, HardwareError>;
}
