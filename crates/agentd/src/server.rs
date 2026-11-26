//! Subida, operação e desligamento do daemon.
//!
//! A ordem de inicialização não é arbitrária — cada passo depende do anterior e
//! nenhum deles pode ser adiado para o primeiro cliente:
//!
//! 1. **logging**, porque tudo que vier depois precisa poder reclamar;
//! 2. **banco**, porque o perfil de hardware é persistido assim que sai;
//! 3. **hardware**, cuja falha vira componente `Failed` e evento — **o daemon
//!    não morre**: uma máquina sem `/proc` legível ainda precisa de um daemon de
//!    pé para explicar isso na tela;
//! 4. **amostrador** e **supervisor do agent-runtime**;
//! 5. **socket**, por último, porque o primeiro cliente a conectar já encontra
//!    tudo respondendo.
//!
//! Sobre a rede: **nenhuma porta TCP é aberta em momento algum**. O único ponto
//! de entrada é um socket Unix com permissão `0600` dentro de um diretório
//! `0700`, o que faz do controle de acesso um problema do sistema de arquivos —
//! e não um esquema de autenticação escrito à mão.

use std::future::{Future, IntoFuture as _};
use std::os::unix::fs::{FileTypeExt as _, PermissionsExt as _};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context as _;
use elara_hardware::{HardwareProvider, LinuxHardwareProvider};
use elara_telemetry::types::{ComponentState, Event};
use elara_telemetry::{EventBus, LoggingConfig, ResourceSampler};
use tokio::net::UnixListener;
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::oneshot;

use crate::api::routes;
use crate::config::Config;
use crate::db::Database;
use crate::hardware;
use crate::state::{AppState, AppStateParts, DATABASE_COMPONENT, TELEMETRY_COMPONENT};
use crate::supervisor::RuntimeSupervision;

/// Quantos eventos o barramento retém para novos assinantes.
///
/// É também o tamanho da janela que um cliente SSE consegue recuperar com
/// `Last-Event-ID` depois de uma reconexão. 256 cobre um desligamento e uma
/// subida inteiros com folga, sem prender memória relevante.
pub const EVENT_BUS_CAPACITY: usize = 256;

/// Permissão do socket: só o dono lê e escreve.
const SOCKET_MODE: u32 = 0o600;

/// Permissão do diretório do socket, quando somos nós que o criamos.
const SOCKET_DIR_MODE: u32 = 0o700;

/// Prazo para as conexões abertas terminarem depois do sinal de desligamento.
///
/// Existe porque um cliente pode estar no meio de uma resposta quando o sinal
/// chega. Streams SSE não dependem dele — eles se fecham sozinhos via
/// [`AppState::shutdown_signal`] —, então o prazo só cobre o caso patológico.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Chave em que o daemon registra a própria versão.
const VERSION_SETTING: &str = "daemon.version";

/// Chave com o instante da última subida bem-sucedida.
const LAST_STARTED_SETTING: &str = "daemon.last_started_at";

/// Ponto de entrada síncrono do binário.
///
/// Monta o runtime aqui e não com `#[tokio::main]` para que a configuração e o
/// logging existam **antes** do runtime: uma variável de ambiente inválida tem
/// de virar uma mensagem de erro legível, não um panic dentro de uma task.
pub fn run() -> anyhow::Result<()> {
    let config = Config::load().context("configuração inválida")?;

    let guard = elara_telemetry::logging::init(LoggingConfig {
        filter: config.log_filter.clone(),
        format: config.log_format,
        file: Some(config.log_file.clone()),
    })
    .with_context(|| format!("não foi possível iniciar o log em {}", config.log_file.display()))?;

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("agentd")
        .build()
        .context("não foi possível criar o runtime tokio")?;

    let result = runtime.block_on(run_daemon(config));

    // O `Drop` do guard também descarrega, mas um `abort` no meio do caminho
    // pularia o `Drop` — e é exatamente aí que as últimas linhas importam.
    guard.flush();
    result
}

/// Sobe tudo, serve até o sinal de parada e desmonta na ordem inversa.
async fn run_daemon(config: Config) -> anyhow::Result<()> {
    let socket_path = config.socket_path.clone();
    let provider: Arc<dyn HardwareProvider> = Arc::new(LinuxHardwareProvider::new());
    let (state, recorder) = bootstrap(config, provider).await?;

    let listener = bind_socket(&socket_path)?;

    state.bus().publish(Event::DaemonStarted {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        pid: state.pid(),
        socket_path: socket_path.display().to_string(),
    });
    tracing::info!(
        socket = %socket_path.display(),
        data_dir = %state.config().data_dir.display(),
        pid = state.pid(),
        "agentd pronto"
    );

    let result = serve(state.clone(), listener, wait_for_signal()).await;

    shutdown(&state, &socket_path, recorder).await;
    result.context("o servidor HTTP terminou com erro")
}

/// Monta o estado do daemon na ordem documentada no topo do módulo.
///
/// O `provider` entra por parâmetro para que o teste de integração suba
/// exatamente este caminho — mesma ordem, mesmo banco, mesmos eventos — com o
/// provider de fixture no lugar do que lê `/proc`.
pub(crate) async fn bootstrap(
    config: Config,
    provider: Arc<dyn HardwareProvider>,
) -> anyhow::Result<(AppState, tokio::task::JoinHandle<()>)> {
    // Abertura e migração são bloqueantes, mas acontecem antes de existir
    // qualquer conexão para atrasar — não há executor a proteger ainda.
    let database = Database::open(&config.database_path)
        .with_context(|| format!("banco em {}", config.database_path.display()))?;

    tracing::info!(database = %database.path().display(), "banco pronto");

    let bus = Arc::new(EventBus::new(EVENT_BUS_CAPACITY));

    // O gravador entra antes da detecção: o barramento só entrega a quem já
    // estava assinando, e `hardware.detected` é justamente o evento que um
    // diagnóstico posterior mais precisa.
    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let recorder = tokio::spawn(record_events(Arc::clone(&bus), database.clone(), shutdown_rx));

    let detection = hardware::detect_and_persist(&provider, &database, &bus).await;

    let sampler =
        ResourceSampler::new(Arc::clone(&provider), config.sample_interval).spawn(Arc::clone(&bus));
    let runtime = RuntimeSupervision::start(&config, Arc::clone(&bus));

    let sample_interval_ms = config.sample_interval.as_millis();
    let state = AppState::new(AppStateParts {
        config,
        hardware: provider,
        bus: Arc::clone(&bus),
        sampler,
        database: database.clone(),
        runtime,
        shutdown: shutdown_tx,
    });

    hardware::apply(&state, &detection);
    state.set_component(DATABASE_COMPONENT, ComponentState::Ready, None);
    state.set_component(
        TELEMETRY_COMPONENT,
        ComponentState::Ready,
        Some(format!("amostrando recursos a cada {sample_interval_ms} ms")),
    );

    let now = jiff::Timestamp::now();
    database.set_setting(VERSION_SETTING, env!("CARGO_PKG_VERSION"), now).await?;
    database.set_setting(LAST_STARTED_SETTING, &now.to_string(), now).await?;

    Ok((state, recorder))
}

/// Serve o `axum` sobre o socket até `trigger` resolver.
///
/// Recebe o gatilho como parâmetro para que um teste consiga desligar o daemon
/// sem mandar sinal para o processo que está rodando os testes.
pub(crate) async fn serve(
    state: AppState,
    listener: UnixListener,
    trigger: impl Future<Output = String> + Send + 'static,
) -> std::io::Result<()> {
    let app = routes::router(state.clone());

    // O sinal precisa chegar a dois lugares: ao `axum`, que para de aceitar
    // conexões, e ao resto do daemon, que precisa saber que está encerrando.
    let (drained_tx, drained_rx) = oneshot::channel();
    let shutdown_state = state.clone();
    let shutdown = async move {
        let reason = trigger.await;
        tracing::info!(reason, "iniciando desligamento ordenado");

        // Publicar antes de marcar: o gravador de eventos usa a marca como
        // sinal de parada e precisa ver este evento passar.
        if !shutdown_state.is_shutting_down() {
            shutdown_state.bus().publish(Event::DaemonStopping { reason });
        }
        shutdown_state.begin_shutdown();
        let _ = drained_tx.send(());
    };

    let served = axum::serve(listener, app).with_graceful_shutdown(shutdown).into_future();
    tokio::pin!(served);

    tokio::select! {
        result = &mut served => result,
        () = async move {
            let _ = drained_rx.await;
            tokio::time::sleep(DRAIN_TIMEOUT).await;
        } => {
            tracing::warn!(
                timeout_ms = DRAIN_TIMEOUT.as_millis(),
                "ainda havia conexão aberta no fim do prazo; encerrando mesmo assim"
            );
            Ok(())
        }
    }
}

/// Desmonta o daemon na ordem inversa da subida.
pub(crate) async fn shutdown(
    state: &AppState,
    socket_path: &Path,
    recorder: tokio::task::JoinHandle<()>,
) {
    state.sampler().shutdown().await;
    state.runtime().shutdown().await;

    // O gravador termina sozinho quando vê a marca de desligamento; o prazo
    // curto é só para o caso de uma escrita no `SQLite` estar em andamento.
    if tokio::time::timeout(Duration::from_secs(2), recorder).await.is_err() {
        tracing::warn!("o gravador de eventos não terminou no prazo");
    }

    // Um socket que sobrevive ao processo faz a próxima subida parecer "já tem
    // um daemon rodando" até alguém tentar conectar.
    match std::fs::remove_file(socket_path) {
        Ok(()) => tracing::info!(socket = %socket_path.display(), "socket removido"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(socket = %socket_path.display(), %error, "socket não pôde ser removido");
        }
    }

    tracing::info!("agentd encerrado");
}

/// Persiste no banco os eventos que não são de alta frequência.
///
/// Roda em uma task própria porque publicar no barramento precisa continuar
/// custando um `send` em memória: se `EventBus::publish` esperasse o `SQLite`,
/// uma escrita lenta pararia o supervisor de processos.
async fn record_events(
    bus: Arc<EventBus>,
    database: Database,
    mut stopping: tokio::sync::watch::Receiver<bool>,
) {
    let mut receiver = bus.subscribe();

    loop {
        tokio::select! {
            // `biased` para que, no instante do desligamento, os eventos já
            // publicados tenham prioridade sobre a marca de parada.
            biased;

            incoming = receiver.recv() => match incoming {
                Ok(envelope) => persist(&database, &envelope).await,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(missed)) => {
                    tracing::warn!(missed, "gravador de eventos ficou para trás");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            },

            changed = stopping.changed() => {
                if changed.is_err() || *stopping.borrow() {
                    break;
                }
            }
        }
    }

    // O `DaemonStopping` foi publicado logo antes da marca de parada; sem esta
    // drenagem ele seria justamente o evento que não sobrevive ao desligamento.
    while let Ok(envelope) = receiver.try_recv() {
        persist(&database, &envelope).await;
    }

    tracing::debug!("gravador de eventos encerrado");
}

/// Grava um evento, tratando a falha como um problema de diagnóstico e não de
/// operação: um `SQLite` cheio não pode derrubar o daemon.
async fn persist(database: &Database, envelope: &elara_telemetry::types::EventEnvelope) {
    if let Err(error) = database.record_event(envelope).await {
        tracing::warn!(
            sequence = envelope.sequence,
            event = envelope.payload.name(),
            %error,
            "evento não pôde ser persistido"
        );
    }
}

/// Espera SIGTERM ou SIGINT e devolve o nome do sinal recebido.
///
/// Um erro ao registrar os handlers é reportado e a espera vira infinita: nesse
/// caso o daemon continua servindo e o desligamento passa a depender de SIGKILL,
/// que é melhor do que recusar a subir por causa de um sinal.
async fn wait_for_signal() -> String {
    let mut terminate = match signal(SignalKind::terminate()) {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(%error, "SIGTERM não pôde ser observado");
            return std::future::pending().await;
        }
    };
    let mut interrupt = match signal(SignalKind::interrupt()) {
        Ok(stream) => stream,
        Err(error) => {
            tracing::error!(%error, "SIGINT não pôde ser observado");
            return std::future::pending().await;
        }
    };

    tokio::select! {
        _ = terminate.recv() => "SIGTERM".to_owned(),
        _ = interrupt.recv() => "SIGINT".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Socket
// ---------------------------------------------------------------------------

/// Falhas específicas de preparação do socket.
#[derive(Debug, thiserror::Error)]
pub enum SocketError {
    /// Já existe um daemon atendendo neste caminho.
    #[error(
        "já existe um daemon atendendo em {path}; \
         encerre-o ou aponte ELARA_SOCKET para outro caminho"
    )]
    AlreadyServing { path: String },

    /// O caminho existe e não é um socket.
    #[error("{path} existe e não é um socket; escolha outro caminho em ELARA_SOCKET")]
    NotASocket { path: String },

    /// Falha de sistema de arquivos ou de bind.
    #[error("{operation} falhou em {path}: {source}")]
    Io {
        operation: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
}

/// Prepara o diretório, limpa um socket órfão e faz o bind.
///
/// Precisa ser chamada de dentro de um runtime `tokio` (o listener se registra
/// no reator na criação).
pub(crate) fn bind_socket(path: &Path) -> Result<UnixListener, SocketError> {
    let display = || path.display().to_string();

    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        ensure_socket_dir(parent)?;
    }

    clear_stale_socket(path)?;

    let listener = UnixListener::bind(path).map_err(|source| SocketError::Io {
        operation: "bind",
        path: display(),
        source,
    })?;

    // A permissão é apertada logo depois do bind. A janela entre as duas
    // operações não é explorável porque o diretório que contém o socket já é
    // 0700 — quem não é o dono não consegue nem enxergar o arquivo.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(SOCKET_MODE)).map_err(
        |source| SocketError::Io { operation: "chmod do socket", path: display(), source },
    )?;

    Ok(listener)
}

/// Garante o diretório do socket com permissão restrita.
///
/// A permissão só é aplicada quando o diretório é criado por nós: um
/// `ELARA_SOCKET=/tmp/agentd.sock` faria o daemon deixar `/tmp` inacessível para
/// o resto do sistema se o `chmod` fosse incondicional.
fn ensure_socket_dir(directory: &Path) -> Result<(), SocketError> {
    let shown = directory.display().to_string();

    if directory.is_dir() {
        if let Ok(metadata) = std::fs::metadata(directory)
            && metadata.permissions().mode() & 0o077 != 0
        {
            tracing::warn!(
                dir = shown.as_str(),
                mode = format!("{:o}", metadata.permissions().mode() & 0o777),
                "o diretório do socket é acessível a outros usuários"
            );
        }
        return Ok(());
    }

    std::fs::create_dir_all(directory).map_err(|source| SocketError::Io {
        operation: "criação do diretório do socket",
        path: shown.clone(),
        source,
    })?;

    std::fs::set_permissions(directory, std::fs::Permissions::from_mode(SOCKET_DIR_MODE)).map_err(
        |source| SocketError::Io {
            operation: "chmod do diretório do socket", path: shown, source
        },
    )
}

/// Remove um socket órfão, mas nunca sequestra o de um daemon vivo.
///
/// A única forma honesta de distinguir os dois casos é tentar conectar: um
/// arquivo de socket sem ninguém do outro lado recusa a conexão, e é aí — e só
/// aí — que apagá-lo é seguro.
fn clear_stale_socket(path: &Path) -> Result<(), SocketError> {
    let shown = path.display().to_string();

    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(SocketError::Io {
                operation: "leitura do caminho do socket",
                path: shown,
                source,
            });
        }
    };

    if !metadata.file_type().is_socket() {
        return Err(SocketError::NotASocket { path: shown });
    }

    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => Err(SocketError::AlreadyServing { path: shown }),
        Err(error) => {
            tracing::warn!(
                socket = shown.as_str(),
                %error,
                "socket órfão de uma execução anterior; removendo"
            );
            std::fs::remove_file(path).map_err(|source| SocketError::Io {
                operation: "remoção do socket órfão",
                path: shown,
                source,
            })
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "em teste, um panic é o relatório de falha desejado"
)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn bind_cria_o_socket_com_modo_0600() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        let path = dir.path().join("elara").join("agentd.sock");

        let _listener = bind_socket(&path).expect("bind");

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, SOCKET_MODE, "o socket precisa ser 0600, veio {mode:o}");

        let dir_mode =
            std::fs::metadata(path.parent().unwrap()).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, SOCKET_DIR_MODE, "o diretório criado precisa ser 0700");
    }

    #[tokio::test]
    async fn socket_orfao_e_removido_e_o_bind_funciona() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        let path = dir.path().join("agentd.sock");

        // Um listener que morre deixa o arquivo para trás: é exatamente o que
        // sobra de um daemon morto por SIGKILL.
        {
            let _orphan = std::os::unix::net::UnixListener::bind(&path).expect("socket órfão");
        }
        assert!(path.exists(), "o arquivo tem de sobreviver ao listener");

        let _listener = bind_socket(&path).expect("bind sobre socket órfão");
        assert!(path.exists());
    }

    #[tokio::test]
    async fn socket_com_daemon_vivo_nao_e_sequestrado() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        let path = dir.path().join("agentd.sock");

        let _vivo = std::os::unix::net::UnixListener::bind(&path).expect("primeiro daemon");

        let error = bind_socket(&path).unwrap_err();
        assert!(matches!(error, SocketError::AlreadyServing { .. }), "erro inesperado: {error}");
    }

    #[tokio::test]
    async fn caminho_que_nao_e_socket_e_recusado_em_vez_de_apagado() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        let path = dir.path().join("arquivo-do-usuario");
        std::fs::write(&path, b"conteudo importante").expect("arquivo");

        let error = bind_socket(&path).unwrap_err();
        assert!(matches!(error, SocketError::NotASocket { .. }), "erro inesperado: {error}");
        assert!(path.exists(), "um arquivo que não é socket não pode ser apagado");
    }

    #[tokio::test]
    async fn diretorio_preexistente_nao_tem_a_permissao_alterada() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755))
            .expect("permissão inicial");

        let path = dir.path().join("agentd.sock");
        let _listener = bind_socket(&path).expect("bind");

        let mode = std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o755, "um diretório que já existia não pode ser reconfigurado");
    }
}
