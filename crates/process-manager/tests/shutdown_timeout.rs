//! Desligamento de um processo que ignora SIGTERM.
//!
//! Vive em um binário de teste próprio porque a montagem do cenário mexe na
//! disposição de SIGTERM **deste** processo: no Unix, uma disposição `SIG_IGN` é
//! herdada pelo filho através do `fork` **e** do `exec`, então basta ignorar
//! SIGTERM enquanto o `/bin/sleep` nasce para obter um filho teimoso sem
//! precisar de shell, de script auxiliar ou de um binário de fixture.
//!
//! Um único teste no arquivo garante que nenhuma outra thread esteja criando
//! processos durante a janela em que a disposição está trocada.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;
use std::time::Duration;

use elara_process_manager::{ProcessError, ProcessSpec, Supervisor};
use elara_telemetry::EventBus;
use elara_telemetry::types::ComponentState;

/// Troca a disposição de SIGTERM e devolve a anterior.
///
/// # Safety
/// Só pode ser chamada com uma thread ativa criando processos — ver o comentário
/// de módulo.
#[allow(unsafe_code)]
fn set_sigterm_disposition(handler: libc::sighandler_t) -> libc::sighandler_t {
    // SAFETY: `signal` é async-signal-safe e este binário de teste tem um único
    // teste, portanto nenhuma outra thread depende da disposição de SIGTERM.
    unsafe { libc::signal(libc::SIGTERM, handler) }
}

#[tokio::test]
async fn processo_que_ignora_sigterm_e_morto_e_o_chamador_fica_sabendo() {
    let bus = Arc::new(EventBus::new(32));
    let supervisor = Supervisor::new(Arc::clone(&bus));

    let mut spec = ProcessSpec::new("teimoso", "/bin/sleep");
    spec.args = vec!["120".to_owned()];

    // O filho herda SIG_IGN e passa a ignorar o SIGTERM do supervisor.
    let previous = set_sigterm_disposition(libc::SIG_IGN);
    let handle = supervisor.spawn(spec);
    set_sigterm_disposition(previous);

    let handle = handle.expect("spawn");
    assert_eq!(handle.status().state, ComponentState::Ready);

    let started = std::time::Instant::now();
    let error = handle
        .shutdown(Duration::from_millis(300))
        .await
        .expect_err("um processo que ignora SIGTERM precisa ser reportado como forçado");

    match error {
        ProcessError::ShutdownTimeout { name, timeout_ms } => {
            assert_eq!(name, "teimoso");
            assert_eq!(timeout_ms, 300);
        }
        other => panic!("erro inesperado: {other:?}"),
    }

    assert!(
        started.elapsed() >= Duration::from_millis(300),
        "o SIGKILL só pode vir depois do prazo"
    );

    let status = handle.status();
    assert_eq!(status.state, ComponentState::Stopped, "forçado, mas parado");
    assert_eq!(status.detail.as_deref(), Some("não respondeu ao SIGTERM; encerrado com SIGKILL"));
}
