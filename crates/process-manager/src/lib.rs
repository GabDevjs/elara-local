//! Supervisão de processos filhos.
//!
//! O daemon precisa manter o agent-runtime vivo sem virar um init system: um
//! processo, política de reinício com backoff exponencial, desligamento
//! ordenado (SIGTERM, prazo, SIGKILL) e estado observável.
//!
//! Regra de segurança que atravessa o módulo: o comando supervisionado nunca
//! passa por um shell. Argumentos são vetores, não strings concatenadas.

pub mod supervisor;

pub use supervisor::{ProcessSpec, RestartPolicy, Supervisor, SupervisorHandle};

/// Falhas ao supervisionar um processo.
#[derive(Debug, thiserror::Error)]
pub enum ProcessError {
    /// O executável não existe ou não é executável.
    #[error("executável não encontrado: {program}")]
    ProgramNotFound { program: String },

    /// O spawn falhou por erro do sistema operacional.
    #[error("não foi possível iniciar {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },

    /// O processo excedeu o número máximo de reinícios.
    #[error("{name} falhou {restarts} vezes; desistindo")]
    RestartLimitExceeded { name: String, restarts: u32 },

    /// O processo ignorou SIGTERM e precisou de SIGKILL.
    #[error("{name} não terminou em {timeout_ms} ms; enviado SIGKILL")]
    ShutdownTimeout { name: String, timeout_ms: u64 },
}
