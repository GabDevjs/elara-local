//! Estado compartilhado entre os handlers HTTP e as tasks de fundo.
//!
//! Tudo aqui vive atrás de um `Arc` e é clonado por referência: o `axum` exige
//! que o estado seja `Clone`, e clonar de verdade um perfil de hardware a cada
//! requisição seria caro sem motivo.
//!
//! Os locks são os síncronos da biblioteca padrão, e não os do `tokio`, porque
//! nenhuma seção crítica daqui contém um `await`: são cópias de struct e
//! inserções em mapa, na casa dos microssegundos. Um `tokio::sync::RwLock`
//! custaria um agendamento por leitura para resolver um problema que não existe.
//! Envenenamento é ignorado em toda parte — o estado protegido é observável, não
//! transacional, e derrubar o daemon porque uma thread entrou em pânico só
//! pioraria o diagnóstico.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::time::Duration;

use elara_hardware::{HardwareProfile, HardwareProvider};
use elara_telemetry::types::{ComponentState, ComponentStatus, Event};
use elara_telemetry::{EventBus, SamplerHandle};
use jiff::Timestamp;

use crate::api::types::{
    RuntimeHeartbeatRequest, RuntimeHeartbeatResponse, RuntimeRegisterRequest,
    RuntimeRegisterResponse,
};
use crate::config::Config;
use crate::db::Database;
use crate::supervisor::{RUNTIME_COMPONENT, RuntimeSupervision};

/// Nome do componente de detecção de hardware.
pub const HARDWARE_COMPONENT: &str = "hardware";

/// Nome do componente de persistência.
pub const DATABASE_COMPONENT: &str = "database";

/// Nome do componente de telemetria (event bus e amostrador).
pub const TELEMETRY_COMPONENT: &str = "telemetry";

/// Intervalo máximo entre heartbeats anunciado ao runtime no registro.
///
/// O runtime bate a metade disso (ver `apps/agent-runtime/src/core/session.ts`),
/// de modo que um atraso de agendamento não vira falso óbito.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// Quantos heartbeats podem faltar antes de a sessão ser considerada morta.
///
/// Três, e não um: uma máquina que hiberna por dois segundos ou um `pnpm` que
/// segura a CPU não podem derrubar um runtime saudável.
pub const MISSED_HEARTBEATS_BEFORE_DEAD: u32 = 3;

/// Perfil de hardware em cache, com a data do primeiro avistamento.
#[derive(Debug, Clone)]
pub struct CachedProfile {
    /// O perfil em si.
    pub profile: HardwareProfile,
    /// Quando este fingerprint foi visto pela primeira vez nesta máquina.
    pub first_seen_at: Timestamp,
}

/// Uma sessão de runtime registrada.
#[derive(Debug, Clone)]
pub struct RuntimeSession {
    /// Token opaco entregue no registro.
    pub id: String,
    /// Nome com que o runtime se identificou.
    pub name: String,
    pub version: String,
    pub pid: u32,
    pub registered_at: Timestamp,
    /// Instante do último heartbeat aceito.
    pub last_seen_at: Timestamp,
    /// Saúde auto-reportada no último heartbeat.
    pub healthy: bool,
    /// Detalhe que acompanha um `healthy: false`.
    pub detail: Option<String>,
}

/// Peças que o daemon monta antes de servir a primeira requisição.
///
/// Um struct e não sete parâmetros: a ordem de sete `Arc` na chamada é o tipo de
/// coisa que troca sem o compilador reclamar.
#[derive(Debug)]
pub struct AppStateParts {
    pub config: Config,
    pub hardware: Arc<dyn HardwareProvider>,
    pub bus: Arc<EventBus>,
    pub sampler: SamplerHandle,
    pub database: Database,
    pub runtime: RuntimeSupervision,
    /// Canal que anuncia o início do desligamento.
    ///
    /// Criado **fora** do estado porque o gravador de eventos precisa assiná-lo
    /// antes de o estado existir: ele sobe logo depois do barramento, para que o
    /// `hardware.detected` da subida também seja persistido.
    pub shutdown: tokio::sync::watch::Sender<bool>,
}

/// Estado compartilhado do daemon.
#[derive(Debug, Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

/// Conteúdo real do estado; existe só para o `Arc` ter um alvo.
#[derive(Debug)]
struct Inner {
    config: Config,
    hardware: Arc<dyn HardwareProvider>,
    bus: Arc<EventBus>,
    sampler: SamplerHandle,
    database: Database,
    runtime: RuntimeSupervision,
    profile: RwLock<Option<CachedProfile>>,
    sessions: RwLock<HashMap<String, RuntimeSession>>,
    components: RwLock<BTreeMap<String, ComponentStatus>>,
    started_at: Timestamp,
    pid: u32,
    /// Vira `true` uma vez, no início do desligamento.
    ///
    /// Um `watch` e não um `AtomicBool` porque há quem precise *esperar* pela
    /// mudança, não só consultá-la: o stream SSE de cada cliente conectado usa
    /// este canal para se fechar sozinho, sem o que o desligamento ordenado
    /// ficaria pendurado esperando conexões que nunca terminam.
    shutdown: tokio::sync::watch::Sender<bool>,
}

impl AppState {
    /// Monta o estado. O relógio de `started_at` começa a contar aqui.
    pub fn new(parts: AppStateParts) -> Self {
        Self {
            inner: Arc::new(Inner {
                config: parts.config,
                hardware: parts.hardware,
                bus: parts.bus,
                sampler: parts.sampler,
                database: parts.database,
                runtime: parts.runtime,
                profile: RwLock::new(None),
                sessions: RwLock::new(HashMap::new()),
                components: RwLock::new(BTreeMap::new()),
                started_at: Timestamp::now(),
                pid: std::process::id(),
                shutdown: parts.shutdown,
            }),
        }
    }

    /// Configuração resolvida na subida.
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// Provider de hardware em uso (real ou de teste).
    pub fn hardware(&self) -> &Arc<dyn HardwareProvider> {
        &self.inner.hardware
    }

    /// Barramento de eventos.
    pub fn bus(&self) -> &Arc<EventBus> {
        &self.inner.bus
    }

    /// Amostrador de recursos.
    pub fn sampler(&self) -> &SamplerHandle {
        &self.inner.sampler
    }

    /// Banco de dados.
    pub fn database(&self) -> &Database {
        &self.inner.database
    }

    /// Supervisão do agent-runtime.
    pub fn runtime(&self) -> &RuntimeSupervision {
        &self.inner.runtime
    }

    /// Instante em que o daemon terminou de montar o estado.
    pub fn started_at(&self) -> Timestamp {
        self.inner.started_at
    }

    /// PID do daemon.
    pub fn pid(&self) -> u32 {
        self.inner.pid
    }

    /// Segundos desde a subida.
    ///
    /// Um relógio que anda para trás (ajuste de NTP) devolve `0` em vez de um
    /// número gigante vindo de uma subtração sem sinal.
    pub fn uptime_seconds(&self) -> u64 {
        let elapsed = Timestamp::now().as_second() - self.inner.started_at.as_second();
        u64::try_from(elapsed).unwrap_or(0)
    }

    // -----------------------------------------------------------------------
    // Perfil de hardware
    // -----------------------------------------------------------------------

    /// Perfil em cache, ou `None` enquanto a detecção não tiver sucedido.
    pub fn cached_profile(&self) -> Option<CachedProfile> {
        read(&self.inner.profile).clone()
    }

    /// Substitui o perfil em cache.
    pub fn store_profile(&self, profile: HardwareProfile, first_seen_at: Timestamp) {
        *write(&self.inner.profile) = Some(CachedProfile { profile, first_seen_at });
    }

    // -----------------------------------------------------------------------
    // Componentes
    // -----------------------------------------------------------------------

    /// Registra o estado de um componente e publica a transição quando ela
    /// muda.
    ///
    /// As duas coisas juntas de propósito: quem lê `/v1/status` e quem escuta o
    /// bus precisam concordar, e um caminho separado para cada um sempre acaba
    /// divergindo.
    ///
    /// Um componente visto pela primeira vez transita de
    /// [`ComponentState::Stopped`] — o estado "ainda não iniciado" do contrato —
    /// para o estado informado, de modo que a subida do daemon apareça inteira
    /// no log de eventos em vez de começar no meio.
    pub fn set_component(&self, name: &str, state: ComponentState, detail: Option<String>) {
        let previous = {
            let mut components = write(&self.inner.components);
            if let Some(entry) = components.get_mut(name) {
                let previous = entry.state;
                if previous != state || entry.detail != detail {
                    entry.state = state;
                    entry.detail.clone_from(&detail);
                    entry.since = Timestamp::now();
                }
                previous
            } else {
                components.insert(
                    name.to_owned(),
                    ComponentStatus {
                        name: name.to_owned(),
                        state,
                        detail: detail.clone(),
                        since: Timestamp::now(),
                        restarts: 0,
                    },
                );
                ComponentState::Stopped
            }
        };

        if previous != state {
            self.inner.bus.publish(Event::ComponentStateChanged {
                component: name.to_owned(),
                from: previous,
                to: state,
                detail,
            });
        }
    }

    /// Estado de todos os componentes, em ordem estável por nome.
    ///
    /// O componente do agent-runtime não sai do mapa: ele é lido ao vivo do
    /// supervisor, porque a contagem de reinícios muda sem passar por aqui.
    pub fn components(&self) -> Vec<ComponentStatus> {
        let mut statuses: BTreeMap<String, ComponentStatus> = read(&self.inner.components).clone();
        statuses.insert(RUNTIME_COMPONENT.to_owned(), self.runtime_status());
        statuses.into_values().collect()
    }

    /// Estado do agent-runtime, combinando o processo supervisionado com a
    /// sessão registrada.
    ///
    /// Os dois existem separadamente: em desenvolvimento o runtime é iniciado
    /// pelo `pnpm` e se registra sozinho, sem processo filho nenhum; em produção
    /// o daemon o inicia e ele também se registra. Reportar só um dos lados
    /// esconderia metade das falhas.
    fn runtime_status(&self) -> ComponentStatus {
        let mut status = self.inner.runtime.status();
        let session = self.live_sessions().into_iter().next();

        match session {
            Some(session) if !session.healthy => {
                status.state = ComponentState::Degraded;
                status.detail = Some(format!(
                    "o runtime {} (pid {}) se declarou doente: {}",
                    session.name,
                    session.pid,
                    session.detail.as_deref().unwrap_or("sem detalhe")
                ));
            }
            // Runtime vivo que o daemon não iniciou: o componente é o que está
            // registrado, não a ausência de processo filho.
            Some(session) if !self.inner.runtime.is_supervised() => {
                status.state = ComponentState::Ready;
                status.since = session.registered_at;
                status.detail = Some(format!(
                    "registrado externamente: {} {} (pid {}) desde {}",
                    session.name, session.version, session.pid, session.registered_at
                ));
            }
            _ => {}
        }

        status
    }

    // -----------------------------------------------------------------------
    // Sessões de runtime
    // -----------------------------------------------------------------------

    /// Registra um runtime e devolve o token da sessão.
    ///
    /// Um `POST /v1/runtime/register` novo do mesmo processo substitui a sessão
    /// anterior: é o que o runtime faz depois de um `acknowledged: false`, e
    /// manter as duas deixaria uma sessão fantasma segurando o estado.
    pub fn register_runtime(
        &self,
        request: &RuntimeRegisterRequest,
        now: Timestamp,
    ) -> RuntimeRegisterResponse {
        let session = RuntimeSession {
            id: uuid::Uuid::new_v4().to_string(),
            name: request.name.clone(),
            version: request.version.clone(),
            pid: request.pid,
            registered_at: now,
            last_seen_at: now,
            healthy: true,
            detail: None,
        };

        {
            let mut sessions = write(&self.inner.sessions);
            sessions.retain(|_, existing| existing.pid != request.pid && !expired(existing, now));
            sessions.insert(session.id.clone(), session.clone());
        }

        tracing::info!(
            session = session.id,
            runtime = session.name,
            version = session.version,
            pid = session.pid,
            "runtime registrado"
        );

        RuntimeRegisterResponse {
            session_id: session.id,
            heartbeat_interval_ms: interval_millis(HEARTBEAT_INTERVAL),
            daemon_version: env!("CARGO_PKG_VERSION").to_owned(),
        }
    }

    /// Processa um heartbeat.
    ///
    /// Sessão desconhecida devolve `acknowledged: false` e **status 200**, não
    /// um erro: é assim que o contrato ([`RuntimeHeartbeatResponse`]) manda o
    /// runtime se registrar de novo. Um 4xx faria o runtime tratar o caso como
    /// definitivo e morrer, que é o oposto do desejado.
    pub fn heartbeat(
        &self,
        request: &RuntimeHeartbeatRequest,
        now: Timestamp,
    ) -> RuntimeHeartbeatResponse {
        let shutdown_requested = self.is_shutting_down();
        let mut sessions = write(&self.inner.sessions);
        sessions.retain(|_, session| !expired(session, now));

        let Some(session) = sessions.get_mut(&request.session_id) else {
            tracing::debug!(session = request.session_id, "heartbeat de sessão desconhecida");
            return RuntimeHeartbeatResponse { acknowledged: false, shutdown_requested };
        };

        session.last_seen_at = now;
        session.healthy = request.healthy;
        session.detail.clone_from(&request.detail);

        RuntimeHeartbeatResponse { acknowledged: true, shutdown_requested }
    }

    /// Sessões ainda dentro do prazo, das mais recentes para as mais antigas.
    pub fn live_sessions(&self) -> Vec<RuntimeSession> {
        let now = Timestamp::now();
        let mut sessions: Vec<RuntimeSession> = read(&self.inner.sessions)
            .values()
            .filter(|session| !expired(session, now))
            .cloned()
            .collect();

        sessions.sort_by_key(|session| std::cmp::Reverse(session.last_seen_at));
        sessions
    }

    // -----------------------------------------------------------------------
    // Desligamento
    // -----------------------------------------------------------------------

    /// Marca o início do desligamento. Devolve `false` se já estava marcado.
    ///
    /// A partir daqui todo heartbeat responde `shutdownRequested: true`, que é
    /// como o runtime descobre que deve encerrar sozinho antes do SIGTERM, e
    /// todo stream SSE aberto se encerra.
    pub fn begin_shutdown(&self) -> bool {
        !self.inner.shutdown.send_replace(true)
    }

    /// `true` depois do primeiro [`AppState::begin_shutdown`].
    pub fn is_shutting_down(&self) -> bool {
        *self.inner.shutdown.borrow()
    }

    /// Canal que avisa quando o desligamento começa.
    ///
    /// O receptor entrega o valor corrente na primeira leitura, então quem
    /// assina depois do início do desligamento descobre isso imediatamente em
    /// vez de esperar por uma mudança que não vem mais.
    pub fn shutdown_signal(&self) -> tokio::sync::watch::Receiver<bool> {
        self.inner.shutdown.subscribe()
    }
}

/// Uma sessão está morta depois de [`MISSED_HEARTBEATS_BEFORE_DEAD`] intervalos
/// sem notícia.
fn expired(session: &RuntimeSession, now: Timestamp) -> bool {
    let deadline = HEARTBEAT_INTERVAL.saturating_mul(MISSED_HEARTBEATS_BEFORE_DEAD);
    let silence = now.as_millisecond() - session.last_seen_at.as_millisecond();

    // Silêncio negativo é relógio andando para trás; nesse caso a sessão não é
    // velha, é o relógio que é novo.
    u128::try_from(silence).is_ok_and(|silence| silence > deadline.as_millis())
}

/// Converte a duração para o `u64` de milissegundos do contrato.
fn interval_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Leitura tolerante a envenenamento.
fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Escrita tolerante a envenenamento.
fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "em teste, um panic é o relatório de falha desejado"
)]
mod tests {
    use std::path::PathBuf;

    use elara_hardware::MockHardwareProvider;
    use elara_telemetry::ResourceSampler;

    use super::*;
    use crate::config::{Cli, Environment};

    fn config() -> Config {
        let environment = Environment {
            runtime_dir: Some("/run/user/1000".to_owned()),
            data_home: None,
            home: Some(PathBuf::from("/home/alguem")),
            uid: 1000,
        };
        Config::resolve(Cli::default(), &environment).expect("configuração válida")
    }

    fn state() -> AppState {
        let bus = Arc::new(EventBus::new(64));
        let hardware: Arc<dyn HardwareProvider> = Arc::new(MockHardwareProvider::new());
        let sampler = ResourceSampler::new(Arc::clone(&hardware), Duration::from_secs(3_600))
            .spawn(Arc::clone(&bus));

        AppState::new(AppStateParts {
            config: config(),
            hardware,
            bus: Arc::clone(&bus),
            sampler,
            database: Database::open_in_memory().expect("banco em memória"),
            runtime: RuntimeSupervision::start(&config(), bus),
            shutdown: tokio::sync::watch::Sender::new(false),
        })
    }

    fn register_request(pid: u32) -> RuntimeRegisterRequest {
        RuntimeRegisterRequest {
            name: "agent-runtime".to_owned(),
            version: "0.1.0".to_owned(),
            pid,
            protocol_version: 1,
            capabilities: Vec::new(),
        }
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("instante válido")
    }

    #[tokio::test]
    async fn registro_devolve_uma_sessao_utilizavel() {
        let state = state();
        let now = Timestamp::now();
        let response = state.register_runtime(&register_request(4242), now);

        assert_eq!(response.heartbeat_interval_ms, 5_000);
        assert_eq!(response.daemon_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(
            uuid::Uuid::parse_str(&response.session_id).map(|id| id.get_version_num()),
            Ok(4),
            "o token precisa ser um UUID v4"
        );

        let heartbeat = state.heartbeat(
            &RuntimeHeartbeatRequest {
                session_id: response.session_id,
                healthy: true,
                detail: None,
            },
            now,
        );
        assert!(heartbeat.acknowledged);
        assert!(!heartbeat.shutdown_requested);
    }

    #[tokio::test]
    async fn dois_registros_produzem_sessoes_diferentes() {
        let state = state();
        let now = Timestamp::now();
        let primeira = state.register_runtime(&register_request(1), now);
        let segunda = state.register_runtime(&register_request(2), now);

        assert_ne!(primeira.session_id, segunda.session_id);
        assert_eq!(state.live_sessions().len(), 2);
    }

    #[tokio::test]
    async fn registrar_de_novo_o_mesmo_pid_substitui_a_sessao() {
        let state = state();
        let now = Timestamp::now();
        let primeira = state.register_runtime(&register_request(7), now);
        let segunda = state.register_runtime(&register_request(7), now);

        let sessions = state.live_sessions();
        assert_eq!(sessions.len(), 1, "a sessão antiga do mesmo processo não pode sobrar");
        assert_eq!(sessions[0].id, segunda.session_id);
        assert_ne!(sessions[0].id, primeira.session_id);
    }

    #[tokio::test]
    async fn heartbeat_de_sessao_desconhecida_pede_novo_registro_sem_erro() {
        let state = state();
        let response = state.heartbeat(
            &RuntimeHeartbeatRequest {
                session_id: "nunca-existiu".to_owned(),
                healthy: true,
                detail: None,
            },
            Timestamp::now(),
        );

        assert!(!response.acknowledged, "o runtime precisa saber que deve se registrar de novo");
        assert!(!response.shutdown_requested);
    }

    #[tokio::test]
    async fn sessao_expira_depois_de_tres_intervalos_sem_heartbeat() {
        let state = state();
        let inicio = at(1_000_000);
        let session = state.register_runtime(&register_request(11), inicio);

        let dentro_do_prazo = inicio + jiff::SignedDuration::from_secs(14);
        assert!(
            state
                .heartbeat(
                    &RuntimeHeartbeatRequest {
                        session_id: session.session_id.clone(),
                        healthy: true,
                        detail: None,
                    },
                    dentro_do_prazo,
                )
                .acknowledged
        );

        // 3 × 5 s a partir do último heartbeat aceito.
        let fora_do_prazo = dentro_do_prazo + jiff::SignedDuration::from_secs(16);
        assert!(
            !state
                .heartbeat(
                    &RuntimeHeartbeatRequest {
                        session_id: session.session_id,
                        healthy: true,
                        detail: None,
                    },
                    fora_do_prazo,
                )
                .acknowledged,
            "depois de três heartbeats perdidos a sessão morre"
        );
    }

    #[tokio::test]
    async fn durante_o_desligamento_o_heartbeat_pede_parada() {
        let state = state();
        let now = Timestamp::now();
        let session = state.register_runtime(&register_request(3), now);

        assert!(state.begin_shutdown(), "a primeira chamada marca");
        assert!(!state.begin_shutdown(), "a segunda é no-op");

        let response = state.heartbeat(
            &RuntimeHeartbeatRequest {
                session_id: session.session_id,
                healthy: true,
                detail: None,
            },
            now,
        );
        assert!(response.acknowledged);
        assert!(response.shutdown_requested);
    }

    #[tokio::test]
    async fn componentes_saem_ordenados_e_com_o_runtime_sempre_presente() {
        let state = state();
        state.set_component(TELEMETRY_COMPONENT, ComponentState::Ready, None);
        state.set_component(HARDWARE_COMPONENT, ComponentState::Degraded, Some("2 avisos".into()));
        state.set_component(DATABASE_COMPONENT, ComponentState::Ready, None);

        let names: Vec<String> = state.components().into_iter().map(|status| status.name).collect();
        assert_eq!(names, vec!["agent-runtime", "database", "hardware", "telemetry"]);
    }

    #[tokio::test]
    async fn mudanca_de_componente_publica_evento_e_repeticao_nao() {
        let state = state();
        let mut receiver = state.bus().subscribe();

        state.set_component(HARDWARE_COMPONENT, ComponentState::Ready, None);
        let envelope = receiver.recv().await.expect("transição publicada");
        match envelope.payload {
            Event::ComponentStateChanged { component, from, to, .. } => {
                assert_eq!(component, HARDWARE_COMPONENT);
                assert_eq!(from, ComponentState::Stopped, "componente novo nasce em Stopped");
                assert_eq!(to, ComponentState::Ready);
            }
            other => panic!("evento inesperado: {other:?}"),
        }

        // Repetir o mesmo estado não pode gerar tráfego.
        state.set_component(HARDWARE_COMPONENT, ComponentState::Ready, None);
        state.set_component(HARDWARE_COMPONENT, ComponentState::Failed, Some("caiu".into()));

        let envelope = receiver.recv().await.expect("segunda transição");
        match envelope.payload {
            Event::ComponentStateChanged { from, to, .. } => {
                assert_eq!(from, ComponentState::Ready);
                assert_eq!(to, ComponentState::Failed);
            }
            other => panic!("evento inesperado: {other:?}"),
        }
    }

    #[tokio::test]
    async fn runtime_registrado_por_fora_aparece_como_pronto() {
        let state = state();
        assert_eq!(state.runtime_status().state, ComponentState::Unavailable);

        state.register_runtime(&register_request(999), Timestamp::now());

        let status = state.runtime_status();
        assert_eq!(status.state, ComponentState::Ready);
        assert!(status.detail.unwrap_or_default().contains("registrado externamente"));
    }

    #[tokio::test]
    async fn runtime_que_se_declara_doente_fica_degradado() {
        let state = state();
        let now = Timestamp::now();
        let session = state.register_runtime(&register_request(5), now);

        state.heartbeat(
            &RuntimeHeartbeatRequest {
                session_id: session.session_id,
                healthy: false,
                detail: Some("modelo não carregou".to_owned()),
            },
            now,
        );

        let status = state.runtime_status();
        assert_eq!(status.state, ComponentState::Degraded);
        assert!(status.detail.unwrap_or_default().contains("modelo não carregou"));
    }

    #[tokio::test]
    async fn perfil_em_cache_comeca_vazio_e_e_substituivel() {
        let state = state();
        assert!(state.cached_profile().is_none());

        let profile = MockHardwareProvider::reference_profile();
        state.store_profile(profile.clone(), at(1_000));

        let cached = state.cached_profile().expect("perfil em cache");
        assert_eq!(cached.profile, profile);
        assert_eq!(cached.first_seen_at, at(1_000));
    }

    #[tokio::test]
    async fn uptime_nunca_e_negativo() {
        let state = state();
        assert_eq!(state.uptime_seconds(), 0);
        assert_eq!(state.pid(), std::process::id());
    }
}
