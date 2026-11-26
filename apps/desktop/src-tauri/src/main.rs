//! Janela desktop do Elara.
//!
//! O processo faz três coisas: sobe o webview, expõe os comandos que falam com
//! o `agentd` e mantém a task que retransmite os eventos do daemon. Nada de
//! lógica de domínio — ela mora no daemon, e a UI a lê pelo mesmo contrato que
//! qualquer outro cliente usaria.

// O console extra do Windows só interessa em depuração; no bundle final ele
// aparece atrás da janela e assusta o usuário.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod daemon;
mod error;
mod events;

use std::process::ExitCode;

use tracing_subscriber::EnvFilter;

use crate::daemon::DaemonClient;

/// Variável que controla o filtro de log, igual à do daemon.
const LOG_ENV: &str = "ELARA_LOG";

/// Filtro aplicado quando [`LOG_ENV`] não está definida ou é inválida.
const LOG_DEFAULT: &str = "info,elara_desktop=info";

fn main() -> ExitCode {
    init_tracing();

    let client = DaemonClient::from_env();
    if let Some(location) = client.location() {
        tracing::info!(
            socket = %location.path.display(),
            source = ?location.source,
            "socket do daemon"
        );
    } else {
        tracing::warn!(
            "sem ELARA_SOCKET nem XDG_RUNTIME_DIR: a janela abre, mas não há daemon para consultar"
        );
    }

    let events_client = client.clone();
    let result = tauri::Builder::default()
        .manage(client)
        .setup(move |app| {
            events::spawn(app.handle().clone(), events_client);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::daemon_health,
            commands::daemon_status,
            commands::hardware_profile,
            commands::hardware_history,
            commands::resources_latest,
            commands::daemon_socket_path,
        ])
        .run(tauri::generate_context!());

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "a janela do Elara terminou com erro");
            eprintln!("elara-desktop: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Configura o log estruturado.
///
/// Um filtro inválido não derruba a janela: cair para o padrão e seguir é
/// melhor do que o aplicativo não abrir por causa de uma variável de ambiente
/// mal digitada.
fn init_tracing() {
    let filter = EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new(LOG_DEFAULT));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_writer(std::io::stderr)
        .init();
}
