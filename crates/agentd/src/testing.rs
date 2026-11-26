//! Aparato de teste do daemon.
//!
//! Não há mock de HTTP aqui: [`Daemon`] sobe o `agentd` inteiro — banco,
//! barramento, amostrador, supervisor, roteador — num socket Unix de verdade
//! dentro de um diretório temporário, e o cliente fala HTTP/1.1 pelo socket como
//! o `@elara/sdk` e a ponte do Tauri falam. O que se testa é a coisa que vai
//! rodar, não um substituto dela.
//!
//! A única peça trocada é o [`HardwareProvider`], que vira
//! [`MockHardwareProvider`] — é o que permite a suíte inteira rodar sem GPU, sem
//! root e sem depender do `/proc` da máquina que a executa.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "este módulo só existe sob cfg(test); um panic aqui é o relatório de falha"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use elara_hardware::{HardwareProvider, MockHardwareProvider};
use elara_telemetry::types::ComponentStatus;
use elara_telemetry::{EventBus, ResourceSampler};
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Incoming};
use hyper::header::{ACCEPT, CONTENT_TYPE, HOST};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use tempfile::TempDir;
use tokio::net::UnixStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::api::PROTOCOL_HEADER;
use crate::api::types::ApiError;
use crate::config::{Cli, Config, Environment};
use crate::db::Database;
use crate::server;
use crate::state::{AppState, AppStateParts};
use crate::supervisor::RuntimeSupervision;

/// Intervalo de amostragem usado nos testes.
///
/// Curto porque o `MockHardwareProvider` é puro em memória e porque
/// `/v1/resources` só devolve um snapshot a partir da **segunda** amostra: com o
/// padrão de um segundo, todo teste de recurso esperaria dois segundos à toa.
pub const TEST_SAMPLE_INTERVAL: Duration = Duration::from_millis(25);

/// Estado do daemon sem servidor HTTP, para testar as camadas de baixo.
#[derive(Debug)]
pub struct Harness {
    pub state: AppState,
    /// Só existe para segurar o diretório temporário: quando o `TempDir` cai, o
    /// banco do teste é apagado, e o estado ainda o teria aberto.
    #[allow(dead_code, reason = "o valor é o RAII do diretório, não um dado a ser lido")]
    dir: TempDir,
}

/// Harness com o provider de referência.
pub fn state_only() -> Harness {
    state_with(Arc::new(MockHardwareProvider::new()))
}

/// Harness com um provider escolhido pelo teste.
///
/// Precisa ser chamada de dentro de um runtime `tokio`: o amostrador e o
/// supervisor sobem tasks.
pub fn state_with(provider: Arc<dyn HardwareProvider>) -> Harness {
    let dir = tempfile::tempdir().expect("diretório temporário");
    let config = test_config(dir.path(), None);
    let state = build_state(config, provider);
    Harness { state, dir }
}

/// Estado de um componente pelo nome, para os testes afirmarem sobre ele.
///
/// Falha o teste quando o componente não existe: um nome errado num assert
/// deve aparecer como erro, não como `None` silencioso.
pub fn component(state: &AppState, name: &str) -> ComponentStatus {
    state
        .components()
        .into_iter()
        .find(|status| status.name == name)
        .unwrap_or_else(|| panic!("componente {name} não existe em /v1/status"))
}

/// Configuração apontando tudo para dentro de `dir`.
pub fn test_config(dir: &Path, runtime_cmd: Option<&str>) -> Config {
    let environment = Environment {
        runtime_dir: Some(dir.display().to_string()),
        data_home: Some(dir.display().to_string()),
        home: Some(dir.to_path_buf()),
        uid: 1000,
    };

    let cli = Cli {
        socket: Some(dir.join("agentd.sock")),
        data_dir: Some(dir.join("dados")),
        sample_interval_ms: Some(u64::try_from(TEST_SAMPLE_INTERVAL.as_millis()).unwrap_or(1_000)),
        runtime_cmd: runtime_cmd.map(str::to_owned),
        ..Cli::default()
    };

    Config::resolve(cli, &environment).expect("configuração de teste válida")
}

/// Monta o estado **sem** rodar a detecção de hardware.
///
/// É o que os testes de [`crate::hardware`] precisam: eles verificam o antes e o
/// depois de uma detecção, e um estado que já nasce com o perfil em cache não
/// deixaria o "antes" existir. O caminho completo de subida é o
/// [`crate::server::bootstrap`], usado por [`Daemon`].
fn build_state(config: Config, provider: Arc<dyn HardwareProvider>) -> AppState {
    let database = Database::open(&config.database_path).expect("banco de teste");
    let bus = Arc::new(EventBus::new(server::EVENT_BUS_CAPACITY));
    let sampler =
        ResourceSampler::new(Arc::clone(&provider), config.sample_interval).spawn(Arc::clone(&bus));
    let runtime = RuntimeSupervision::start(&config, Arc::clone(&bus));

    AppState::new(AppStateParts {
        config,
        hardware: provider,
        bus,
        sampler,
        database,
        runtime,
        shutdown: tokio::sync::watch::Sender::new(false),
    })
}

/// Daemon completo servindo em um socket temporário.
#[derive(Debug)]
pub struct Daemon {
    pub state: AppState,
    pub socket: PathBuf,
    /// `None` depois de [`Daemon::abandon`], que devolve a posse ao chamador.
    dir: Option<TempDir>,
    stop: Option<oneshot::Sender<String>>,
    task: Option<JoinHandle<std::io::Result<()>>>,
    recorder: Option<JoinHandle<()>>,
}

impl Daemon {
    /// Sobe o daemon com o perfil de hardware de referência.
    pub async fn start() -> Self {
        Self::start_with(Arc::new(MockHardwareProvider::new()), None).await
    }

    /// Sobe o daemon com um provider e um comando de runtime escolhidos.
    pub async fn start_with(
        provider: Arc<dyn HardwareProvider>,
        runtime_cmd: Option<&str>,
    ) -> Self {
        Self::start_at(tempfile::tempdir().expect("diretório temporário"), provider, runtime_cmd)
            .await
    }

    /// Sobe o daemon dentro de um diretório já existente.
    ///
    /// Usa o mesmo [`crate::server::bootstrap`] do binário: banco, detecção,
    /// persistência do perfil, amostrador, supervisor e gravador de eventos, na
    /// mesma ordem. Receber o diretório pronto é o que permite a um teste
    /// reaproveitar o caminho de um daemon anterior — o caso do socket órfão.
    pub async fn start_at(
        dir: TempDir,
        provider: Arc<dyn HardwareProvider>,
        runtime_cmd: Option<&str>,
    ) -> Self {
        let config = test_config(dir.path(), runtime_cmd);
        let socket = config.socket_path.clone();

        let (state, recorder) =
            server::bootstrap(config, provider).await.expect("subida do daemon de teste");
        let listener = server::bind_socket(&socket).expect("bind do socket de teste");

        state.bus().publish(elara_telemetry::types::Event::DaemonStarted {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            pid: state.pid(),
            socket_path: socket.display().to_string(),
        });

        let (stop_tx, stop_rx) = oneshot::channel::<String>();
        let task = tokio::spawn(server::serve(state.clone(), listener, async move {
            stop_rx.await.unwrap_or_else(|_| "canal de parada fechado".to_owned())
        }));

        Self {
            state,
            socket,
            dir: Some(dir),
            stop: Some(stop_tx),
            task: Some(task),
            recorder: Some(recorder),
        }
    }

    /// Encerra tudo **sem** o desligamento ordenado, deixando o arquivo de
    /// socket para trás — é o que sobra de um daemon morto por `SIGKILL`.
    ///
    /// Devolve o diretório temporário para que o caminho continue existindo e um
    /// segundo daemon possa tentar subir por cima dele.
    pub async fn abandon(&mut self) -> TempDir {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(recorder) = self.recorder.take() {
            recorder.abort();
        }
        self.state.sampler().shutdown().await;
        self.state.runtime().shutdown().await;
        self.dir.take().expect("o diretório temporário ainda pertence a este daemon")
    }

    /// `GET` com o header de protocolo correto.
    pub async fn get(&self, path: &str) -> TestResponse {
        self.request(Method::GET, path, None, Some("1")).await
    }

    /// `POST` com corpo JSON (ou sem corpo) e o header de protocolo correto.
    pub async fn post(&self, path: &str, body: Option<serde_json::Value>) -> TestResponse {
        let body = body.map(|value| value.to_string());
        self.request(Method::POST, path, body, Some("1")).await
    }

    /// Requisição crua, com controle sobre o header de protocolo.
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<String>,
        protocol: Option<&str>,
    ) -> TestResponse {
        let (mut sender, connection) = connect(&self.socket).await;

        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(HOST, "elara.localhost")
            .header(ACCEPT, "application/json");
        if let Some(version) = protocol {
            builder = builder.header(PROTOCOL_HEADER, version);
        }
        if body.is_some() {
            builder = builder.header(CONTENT_TYPE, "application/json");
        }

        let request = builder
            .body(Full::new(Bytes::from(body.unwrap_or_default())))
            .expect("requisição válida");

        let response = sender.send_request(request).await.expect("resposta do daemon");
        let status = response.status();
        let protocol_header = response
            .headers()
            .get(PROTOCOL_HEADER)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let bytes = response.into_body().collect().await.expect("corpo da resposta").to_bytes();
        connection.abort();

        TestResponse {
            status,
            protocol_header,
            body: String::from_utf8_lossy(bytes.as_ref()).into_owned(),
        }
    }

    /// Abre `GET /v1/events` e devolve o leitor do stream.
    pub async fn open_events(&self, last_event_id: Option<u64>) -> EventReader {
        let (mut sender, connection) = connect(&self.socket).await;

        let mut builder = Request::builder()
            .method(Method::GET)
            .uri("/v1/events")
            .header(HOST, "elara.localhost")
            .header(ACCEPT, "text/event-stream")
            .header(PROTOCOL_HEADER, "1");
        if let Some(sequence) = last_event_id {
            builder = builder.header("last-event-id", sequence.to_string());
        }

        let request = builder.body(Full::new(Bytes::new())).expect("requisição válida");
        let response = sender.send_request(request).await.expect("stream aberto");
        assert_eq!(response.status(), StatusCode::OK, "o stream de eventos precisa abrir com 200");

        EventReader { body: response.into_body(), buffer: Vec::new(), _sender: sender, connection }
    }

    /// Manda o daemon parar e roda o desligamento ordenado de verdade.
    pub async fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send("teste pediu parada".to_owned());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
        if let Some(recorder) = self.recorder.take() {
            server::shutdown(&self.state, &self.socket, recorder).await;
        }
    }
}

impl Drop for Daemon {
    /// Um teste que esquece de chamar `stop` não pode deixar uma task servindo
    /// um socket que o `TempDir` acabou de apagar.
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        if let Some(recorder) = self.recorder.take() {
            recorder.abort();
        }
    }
}

/// Resposta de uma requisição de teste, já com o corpo lido.
#[derive(Debug)]
pub struct TestResponse {
    pub status: StatusCode,
    /// Valor de `x-elara-protocol` na resposta, se houver.
    pub protocol_header: Option<String>,
    pub body: String,
}

impl TestResponse {
    /// Corpo desserializado, falhando com o texto original quando não casa.
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_str(&self.body).unwrap_or_else(|error| {
            panic!("corpo fora do contrato ({error}): {}", self.body);
        })
    }

    /// Corpo interpretado como erro do contrato.
    pub fn error(&self) -> ApiError {
        self.json()
    }
}

/// Leitor do stream SSE.
#[derive(Debug)]
pub struct EventReader {
    body: Incoming,
    buffer: Vec<u8>,
    /// Mantém a requisição viva: sem ele o corpo para de receber frames.
    _sender: hyper::client::conn::http1::SendRequest<Full<Bytes>>,
    connection: JoinHandle<()>,
}

impl EventReader {
    /// Próximo frame com dados, ou `None` quando o daemon fecha o stream.
    ///
    /// Comentários de keep-alive são consumidos e não viram frame: eles existem
    /// para manter a conexão, não para o cliente reagir.
    pub async fn next_frame(&mut self) -> Option<SseFrame> {
        let mut frame = SseFrame::default();

        loop {
            while let Some(line) = self.take_line() {
                if line.is_empty() {
                    if frame.data.is_empty() {
                        continue;
                    }
                    return Some(std::mem::take(&mut frame));
                }
                if let Some(rest) = line.strip_prefix("id:") {
                    frame.id = Some(rest.trim().to_owned());
                } else if let Some(rest) = line.strip_prefix("event:") {
                    frame.event = Some(rest.trim().to_owned());
                } else if let Some(rest) = line.strip_prefix("data:") {
                    if !frame.data.is_empty() {
                        frame.data.push('\n');
                    }
                    frame.data.push_str(rest.strip_prefix(' ').unwrap_or(rest));
                }
            }

            match self.body.frame().await {
                Some(Ok(chunk)) => {
                    if let Ok(data) = chunk.into_data() {
                        self.buffer.extend_from_slice(data.as_ref());
                    }
                }
                Some(Err(_)) | None => return None,
            }
        }
    }

    /// Espera um frame com prazo, para um teste falhar em vez de travar.
    pub async fn next_frame_within(&mut self, timeout: Duration) -> Option<SseFrame> {
        tokio::time::timeout(timeout, self.next_frame()).await.unwrap_or_else(|_| {
            panic!("nenhum evento SSE chegou em {} ms", timeout.as_millis());
        })
    }

    /// Extrai a próxima linha completa do buffer.
    fn take_line(&mut self) -> Option<String> {
        let end = self.buffer.iter().position(|byte| *byte == b'\n')?;
        let raw: Vec<u8> = self.buffer.drain(..=end).collect();
        Some(String::from_utf8_lossy(&raw).trim_end_matches(['\n', '\r']).to_owned())
    }
}

impl Drop for EventReader {
    fn drop(&mut self) {
        self.connection.abort();
    }
}

/// Um evento SSE já montado.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SseFrame {
    pub id: Option<String>,
    pub event: Option<String>,
    pub data: String,
}

impl SseFrame {
    /// Carga do evento desserializada.
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_str(&self.data)
            .unwrap_or_else(|error| panic!("evento fora do contrato ({error}): {}", self.data))
    }
}

/// Abre uma conexão HTTP/1.1 sobre o socket.
///
/// A conexão do `hyper` é um futuro próprio: sem alguém puxando-a, nenhum byte
/// trafega, por isso ela vai para uma task e o handle volta junto.
async fn connect(
    socket: &Path,
) -> (hyper::client::conn::http1::SendRequest<Full<Bytes>>, JoinHandle<()>) {
    let stream = UnixStream::connect(socket)
        .await
        .unwrap_or_else(|error| panic!("conexão com {}: {error}", socket.display()));

    let (sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
        .await
        .expect("handshake HTTP/1.1");

    let task = tokio::spawn(async move {
        let _ = connection.await;
    });

    (sender, task)
}
