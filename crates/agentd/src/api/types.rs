//! Contrato HTTP do daemon.
//!
//! Superfície servida no Unix socket. Cada tipo aqui é gerado em TypeScript
//! (`packages/schemas/src/generated/`) e validado em runtime pelo `@elara/sdk`,
//! de modo que uma UI nova falando com um daemon velho produz um erro claro em
//! vez de `undefined`.
//!
//! Só existem aqui as rotas que a Fase 1 e a Fase 2 realmente implementam. As
//! rotas de modelos, tasks e workflows previstas na especificação entram junto
//! com suas fases.

use elara_hardware::HardwareProfile;
use elara_telemetry::types::{ComponentStatus, ResourceSnapshot};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

macro_rules! ts_export {
    () => {
        "../../../packages/schemas/src/generated/"
    };
}

/// Versão do contrato HTTP. O cliente envia a sua no header
/// `x-elara-protocol`; divergência de major vira erro explícito.
pub const PROTOCOL_VERSION: u32 = 1;

/// Header em que cliente e servidor anunciam [`PROTOCOL_VERSION`].
pub const PROTOCOL_HEADER: &str = "x-elara-protocol";

// ---------------------------------------------------------------------------
// Erros
// ---------------------------------------------------------------------------

/// Corpo de toda resposta de erro do daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct ApiError {
    /// Código estável e legível por máquina, ex.: `hardware.unavailable`.
    pub code: String,
    /// Mensagem para humanos.
    pub message: String,
    /// Contexto adicional, quando útil para diagnóstico.
    pub details: Option<String>,
}

// ---------------------------------------------------------------------------
// Saúde e status
// ---------------------------------------------------------------------------

/// Resposta de `GET /v1/health`. Barata o bastante para polling agressivo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct HealthResponse {
    pub ok: bool,
    pub version: String,
    pub protocol_version: u32,
    pub uptime_seconds: u64,
}

/// Resposta de `GET /v1/status`: visão completa do daemon e seus componentes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct DaemonStatus {
    pub version: String,
    pub protocol_version: u32,
    pub pid: u32,
    #[ts(type = "string")]
    pub started_at: jiff::Timestamp,
    pub uptime_seconds: u64,
    pub socket_path: String,
    pub data_dir: String,
    /// Estado de `hardware`, `telemetry`, `agent-runtime`, etc.
    pub components: Vec<ComponentStatus>,
}

// ---------------------------------------------------------------------------
// Hardware
// ---------------------------------------------------------------------------

/// Resposta de `GET /v1/hardware` e `POST /v1/hardware/refresh`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct HardwareResponse {
    pub profile: HardwareProfile,
    /// `true` quando veio do cache em memória/SQLite em vez de nova detecção.
    pub cached: bool,
    /// Quando este perfil foi detectado pela primeira vez.
    #[ts(type = "string")]
    pub first_seen_at: jiff::Timestamp,
}

/// Item de `GET /v1/hardware/history`: um perfil já visto nesta máquina.
///
/// Um fingerprint novo significa que o hardware mudou e que benchmarks
/// anteriores não valem mais.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct HardwareHistoryEntry {
    pub fingerprint: String,
    #[ts(type = "string")]
    pub first_seen_at: jiff::Timestamp,
    #[ts(type = "string")]
    pub last_seen_at: jiff::Timestamp,
    pub cpu_model: String,
    pub ram_total_mb: u64,
    pub gpu_summary: String,
}

// ---------------------------------------------------------------------------
// Recursos
// ---------------------------------------------------------------------------

/// Resposta de `GET /v1/resources`: última amostra calculada.
///
/// `snapshot` é `None` só na janela entre o start do daemon e a segunda
/// amostra — antes disso não há intervalo para calcular percentuais.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct ResourcesResponse {
    pub snapshot: Option<ResourceSnapshot>,
    pub sample_interval_ms: u64,
}

// ---------------------------------------------------------------------------
// Registro do agent-runtime
// ---------------------------------------------------------------------------

/// Corpo de `POST /v1/runtime/register`, enviado pelo agent-runtime ao subir.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct RuntimeRegisterRequest {
    /// Identificador do runtime, ex.: `agent-runtime`.
    pub name: String,
    pub version: String,
    pub pid: u32,
    pub protocol_version: u32,
    /// Capacidades anunciadas; vazio na Fase 1.
    pub capabilities: Vec<String>,
}

/// Resposta de `POST /v1/runtime/register`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct RuntimeRegisterResponse {
    /// Token opaco que identifica esta sessão de runtime nos heartbeats.
    pub session_id: String,
    /// Intervalo máximo entre heartbeats antes de o daemon considerar o
    /// runtime morto.
    pub heartbeat_interval_ms: u64,
    pub daemon_version: String,
}

/// Corpo de `POST /v1/runtime/heartbeat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct RuntimeHeartbeatRequest {
    pub session_id: String,
    /// Estado auto-reportado pelo runtime.
    pub healthy: bool,
    /// Detalhe obrigatório quando `healthy` é `false`.
    pub detail: Option<String>,
}

/// Resposta de `POST /v1/runtime/heartbeat`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export, export_to = ts_export!())]
pub struct RuntimeHeartbeatResponse {
    /// `false` quando o daemon não reconhece a sessão: o runtime deve se
    /// registrar de novo.
    pub acknowledged: bool,
    /// Sinaliza desligamento em andamento; o runtime deve encerrar.
    pub shutdown_requested: bool,
}
