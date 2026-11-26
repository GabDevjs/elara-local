//! Observabilidade do Elara: logging estruturado, event bus e amostragem de
//! recursos ao vivo.
//!
//! Três responsabilidades, deliberadamente separadas:
//!
//! - [`logging`] configura o `tracing` uma única vez por processo;
//! - [`bus`] distribui [`types::EventEnvelope`] para assinantes (SSE, log, DB);
//! - [`sampler`] transforma contadores brutos do hardware em
//!   [`types::ResourceSnapshot`] periodicamente.

pub mod bus;
pub mod logging;
pub mod sampler;
pub mod types;

pub use bus::EventBus;
pub use logging::{LogFormat, LoggingConfig};
pub use sampler::{ResourceSampler, SamplerHandle};
pub use types::{
    ComponentState, ComponentStatus, CpuUsage, Event, EventEnvelope, GpuUsage, MemoryUsage,
    ResourceSnapshot,
};

/// Falhas na inicialização da telemetria.
#[derive(Debug, thiserror::Error)]
pub enum TelemetryError {
    /// O subscriber global do `tracing` já foi instalado por outro caminho.
    #[error("logging já foi inicializado neste processo")]
    LoggingAlreadyInitialized,

    /// Não foi possível abrir o arquivo de log.
    #[error("não foi possível abrir o log em {path}: {source}")]
    LogFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// O filtro de log (`ELARA_LOG`) é inválido.
    #[error("filtro de log inválido {filter:?}: {detail}")]
    InvalidFilter { filter: String, detail: String },
}
