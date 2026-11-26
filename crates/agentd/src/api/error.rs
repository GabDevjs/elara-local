//! Erros da superfície HTTP.
//!
//! Duas audiências, dois canais. O cliente recebe um [`ApiError`] com um código
//! estável e uma mensagem acionável; o log recebe a causa técnica inteira. A
//! separação não é preciosismo: o socket é local, mas o corpo da resposta
//! atravessa a ponte do Tauri e acaba na tela do usuário, e um caminho de
//! arquivo ou um fragmento de SQL ali não ajuda ninguém e revela a estrutura
//! interna do daemon.
//!
//! Por isso [`ApiFailure`] tem dois campos de texto: `details` vai para o
//! cliente e é escrito para ele; `internal` nunca sai daqui — só é registrado
//! quando a resposta é construída.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use super::types::ApiError;
use crate::db::DbError;

/// Resultado de um handler: ou o corpo de sucesso, ou uma falha já classificada.
pub type ApiResult<T> = Result<T, ApiFailure>;

/// Classes de falha que o daemon sabe reportar.
///
/// Só existem aqui as variantes que algum handler realmente produz — um enum
/// com códigos que ninguém emite viraria documentação falsa para o SDK.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiErrorKind {
    /// Não há perfil de hardware: a detecção falhou e não há cache.
    HardwareUnavailable,
    /// O cliente anunciou uma versão de protocolo diferente da nossa.
    ProtocolVersionMismatch,
    /// O corpo ou os parâmetros da requisição não fazem sentido.
    InvalidRequest,
    /// O daemon está encerrando e não aceita mais registro.
    ShuttingDown,
    /// Nenhuma rota casa com o caminho pedido.
    RouteNotFound,
    /// A rota existe, o método não.
    MethodNotAllowed,
    /// Falha do daemon, não do cliente.
    Internal,
}

impl ApiErrorKind {
    /// Código estável, legível por máquina.
    ///
    /// É contrato: o SDK e a ponte do desktop decidem o que mostrar a partir
    /// dele, então mudar um destes textos é mudar a API.
    pub fn code(self) -> &'static str {
        match self {
            Self::HardwareUnavailable => "hardware.unavailable",
            Self::ProtocolVersionMismatch => "protocol.version_mismatch",
            Self::InvalidRequest => "request.invalid",
            Self::ShuttingDown => "daemon.shutting_down",
            Self::RouteNotFound => "route.not_found",
            Self::MethodNotAllowed => "route.method_not_allowed",
            Self::Internal => "internal",
        }
    }

    /// Status HTTP correspondente.
    pub fn status(self) -> StatusCode {
        match self {
            Self::HardwareUnavailable | Self::ShuttingDown => StatusCode::SERVICE_UNAVAILABLE,
            Self::ProtocolVersionMismatch | Self::InvalidRequest => StatusCode::BAD_REQUEST,
            Self::RouteNotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Uma falha de requisição, pronta para virar resposta.
#[derive(Debug)]
pub struct ApiFailure {
    kind: ApiErrorKind,
    message: String,
    /// Contexto que o cliente pode usar. Escrito para ele.
    details: Option<String>,
    /// Causa técnica. Vai para o log e **nunca** para a resposta.
    internal: Option<String>,
}

impl ApiFailure {
    /// Falha com mensagem para o cliente.
    pub fn new(kind: ApiErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into(), details: None, internal: None }
    }

    /// Acrescenta contexto seguro para o cliente.
    #[must_use]
    pub fn with_details(mut self, details: impl Into<String>) -> Self {
        self.details = Some(details.into());
        self
    }

    /// Acrescenta a causa técnica, que só o log verá.
    #[must_use]
    pub fn with_internal(mut self, internal: impl std::fmt::Display) -> Self {
        self.internal = Some(internal.to_string());
        self
    }

    /// Falha genérica do daemon.
    ///
    /// A mensagem para o cliente é sempre a mesma de propósito: se ela variasse
    /// com a causa, cada bug novo viraria um contrato novo.
    pub fn internal(operation: &'static str, cause: impl std::fmt::Display) -> Self {
        Self::new(ApiErrorKind::Internal, "o daemon não conseguiu completar a operação")
            .with_details(format!("operação: {operation}"))
            .with_internal(cause)
    }

    /// Requisição malformada.
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::new(ApiErrorKind::InvalidRequest, message)
    }

    /// Classe desta falha, útil em teste e no log.
    pub fn kind(&self) -> ApiErrorKind {
        self.kind
    }

    /// Corpo que será enviado ao cliente.
    fn body(&self) -> ApiError {
        ApiError {
            code: self.kind().code().to_owned(),
            message: self.message.clone(),
            details: self.details.clone(),
        }
    }
}

impl IntoResponse for ApiFailure {
    fn into_response(self) -> Response {
        let status = self.kind().status();

        // Falha do daemon é `error`; falha do cliente é `debug`. Um cliente
        // insistindo numa rota inexistente não pode encher o log de erros.
        if self.kind() == ApiErrorKind::Internal {
            tracing::error!(
                code = self.kind().code(),
                status = status.as_u16(),
                internal = self.internal.as_deref().unwrap_or("sem detalhe"),
                "falha interna ao atender a requisição"
            );
        } else {
            tracing::debug!(
                code = self.kind().code(),
                status = status.as_u16(),
                internal = self.internal.as_deref().unwrap_or("sem detalhe"),
                message = self.message,
                "requisição recusada"
            );
        }

        (status, Json(self.body())).into_response()
    }
}

/// Qualquer falha de persistência é problema do daemon, nunca do cliente.
impl From<DbError> for ApiFailure {
    fn from(error: DbError) -> Self {
        Self::internal("persistência", error)
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
    use axum::body::to_bytes;

    use super::*;

    async fn into_body(failure: ApiFailure) -> (StatusCode, ApiError) {
        let response = failure.into_response();
        let status = response.status();
        let bytes = to_bytes(response.into_body(), 64 * 1024).await.expect("corpo da resposta");
        (status, serde_json::from_slice(&bytes).expect("corpo no formato ApiError"))
    }

    #[test]
    fn cada_classe_tem_codigo_e_status_estaveis() {
        let esperado = [
            (ApiErrorKind::HardwareUnavailable, "hardware.unavailable", 503),
            (ApiErrorKind::ProtocolVersionMismatch, "protocol.version_mismatch", 400),
            (ApiErrorKind::InvalidRequest, "request.invalid", 400),
            (ApiErrorKind::ShuttingDown, "daemon.shutting_down", 503),
            (ApiErrorKind::RouteNotFound, "route.not_found", 404),
            (ApiErrorKind::MethodNotAllowed, "route.method_not_allowed", 405),
            (ApiErrorKind::Internal, "internal", 500),
        ];

        for (kind, code, status) in esperado {
            assert_eq!(kind.code(), code);
            assert_eq!(kind.status().as_u16(), status);
        }
    }

    #[tokio::test]
    async fn o_detalhe_interno_nunca_entra_na_resposta() {
        let failure = ApiFailure::internal(
            "persistência",
            "UNIQUE constraint failed em /home/alguem/.local/share/elara/agentd.sqlite3",
        );

        let (status, body) = into_body(failure).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body.code, "internal");

        let serialized = serde_json::to_string(&body).expect("serialização");
        assert!(!serialized.contains("sqlite3"), "caminho de arquivo vazou: {serialized}");
        assert!(!serialized.contains("UNIQUE"), "fragmento de SQL vazou: {serialized}");
    }

    #[tokio::test]
    async fn erro_de_banco_vira_falha_interna_sem_vazar_nada() {
        let failure = ApiFailure::from(DbError::InvalidValue {
            field: "hardware_profiles.ram_total_mb",
            detail: "valor negativo: -1".to_owned(),
        });

        let (status, body) = into_body(failure).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body.code, "internal");
        assert_eq!(body.details.as_deref(), Some("operação: persistência"));
        assert!(!body.message.contains("ram_total_mb"));
    }

    #[tokio::test]
    async fn detalhe_explicito_chega_ao_cliente() {
        let failure = ApiFailure::new(ApiErrorKind::HardwareUnavailable, "hardware indisponível")
            .with_details("tente POST /v1/hardware/refresh")
            .with_internal("io: /proc/cpuinfo: permission denied");

        let (status, body) = into_body(failure).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body.code, "hardware.unavailable");
        assert_eq!(body.details.as_deref(), Some("tente POST /v1/hardware/refresh"));
        assert!(!body.message.contains("/proc/cpuinfo"));
    }
}
