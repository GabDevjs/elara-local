//! `agentd` — daemon local do Elara.
//!
//! Um único processo, um Unix socket, sem porta TCP aberta. Ele detecta o
//! hardware, amostra recursos, mantém o event bus, persiste o que precisa
//! sobreviver a reinícios e supervisiona o agent-runtime.

mod api;
mod config;
mod db;
mod hardware;
mod server;
mod state;
mod supervisor;

/// Aparato compartilhado pelos testes: sobe o daemon num socket temporário com
/// um provider de hardware de fixture.
#[cfg(test)]
mod testing;

/// Testes de integração. Moram dentro do crate porque `agentd` é um binário —
/// um teste em `tests/` não conseguiria montar o estado nem chamar o servidor.
#[cfg(test)]
mod tests;

use std::process::ExitCode;

fn main() -> ExitCode {
    match server::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            // O logging pode nem ter subido ainda; stderr é o único canal garantido.
            eprintln!("agentd: {error:#}");
            ExitCode::FAILURE
        }
    }
}
