//! Detecção do perfil de hardware, sua persistência e seu cache.
//!
//! A detecção é cara (dezenas de milissegundos lendo `/proc`, `/sys` e NVML) e
//! **bloqueante**. Chamá-la direto de um handler `async` prenderia uma thread do
//! executor e travaria todas as outras conexões, inclusive o stream de eventos —
//! por isso tudo aqui passa por [`tokio::task::spawn_blocking`].
//!
//! A outra regra é que **falha de hardware não derruba o daemon**. Uma máquina
//! com `/proc` restrito ainda precisa de um daemon de pé para dizer, em
//! `/v1/status`, por que o hardware é desconhecido. O erro vira componente
//! `Failed` e um evento; `GET /v1/hardware` responde 503 com um código estável.

use std::sync::Arc;

use elara_hardware::{HardwareError, HardwareProfile, HardwareProvider, WarningSeverity};
use elara_telemetry::EventBus;
use elara_telemetry::types::{ComponentState, Event};
use jiff::Timestamp;

use crate::db::{Database, DbError};
use crate::state::{AppState, CachedProfile, HARDWARE_COMPONENT};

/// Chave em que o fingerprint corrente fica registrado.
///
/// Persistido para que uma fase futura (calibração, benchmarks) consiga
/// perguntar "o hardware mudou desde a última execução?" sem reler o perfil
/// inteiro.
const FINGERPRINT_SETTING: &str = "hardware.fingerprint";

/// Falhas de uma detecção completa.
#[derive(Debug, thiserror::Error)]
pub enum RefreshError {
    /// As sondas não produziram um perfil.
    #[error("a detecção de hardware falhou: {0}")]
    Probe(#[from] HardwareError),

    /// O perfil foi detectado mas não pôde ser gravado.
    #[error("o perfil foi detectado mas não pôde ser gravado: {0}")]
    Persist(#[from] DbError),

    /// A thread bloqueante da detecção morreu.
    #[error("a detecção de hardware não terminou")]
    TaskFailed,
}

impl RefreshError {
    /// Explicação para o usuário, sem detalhe interno de persistência.
    ///
    /// A mensagem da sonda entra inteira porque ela **é** a informação útil
    /// (`não foi possível ler /proc/cpuinfo: permission denied` diz o que
    /// corrigir); a do banco não, porque expõe o caminho do arquivo sem ajudar
    /// quem lê a tela.
    pub fn user_detail(&self) -> String {
        match self {
            Self::Probe(error) => format!("a detecção de hardware falhou: {error}"),
            Self::Persist(_) => {
                "o perfil foi detectado mas não pôde ser gravado no banco".to_owned()
            }
            Self::TaskFailed => "a detecção de hardware não terminou".to_owned(),
        }
    }
}

/// Detecta, persiste e devolve o perfil — sem tocar em nenhum estado
/// compartilhado.
///
/// Separada de [`refresh`] porque a subida do daemon precisa dela **antes** de
/// existir um [`AppState`]: a ordem de inicialização manda o hardware vir antes
/// do amostrador e do supervisor.
pub async fn detect_and_persist(
    provider: &Arc<dyn HardwareProvider>,
    database: &Database,
    bus: &EventBus,
) -> Result<CachedProfile, RefreshError> {
    let profile = detect(provider).await?;
    let now = Timestamp::now();

    // Comparar antes de gravar é a única chance de notar que a máquina mudou:
    // depois do upsert o valor antigo já se foi, e um benchmark calibrado no
    // hardware anterior deixou de valer sem ninguém perceber.
    if let Some(previous) = database.setting(FINGERPRINT_SETTING).await?
        && previous != profile.fingerprint
    {
        tracing::warn!(
            antes = previous,
            agora = profile.fingerprint,
            "o hardware mudou desde a última execução"
        );
    }

    let first_seen_at = database.upsert_hardware_profile(&profile, now).await?;
    database.set_setting(FINGERPRINT_SETTING, &profile.fingerprint, now).await?;

    bus.publish(Event::HardwareDetected {
        fingerprint: profile.fingerprint.clone(),
        probe_duration_ms: profile.probe_duration_ms,
        warning_count: u32::try_from(profile.warnings.len()).unwrap_or(u32::MAX),
    });

    tracing::info!(
        fingerprint = profile.fingerprint,
        probe_duration_ms = profile.probe_duration_ms,
        warnings = profile.warnings.len(),
        first_seen_at = %first_seen_at,
        "hardware detectado"
    );

    Ok(CachedProfile { profile, first_seen_at })
}

/// Executa uma detecção e reflete o resultado no estado do daemon.
///
/// É o caminho de `POST /v1/hardware/refresh` e o do fim da subida: os dois
/// precisam atualizar cache, componente e banco exatamente do mesmo jeito.
pub async fn refresh(state: &AppState) -> Result<CachedProfile, RefreshError> {
    let outcome = detect_and_persist(state.hardware(), state.database(), state.bus()).await;
    apply(state, &outcome);
    outcome
}

/// Reflete o resultado de uma detecção no cache e no componente `hardware`.
pub fn apply(state: &AppState, outcome: &Result<CachedProfile, RefreshError>) {
    match outcome {
        Ok(cached) => {
            state.store_profile(cached.profile.clone(), cached.first_seen_at);
            let (component, detail) = describe(&cached.profile);
            state.set_component(HARDWARE_COMPONENT, component, detail);
        }
        Err(error) => {
            let detail = error.user_detail();
            tracing::error!(error = %error, "detecção de hardware falhou");
            state.bus().publish(Event::HardwareProbeFailed { error: detail.clone() });
            state.set_component(HARDWARE_COMPONENT, ComponentState::Failed, Some(detail));
        }
    }
}

/// Traduz os avisos das sondas em estado de componente.
///
/// Avisos `info` não degradam nada — são coisas como "não achei o modelo do
/// disco". `warning` e `error` significam que alguma decisão do Resource Broker
/// será mais conservadora do que precisaria, e isso o usuário tem de ver.
fn describe(profile: &HardwareProfile) -> (ComponentState, Option<String>) {
    let relevant = profile
        .warnings
        .iter()
        .filter(|warning| {
            matches!(warning.severity, WarningSeverity::Warning | WarningSeverity::Error)
        })
        .count();

    if relevant == 0 {
        return (ComponentState::Ready, None);
    }

    (
        ComponentState::Degraded,
        Some(format!(
            "{relevant} de {} sondas entregaram dados parciais; veja profile.warnings",
            profile.warnings.len()
        )),
    )
}

/// Roda a detecção fora do executor async.
async fn detect(provider: &Arc<dyn HardwareProvider>) -> Result<HardwareProfile, RefreshError> {
    let provider = Arc::clone(provider);

    match tokio::task::spawn_blocking(move || provider.detect()).await {
        Ok(result) => result.map_err(RefreshError::Probe),
        Err(error) => {
            tracing::error!(%error, "a task de detecção de hardware não terminou");
            Err(RefreshError::TaskFailed)
        }
    }
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
    use elara_hardware::{MockHardwareProvider, ProbeWarning};

    use super::*;
    use crate::testing;

    #[tokio::test]
    async fn deteccao_bem_sucedida_grava_publica_e_deixa_o_componente_pronto() {
        let harness = testing::state_only();
        let mut events = harness.state.bus().subscribe();

        let cached = refresh(&harness.state).await.expect("detecção");
        assert_eq!(
            cached.profile.fingerprint,
            MockHardwareProvider::reference_profile().fingerprint
        );

        let component = testing::component(&harness.state, HARDWARE_COMPONENT);
        assert_eq!(component.state, ComponentState::Ready);
        assert_eq!(component.detail, None);

        // O evento de detecção precisa sair com os números do perfil.
        let envelope = events.recv().await.expect("evento publicado");
        match envelope.payload {
            Event::HardwareDetected { fingerprint, probe_duration_ms, warning_count } => {
                assert_eq!(fingerprint, cached.profile.fingerprint);
                assert_eq!(probe_duration_ms, cached.profile.probe_duration_ms);
                assert_eq!(warning_count, 0);
            }
            other => panic!("evento inesperado: {other:?}"),
        }

        // E o perfil precisa estar no cache e no banco.
        assert!(harness.state.cached_profile().is_some());
        let history = harness.state.database().hardware_history(10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].fingerprint, cached.profile.fingerprint);
    }

    #[tokio::test]
    async fn o_fingerprint_fica_persistido_nas_configuracoes() {
        let harness = testing::state_only();
        let cached = refresh(&harness.state).await.expect("detecção");

        assert_eq!(
            harness.state.database().setting(FINGERPRINT_SETTING).await.unwrap(),
            Some(cached.profile.fingerprint)
        );
    }

    #[tokio::test]
    async fn duas_deteccoes_do_mesmo_hardware_nao_duplicam_o_historico() {
        let harness = testing::state_only();
        let primeira = refresh(&harness.state).await.expect("primeira detecção");
        let segunda = refresh(&harness.state).await.expect("segunda detecção");

        assert_eq!(primeira.first_seen_at, segunda.first_seen_at);
        assert_eq!(harness.state.database().hardware_history(10).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn avisos_relevantes_degradam_o_componente() {
        let mut profile = MockHardwareProvider::reference_profile();
        profile.warnings.push(ProbeWarning::info("storage.model", "modelo do disco ausente"));
        profile.warnings.push(ProbeWarning::warning("gpu.nvml", "NVML não carregou"));

        let harness = testing::state_with(Arc::new(MockHardwareProvider::with_profile(profile)));
        refresh(&harness.state).await.expect("detecção");

        let component = testing::component(&harness.state, HARDWARE_COMPONENT);
        assert_eq!(component.state, ComponentState::Degraded);
        let detail = component.detail.unwrap_or_default();
        assert!(detail.starts_with("1 de 2"), "detalhe inesperado: {detail}");
    }

    #[tokio::test]
    async fn so_avisos_informativos_mantem_o_componente_pronto() {
        let mut profile = MockHardwareProvider::reference_profile();
        profile.warnings.push(ProbeWarning::info("storage.mounts", "LVM não associado ao disco"));

        let harness = testing::state_with(Arc::new(MockHardwareProvider::with_profile(profile)));
        refresh(&harness.state).await.expect("detecção");

        assert_eq!(
            testing::component(&harness.state, HARDWARE_COMPONENT).state,
            ComponentState::Ready
        );
    }

    #[tokio::test]
    async fn falha_de_sonda_nao_derruba_nada_e_publica_o_motivo() {
        let provider = MockHardwareProvider::failing(HardwareError::Io {
            path: "/proc/cpuinfo".to_owned(),
            source: std::io::Error::new(std::io::ErrorKind::PermissionDenied, "sem permissão"),
        });
        let harness = testing::state_with(Arc::new(provider));
        let mut events = harness.state.bus().subscribe();

        let error = refresh(&harness.state).await.unwrap_err();
        assert!(matches!(error, RefreshError::Probe(_)));

        let envelope = events.recv().await.expect("evento publicado");
        match envelope.payload {
            Event::HardwareProbeFailed { error } => {
                assert!(error.contains("/proc/cpuinfo"), "motivo inútil: {error}");
            }
            other => panic!("evento inesperado: {other:?}"),
        }

        let component = testing::component(&harness.state, HARDWARE_COMPONENT);
        assert_eq!(component.state, ComponentState::Failed);
        assert!(harness.state.cached_profile().is_none(), "nenhum perfil inventado");
    }

    #[tokio::test]
    async fn o_detalhe_de_falha_de_banco_nao_expoe_o_arquivo() {
        let error = RefreshError::Persist(DbError::Open {
            path: "/home/alguem/.local/share/elara/agentd.sqlite3".to_owned(),
            source: rusqlite::Error::InvalidQuery,
        });

        let detail = error.user_detail();
        assert!(!detail.contains("sqlite3"), "caminho vazou: {detail}");
        assert!(detail.contains("banco"));
    }
}
