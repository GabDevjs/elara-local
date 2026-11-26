//! Retransmissão do stream SSE do daemon para o webview.
//!
//! O daemon pode subir depois da janela, cair e voltar. A task abaixo assume
//! isso como o caso normal: ela reconecta indefinidamente e avisa a UI em cada
//! transição, para que a tela de "daemon offline" apareça e suma sozinha sem o
//! usuário precisar recarregar nada.

use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Emitter, Runtime};

use crate::daemon::DaemonClient;

/// Evento Tauri que carrega cada `EventEnvelope` do daemon, sem alteração.
pub const EVENT_CHANNEL: &str = "elara://event";

/// Evento Tauri com `{ connected: bool }`, emitido só nas transições.
pub const CONNECTION_CHANNEL: &str = "elara://connection";

/// Primeira espera após uma queda.
const BACKOFF_MIN: Duration = Duration::from_millis(500);

/// Teto da espera entre tentativas.
///
/// Cinco segundos: quem está subindo o daemon num terminal ao lado não deve
/// esperar mais do que isso para a janela reagir.
const BACKOFF_MAX: Duration = Duration::from_secs(5);

/// Sobe a task de eventos. Ela vive enquanto o aplicativo viver.
pub fn spawn<R: Runtime>(app: AppHandle<R>, client: DaemonClient) {
    tauri::async_runtime::spawn(async move {
        run(app, client).await;
    });
}

/// Laço de conexão, leitura e reconexão.
async fn run<R: Runtime>(app: AppHandle<R>, client: DaemonClient) {
    let mut connected = false;
    let mut backoff = BACKOFF_MIN;

    loop {
        match client.open_events().await {
            Ok(mut stream) => {
                backoff = BACKOFF_MIN;
                set_connected(&app, &mut connected, true);

                loop {
                    match stream.next_envelope().await {
                        Ok(Some(envelope)) => {
                            if let Err(error) = app.emit(EVENT_CHANNEL, envelope) {
                                tracing::warn!(%error, "falha ao entregar evento ao webview");
                            }
                        }
                        Ok(None) => {
                            tracing::info!("o daemon encerrou o stream de eventos");
                            break;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "stream de eventos interrompido");
                            break;
                        }
                    }
                }

                set_connected(&app, &mut connected, false);
            }
            Err(error) => {
                // Daemon fora do ar é o estado esperado antes de alguém
                // iniciá-lo: registrar em `warn` a cada meio segundo só
                // enterraria os erros de verdade.
                if error.is_offline() {
                    tracing::debug!(%error, "aguardando o daemon");
                } else {
                    tracing::warn!(%error, "não foi possível assinar o stream de eventos");
                }
                set_connected(&app, &mut connected, false);
            }
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(BACKOFF_MAX);
    }
}

/// Emite [`CONNECTION_CHANNEL`] apenas quando o estado muda.
///
/// Reemitir a cada tentativa faria a UI piscar entre "offline" e "offline" a
/// cada meio segundo.
fn set_connected<R: Runtime>(app: &AppHandle<R>, current: &mut bool, next: bool) {
    if *current == next {
        return;
    }
    *current = next;
    if let Err(error) = app.emit(CONNECTION_CHANNEL, json!({ "connected": next })) {
        tracing::warn!(%error, "falha ao anunciar a conectividade ao webview");
    }
}
