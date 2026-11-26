//! Testes de integração do daemon.
//!
//! Cada teste sobe o `agentd` inteiro num socket Unix temporário e fala HTTP/1.1
//! com ele — a mesma coisa que o `@elara/sdk` e a ponte do Tauri fazem em
//! produção. O único substituto é o provider de hardware.
//!
//! Ficam dentro do crate, e não em `tests/`, por um motivo concreto: `agentd` é
//! um binário, não uma biblioteca, e um teste de `tests/` não conseguiria montar
//! o estado do daemon nem chamar `server::serve`. O acesso ao interior também é
//! útil — vários testes conferem o efeito de uma requisição no banco e no
//! barramento, não só o corpo da resposta.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "em teste, um panic é exatamente o relatório de falha desejado"
)]

use std::os::unix::fs::PermissionsExt as _;
use std::sync::Arc;
use std::time::Duration;

use elara_hardware::{HardwareError, MockHardwareProvider};
use elara_telemetry::types::{ComponentState, Event, EventEnvelope};
use hyper::{Method, StatusCode};
use serde_json::json;

use crate::api::types::{
    DaemonStatus, HardwareHistoryEntry, HardwareResponse, HealthResponse, ResourcesResponse,
    RuntimeHeartbeatResponse, RuntimeRegisterResponse,
};
use crate::state::{DATABASE_COMPONENT, HARDWARE_COMPONENT, TELEMETRY_COMPONENT};
use crate::supervisor::RUNTIME_COMPONENT;
use crate::testing::Daemon;

/// Prazo para um evento SSE chegar. Generoso porque a máquina de CI pode estar
/// carregada; um teste que trava é pior que um teste lento.
const SSE_TIMEOUT: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// Saúde e status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn health_responde_a_versao_e_o_protocolo() {
    let mut daemon = Daemon::start().await;

    let response = daemon.get("/v1/health").await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.protocol_header.as_deref(), Some("1"));

    let body: HealthResponse = response.json();
    assert!(body.ok);
    assert_eq!(body.protocol_version, 1);
    assert_eq!(body.version, env!("CARGO_PKG_VERSION"));

    daemon.stop().await;
}

#[tokio::test]
async fn status_lista_todos_os_componentes_e_os_caminhos() {
    let mut daemon = Daemon::start().await;

    let body: DaemonStatus = daemon.get("/v1/status").await.json();
    assert_eq!(body.pid, std::process::id());
    assert_eq!(body.socket_path, daemon.socket.display().to_string());
    assert_eq!(body.data_dir, daemon.state.config().data_dir.display().to_string());

    let names: Vec<&str> = body.components.iter().map(|status| status.name.as_str()).collect();
    assert_eq!(
        names,
        vec![RUNTIME_COMPONENT, DATABASE_COMPONENT, HARDWARE_COMPONENT, TELEMETRY_COMPONENT]
    );

    let by_name = |name: &str| {
        body.components.iter().find(|status| status.name == name).expect("componente").clone()
    };
    assert_eq!(by_name(HARDWARE_COMPONENT).state, ComponentState::Ready);
    assert_eq!(by_name(DATABASE_COMPONENT).state, ComponentState::Ready);
    assert_eq!(by_name(TELEMETRY_COMPONENT).state, ComponentState::Ready);

    // Sem ELARA_RUNTIME_CMD o componente é indisponível, e isso não é erro.
    let runtime = by_name(RUNTIME_COMPONENT);
    assert_eq!(runtime.state, ComponentState::Unavailable);
    assert!(runtime.detail.unwrap_or_default().contains("ELARA_RUNTIME_CMD"));

    daemon.stop().await;
}

#[tokio::test]
async fn status_reflete_um_agent_runtime_supervisionado() {
    let mut daemon =
        Daemon::start_with(Arc::new(MockHardwareProvider::new()), Some("/bin/sleep 120")).await;

    let body: DaemonStatus = daemon.get("/v1/status").await.json();
    let runtime = body
        .components
        .iter()
        .find(|status| status.name == RUNTIME_COMPONENT)
        .expect("componente do runtime");

    assert_eq!(runtime.state, ComponentState::Ready);
    assert_eq!(runtime.restarts, 0);

    daemon.stop().await;
}

// ---------------------------------------------------------------------------
// Hardware
// ---------------------------------------------------------------------------

#[tokio::test]
async fn hardware_vem_do_cache_e_o_refresh_redetecta() {
    let mut daemon = Daemon::start().await;

    let cached: HardwareResponse = daemon.get("/v1/hardware").await.json();
    assert!(cached.cached, "GET /v1/hardware tem de servir do cache");
    assert_eq!(cached.profile, MockHardwareProvider::reference_profile());

    let fresh: HardwareResponse = daemon.post("/v1/hardware/refresh", None).await.json();
    assert!(!fresh.cached, "o refresh redetecta e não pode dizer que veio do cache");
    assert_eq!(fresh.profile.fingerprint, cached.profile.fingerprint);
    assert_eq!(
        fresh.first_seen_at, cached.first_seen_at,
        "o mesmo hardware não pode ganhar um primeiro avistamento novo"
    );

    daemon.stop().await;
}

#[tokio::test]
async fn sem_hardware_detectavel_o_daemon_sobe_e_responde_503() {
    let provider = MockHardwareProvider::failing(HardwareError::Io {
        path: "/proc/cpuinfo".to_owned(),
        source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "sem permissão"),
    });
    let mut daemon = Daemon::start_with(Arc::new(provider), None).await;

    // O daemon está de pé: é justamente o ponto.
    assert_eq!(daemon.get("/v1/health").await.status, StatusCode::OK);

    let response = daemon.get("/v1/hardware").await;
    assert_eq!(response.status, StatusCode::SERVICE_UNAVAILABLE);
    let error = response.error();
    assert_eq!(error.code, "hardware.unavailable");
    assert!(error.details.unwrap_or_default().contains("/v1/status"));

    // E o /v1/status explica o motivo em vez de fingir que está tudo bem.
    let status: DaemonStatus = daemon.get("/v1/status").await.json();
    let hardware = status
        .components
        .iter()
        .find(|component| component.name == HARDWARE_COMPONENT)
        .expect("componente de hardware");
    assert_eq!(hardware.state, ComponentState::Failed);
    assert!(hardware.detail.clone().unwrap_or_default().contains("/proc/cpuinfo"));

    let refresh = daemon.post("/v1/hardware/refresh", None).await;
    assert_eq!(refresh.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(refresh.error().code, "hardware.unavailable");

    daemon.stop().await;
}

#[tokio::test]
async fn historico_traz_o_perfil_persistido_na_subida() {
    let mut daemon = Daemon::start().await;

    let history: Vec<HardwareHistoryEntry> = daemon.get("/v1/hardware/history").await.json();
    assert_eq!(history.len(), 1);

    let profile = MockHardwareProvider::reference_profile();
    assert_eq!(history[0].fingerprint, profile.fingerprint);
    assert_eq!(history[0].cpu_model, profile.cpu.model);
    assert_eq!(history[0].ram_total_mb, profile.ram.total_mb);
    assert!(history[0].gpu_summary.contains("RTX"));

    daemon.stop().await;
}

#[tokio::test]
async fn historico_valida_o_limite() {
    let mut daemon = Daemon::start().await;

    assert_eq!(daemon.get("/v1/hardware/history?limit=1").await.status, StatusCode::OK);

    let zero = daemon.get("/v1/hardware/history?limit=0").await;
    assert_eq!(zero.status, StatusCode::BAD_REQUEST);
    assert_eq!(zero.error().code, "request.invalid");

    let negativo = daemon.get("/v1/hardware/history?limit=-1").await;
    assert_eq!(negativo.status, StatusCode::BAD_REQUEST);
    assert_eq!(negativo.error().code, "request.invalid");

    // Acima do teto o pedido é atendido, não recusado.
    assert_eq!(daemon.get("/v1/hardware/history?limit=100000").await.status, StatusCode::OK);

    daemon.stop().await;
}

// ---------------------------------------------------------------------------
// Recursos
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recursos_anunciam_o_intervalo_e_entregam_um_snapshot() {
    let mut daemon = Daemon::start().await;

    let imediato: ResourcesResponse = daemon.get("/v1/resources").await.json();
    assert_eq!(
        imediato.sample_interval_ms,
        u64::try_from(crate::testing::TEST_SAMPLE_INTERVAL.as_millis()).unwrap()
    );

    // O primeiro tick só coleta a linha de base; o snapshot sai no segundo.
    let snapshot = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let body: ResourcesResponse = daemon.get("/v1/resources").await.json();
            if let Some(snapshot) = body.snapshot {
                return snapshot;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("o amostrador precisa produzir um snapshot");

    // O mock avança 50 jiffies ocupados e 50 ociosos por amostra: 50% exatos.
    assert!(
        (snapshot.cpu.total_percent - 50.0).abs() < 0.01,
        "veio {}",
        snapshot.cpu.total_percent
    );
    assert_eq!(snapshot.cpu.per_core_percent.len(), 16);
    assert_eq!(snapshot.memory.total_mb, 32_768);
    assert_eq!(snapshot.gpus.len(), 1);
    assert_eq!(snapshot.gpus[0].name, "NVIDIA GeForce RTX 4060 Ti");

    daemon.stop().await;
}

// ---------------------------------------------------------------------------
// Registro do agent-runtime
// ---------------------------------------------------------------------------

#[tokio::test]
async fn registro_e_heartbeat_completam_o_ciclo() {
    let mut daemon = Daemon::start().await;

    let registro: RuntimeRegisterResponse = daemon
        .post(
            "/v1/runtime/register",
            Some(json!({
                "name": "agent-runtime",
                "version": "0.1.0",
                "pid": 4242,
                "protocolVersion": 1,
                "capabilities": []
            })),
        )
        .await
        .json();

    assert_eq!(registro.heartbeat_interval_ms, 5_000);
    assert_eq!(registro.daemon_version, env!("CARGO_PKG_VERSION"));
    assert!(!registro.session_id.is_empty());

    let batida: RuntimeHeartbeatResponse = daemon
        .post(
            "/v1/runtime/heartbeat",
            Some(json!({ "sessionId": registro.session_id, "healthy": true, "detail": null })),
        )
        .await
        .json();
    assert!(batida.acknowledged);
    assert!(!batida.shutdown_requested);

    // Um runtime registrado por fora aparece no /v1/status.
    let status: DaemonStatus = daemon.get("/v1/status").await.json();
    let runtime = status
        .components
        .iter()
        .find(|component| component.name == RUNTIME_COMPONENT)
        .expect("componente do runtime");
    assert_eq!(runtime.state, ComponentState::Ready);
    assert!(runtime.detail.clone().unwrap_or_default().contains("registrado externamente"));

    daemon.stop().await;
}

#[tokio::test]
async fn heartbeat_de_sessao_desconhecida_responde_200_pedindo_novo_registro() {
    let mut daemon = Daemon::start().await;

    let response = daemon
        .post(
            "/v1/runtime/heartbeat",
            Some(
                json!({ "sessionId": "sessao-que-nunca-existiu", "healthy": true, "detail": null }),
            ),
        )
        .await;

    // 200 e não 4xx: o agent-runtime trata qualquer 4xx como fatal, e "o daemon
    // reiniciou" não pode matar o runtime.
    assert_eq!(response.status, StatusCode::OK);
    let body: RuntimeHeartbeatResponse = response.json();
    assert!(!body.acknowledged);

    daemon.stop().await;
}

#[tokio::test]
async fn heartbeat_doente_precisa_de_motivo() {
    let mut daemon = Daemon::start().await;

    let registro: RuntimeRegisterResponse = daemon
        .post(
            "/v1/runtime/register",
            Some(json!({
                "name": "agent-runtime", "version": "0.1.0", "pid": 7,
                "protocolVersion": 1, "capabilities": []
            })),
        )
        .await
        .json();

    let sem_motivo = daemon
        .post(
            "/v1/runtime/heartbeat",
            Some(json!({ "sessionId": registro.session_id, "healthy": false, "detail": null })),
        )
        .await;
    assert_eq!(sem_motivo.status, StatusCode::BAD_REQUEST);
    assert_eq!(sem_motivo.error().code, "request.invalid");

    let com_motivo: RuntimeHeartbeatResponse = daemon
        .post(
            "/v1/runtime/heartbeat",
            Some(json!({
                "sessionId": registro.session_id,
                "healthy": false,
                "detail": "modelo não carregou"
            })),
        )
        .await
        .json();
    assert!(com_motivo.acknowledged);

    let status: DaemonStatus = daemon.get("/v1/status").await.json();
    let runtime = status
        .components
        .iter()
        .find(|component| component.name == RUNTIME_COMPONENT)
        .expect("componente do runtime");
    assert_eq!(runtime.state, ComponentState::Degraded);
    assert!(runtime.detail.clone().unwrap_or_default().contains("modelo não carregou"));

    daemon.stop().await;
}

#[tokio::test]
async fn runtime_de_outro_protocolo_e_recusado() {
    let mut daemon = Daemon::start().await;

    let response = daemon
        .post(
            "/v1/runtime/register",
            Some(json!({
                "name": "agent-runtime", "version": "9.0.0", "pid": 1,
                "protocolVersion": 99, "capabilities": []
            })),
        )
        .await;

    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.error().code, "protocol.version_mismatch");

    daemon.stop().await;
}

// ---------------------------------------------------------------------------
// Contrato de transporte
// ---------------------------------------------------------------------------

#[tokio::test]
async fn versao_de_protocolo_divergente_no_header_e_recusada() {
    let mut daemon = Daemon::start().await;

    let response = daemon.request(Method::GET, "/v1/health", None, Some("2")).await;
    assert_eq!(response.status, StatusCode::BAD_REQUEST);
    assert_eq!(response.error().code, "protocol.version_mismatch");
    assert_eq!(
        response.protocol_header.as_deref(),
        Some("1"),
        "mesmo ao recusar, o daemon precisa anunciar a própria versão"
    );

    daemon.stop().await;
}

#[tokio::test]
async fn header_de_protocolo_ausente_e_tolerado() {
    let mut daemon = Daemon::start().await;

    let response = daemon.request(Method::GET, "/v1/health", None, None).await;
    assert_eq!(response.status, StatusCode::OK);
    assert_eq!(response.protocol_header.as_deref(), Some("1"));

    daemon.stop().await;
}

#[tokio::test]
async fn rota_inexistente_e_metodo_errado_saem_no_formato_do_contrato() {
    let mut daemon = Daemon::start().await;

    let inexistente = daemon.get("/v1/nao-existe").await;
    assert_eq!(inexistente.status, StatusCode::NOT_FOUND);
    assert_eq!(inexistente.error().code, "route.not_found");
    assert_eq!(inexistente.protocol_header.as_deref(), Some("1"));

    let metodo = daemon.post("/v1/health", None).await;
    assert_eq!(metodo.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(metodo.error().code, "route.method_not_allowed");

    daemon.stop().await;
}

#[tokio::test]
async fn corpo_invalido_e_corpo_gigante_nao_derrubam_o_daemon() {
    let mut daemon = Daemon::start().await;

    let quebrado = daemon
        .request(
            Method::POST,
            "/v1/runtime/register",
            Some("{isto não é json".to_owned()),
            Some("1"),
        )
        .await;
    assert_eq!(quebrado.status, StatusCode::BAD_REQUEST);
    assert_eq!(quebrado.error().code, "request.invalid");

    let faltando =
        daemon.post("/v1/runtime/register", Some(json!({ "name": "agent-runtime" }))).await;
    assert_eq!(faltando.status, StatusCode::BAD_REQUEST);
    assert_eq!(faltando.error().code, "request.invalid");

    // 1 MiB contra o teto de 64 KiB.
    let gigante = json!({
        "name": "x".repeat(1024 * 1024),
        "version": "0.1.0", "pid": 1, "protocolVersion": 1, "capabilities": []
    });
    let resposta = daemon.post("/v1/runtime/register", Some(gigante)).await;
    assert!(
        resposta.status == StatusCode::PAYLOAD_TOO_LARGE
            || resposta.status == StatusCode::BAD_REQUEST,
        "corpo gigante precisa ser recusado, veio {}",
        resposta.status
    );

    // E o daemon continua atendendo.
    assert_eq!(daemon.get("/v1/health").await.status, StatusCode::OK);

    daemon.stop().await;
}

// ---------------------------------------------------------------------------
// Stream de eventos
// ---------------------------------------------------------------------------

#[tokio::test]
async fn o_stream_entrega_eventos_ao_vivo_com_id_e_nome() {
    let mut daemon = Daemon::start().await;
    let mut stream = daemon.open_events(None).await;

    // Um evento previsível e sob controle do teste: o refresh publica
    // `hardware.detected`. Cada requisição abre a própria conexão, então a do
    // stream continua aberta em paralelo.
    assert_eq!(daemon.post("/v1/hardware/refresh", None).await.status, StatusCode::OK);

    let frame = loop {
        let frame = stream.next_frame_within(SSE_TIMEOUT).await.expect("stream aberto");
        if frame.event.as_deref() == Some("hardware.detected") {
            break frame;
        }
    };

    assert!(frame.id.is_some(), "o frame precisa carregar o id para o Last-Event-ID");
    let envelope: EventEnvelope = frame.json();
    assert_eq!(frame.id.as_deref(), Some(envelope.sequence.to_string().as_str()));
    assert!(matches!(envelope.payload, Event::HardwareDetected { .. }));

    daemon.stop().await;
}

#[tokio::test]
async fn last_event_id_reenvia_o_que_ficou_para_tras() {
    let mut daemon = Daemon::start().await;

    // A subida já publicou `hardware.detected` (1) e `daemon.started` (2).
    let recentes = daemon.state.bus().recent(10);
    assert!(recentes.len() >= 2, "a subida precisa ter publicado eventos");
    let primeiro = recentes.last().expect("evento mais antigo").sequence;

    let mut stream = daemon.open_events(Some(primeiro)).await;
    let frame = stream.next_frame_within(SSE_TIMEOUT).await.expect("reenvio do histórico");
    let envelope: EventEnvelope = frame.json();

    assert_eq!(envelope.sequence, primeiro + 1, "o reenvio começa no evento seguinte ao informado");

    daemon.stop().await;
}

#[tokio::test]
async fn o_stream_se_fecha_no_desligamento() {
    let mut daemon = Daemon::start().await;
    let mut stream = daemon.open_events(None).await;

    daemon.stop().await;

    // O cliente recebe o `daemon.stopping` e depois o fim do stream — em vez de
    // ficar pendurado numa conexão que ninguém mais atende.
    let mut viu_parada = false;
    while let Some(frame) = stream.next_frame_within(SSE_TIMEOUT).await {
        if frame.event.as_deref() == Some("daemon.stopping") {
            viu_parada = true;
        }
    }
    assert!(viu_parada, "o desligamento precisa ser anunciado antes de o stream fechar");
}

// ---------------------------------------------------------------------------
// Ciclo de vida do socket
// ---------------------------------------------------------------------------

#[tokio::test]
async fn o_socket_e_criado_com_modo_0600() {
    let mut daemon = Daemon::start().await;

    let mode = std::fs::metadata(&daemon.socket).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "o socket precisa ser 0600, veio {mode:o}");

    daemon.stop().await;
}

#[tokio::test]
async fn o_desligamento_remove_o_arquivo_de_socket() {
    let mut daemon = Daemon::start().await;
    assert!(daemon.socket.exists());

    daemon.stop().await;
    assert!(!daemon.socket.exists(), "um socket órfão faz a próxima subida parecer ocupada");
}

#[tokio::test]
async fn o_daemon_sobe_por_cima_de_um_socket_orfao() {
    let mut primeiro = Daemon::start().await;
    let socket = primeiro.socket.clone();

    // Simula um SIGKILL: nada de desligamento ordenado, o arquivo fica para trás.
    let dir = primeiro.abandon().await;
    drop(primeiro);
    assert!(socket.exists(), "o socket precisa sobreviver ao processo morto");

    let mut segundo = Daemon::start_at(dir, Arc::new(MockHardwareProvider::new()), None).await;
    assert_eq!(segundo.socket, socket, "o segundo daemon usa o mesmo caminho");
    assert_eq!(segundo.get("/v1/health").await.status, StatusCode::OK);

    segundo.stop().await;
}

// ---------------------------------------------------------------------------
// Persistência
// ---------------------------------------------------------------------------

#[tokio::test]
async fn os_eventos_da_subida_e_do_desligamento_ficam_no_banco() {
    let mut daemon = Daemon::start().await;
    daemon.stop().await;

    let connection =
        rusqlite::Connection::open(&daemon.state.config().database_path).expect("banco");
    let mut statement =
        connection.prepare("SELECT kind FROM events ORDER BY id").expect("consulta");
    let kinds: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("linhas")
        .collect::<Result<_, _>>()
        .expect("tipos");

    assert!(kinds.contains(&"hardware.detected".to_owned()), "eventos: {kinds:?}");
    assert!(kinds.contains(&"daemon.started".to_owned()), "eventos: {kinds:?}");
    assert!(kinds.contains(&"daemon.stopping".to_owned()), "eventos: {kinds:?}");
    assert!(
        !kinds.contains(&"resources.sampled".to_owned()),
        "amostras de recurso não podem entupir a tabela: {kinds:?}"
    );
}

#[tokio::test]
async fn a_subida_registra_a_versao_e_o_instante_nas_configuracoes() {
    let mut daemon = Daemon::start().await;

    assert_eq!(
        daemon.state.database().setting("daemon.version").await.unwrap().as_deref(),
        Some(env!("CARGO_PKG_VERSION"))
    );
    let started = daemon
        .state
        .database()
        .setting("daemon.last_started_at")
        .await
        .unwrap()
        .expect("instante da subida");
    assert!(started.parse::<jiff::Timestamp>().is_ok(), "instante ilegível: {started}");

    daemon.stop().await;
}
