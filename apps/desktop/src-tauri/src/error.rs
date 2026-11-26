//! Erro único da ponte, desenhado para ser consumido pela UI.
//!
//! A UI precisa distinguir três situações que, num cliente HTTP comum, viram a
//! mesma exceção: o daemon não está no ar, o daemon está no ar mas recusou a
//! requisição, e o daemon respondeu algo que não conseguimos ler. Cada uma pede
//! uma tela diferente — a primeira nem é erro, é o estado inicial esperado de
//! quem abriu só o desktop. Por isso o `code` é parte do contrato e não um
//! detalhe de log.

use serde::Serialize;
use serde::ser::SerializeStruct;

/// Falha ao falar com o `agentd`.
#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    /// Nem `ELARA_SOCKET` nem `XDG_RUNTIME_DIR` existem no ambiente.
    ///
    /// Separado de [`BridgeError::Unreachable`] porque aqui não há nem caminho
    /// para mostrar ao usuário: o problema é de ambiente, não do daemon.
    #[error(
        "não foi possível determinar o caminho do socket: defina ELARA_SOCKET \
         ou garanta que XDG_RUNTIME_DIR esteja no ambiente"
    )]
    SocketUnresolved,

    /// O socket existe no ambiente, mas ninguém atende nele.
    #[error("daemon não está atendendo em {socket_path} ({detail})")]
    Unreachable {
        /// Caminho tentado, repetido aqui para a UI poder exibi-lo.
        socket_path: String,
        /// Mensagem do sistema operacional, ex.: `No such file or directory`.
        detail: String,
    },

    /// O daemon aceitou a conexão mas não respondeu dentro do orçamento.
    #[error("o daemon não respondeu {target} em {timeout_ms} ms")]
    Timeout {
        /// Rota ou caminho de socket que estourou o tempo.
        target: String,
        /// Orçamento aplicado, em milissegundos.
        timeout_ms: u64,
    },

    /// Falha de HTTP/1.1 ou do socket depois da conexão estabelecida.
    #[error("falha de transporte com o daemon: {0}")]
    Transport(String),

    /// O daemon respondeu com status de erro.
    ///
    /// O `code` vem do corpo `ApiError` quando ele existe: repassar o código do
    /// daemon é o que permite à UI reagir a `hardware.unavailable` sem que a
    /// ponte precise conhecer o domínio.
    #[error("o daemon recusou a requisição (HTTP {status}): {message}")]
    Daemon {
        /// Status HTTP recebido.
        status: u16,
        /// Código do `ApiError`, ou `daemon.http-<status>` quando ilegível.
        code: String,
        /// Mensagem já concatenada com `details`, quando havia.
        message: String,
    },

    /// A resposta chegou, mas não é o JSON que esperávamos.
    #[error("resposta ilegível do daemon: {0}")]
    InvalidResponse(String),
}

impl BridgeError {
    /// Código estável consumido pela UI para escolher a tela.
    ///
    /// Estável significa: mudar um destes valores é mudar o contrato com o
    /// TypeScript, não um detalhe interno.
    pub fn code(&self) -> &str {
        match self {
            Self::SocketUnresolved => "daemon.socket-unresolved",
            Self::Unreachable { .. } => "daemon.unreachable",
            Self::Timeout { .. } => "daemon.timeout",
            Self::Transport(_) => "daemon.transport",
            Self::Daemon { code, .. } => code,
            Self::InvalidResponse(_) => "daemon.invalid-response",
        }
    }

    /// `true` quando o daemon simplesmente não está no ar.
    ///
    /// A ponte usa isto para não poluir o log com stack de erro a cada
    /// tentativa de reconexão do stream de eventos.
    pub fn is_offline(&self) -> bool {
        matches!(self, Self::SocketUnresolved | Self::Unreachable { .. })
    }
}

/// Serializa como `{ code, message }` — a forma que o `invoke` entrega ao
/// TypeScript quando o comando falha.
impl Serialize for BridgeError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("BridgeError", 2)?;
        state.serialize_field("code", self.code())?;
        state.serialize_field("message", &self.to_string())?;
        state.end()
    }
}
