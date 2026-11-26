//! Supervisão de um processo filho: spawn, reinício com backoff e desligamento.
//!
//! O modelo é deliberadamente pequeno — um handle supervisiona um processo — e
//! as decisões estão todas explícitas aqui:
//!
//! - **Nunca há shell.** `program` e `args` viram `argv` diretamente, então
//!   nenhum valor vindo de configuração pode virar comando.
//! - **Saída limpa é fim de vida.** Um processo que termina com código 0 fez o
//!   que tinha de fazer; reiniciá-lo transformaria o supervisor em um laço.
//!   Só falha (código != 0 ou sinal) dispara reinício.
//! - **Reinício tem teto.** Depois de `max_restarts` o estado final é
//!   [`ComponentState::Failed`] e a task termina. Um supervisor que insiste para
//!   sempre em um binário quebrado é indistinguível de um bug de CPU 100%.
//! - **A saída do filho é reemitida, não herdada.** `stdout` e `stderr` são
//!   lidos linha a linha e viram eventos do `tracing` com o campo `component`;
//!   herdar os descritores misturaria texto solto no log estruturado do daemon.

use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Stdio};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use elara_telemetry::EventBus;
use elara_telemetry::types::{ComponentState, ComponentStatus, Event};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt as _, AsyncRead, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::ProcessError;

/// Prazo dado ao filho quando o handle é descartado sem `shutdown` explícito.
///
/// Existe para que um daemon que entra em pânico não deixe processos órfãos
/// segurando GPU ou porta.
const DROP_GRACE: Duration = Duration::from_secs(5);

/// O que rodar e como reagir quando terminar.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessSpec {
    /// Identificador do componente, usado no [`ComponentStatus`], nos eventos e
    /// como campo dos logs reemitidos.
    pub name: String,
    /// Caminho do executável, ou um nome simples a ser resolvido no `PATH`.
    pub program: PathBuf,
    /// `argv[1..]`. Cada elemento é um argumento inteiro: nada é dividido por
    /// espaço, nada é expandido por shell.
    pub args: Vec<String>,
    /// Diretório de trabalho do filho. `None` herda o do daemon.
    pub cwd: Option<PathBuf>,
    /// Variáveis adicionadas ao ambiente herdado do daemon.
    pub env: Vec<(String, String)>,
    /// Quando e quantas vezes tentar de novo depois de uma falha.
    pub restart: RestartPolicy,
}

impl ProcessSpec {
    /// Cria uma spec mínima: sem argumentos, sem ambiente extra e com a política
    /// de reinício padrão.
    pub fn new(name: impl Into<String>, program: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            program: program.into(),
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            restart: RestartPolicy::default(),
        }
    }
}

/// Quantas vezes e com que espera reiniciar um processo que falhou.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartPolicy {
    /// Teto de reinícios. `0` desliga o reinício automático.
    pub max_restarts: u32,
    /// Espera antes do primeiro reinício.
    pub initial_backoff: Duration,
    /// Teto da progressão exponencial.
    pub max_backoff: Duration,
}

impl Default for RestartPolicy {
    /// Cinco tentativas entre 500 ms e 30 s.
    ///
    /// O número vem do caso que interessa: um runtime que falha por uma condição
    /// transitória (porta ocupada, disco momentaneamente cheio) costuma se
    /// resolver em segundos; um que falha por bug de código falha igual na
    /// quinta vez, e aí é melhor parar e mostrar o erro do que mascará-lo.
    fn default() -> Self {
        Self {
            max_restarts: 5,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
        }
    }
}

impl RestartPolicy {
    /// Espera antes do reinício de índice `attempt` (base zero), dobrando a cada
    /// tentativa até `max_backoff`.
    pub fn backoff_for(&self, attempt: u32) -> Duration {
        // O `min(31)` evita o overflow do deslocamento; a saturação da
        // multiplicação e o `min` final cuidam do resto, então nenhuma
        // tentativa muito alta produz uma espera absurda ou um pânico.
        let factor = 1u32 << attempt.min(31);
        self.initial_backoff.saturating_mul(factor).min(self.max_backoff)
    }
}

/// Fábrica de supervisões ligada a um barramento de eventos.
#[derive(Debug)]
pub struct Supervisor {
    bus: Arc<EventBus>,
}

impl Supervisor {
    /// Cria o supervisor. Todas as transições de estado dos filhos serão
    /// publicadas em `bus`.
    pub fn new(bus: Arc<EventBus>) -> Self {
        Self { bus }
    }

    /// Inicia o processo e a task que o vigia.
    ///
    /// A resolução do executável e o primeiro spawn são síncronos, para que um
    /// erro de configuração apareça imediatamente para quem chamou em vez de
    /// virar um estado `Failed` assíncrono difícil de correlacionar.
    ///
    /// Precisa ser chamado de dentro de um runtime `tokio`.
    pub fn spawn(&self, spec: ProcessSpec) -> Result<SupervisorHandle, ProcessError> {
        let program = resolve_program(&spec.program)?;

        let status = Arc::new(Mutex::new(ComponentStatus {
            name: spec.name.clone(),
            state: ComponentState::Stopped,
            detail: Some("ainda não iniciado".to_owned()),
            since: jiff::Timestamp::now(),
            restarts: 0,
        }));

        let context = SupervisionContext {
            spec,
            program,
            bus: Arc::clone(&self.bus),
            status: Arc::clone(&status),
        };

        context.transition(ComponentState::Starting, Some("iniciando processo".to_owned()), 0);
        let mut child = match spawn_child(&context.spec, &context.program) {
            Ok(child) => child,
            Err(error) => {
                context.transition(ComponentState::Failed, Some(error.to_string()), 0);
                return Err(error);
            }
        };
        forward_output(&mut child, &context.spec.name);
        context.transition(ComponentState::Ready, None, 0);

        let name = context.spec.name.clone();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(supervise(context, child, shutdown_rx));

        Ok(SupervisorHandle {
            name,
            status,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            task: Mutex::new(Some(task)),
        })
    }
}

/// Controle sobre um processo supervisionado.
///
/// Descartar o handle sem chamar [`SupervisorHandle::shutdown`] também encerra o
/// processo — com o prazo fixo de [`DROP_GRACE`] —, porque um filho sem
/// supervisor é um órfão.
#[derive(Debug)]
pub struct SupervisorHandle {
    name: String,
    status: Arc<Mutex<ComponentStatus>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<ShutdownRequest>>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl SupervisorHandle {
    /// Estado corrente do componente, já com contagem de reinícios e o motivo do
    /// último estado não-`Ready`.
    pub fn status(&self) -> ComponentStatus {
        lock(&self.status).clone()
    }

    /// Encerra o processo de forma ordenada.
    ///
    /// Envia SIGTERM, espera até `timeout` e só então manda SIGKILL. Devolver
    /// [`ProcessError::ShutdownTimeout`] não é um erro fatal: é a informação de
    /// que o filho foi morto à força, que o chamador precisa para decidir se
    /// pode reutilizar recursos (socket, lock de arquivo, GPU) na sequência.
    ///
    /// Idempotente: a segunda chamada apenas espera a task terminar.
    pub async fn shutdown(&self, timeout: Duration) -> Result<(), ProcessError> {
        let sender = lock(&self.shutdown_tx).take();
        let mut forced = false;

        if let Some(sender) = sender {
            let (ack_tx, ack_rx) = oneshot::channel();
            if sender.send(ShutdownRequest { timeout, ack: ack_tx }).is_ok() {
                forced = matches!(ack_rx.await, Ok(ShutdownOutcome::Forced));
            }
        }

        let task = lock(&self.task).take();
        if let Some(task) = task {
            let _ = task.await;
        }

        if forced {
            return Err(ProcessError::ShutdownTimeout {
                name: self.name.clone(),
                timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
            });
        }
        Ok(())
    }
}

/// Pedido de desligamento enviado do handle para a task supervisora.
#[derive(Debug)]
struct ShutdownRequest {
    timeout: Duration,
    ack: oneshot::Sender<ShutdownOutcome>,
}

/// Como o processo efetivamente terminou.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShutdownOutcome {
    /// Saiu sozinho depois do SIGTERM, dentro do prazo.
    Graceful,
    /// Precisou de SIGKILL.
    Forced,
}

/// Tudo que a task supervisora precisa carregar entre reinícios.
#[derive(Debug)]
struct SupervisionContext {
    spec: ProcessSpec,
    program: PathBuf,
    bus: Arc<EventBus>,
    status: Arc<Mutex<ComponentStatus>>,
}

impl SupervisionContext {
    /// Atualiza o estado compartilhado e publica a transição no bus.
    ///
    /// As duas coisas juntas de propósito: quem lê `status()` e quem escuta o
    /// bus precisam concordar, e um caminho separado para cada um sempre acaba
    /// divergindo.
    fn transition(&self, to: ComponentState, detail: Option<String>, restarts: u32) {
        let from = {
            let mut status = lock(&self.status);
            let from = status.state;
            status.state = to;
            status.detail.clone_from(&detail);
            status.since = jiff::Timestamp::now();
            status.restarts = restarts;
            from
        };

        self.bus.publish(Event::ComponentStateChanged {
            component: self.spec.name.clone(),
            from,
            to,
            detail,
        });
    }
}

/// Laço de supervisão: espera o processo, decide se reinicia, atende o
/// desligamento.
async fn supervise(
    context: SupervisionContext,
    mut child: Child,
    mut shutdown_rx: oneshot::Receiver<ShutdownRequest>,
) {
    let mut restarts: u32 = 0;

    loop {
        let exit = tokio::select! {
            request = &mut shutdown_rx => {
                stop_now(&context, &mut child, request, restarts).await;
                return;
            }
            result = child.wait() => result,
        };

        let mut failure = match exit {
            Ok(status) if status.success() => {
                context.transition(
                    ComponentState::Stopped,
                    Some("processo terminou com código 0".to_owned()),
                    restarts,
                );
                return;
            }
            Ok(status) => describe_exit(status),
            Err(error) => format!("não foi possível aguardar o processo: {error}"),
        };
        context.transition(ComponentState::Failed, Some(failure.clone()), restarts);

        // Sequência de reinício: cada volta deste laço é uma tentativa, e uma
        // tentativa que nem consegue dar spawn também conta para o teto.
        loop {
            if restarts >= context.spec.restart.max_restarts {
                // O motivo da última falha entra junto com o limite: sem ele o
                // usuário sabe que desistimos, mas não do quê.
                let limit = ProcessError::RestartLimitExceeded {
                    name: context.spec.name.clone(),
                    restarts,
                };
                let detail = format!("{limit} (última falha: {failure})");
                context.transition(ComponentState::Failed, Some(detail), restarts);
                return;
            }

            let backoff = context.spec.restart.backoff_for(restarts);
            tokio::select! {
                request = &mut shutdown_rx => {
                    // Sem filho vivo neste ponto: só registrar e confirmar.
                    context.transition(
                        ComponentState::Stopped,
                        Some("desligamento pedido durante o backoff".to_owned()),
                        restarts,
                    );
                    if let Ok(request) = request {
                        let _ = request.ack.send(ShutdownOutcome::Graceful);
                    }
                    return;
                }
                () = tokio::time::sleep(backoff) => {}
            }

            restarts = restarts.saturating_add(1);
            context.transition(
                ComponentState::Starting,
                Some(format!("reiniciando (tentativa {restarts})")),
                restarts,
            );

            match spawn_child(&context.spec, &context.program) {
                Ok(mut new_child) => {
                    forward_output(&mut new_child, &context.spec.name);
                    child = new_child;
                    context.transition(ComponentState::Ready, None, restarts);
                    break;
                }
                Err(error) => {
                    failure = error.to_string();
                    context.transition(ComponentState::Failed, Some(failure.clone()), restarts);
                }
            }
        }
    }
}

/// Executa o desligamento pedido (ou implícito, quando o handle sumiu).
async fn stop_now(
    context: &SupervisionContext,
    child: &mut Child,
    request: Result<ShutdownRequest, oneshot::error::RecvError>,
    restarts: u32,
) {
    let (timeout, ack) = match request {
        Ok(request) => (request.timeout, Some(request.ack)),
        // Handle descartado: ninguém vai ler o resultado, mas o filho ainda
        // precisa morrer.
        Err(_) => (DROP_GRACE, None),
    };

    let outcome = terminate(child, timeout).await;
    let detail = match outcome {
        ShutdownOutcome::Graceful => "encerrado a pedido".to_owned(),
        ShutdownOutcome::Forced => "não respondeu ao SIGTERM; encerrado com SIGKILL".to_owned(),
    };
    context.transition(ComponentState::Stopped, Some(detail), restarts);

    if let Some(ack) = ack {
        let _ = ack.send(outcome);
    }
}

/// SIGTERM, prazo, SIGKILL.
async fn terminate(child: &mut Child, timeout: Duration) -> ShutdownOutcome {
    // `try_wait` colhe o filho se ele já morreu; a partir daí `id()` devolve
    // `None` e não há risco de sinalizar um PID reciclado.
    if matches!(child.try_wait(), Ok(Some(_))) {
        return ShutdownOutcome::Graceful;
    }

    match child.id() {
        Some(pid) => send_sigterm(pid),
        None => return ShutdownOutcome::Graceful,
    }

    if tokio::time::timeout(timeout, child.wait()).await.is_ok() {
        return ShutdownOutcome::Graceful;
    }

    let _ = child.kill().await;
    let _ = child.wait().await;
    ShutdownOutcome::Forced
}

/// Envia SIGTERM ao processo.
///
/// `libc::kill` é a única forma de mandar SIGTERM: `std` e `tokio` só expõem
/// SIGKILL (`Child::kill`), que não dá ao filho chance de fechar arquivos.
#[allow(unsafe_code)]
fn send_sigterm(pid: u32) {
    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    // SAFETY: `kill` só lê os dois inteiros passados; o PID pertence a um filho
    // ainda não colhido (garantido pelo `try_wait` anterior), portanto não pode
    // ter sido reciclado por outro processo.
    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
}

/// Descreve uma saída anormal em português, distinguindo código de sinal.
fn describe_exit(status: ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt as _;

    if let Some(code) = status.code() {
        return format!("processo terminou com código {code}");
    }
    match status.signal() {
        Some(signal) => format!("processo terminou por sinal {signal}"),
        None => "processo terminou por motivo desconhecido".to_owned(),
    }
}

// ---------------------------------------------------------------------------
// Spawn e resolução do executável
// ---------------------------------------------------------------------------

/// Monta e dispara o `Command`.
///
/// `stdin` é `/dev/null` e as duas saídas são pipes: um filho supervisionado não
/// tem terminal para ler nem deve escrever no terminal do daemon.
fn spawn_child(spec: &ProcessSpec, program: &Path) -> Result<Child, ProcessError> {
    let mut command = Command::new(program);
    command
        .args(&spec.args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Rede de segurança: se o runtime do daemon cair, a task some junto com
        // o `Child` e o filho é morto em vez de ficar órfão.
        .kill_on_drop(true);

    if let Some(cwd) = &spec.cwd {
        command.current_dir(cwd);
    }
    for (key, value) in &spec.env {
        command.env(key, value);
    }

    command
        .spawn()
        .map_err(|source| ProcessError::Spawn { program: program.display().to_string(), source })
}

/// Localiza o executável antes de tentar o spawn.
///
/// Sem isto, um caminho errado voltaria como um `io::Error` genérico de
/// `NotFound` já dentro da task de supervisão — depois de o estado ter oscilado
/// e de o backoff ter começado.
fn resolve_program(program: &Path) -> Result<PathBuf, ProcessError> {
    let not_found = || ProcessError::ProgramNotFound { program: program.display().to_string() };

    // Com separador, o caminho é literal: procurar no `PATH` mascararia um erro
    // de configuração.
    if program.components().count() > 1 || program.is_absolute() {
        return if is_executable(program) { Ok(program.to_path_buf()) } else { Err(not_found()) };
    }

    let path = std::env::var_os("PATH").ok_or_else(not_found)?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(program))
        .find(|candidate| is_executable(candidate))
        .ok_or_else(not_found)
}

/// Verdadeiro para arquivo regular com algum bit de execução.
///
/// Diretório e arquivo sem `+x` são tratados como "não encontrado": para quem
/// configurou o daemon, os três casos têm a mesma correção.
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

// ---------------------------------------------------------------------------
// Reemissão da saída do filho
// ---------------------------------------------------------------------------

/// Liga `stdout` e `stderr` do filho ao `tracing`.
fn forward_output(child: &mut Child, component: &str) {
    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(forward_lines(stdout, component.to_owned(), OutputStream::Stdout));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(forward_lines(stderr, component.to_owned(), OutputStream::Stderr));
    }
}

/// Qual descritor do filho originou a linha.
#[derive(Debug, Clone, Copy)]
enum OutputStream {
    Stdout,
    Stderr,
}

/// Lê linha a linha até o EOF (que acontece quando o filho morre) e reemite cada
/// linha como um evento do `tracing`.
///
/// `stderr` sai em nível `warn` porque é lá que quase todo runtime escreve o que
/// deu errado; se for ruído, o filtro de log resolve sem perder a linha.
async fn forward_lines<R>(reader: R, component: String, stream: OutputStream)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut lines = BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => match stream {
                OutputStream::Stdout => {
                    tracing::info!(component = %component, stream = "stdout", "{line}");
                }
                OutputStream::Stderr => {
                    tracing::warn!(component = %component, stream = "stderr", "{line}");
                }
            },
            Ok(None) => break,
            Err(error) => {
                tracing::debug!(component = %component, %error, "leitura da saída interrompida");
                break;
            }
        }
    }
}

/// Lock que ignora envenenamento: o estado protegido é um retrato de
/// diagnóstico, e recusar a leitura por causa de um pânico alheio esconderia
/// justamente o que se quer diagnosticar.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use std::io::Write as _;

    use tokio::io::AsyncReadExt as _;

    use super::*;

    /// Espera o estado satisfazer `predicate`, com teto para não travar a suíte.
    async fn wait_until(
        handle: &SupervisorHandle,
        what: &str,
        predicate: impl Fn(&ComponentStatus) -> bool,
    ) -> ComponentStatus {
        for _ in 0..600 {
            let status = handle.status();
            if predicate(&status) {
                return status;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("{what} não aconteceu; estado atual: {:?}", handle.status());
    }

    /// Espera o componente atingir `expected`.
    async fn wait_for_state(
        handle: &SupervisorHandle,
        expected: ComponentState,
    ) -> ComponentStatus {
        wait_until(handle, &format!("estado {expected:?}"), |status| status.state == expected).await
    }

    /// Espera o supervisor desistir de vez (estado final, não um `Failed`
    /// intermediário entre dois reinícios).
    async fn wait_for_give_up(handle: &SupervisorHandle) -> ComponentStatus {
        wait_until(handle, "desistência final", |status| {
            status.state == ComponentState::Failed
                && status.detail.as_deref().is_some_and(|detail| detail.contains("desistindo"))
        })
        .await
    }

    /// Lê tudo o que o filho escreveu em `stdout`, provando que o descritor é um
    /// pipe nosso e não o do daemon.
    async fn read_stdout(child: &mut Child) -> String {
        let mut stdout = child.stdout.take().expect("stdout precisa ser um pipe");
        let mut captured = String::new();
        stdout.read_to_string(&mut captured).await.expect("ler stdout");
        captured
    }

    fn fast_policy(max_restarts: u32) -> RestartPolicy {
        RestartPolicy {
            max_restarts,
            initial_backoff: Duration::from_millis(5),
            max_backoff: Duration::from_millis(20),
        }
    }

    fn collect_transitions(bus: &EventBus) -> Vec<(ComponentState, ComponentState)> {
        bus.recent(256)
            .into_iter()
            .rev()
            .filter_map(|envelope| match envelope.payload {
                Event::ComponentStateChanged { from, to, .. } => Some((from, to)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn backoff_dobra_ate_o_teto() {
        let policy = RestartPolicy {
            max_restarts: 10,
            initial_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(4),
        };

        assert_eq!(policy.backoff_for(0), Duration::from_millis(500));
        assert_eq!(policy.backoff_for(1), Duration::from_secs(1));
        assert_eq!(policy.backoff_for(2), Duration::from_secs(2));
        assert_eq!(policy.backoff_for(3), Duration::from_secs(4));
        assert_eq!(
            policy.backoff_for(4),
            Duration::from_secs(4),
            "o teto não pode ser ultrapassado"
        );
        assert_eq!(policy.backoff_for(u32::MAX), Duration::from_secs(4));
    }

    #[test]
    fn politica_padrao_e_a_documentada() {
        let policy = RestartPolicy::default();
        assert_eq!(policy.max_restarts, 5);
        assert_eq!(policy.initial_backoff, Duration::from_millis(500));
        assert_eq!(policy.max_backoff, Duration::from_secs(30));
    }

    #[test]
    fn executavel_no_path_e_resolvido() {
        let resolved = resolve_program(Path::new("sleep")).expect("sleep está no PATH");
        assert!(resolved.is_absolute());
        assert!(is_executable(&resolved));
    }

    #[tokio::test]
    async fn programa_inexistente_falha_antes_do_spawn() {
        let bus = Arc::new(EventBus::new(32));
        let supervisor = Supervisor::new(Arc::clone(&bus));

        let error = supervisor
            .spawn(ProcessSpec::new("fantasma", "/nao/existe/mesmo"))
            .expect_err("deveria recusar");

        assert!(matches!(error, ProcessError::ProgramNotFound { .. }), "erro: {error:?}");
        assert!(collect_transitions(&bus).is_empty(), "nada foi iniciado, nada a publicar");
    }

    #[tokio::test]
    async fn arquivo_sem_permissao_de_execucao_nao_e_programa() {
        let directory = tempfile::tempdir().expect("tempdir");
        let path = directory.path().join("dados.txt");
        let mut file = std::fs::File::create(&path).expect("criar");
        file.write_all(b"nao sou um binario").expect("escrever");
        drop(file);

        let bus = Arc::new(EventBus::new(32));
        let supervisor = Supervisor::new(bus);
        let error =
            supervisor.spawn(ProcessSpec::new("dados", &path)).expect_err("deveria recusar");

        assert!(matches!(error, ProcessError::ProgramNotFound { .. }), "erro: {error:?}");
    }

    #[tokio::test]
    async fn processo_que_sai_com_sucesso_termina_em_stopped() {
        let bus = Arc::new(EventBus::new(64));
        let supervisor = Supervisor::new(Arc::clone(&bus));

        let mut spec = ProcessSpec::new("eco", "/bin/echo");
        spec.args = vec!["linha reemitida".to_owned()];
        let handle = supervisor.spawn(spec).expect("spawn");

        let status = wait_for_state(&handle, ComponentState::Stopped).await;
        assert_eq!(status.restarts, 0, "saída limpa não é motivo de reinício");
        assert_eq!(status.detail.as_deref(), Some("processo terminou com código 0"));

        assert_eq!(
            collect_transitions(&bus),
            vec![
                (ComponentState::Stopped, ComponentState::Starting),
                (ComponentState::Starting, ComponentState::Ready),
                (ComponentState::Ready, ComponentState::Stopped),
            ]
        );
    }

    #[tokio::test]
    async fn processo_que_falha_reinicia_ate_o_teto_e_para() {
        let bus = Arc::new(EventBus::new(256));
        let supervisor = Supervisor::new(Arc::clone(&bus));

        let mut spec = ProcessSpec::new("teimoso", "/bin/false");
        spec.restart = fast_policy(2);
        let handle = supervisor.spawn(spec).expect("spawn");

        let settled = wait_for_give_up(&handle).await;

        assert_eq!(settled.state, ComponentState::Failed);
        assert_eq!(settled.restarts, 2, "exatamente max_restarts reinícios");
        assert_eq!(
            settled.detail.as_deref(),
            Some(
                "teimoso falhou 2 vezes; desistindo (última falha: processo terminou com código 1)"
            ),
            "o estado final precisa dizer por que desistiu"
        );

        let transitions = collect_transitions(&bus);
        let restarts = transitions
            .iter()
            .filter(|(from, to)| *from == ComponentState::Failed && *to == ComponentState::Starting)
            .count();
        assert_eq!(restarts, 2, "houve backoff e reinício entre as falhas");

        // A task terminou: nada mais é publicado depois de assentar.
        let after = collect_transitions(&bus).len();
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert_eq!(collect_transitions(&bus).len(), after, "o supervisor parou de tentar");

        handle.shutdown(Duration::from_secs(1)).await.expect("shutdown de task já encerrada");
    }

    #[tokio::test]
    async fn a_espera_entre_reinicios_e_exponencial_de_verdade() {
        let bus = Arc::new(EventBus::new(64));
        let supervisor = Supervisor::new(Arc::clone(&bus));

        let mut spec = ProcessSpec::new("lento", "/bin/false");
        spec.restart = RestartPolicy {
            max_restarts: 2,
            initial_backoff: Duration::from_millis(120),
            max_backoff: Duration::from_secs(5),
        };

        let started = std::time::Instant::now();
        let handle = supervisor.spawn(spec).expect("spawn");
        let settled = wait_for_give_up(&handle).await;
        let elapsed = started.elapsed();

        assert_eq!(settled.restarts, 2);
        // 120 ms antes do primeiro reinício e 240 ms antes do segundo: se a
        // progressão fosse linear, o total ficaria em 240 ms.
        assert!(
            elapsed >= Duration::from_millis(360),
            "backoff exponencial não foi respeitado: {elapsed:?}"
        );
    }

    #[tokio::test]
    async fn sem_reinicio_permitido_a_primeira_falha_e_final() {
        let bus = Arc::new(EventBus::new(64));
        let supervisor = Supervisor::new(Arc::clone(&bus));

        let mut spec = ProcessSpec::new("frágil", "/bin/false");
        spec.restart = fast_policy(0);
        let handle = supervisor.spawn(spec).expect("spawn");

        let status = wait_for_give_up(&handle).await;
        assert_eq!(status.restarts, 0);
        let detail = status.detail.unwrap_or_default();
        assert!(detail.starts_with("frágil falhou 0 vezes; desistindo"), "detalhe: {detail}");
        assert!(detail.contains("código 1"), "detalhe: {detail}");
    }

    #[tokio::test]
    async fn shutdown_ordenado_encerra_com_sigterm() {
        let bus = Arc::new(EventBus::new(64));
        let supervisor = Supervisor::new(Arc::clone(&bus));

        let mut spec = ProcessSpec::new("dorminhoco", "/bin/sleep");
        spec.args = vec!["120".to_owned()];
        let handle = supervisor.spawn(spec).expect("spawn");
        wait_for_state(&handle, ComponentState::Ready).await;

        handle.shutdown(Duration::from_secs(5)).await.expect("sleep respeita SIGTERM");

        let status = handle.status();
        assert_eq!(status.state, ComponentState::Stopped);
        assert_eq!(status.detail.as_deref(), Some("encerrado a pedido"));
        assert_eq!(status.restarts, 0, "desligamento pedido não conta como falha");

        // Idempotente.
        handle.shutdown(Duration::from_secs(1)).await.expect("segunda chamada é no-op");
    }

    #[tokio::test]
    async fn saida_do_filho_vai_para_um_pipe_e_nao_e_herdada() {
        let mut spec = ProcessSpec::new("eco", "/bin/echo");
        spec.args = vec!["linha do filho".to_owned()];

        let mut child = spawn_child(&spec, Path::new("/bin/echo")).expect("spawn");
        assert_eq!(read_stdout(&mut child).await, "linha do filho\n");
        assert!(child.wait().await.expect("wait").success());
    }

    #[tokio::test]
    async fn cwd_do_spec_e_aplicado_ao_filho() {
        // `pwd` faz parte do mesmo coreutils que `echo`/`true`; se faltar, o
        // ambiente não tem como sustentar nenhum destes testes.
        let program = Path::new("/bin/pwd");
        assert!(is_executable(program), "/bin/pwd é necessário para este teste");

        let directory = tempfile::tempdir().expect("tempdir");
        let expected = directory.path().canonicalize().expect("canonicalizar");

        let mut spec = ProcessSpec::new("pwd", program);
        spec.cwd = Some(expected.clone());

        let mut child = spawn_child(&spec, program).expect("spawn");
        let printed = read_stdout(&mut child).await;
        child.wait().await.expect("wait");

        assert_eq!(Path::new(printed.trim_end()), expected);
    }

    #[tokio::test]
    async fn env_do_spec_chega_ao_filho_sem_apagar_o_herdado() {
        let program = Path::new("/usr/bin/env");
        assert!(is_executable(program), "/usr/bin/env é necessário para este teste");

        let mut spec = ProcessSpec::new("env", program);
        spec.env = vec![("ELARA_EXTRA".to_owned(), "do-spec".to_owned())];

        let mut child = spawn_child(&spec, program).expect("spawn");
        let printed = read_stdout(&mut child).await;
        child.wait().await.expect("wait");

        assert!(printed.contains("ELARA_EXTRA=do-spec"), "ambiente: {printed}");
        // `PATH` não está no spec: se chegou ao filho, foi por herança.
        assert!(
            printed.contains("PATH="),
            "o ambiente do daemon precisa ser herdado, não substituído: {printed}"
        );
    }
}
