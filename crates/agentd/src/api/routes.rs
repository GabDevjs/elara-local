//! Rotas HTTP do daemon.
//!
//! A superfície é pequena e fixa: nove rotas, todas sob `/v1`. O SDK
//! (`packages/sdk`) e a ponte do desktop já foram escritos contra elas, então
//! caminho, método e forma do corpo são contrato — o lugar de mudar qualquer um
//! dos três é `api/types.rs`, junto com as definições TypeScript geradas.
//!
//! Três decisões atravessam o arquivo:
//!
//! - **Todo erro sai como [`ApiError`](super::types::ApiError).** Inclusive os
//!   que o `axum` geraria sozinho: rota inexistente, método errado e corpo
//!   ilegível passam pelos fallbacks daqui para que o cliente nunca receba um
//!   `text/plain` que o zod não sabe interpretar.
//! - **Toda resposta anuncia o protocolo.** O `AgentdClient` compara o header
//!   `x-elara-protocol` com o dele e recusa um daemon incompatível antes de
//!   tentar interpretar o corpo.
//! - **Nada de bloqueante dentro de um handler.** Hardware vai para
//!   `spawn_blocking` (ver [`crate::hardware`]) e `SQLite` também (ver
//!   [`crate::db`]).

use std::convert::Infallible;
use std::sync::LazyLock;
use std::time::Duration;

use axum::Json;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, Query, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue, Uri};
use axum::middleware::{self, Next};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use elara_telemetry::types::EventEnvelope;
use serde::Deserialize;
use tokio_stream::wrappers::errors::BroadcastStreamRecvError;
use tokio_stream::wrappers::{BroadcastStream, WatchStream};
use tokio_stream::{Stream, StreamExt as _};
use tower_http::trace::TraceLayer;

use super::error::{ApiErrorKind, ApiFailure};
use super::types::{
    DaemonStatus, HardwareHistoryEntry, HardwareResponse, HealthResponse, ResourcesResponse,
    RuntimeHeartbeatRequest, RuntimeHeartbeatResponse, RuntimeRegisterRequest,
    RuntimeRegisterResponse,
};
use super::{ApiResult, PROTOCOL_HEADER, PROTOCOL_VERSION};
use crate::hardware::{self, RefreshError};
use crate::server::EVENT_BUS_CAPACITY;
use crate::state::AppState;

/// Teto do corpo de uma requisição.
///
/// O maior corpo legítimo do contrato é um `RuntimeRegisterRequest` com uma
/// lista de capacidades — centenas de bytes. 64 KiB dá três ordens de grandeza
/// de folga e ainda impede que um cliente com bug consuma a memória do daemon.
const MAX_BODY_BYTES: usize = 64 * 1024;

/// Intervalo entre comentários de keep-alive no stream SSE.
///
/// Sem tráfego, um proxy ou um `undici` com timeout fecharia a conexão de um
/// daemon perfeitamente saudável que apenas não teve nada a dizer.
const SSE_KEEPALIVE: Duration = Duration::from_secs(15);

/// Quantas entradas do histórico de hardware devolver quando o cliente não pede
/// um número.
const DEFAULT_HISTORY_LIMIT: u32 = 20;

/// Teto do histórico. Espelha o limite que a ponte do desktop já aplica.
const MAX_HISTORY_LIMIT: u32 = 200;

/// Nome do header de protocolo, validado uma vez por processo.
static PROTOCOL_HEADER_NAME: LazyLock<HeaderName> =
    LazyLock::new(|| HeaderName::from_static(PROTOCOL_HEADER));

/// Monta o roteador completo, com middlewares e fallbacks.
pub fn router(state: AppState) -> axum::Router {
    axum::Router::new()
        .route("/v1/health", get(health))
        .route("/v1/status", get(status))
        .route("/v1/hardware", get(hardware_current))
        .route("/v1/hardware/refresh", post(hardware_refresh))
        .route("/v1/hardware/history", get(hardware_history))
        .route("/v1/resources", get(resources))
        .route("/v1/events", get(events))
        .route("/v1/runtime/register", post(runtime_register))
        .route("/v1/runtime/heartbeat", post(runtime_heartbeat))
        .fallback(route_not_found)
        .method_not_allowed_fallback(method_not_allowed)
        // O limite é o do `axum` e não o `RequestBodyLimitLayer` do `tower-http`
        // porque este último troca o tipo do corpo (`Limited<Body>`) e só volta
        // a casar com o `Router` através do `MapRequestBodyLayer`, que exige a
        // feature `map-request-body` — não habilitada no workspace. O efeito é o
        // mesmo: todo corpo que este daemon lê passa por um extrator `Json`.
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(middleware::from_fn(protocol_middleware))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

// ---------------------------------------------------------------------------
// Middleware de protocolo
// ---------------------------------------------------------------------------

/// Recusa clientes de outra versão e anuncia a nossa em toda resposta.
///
/// A ausência do header é tolerada de propósito: um cliente novo ainda em
/// desenvolvimento, um `curl` de diagnóstico ou um proxy que remove headers
/// desconhecidos não deveriam receber um 400. A divergência declarada, sim — ela
/// é a única forma de dizer "atualize" antes de o corpo chegar embaralhado ao
/// zod do outro lado.
async fn protocol_middleware(request: Request, next: Next) -> Response {
    let mut response = match check_protocol(request.headers()) {
        Ok(()) => next.run(request).await,
        Err(failure) => failure.into_response(),
    };

    response
        .headers_mut()
        .insert(PROTOCOL_HEADER_NAME.clone(), HeaderValue::from(PROTOCOL_VERSION));

    response
}

/// Compara a versão anunciada pelo cliente com a deste daemon.
fn check_protocol(headers: &HeaderMap) -> Result<(), ApiFailure> {
    let Some(raw) = headers.get(&*PROTOCOL_HEADER_NAME) else {
        tracing::debug!("requisição sem o header {PROTOCOL_HEADER}");
        return Ok(());
    };

    let announced = raw.to_str().ok().map(str::trim).and_then(|value| value.parse::<u32>().ok());

    match announced {
        Some(PROTOCOL_VERSION) => Ok(()),
        Some(version) => Err(ApiFailure::new(
            ApiErrorKind::ProtocolVersionMismatch,
            format!(
                "este daemon fala a versão {PROTOCOL_VERSION} do protocolo; \
                 o cliente anunciou a versão {version}"
            ),
        )
        .with_details("atualize o cliente ou o daemon para que as versões coincidam")),
        None => Err(ApiFailure::new(
            ApiErrorKind::ProtocolVersionMismatch,
            format!("o header {PROTOCOL_HEADER} não contém um número de versão"),
        )
        .with_details(format!("envie {PROTOCOL_HEADER}: {PROTOCOL_VERSION}"))),
    }
}

// ---------------------------------------------------------------------------
// Saúde e status
// ---------------------------------------------------------------------------

/// `GET /v1/health` — barato o bastante para polling agressivo.
///
/// Não consulta banco nem hardware: a pergunta que responde é "há um daemon
/// atendendo neste socket?", e qualquer I/O aqui transformaria a sonda mais
/// usada da UI na mais cara.
async fn health(State(state): State<AppState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        ok: !state.is_shutting_down(),
        version: daemon_version(),
        protocol_version: PROTOCOL_VERSION,
        uptime_seconds: state.uptime_seconds(),
    })
}

/// `GET /v1/status` — visão completa do daemon e de seus componentes.
async fn status(State(state): State<AppState>) -> Json<DaemonStatus> {
    Json(DaemonStatus {
        version: daemon_version(),
        protocol_version: PROTOCOL_VERSION,
        pid: state.pid(),
        started_at: state.started_at(),
        uptime_seconds: state.uptime_seconds(),
        socket_path: state.config().socket_path.display().to_string(),
        data_dir: state.config().data_dir.display().to_string(),
        components: state.components(),
    })
}

// ---------------------------------------------------------------------------
// Hardware
// ---------------------------------------------------------------------------

/// `GET /v1/hardware` — perfil corrente, servido do cache em memória.
///
/// Não redetecta: a sonda custa dezenas de milissegundos e o perfil só muda
/// quando a máquina muda. Quem quer um número novo pede `refresh`.
async fn hardware_current(State(state): State<AppState>) -> ApiResult<Json<HardwareResponse>> {
    let cached = state.cached_profile().ok_or_else(|| {
        ApiFailure::new(
            ApiErrorKind::HardwareUnavailable,
            "o perfil de hardware desta máquina não foi detectado",
        )
        .with_details(
            "consulte /v1/status para ver por que a sonda falhou e tente \
             POST /v1/hardware/refresh",
        )
    })?;

    Ok(Json(HardwareResponse {
        profile: cached.profile,
        cached: true,
        first_seen_at: cached.first_seen_at,
    }))
}

/// `POST /v1/hardware/refresh` — força uma detecção nova.
async fn hardware_refresh(State(state): State<AppState>) -> ApiResult<Json<HardwareResponse>> {
    let cached = hardware::refresh(&state).await.map_err(|error| match error {
        RefreshError::Probe(_) => ApiFailure::new(
            ApiErrorKind::HardwareUnavailable,
            "a detecção de hardware falhou nesta máquina",
        )
        .with_details(error.user_detail())
        .with_internal(error),
        RefreshError::Persist(_) | RefreshError::TaskFailed => {
            ApiFailure::internal("detecção de hardware", error)
        }
    })?;

    Ok(Json(HardwareResponse {
        profile: cached.profile,
        cached: false,
        first_seen_at: cached.first_seen_at,
    }))
}

/// Parâmetros de `GET /v1/hardware/history`.
#[derive(Debug, Deserialize)]
struct HistoryQuery {
    /// Quantas entradas devolver. Ausente vale [`DEFAULT_HISTORY_LIMIT`].
    limit: Option<u32>,
}

/// `GET /v1/hardware/history` — perfis já vistos nesta máquina.
async fn hardware_history(
    State(state): State<AppState>,
    query: Result<Query<HistoryQuery>, QueryRejection>,
) -> ApiResult<Json<Vec<HardwareHistoryEntry>>> {
    let Query(query) = query.map_err(|rejection| {
        ApiFailure::invalid_request("o parâmetro limit precisa ser um inteiro sem sinal")
            .with_details(rejection.body_text())
    })?;

    let limit = match query.limit {
        None => DEFAULT_HISTORY_LIMIT,
        Some(0) => {
            return Err(ApiFailure::invalid_request("limit precisa ser maior que zero")
                .with_details(format!("faixa aceita: 1 a {MAX_HISTORY_LIMIT}")));
        }
        // Acima do teto o pedido é atendido com o teto, e não recusado: o
        // cliente que pediu "tudo" recebe o máximo que existe.
        Some(limit) => limit.min(MAX_HISTORY_LIMIT),
    };

    Ok(Json(state.database().hardware_history(limit).await?))
}

// ---------------------------------------------------------------------------
// Recursos
// ---------------------------------------------------------------------------

/// `GET /v1/resources` — última amostra já calculada.
///
/// `snapshot` é `None` na janela entre a subida e a segunda amostra: percentual
/// de CPU não existe em um instante, ele nasce da diferença entre duas leituras.
async fn resources(State(state): State<AppState>) -> Json<ResourcesResponse> {
    let interval = state.sampler().interval();

    Json(ResourcesResponse {
        snapshot: state.sampler().latest(),
        sample_interval_ms: u64::try_from(interval.as_millis()).unwrap_or(u64::MAX),
    })
}

// ---------------------------------------------------------------------------
// Eventos
// ---------------------------------------------------------------------------

/// `GET /v1/events` — stream SSE do barramento interno.
///
/// A assinatura acontece **antes** da leitura do histórico. Invertida, a ordem
/// deixaria uma janela em que um evento publicado entre as duas operações não
/// estaria nem no histórico lido nem no fluxo ao vivo — a lacuna mais difícil de
/// diagnosticar que existe, porque some quando a máquina está ociosa.
async fn events(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>> + Send + 'static> {
    let receiver = state.bus().subscribe();
    let resume_from = last_event_id(&headers);

    let replay = match resume_from {
        Some(sequence) => {
            let mut pending: Vec<EventEnvelope> = state
                .bus()
                .recent(EVENT_BUS_CAPACITY)
                .into_iter()
                .filter(|envelope| envelope.sequence > sequence)
                .collect();
            // `recent` vem do mais novo para o mais antigo; o stream tem de sair
            // em ordem cronológica.
            pending.reverse();
            pending
        }
        None => Vec::new(),
    };

    // Tudo que o histórico já entregou não pode sair de novo pelo fluxo ao vivo.
    let replayed_up_to = replay.last().map_or(resume_from.unwrap_or(0), |last| last.sequence);

    tracing::debug!(
        resume_from = resume_from.unwrap_or(0),
        replayed = replay.len(),
        "novo assinante do stream de eventos"
    );

    let live = BroadcastStream::new(receiver).filter_map(move |item| match item {
        Ok(envelope) if envelope.sequence > replayed_up_to => Some(Frame::Event(envelope)),
        Ok(_) => None,
        Err(BroadcastStreamRecvError::Lagged(missed)) => {
            // Não é motivo para fechar o stream: o cliente detecta a lacuna pelo
            // salto no `sequence` e decide sozinho se recarrega o estado.
            tracing::warn!(missed, "assinante SSE lento; eventos descartados");
            None
        }
    });

    // Sem este ramo, uma UI conectada seguraria o desligamento ordenado do
    // daemon para sempre: um stream SSE, por construção, nunca termina sozinho.
    let stopping = WatchStream::new(state.shutdown_signal())
        .filter(|shutting_down| *shutting_down)
        .map(|_| Frame::Stop);

    let stream = tokio_stream::iter(replay.into_iter().map(Frame::Event))
        .chain(live)
        .merge(stopping)
        .take_while(|frame| matches!(frame, Frame::Event(_)))
        .filter_map(|frame| match frame {
            Frame::Event(envelope) => to_sse_event(&envelope).map(Ok::<SseEvent, Infallible>),
            Frame::Stop => None,
        });

    Sse::new(stream).keep_alive(KeepAlive::new().interval(SSE_KEEPALIVE))
}

/// O que trafega no stream antes de virar SSE.
///
/// O `Stop` existe para que o desligamento entre na mesma fila dos eventos e
/// encerre o stream de forma ordenada, em vez de a conexão ser cortada no meio
/// de um frame.
#[derive(Debug)]
enum Frame {
    Event(EventEnvelope),
    Stop,
}

/// Converte um envelope em um frame SSE.
///
/// `id` é o número de sequência (é o que volta em `Last-Event-ID`), `event` é o
/// nome estável do evento e `data` é o envelope inteiro em JSON. Um envelope que
/// não serializa é descartado com log em vez de virar um `data: {}` que o zod do
/// cliente rejeitaria sem dizer por quê.
fn to_sse_event(envelope: &EventEnvelope) -> Option<SseEvent> {
    let event =
        SseEvent::default().id(envelope.sequence.to_string()).event(envelope.payload.name());

    match event.json_data(envelope) {
        Ok(event) => Some(event),
        Err(error) => {
            tracing::error!(
                sequence = envelope.sequence,
                event = envelope.payload.name(),
                %error,
                "evento não pôde ser serializado; descartado do stream"
            );
            None
        }
    }
}

/// Lê o `Last-Event-ID` que o cliente mandou para retomar de onde parou.
///
/// Valor ilegível é ignorado (o cliente recebe o fluxo ao vivo) em vez de virar
/// erro: perder o histórico é bem menos grave do que recusar a conexão de uma UI
/// que reconectou com um id corrompido.
fn last_event_id(headers: &HeaderMap) -> Option<u64> {
    let raw = headers.get("last-event-id")?.to_str().ok()?.trim();

    match raw.parse::<u64>() {
        Ok(sequence) => Some(sequence),
        Err(error) => {
            tracing::debug!(%error, raw, "Last-Event-ID ilegível; começando do fluxo ao vivo");
            None
        }
    }
}

// ---------------------------------------------------------------------------
// Registro do agent-runtime
// ---------------------------------------------------------------------------

/// `POST /v1/runtime/register` — anuncia um runtime ao daemon.
async fn runtime_register(
    State(state): State<AppState>,
    body: Result<Json<RuntimeRegisterRequest>, JsonRejection>,
) -> ApiResult<Json<RuntimeRegisterResponse>> {
    let Json(request) = body.map_err(|rejection| json_rejection(&rejection))?;

    if state.is_shutting_down() {
        return Err(ApiFailure::new(
            ApiErrorKind::ShuttingDown,
            "o daemon está encerrando e não aceita novos runtimes",
        )
        .with_details("tente de novo depois que o daemon reiniciar"));
    }

    if request.name.trim().is_empty() {
        return Err(ApiFailure::invalid_request("o runtime precisa se identificar com um nome"));
    }

    if request.protocol_version != PROTOCOL_VERSION {
        return Err(ApiFailure::new(
            ApiErrorKind::ProtocolVersionMismatch,
            format!(
                "este daemon fala a versão {PROTOCOL_VERSION} do protocolo; \
                 o runtime {} anunciou a versão {}",
                request.name, request.protocol_version
            ),
        )
        .with_details("atualize o agent-runtime ou o daemon"));
    }

    Ok(Json(state.register_runtime(&request, jiff::Timestamp::now())))
}

/// `POST /v1/runtime/heartbeat` — confirma que o runtime segue vivo.
///
/// Sessão desconhecida **não** é erro HTTP: responde 200 com
/// `acknowledged: false`, que é como o contrato manda o runtime se registrar de
/// novo. Um 4xx aqui seria fatal para ele (ver `rethrowIfFatal` em
/// `apps/agent-runtime/src/core/session.ts`) e transformaria "o daemon
/// reiniciou" em "o runtime morreu".
async fn runtime_heartbeat(
    State(state): State<AppState>,
    body: Result<Json<RuntimeHeartbeatRequest>, JsonRejection>,
) -> ApiResult<Json<RuntimeHeartbeatResponse>> {
    let Json(request) = body.map_err(|rejection| json_rejection(&rejection))?;

    if request.session_id.trim().is_empty() {
        return Err(ApiFailure::invalid_request("sessionId não pode ser vazio")
            .with_details("use o sessionId devolvido por POST /v1/runtime/register"));
    }

    // O contrato manda um detalhe junto com `healthy: false`; sem ele o
    // `/v1/status` mostraria um componente degradado sem dizer por quê.
    if !request.healthy && request.detail.as_deref().unwrap_or("").trim().is_empty() {
        return Err(ApiFailure::invalid_request(
            "um heartbeat com healthy: false precisa explicar o motivo em detail",
        ));
    }

    Ok(Json(state.heartbeat(&request, jiff::Timestamp::now())))
}

// ---------------------------------------------------------------------------
// Fallbacks
// ---------------------------------------------------------------------------

/// Nenhuma rota casa com o caminho pedido.
async fn route_not_found(uri: Uri) -> ApiFailure {
    ApiFailure::new(ApiErrorKind::RouteNotFound, "esta rota não existe neste daemon")
        .with_details(format!("caminho: {}", uri.path()))
}

/// A rota existe, mas não com este método.
async fn method_not_allowed(uri: Uri) -> ApiFailure {
    ApiFailure::new(ApiErrorKind::MethodNotAllowed, "método não permitido nesta rota")
        .with_details(format!("caminho: {}", uri.path()))
}

/// Traduz a recusa do extrator de JSON.
///
/// O texto que o `axum` produz — do tipo "campo `name` faltando na linha 1,
/// coluna 20" — é escrito para quem manda a requisição e não expõe nada do
/// daemon, então vai inteiro para `details`.
fn json_rejection(rejection: &JsonRejection) -> ApiFailure {
    ApiFailure::invalid_request("o corpo da requisição não casa com o contrato")
        .with_details(rejection.body_text())
}

/// Versão deste binário, anunciada em `/v1/health` e `/v1/status`.
fn daemon_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "em teste, um panic é o relatório de falha desejado"
)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                HeaderName::from_bytes(name.as_bytes()).expect("nome de header"),
                HeaderValue::from_str(value).expect("valor de header"),
            );
        }
        map
    }

    #[test]
    fn header_ausente_e_tolerado() {
        assert!(check_protocol(&HeaderMap::new()).is_ok());
    }

    #[test]
    fn versao_igual_passa() {
        assert!(check_protocol(&headers(&[(PROTOCOL_HEADER, "1")])).is_ok());
        assert!(check_protocol(&headers(&[(PROTOCOL_HEADER, " 1 ")])).is_ok());
    }

    #[test]
    fn versao_diferente_e_recusada_com_o_codigo_do_contrato() {
        let failure = check_protocol(&headers(&[(PROTOCOL_HEADER, "2")])).unwrap_err();
        assert_eq!(failure.kind(), ApiErrorKind::ProtocolVersionMismatch);
        assert_eq!(failure.kind().code(), "protocol.version_mismatch");
    }

    #[test]
    fn versao_ilegivel_tambem_e_recusada() {
        for valor in ["", "um", "1.0", "-1"] {
            let failure = check_protocol(&headers(&[(PROTOCOL_HEADER, valor)])).unwrap_err();
            assert_eq!(failure.kind(), ApiErrorKind::ProtocolVersionMismatch, "valor {valor:?}");
        }
    }

    #[test]
    fn last_event_id_le_o_numero_e_ignora_lixo() {
        assert_eq!(last_event_id(&headers(&[("last-event-id", "42")])), Some(42));
        assert_eq!(last_event_id(&headers(&[("last-event-id", " 7 ")])), Some(7));
        assert_eq!(last_event_id(&headers(&[("last-event-id", "abc")])), None);
        assert_eq!(last_event_id(&headers(&[("last-event-id", "-1")])), None);
        assert_eq!(last_event_id(&HeaderMap::new()), None);
    }

    /// Renderiza um frame exatamente como ele sairia no socket.
    async fn render(event: SseEvent) -> String {
        let response =
            Sse::new(tokio_stream::iter(vec![Ok::<SseEvent, Infallible>(event)])).into_response();
        let bytes =
            axum::body::to_bytes(response.into_body(), 64 * 1024).await.expect("corpo do stream");
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn o_frame_sse_carrega_id_nome_e_envelope() {
        let envelope = EventEnvelope {
            sequence: 12,
            emitted_at: jiff::Timestamp::UNIX_EPOCH,
            payload: elara_telemetry::types::Event::DaemonStopping {
                reason: "sinal recebido".to_owned(),
            },
        };

        let frame = to_sse_event(&envelope).expect("frame serializável");
        let rendered = render(frame).await;

        assert!(rendered.contains("id: 12"), "frame: {rendered}");
        assert!(rendered.contains("event: daemon.stopping"), "frame: {rendered}");
        assert!(rendered.contains(r#""sequence":12"#), "frame: {rendered}");
        assert!(rendered.contains("sinal recebido"), "frame: {rendered}");
    }
}
