//! Comandos expostos ao webview.
//!
//! Cada um repassa o JSON do daemon sem tocá-lo. A tentação de "arrumar" um
//! campo aqui é o começo de um segundo contrato: se a resposta do daemon
//! precisa de ajuste, o lugar do ajuste é o daemon.

use serde_json::{Value, json};
use tauri::State;

use crate::daemon::DaemonClient;
use crate::error::BridgeError;

/// Quantidade máxima de entradas devolvidas por [`hardware_history`].
///
/// O daemon também limita; repetir o teto aqui evita mandar um `limit=0` ou um
/// número absurdo e receber um 400 que a UI teria de explicar.
const MAX_HISTORY_LIMIT: u32 = 200;

/// `GET /v1/health` — sonda barata usada para decidir se a UI mostra a tela de
/// daemon offline.
#[tauri::command]
pub async fn daemon_health(client: State<'_, DaemonClient>) -> Result<Value, BridgeError> {
    client.get_json("/v1/health").await
}

/// `GET /v1/status` — versão, pid, socket e estado de cada componente.
#[tauri::command]
pub async fn daemon_status(client: State<'_, DaemonClient>) -> Result<Value, BridgeError> {
    client.get_json("/v1/status").await
}

/// `GET /v1/hardware`, ou `POST /v1/hardware/refresh` quando `refresh`.
///
/// São a mesma tela do ponto de vista da UI, e o daemon devolve o mesmo corpo:
/// separar em dois comandos duplicaria o parsing do outro lado.
#[tauri::command]
pub async fn hardware_profile(
    client: State<'_, DaemonClient>,
    refresh: bool,
) -> Result<Value, BridgeError> {
    if refresh {
        client.post_json("/v1/hardware/refresh").await
    } else {
        client.get_json("/v1/hardware").await
    }
}

/// `GET /v1/hardware/history` — perfis já vistos nesta máquina.
#[tauri::command]
pub async fn hardware_history(
    client: State<'_, DaemonClient>,
    limit: u32,
) -> Result<Value, BridgeError> {
    let limit = limit.clamp(1, MAX_HISTORY_LIMIT);
    client.get_json(&format!("/v1/hardware/history?limit={limit}")).await
}

/// `GET /v1/resources` — última amostra calculada.
///
/// A UI a usa para semear o gráfico no primeiro render; depois disso quem
/// alimenta o histórico é o stream de eventos.
#[tauri::command]
pub async fn resources_latest(client: State<'_, DaemonClient>) -> Result<Value, BridgeError> {
    client.get_json("/v1/resources").await
}

/// Caminho do socket e de onde ele veio, sem tocar na rede.
///
/// É o único comando que funciona com o daemon fora do ar — e por isso é o que
/// alimenta a tela de daemon offline.
#[tauri::command]
#[allow(
    clippy::needless_pass_by_value,
    reason = "a assinatura é ditada pelo macro de comando do Tauri: `State` entra por valor"
)]
pub fn daemon_socket_path(client: State<'_, DaemonClient>) -> Result<Value, BridgeError> {
    let location = client.location().ok_or(BridgeError::SocketUnresolved)?;
    Ok(json!({
        "socketPath": location.path.display().to_string(),
        "source": location.source,
    }))
}
