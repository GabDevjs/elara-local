//! Contrato de telemetria: eventos do event bus e snapshots de recursos.
//!
//! Assim como o perfil de hardware, estes tipos são a fonte da verdade para as
//! definições TypeScript geradas em `packages/schemas/src/generated/`.
//!
//! Sobre o catálogo de eventos: a especificação do projeto prevê eventos de
//! modelo, task, agente, tool e inferência. Só existem aqui as variantes que
//! **algum código realmente emite hoje** (Fases 1 e 2). As demais entram junto
//! com os subsistemas que as produzem — um enum cheio de variantes mortas
//! esconde o que já funciona de verdade.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Diretório de saída das definições TypeScript geradas.
///
/// Relativo a `<crate>/bindings/` (o `TS_RS_EXPORT_DIR` padrão do `ts-rs`):
/// `crates/telemetry/bindings/../../..` é a raiz do repositório.
macro_rules! ts_export {
    () => {
        "../../../packages/schemas/src/generated/"
    };
}

// ---------------------------------------------------------------------------
// Eventos
// ---------------------------------------------------------------------------

/// Envelope de um evento publicado no bus interno e retransmitido por SSE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct EventEnvelope {
    /// Número monotônico por processo. Permite ao cliente detectar lacunas
    /// depois de uma reconexão.
    pub sequence: u64,
    #[ts(type = "string")]
    pub emitted_at: jiff::Timestamp,
    pub payload: Event,
}

/// Evento de domínio. Serializado como `{ "type": "...", ... }`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case")]
#[ts(export, export_to = ts_export!())]
pub enum Event {
    /// Daemon terminou a inicialização e está aceitando conexões.
    #[serde(rename = "daemon.started")]
    #[serde(rename_all = "camelCase")]
    DaemonStarted { version: String, pid: u32, socket_path: String },

    /// Daemon começou o desligamento ordenado.
    #[serde(rename = "daemon.stopping")]
    DaemonStopping { reason: String },

    /// Perfil de hardware detectado com sucesso.
    #[serde(rename = "hardware.detected")]
    #[serde(rename_all = "camelCase")]
    HardwareDetected { fingerprint: String, probe_duration_ms: u64, warning_count: u32 },

    /// A detecção de hardware falhou por completo.
    #[serde(rename = "hardware.probe-failed")]
    HardwareProbeFailed { error: String },

    /// Nova amostra de uso de recursos.
    #[serde(rename = "resources.sampled")]
    ResourcesSampled { snapshot: ResourceSnapshot },

    /// Um componente supervisionado mudou de estado.
    #[serde(rename = "component.state-changed")]
    #[serde(rename_all = "camelCase")]
    ComponentStateChanged {
        component: String,
        from: ComponentState,
        to: ComponentState,
        detail: Option<String>,
    },
}

impl Event {
    /// Nome estável do evento, usado como `event:` no stream SSE e como chave
    /// de filtro nos logs.
    pub fn name(&self) -> &'static str {
        match self {
            Self::DaemonStarted { .. } => "daemon.started",
            Self::DaemonStopping { .. } => "daemon.stopping",
            Self::HardwareDetected { .. } => "hardware.detected",
            Self::HardwareProbeFailed { .. } => "hardware.probe-failed",
            Self::ResourcesSampled { .. } => "resources.sampled",
            Self::ComponentStateChanged { .. } => "component.state-changed",
        }
    }

    /// Eventos de alta frequência não vão para o log estruturado nem para o
    /// `SQLite` — só para os assinantes ao vivo.
    pub fn is_high_frequency(&self) -> bool {
        matches!(self, Self::ResourcesSampled { .. })
    }
}

// ---------------------------------------------------------------------------
// Estado de componentes
// ---------------------------------------------------------------------------

/// Ciclo de vida de um componente supervisionado pelo daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
#[ts(export, export_to = ts_export!())]
pub enum ComponentState {
    /// Ainda não iniciado.
    Stopped,
    /// Iniciando; ainda não confirmou saúde.
    Starting,
    /// Operacional.
    Ready,
    /// Funcionando parcialmente (ex.: hardware detectado com warnings).
    Degraded,
    /// Falhou e será reiniciado.
    Failed,
    /// Não existe nesta máquina e não será iniciado (ex.: runtime não instalado).
    Unavailable,
}

impl ComponentState {
    /// `true` quando o componente consegue atender requisições.
    pub fn is_serving(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }
}

/// Estado reportado de um componente, com histórico mínimo para diagnóstico.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct ComponentStatus {
    /// Identificador estável, ex.: `hardware`, `agent-runtime`.
    pub name: String,
    pub state: ComponentState,
    /// Explicação legível do estado atual — obrigatória quando não é `Ready`.
    pub detail: Option<String>,
    #[ts(type = "string")]
    pub since: jiff::Timestamp,
    /// Quantas vezes o supervisor reiniciou este componente.
    pub restarts: u32,
}

// ---------------------------------------------------------------------------
// Snapshot de recursos
// ---------------------------------------------------------------------------

/// Uso de recursos já convertido em percentuais, pronto para a UI.
///
/// Diferente de `LiveSample` (contadores brutos), este tipo é o resultado da
/// comparação entre duas amostras consecutivas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct ResourceSnapshot {
    #[ts(type = "string")]
    pub taken_at: jiff::Timestamp,
    /// Intervalo coberto por este cálculo.
    pub interval_ms: u64,
    pub cpu: CpuUsage,
    pub memory: MemoryUsage,
    pub gpus: Vec<GpuUsage>,
}

/// Uso de CPU no intervalo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct CpuUsage {
    /// 0.0 – 100.0, agregado.
    pub total_percent: f32,
    /// 0.0 – 100.0 por CPU lógica.
    pub per_core_percent: Vec<f32>,
    pub load_average: [f64; 3],
}

/// Uso de memória no instante da amostra.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct MemoryUsage {
    pub total_mb: u64,
    /// `total - available`: o que está de fato comprometido.
    pub used_mb: u64,
    pub available_mb: u64,
    pub cached_mb: u64,
    pub used_percent: f32,
    pub swap_used_mb: u64,
    pub swap_total_mb: u64,
}

/// Uso de uma GPU no instante da amostra.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct GpuUsage {
    pub index: u32,
    pub name: String,
    pub utilization_percent: Option<f32>,
    pub vram_used_mb: Option<u64>,
    pub vram_total_mb: Option<u64>,
    pub vram_used_percent: Option<f32>,
    pub temperature_c: Option<f32>,
    pub power_watts: Option<f32>,
}
