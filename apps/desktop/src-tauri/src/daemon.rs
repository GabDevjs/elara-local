//! Cliente HTTP/1.1 sobre o Unix socket do `agentd`.
//!
//! É deliberadamente um proxy de JSON: nada aqui conhece `HardwareProfile` ou
//! `ResourceSnapshot`. Reimplementar esses tipos em terceiro lugar (Rust do
//! daemon, TypeScript gerado, Rust da ponte) só criaria uma cópia para
//! divergir. Quem valida a forma é o zod, do lado da UI, contra os schemas
//! gerados do próprio daemon.

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use http_body_util::{BodyExt, Collected, Full};
use hyper::body::{Bytes, Incoming};
use hyper::client::conn::http1::SendRequest;
use hyper::header::{ACCEPT, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::net::UnixStream;

use crate::error::BridgeError;

/// Header em que cliente e servidor anunciam a versão do contrato HTTP.
pub const PROTOCOL_HEADER: &str = "x-elara-protocol";

/// Versão do contrato HTTP falada por esta ponte.
///
/// Espelha `PROTOCOL_VERSION` de `crates/agentd/src/api/types.rs`. É texto
/// porque vai direto para o header.
pub const PROTOCOL_VERSION: &str = "1";

/// Orçamento de tempo de toda requisição unária.
///
/// Dez segundos porque `POST /v1/hardware/refresh` re-executa todas as sondas;
/// as demais rotas respondem em milissegundos. Sem timeout, um daemon travado
/// deixaria a UI num "carregando" eterno, que é pior do que um erro.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// `Host` enviado nas requisições.
///
/// HTTP/1.1 exige o header mesmo sem TCP; o valor é ignorado pelo daemon.
const HOST_HEADER: &str = "elara.localhost";

/// Rota do stream de eventos.
pub const EVENTS_PATH: &str = "/v1/events";

// ---------------------------------------------------------------------------
// Localização do socket
// ---------------------------------------------------------------------------

/// De onde veio o caminho do socket.
///
/// A UI mostra isto na tela de daemon offline: "configurado por
/// `ELARA_SOCKET`" e "caminho padrão" levam o usuário a conferir coisas
/// diferentes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SocketSource {
    /// Variável de ambiente `ELARA_SOCKET`.
    Env,
    /// Derivado de `XDG_RUNTIME_DIR`.
    XdgRuntimeDir,
}

/// Caminho do socket e sua procedência.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SocketLocation {
    /// Caminho no filesystem.
    pub path: PathBuf,
    /// Como o caminho foi decidido.
    pub source: SocketSource,
}

/// Resolve o socket a partir do ambiente.
///
/// Devolve `None` quando nenhuma das duas variáveis existe — não há caminho
/// padrão razoável nesse caso, e chutar `/tmp/...` produziria uma mensagem de
/// erro apontando para um lugar onde o daemon nunca esteve.
fn resolve_socket() -> Option<SocketLocation> {
    if let Some(raw) = std::env::var_os("ELARA_SOCKET") {
        let path = PathBuf::from(raw);
        if !path.as_os_str().is_empty() {
            return Some(SocketLocation { path, source: SocketSource::Env });
        }
    }

    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR")?;
    let path = Path::new(&runtime_dir).join("elara").join("agentd.sock");
    Some(SocketLocation { path, source: SocketSource::XdgRuntimeDir })
}

// ---------------------------------------------------------------------------
// Cliente
// ---------------------------------------------------------------------------

/// Cliente do daemon. Barato de clonar: só guarda caminho e orçamento.
///
/// Uma conexão por requisição, e não um pool: o custo de um `connect(2)` em
/// socket Unix é irrelevante perto de qualquer sonda de hardware, e sem pool
/// não existe o modo de falha "conexão morta reaproveitada".
#[derive(Debug, Clone)]
pub struct DaemonClient {
    socket: Option<SocketLocation>,
    timeout: Duration,
}

impl DaemonClient {
    /// Constrói o cliente lendo o ambiente uma única vez.
    pub fn from_env() -> Self {
        Self { socket: resolve_socket(), timeout: REQUEST_TIMEOUT }
    }

    /// Onde este cliente tentará falar, quando foi possível descobrir.
    pub fn location(&self) -> Option<&SocketLocation> {
        self.socket.as_ref()
    }

    /// `GET` que devolve o JSON do corpo já desserializado.
    pub async fn get_json(&self, path: &str) -> Result<Value, BridgeError> {
        self.json(Method::GET, path).await
    }

    /// `POST` sem corpo — as rotas de Fase 2 que mutam estado não recebem
    /// payload; quando alguma receber, este método ganha um parâmetro.
    pub async fn post_json(&self, path: &str) -> Result<Value, BridgeError> {
        self.json(Method::POST, path).await
    }

    /// Abre `GET /v1/events` e devolve o stream SSE ainda não consumido.
    pub async fn open_events(&self) -> Result<EventStream, BridgeError> {
        let (mut sender, connection) = self.open().await?;
        let request = build_request(Method::GET, EVENTS_PATH, "text/event-stream")?;

        let response = self.with_timeout(EVENTS_PATH, sender.send_request(request)).await??;
        let status = response.status();
        if !status.is_success() {
            let body = response.into_body().collect().await.map(Collected::to_bytes);
            return Err(api_error(status, body.unwrap_or_default().as_ref()));
        }

        Ok(EventStream {
            body: response.into_body(),
            buffer: Vec::new(),
            data: String::new(),
            _sender: sender,
            _connection: connection,
        })
    }

    /// Aplica [`Self::timeout`] a qualquer futuro, traduzindo o estouro em
    /// [`BridgeError::Timeout`] com o alvo que falhou.
    async fn with_timeout<F: Future>(
        &self,
        target: &str,
        future: F,
    ) -> Result<F::Output, BridgeError> {
        tokio::time::timeout(self.timeout, future).await.map_err(|_| BridgeError::Timeout {
            target: target.to_owned(),
            timeout_ms: u64::try_from(self.timeout.as_millis()).unwrap_or(u64::MAX),
        })
    }

    /// Estabelece uma conexão HTTP/1.1 sobre o socket.
    async fn open(&self) -> Result<(SendRequest<Full<Bytes>>, TaskGuard), BridgeError> {
        let location = self.socket.as_ref().ok_or(BridgeError::SocketUnresolved)?;
        let display = location.path.display().to_string();

        let stream = self
            .with_timeout(&display, UnixStream::connect(&location.path))
            .await?
            .map_err(|error| BridgeError::Unreachable {
                socket_path: display.clone(),
                detail: error.to_string(),
            })?;

        let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|error| BridgeError::Transport(error.to_string()))?;

        // A conexão é um futuro próprio: sem alguém puxando-a, nenhum byte
        // trafega. O guard a mata junto com o `SendRequest`, para que uma
        // requisição abandonada não deixe task pendurada.
        let guard = TaskGuard(tauri::async_runtime::spawn(async move {
            if let Err(error) = connection.await {
                tracing::debug!(%error, "conexão com o daemon encerrada");
            }
        }));

        Ok((sender, guard))
    }

    /// Executa a requisição e interpreta o corpo.
    async fn json(&self, method: Method, path: &str) -> Result<Value, BridgeError> {
        let (mut sender, _connection) = self.open().await?;
        let request = build_request(method, path, "application/json")?;

        let response = self.with_timeout(path, sender.send_request(request)).await??;
        let status = response.status();

        let bytes = self.with_timeout(path, response.into_body().collect()).await??.to_bytes();

        if !status.is_success() {
            return Err(api_error(status, bytes.as_ref()));
        }

        // 204 e afins: o corpo vazio vira `null`, que o zod do outro lado
        // rejeita com o caminho do campo — melhor do que um `{}` inventado.
        if bytes.is_empty() {
            return Ok(Value::Null);
        }

        serde_json::from_slice(bytes.as_ref()).map_err(|error| {
            BridgeError::InvalidResponse(format!("{path} não devolveu JSON válido: {error}"))
        })
    }
}

/// Converte o erro de transporte do hyper, para que `?` funcione nos pontos em
/// que o resultado interno do `with_timeout` também é um `Result`.
impl From<hyper::Error> for BridgeError {
    fn from(error: hyper::Error) -> Self {
        Self::Transport(error.to_string())
    }
}

/// Monta a requisição no formato que o daemon espera.
fn build_request(
    method: Method,
    path: &str,
    accept: &str,
) -> Result<Request<Full<Bytes>>, BridgeError> {
    Request::builder()
        .method(method)
        .uri(path)
        .header(HOST, HOST_HEADER)
        .header(ACCEPT, accept)
        .header(PROTOCOL_HEADER, PROTOCOL_VERSION)
        .body(Full::new(Bytes::new()))
        .map_err(|error| {
            BridgeError::Transport(format!("requisição inválida para {path}: {error}"))
        })
}

/// Traduz uma resposta de erro do daemon preservando o `code` dele.
fn api_error(status: StatusCode, body: &[u8]) -> BridgeError {
    /// Espelho mínimo do `ApiError` do daemon — só o suficiente para repassar.
    #[derive(Deserialize)]
    struct ApiErrorBody {
        code: String,
        message: String,
        details: Option<String>,
    }

    match serde_json::from_slice::<ApiErrorBody>(body) {
        Ok(parsed) => BridgeError::Daemon {
            status: status.as_u16(),
            code: parsed.code,
            message: match parsed.details {
                Some(details) => format!("{} ({details})", parsed.message),
                None => parsed.message,
            },
        },
        Err(_) => BridgeError::Daemon {
            status: status.as_u16(),
            code: format!("daemon.http-{}", status.as_u16()),
            message: format!("o daemon respondeu {status} sem um corpo de erro reconhecível"),
        },
    }
}

// ---------------------------------------------------------------------------
// Stream de eventos
// ---------------------------------------------------------------------------

/// Mata a task que bombeia a conexão quando o dono do stream some.
///
/// Sem isto, fechar a janela deixaria uma task viva lendo um socket que
/// ninguém mais consulta.
struct TaskGuard(tauri::async_runtime::JoinHandle<()>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl fmt::Debug for TaskGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("TaskGuard")
    }
}

/// Stream `text/event-stream` do daemon, entregue um envelope por vez.
///
/// Guarda o `SendRequest` e a task da conexão porque o corpo sozinho não os
/// mantém vivos, e um stream infinito precisa dos três.
pub struct EventStream {
    body: Incoming,
    /// Bytes ainda não quebrados em linhas.
    buffer: Vec<u8>,
    /// Linhas `data:` acumuladas do evento em construção.
    data: String,
    _sender: SendRequest<Full<Bytes>>,
    _connection: TaskGuard,
}

impl fmt::Debug for EventStream {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventStream")
            .field("buffered_bytes", &self.buffer.len())
            .finish_non_exhaustive()
    }
}

impl EventStream {
    /// Próximo envelope, ou `None` quando o daemon fechou o stream.
    ///
    /// Só o campo `data:` importa: o `event:` do SSE é redundante com o campo
    /// `payload.type` do envelope, e confiar nos dois obrigaria a decidir qual
    /// vence quando divergissem.
    pub async fn next_envelope(&mut self) -> Result<Option<Value>, BridgeError> {
        loop {
            while let Some(line) = take_line(&mut self.buffer) {
                if line.is_empty() {
                    // Linha em branco fecha o evento; comentários de keep-alive
                    // chegam sozinhos e não têm `data:` para despachar.
                    if self.data.is_empty() {
                        continue;
                    }
                    let raw = std::mem::take(&mut self.data);
                    return serde_json::from_str(&raw).map(Some).map_err(|error| {
                        BridgeError::InvalidResponse(format!("evento SSE ilegível: {error}"))
                    });
                }

                if let Some(rest) = line.strip_prefix("data:") {
                    if !self.data.is_empty() {
                        self.data.push('\n');
                    }
                    self.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
            }

            match self.body.frame().await {
                Some(Ok(frame)) => {
                    if let Ok(chunk) = frame.into_data() {
                        self.buffer.extend_from_slice(chunk.as_ref());
                    }
                }
                Some(Err(error)) => return Err(BridgeError::Transport(error.to_string())),
                None => return Ok(None),
            }
        }
    }
}

/// Extrai a próxima linha completa do buffer, sem o terminador.
///
/// Devolve `None` enquanto não houver `\n`: um frame TCP pode cortar um evento
/// no meio, e tratar o pedaço como linha corromperia o JSON.
fn take_line(buffer: &mut Vec<u8>) -> Option<String> {
    let end = buffer.iter().position(|byte| *byte == b'\n')?;
    let raw: Vec<u8> = buffer.drain(..=end).collect();
    let text = String::from_utf8_lossy(&raw);
    Some(text.trim_end_matches(['\n', '\r']).to_owned())
}

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "em teste, um panic é exatamente o relatório de falha desejado"
)]
mod tests {
    use super::*;

    #[test]
    fn take_line_espera_o_terminador() {
        let mut buffer = b"data: {\"a\":1}".to_vec();
        assert_eq!(take_line(&mut buffer), None);

        buffer.extend_from_slice(b"\n\n");
        assert_eq!(take_line(&mut buffer).as_deref(), Some("data: {\"a\":1}"));
        assert_eq!(take_line(&mut buffer).as_deref(), Some(""));
        assert_eq!(take_line(&mut buffer), None);
    }

    #[test]
    fn take_line_remove_crlf() {
        let mut buffer = b"event: resources.sampled\r\n".to_vec();
        assert_eq!(take_line(&mut buffer).as_deref(), Some("event: resources.sampled"));
    }

    #[test]
    fn api_error_preserva_o_codigo_do_daemon() {
        let body = br#"{"code":"hardware.unavailable","message":"sonda falhou","details":null}"#;
        let error = api_error(StatusCode::SERVICE_UNAVAILABLE, body);
        assert_eq!(error.code(), "hardware.unavailable");
        assert!(error.to_string().contains("sonda falhou"));
    }

    #[test]
    fn api_error_sem_corpo_reconhecivel_usa_o_status() {
        let error = api_error(StatusCode::BAD_GATEWAY, b"<html>oops</html>");
        assert_eq!(error.code(), "daemon.http-502");
    }

    #[test]
    fn erro_serializa_como_code_e_message() {
        let error = BridgeError::Unreachable {
            socket_path: "/run/user/1000/elara/agentd.sock".to_owned(),
            detail: "No such file or directory".to_owned(),
        };
        let json = serde_json::to_value(&error).expect("serialização do erro");
        assert_eq!(json["code"], "daemon.unreachable");
        assert!(json["message"].as_str().is_some_and(|m| m.contains("agentd.sock")));
    }
}
