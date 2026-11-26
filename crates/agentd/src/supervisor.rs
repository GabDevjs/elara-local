//! Supervisão do agent-runtime.
//!
//! O daemon não é um init system: ele supervisiona **um** processo, o
//! agent-runtime, e só quando `ELARA_RUNTIME_CMD` diz qual é. A mecânica de
//! spawn, backoff e desligamento inteira vem do `elara-process-manager`; o que
//! está aqui é a política de quando nem tentar.
//!
//! A regra que importa: **não ter um runtime não é erro**. Numa instalação de
//! desenvolvimento o runtime é iniciado à mão pelo `pnpm`; num pacote futuro o
//! binário pode não ter sido instalado. Nos dois casos o daemon precisa subir,
//! responder `/v1/hardware` e dizer, em `/v1/status`, exatamente por que o
//! componente está indisponível. Matar o daemon transformaria um recurso
//! ausente em uma máquina sem telemetria.
//!
//! O comando **nunca** passa por um shell: [`split_command`] quebra a string em
//! `argv` respeitando aspas, e é só isso. Não há expansão de variável, de `~`,
//! de glob nem de subcomando — um `ELARA_RUNTIME_CMD` mal escrito produz um
//! executável não encontrado, jamais um comando extra executado.

use std::sync::Arc;
use std::time::Duration;

use elara_process_manager::{ProcessError, ProcessSpec, Supervisor, SupervisorHandle};
use elara_telemetry::EventBus;
use elara_telemetry::types::{ComponentState, ComponentStatus};

use crate::config::Config;

/// Nome do componente do agent-runtime em `/v1/status` e nos eventos.
pub const RUNTIME_COMPONENT: &str = "agent-runtime";

/// Prazo dado ao runtime entre o SIGTERM e o SIGKILL.
///
/// Cinco segundos porque o desligamento do runtime é só fechar o socket e
/// anunciar a saída; mais que isso significa processo travado, e nesse caso o
/// desligamento do daemon não pode ficar refém dele.
pub const RUNTIME_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Falhas ao interpretar `ELARA_RUNTIME_CMD`.
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    /// A variável existe mas não contém nenhuma palavra.
    #[error("o comando do runtime está vazio")]
    Empty,

    /// Aspas abertas e nunca fechadas: dividir mesmo assim produziria um `argv`
    /// diferente do que quem escreveu quis dizer.
    #[error("aspas não fechadas no comando do runtime: {command:?}")]
    UnclosedQuote { command: String },

    /// Uma barra invertida no fim da string não tem o que proteger.
    #[error("barra invertida sem caractere seguinte no comando do runtime: {command:?}")]
    TrailingEscape { command: String },
}

/// Estado da supervisão do agent-runtime.
#[derive(Debug)]
pub struct RuntimeSupervision {
    state: Supervision,
}

/// Há um processo vigiado, ou há um motivo para não haver.
#[derive(Debug)]
enum Supervision {
    /// Processo iniciado; o estado vem do handle, ao vivo.
    Running(Box<SupervisorHandle>),
    /// Nenhum processo. O estado é fixo e carrega o motivo.
    Absent(ComponentStatus),
}

impl RuntimeSupervision {
    /// Decide o que fazer com `ELARA_RUNTIME_CMD` e, se houver o que fazer,
    /// inicia a supervisão.
    ///
    /// Precisa ser chamada de dentro de um runtime `tokio`.
    pub fn start(config: &Config, bus: Arc<EventBus>) -> Self {
        let Some(command) = config.runtime_command.as_deref() else {
            return Self::absent(
                ComponentState::Unavailable,
                "ELARA_RUNTIME_CMD não está definida; nenhum agent-runtime será iniciado por \
                 este daemon",
            );
        };

        let tokens = match split_command(command) {
            Ok(tokens) => tokens,
            Err(error) => return Self::absent(ComponentState::Failed, error.to_string()),
        };

        let Some((program, arguments)) = tokens.split_first() else {
            return Self::absent(ComponentState::Failed, CommandError::Empty.to_string());
        };

        let spec = ProcessSpec {
            args: arguments.to_vec(),
            cwd: config.runtime_cwd.clone(),
            ..ProcessSpec::new(RUNTIME_COMPONENT, program)
        };

        match Supervisor::new(bus).spawn(spec) {
            Ok(handle) => Self { state: Supervision::Running(Box::new(handle)) },
            Err(ProcessError::ProgramNotFound { program }) => Self::absent(
                ComponentState::Unavailable,
                format!(
                    "o executável {program:?} de ELARA_RUNTIME_CMD não existe ou não é \
                     executável; o daemon segue sem agent-runtime"
                ),
            ),
            Err(error) => Self::absent(ComponentState::Failed, error.to_string()),
        }
    }

    /// Supervisão vazia, para o daemon que roda sem runtime nenhum.
    fn absent(state: ComponentState, detail: impl Into<String>) -> Self {
        Self {
            state: Supervision::Absent(ComponentStatus {
                name: RUNTIME_COMPONENT.to_owned(),
                state,
                detail: Some(detail.into()),
                since: jiff::Timestamp::now(),
                restarts: 0,
            }),
        }
    }

    /// `true` quando existe um processo filho sendo vigiado.
    ///
    /// Distingue "não supervisionamos" de "supervisionamos e ele caiu": quem lê
    /// `/v1/status` precisa saber se um runtime registrado por fora é esperado.
    pub fn is_supervised(&self) -> bool {
        matches!(self.state, Supervision::Running(_))
    }

    /// Estado corrente do componente.
    pub fn status(&self) -> ComponentStatus {
        match &self.state {
            Supervision::Running(handle) => handle.status(),
            Supervision::Absent(status) => status.clone(),
        }
    }

    /// Encerra o processo, se houver.
    ///
    /// Não devolve erro: um SIGKILL depois do prazo já é a informação relevante,
    /// e ela vira log — o desligamento do daemon não tem o que fazer com um
    /// `Result` além de registrá-lo.
    pub async fn shutdown(&self) {
        let Supervision::Running(handle) = &self.state else {
            return;
        };

        match handle.shutdown(RUNTIME_SHUTDOWN_TIMEOUT).await {
            Ok(()) => tracing::info!(component = RUNTIME_COMPONENT, "processo encerrado"),
            Err(error) => {
                tracing::warn!(component = RUNTIME_COMPONENT, %error, "encerramento forçado");
            }
        }
    }
}

/// Quebra uma linha de comando em `argv`, **sem shell**.
///
/// Reconhece exatamente três coisas: espaço em branco separa argumentos, aspas
/// simples e duplas agrupam, e `\` **fora de aspas** protege o caractere
/// seguinte. Dentro de aspas, de qualquer tipo, a barra invertida é literal —
/// é o comportamento mais previsível para quem escreve um caminho do Windows ou
/// uma expressão regular num argumento.
///
/// Tudo o mais — `$VAR`, `~`, `*`, `|`, `;` — é texto literal, porque
/// interpretá-lo transformaria uma variável de ambiente em execução arbitrária
/// de comando.
pub fn split_command(command: &str) -> Result<Vec<String>, CommandError> {
    let mut argv = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut characters = command.chars();

    while let Some(character) = characters.next() {
        match (quote, character) {
            // Dentro de aspas simples nada é especial, nem a barra invertida:
            // é a única forma de passar um `\` literal como argumento.
            (Some('\''), '\'') | (Some('"'), '"') => quote = None,
            (Some(_), _) => current.push(character),

            (None, '\'' | '"') => {
                quote = Some(character);
                started = true;
            }
            (None, '\\') => match characters.next() {
                Some(escaped) => {
                    current.push(escaped);
                    started = true;
                }
                None => return Err(CommandError::TrailingEscape { command: command.to_owned() }),
            },
            (None, _) if character.is_whitespace() => {
                if started {
                    argv.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            (None, _) => {
                current.push(character);
                started = true;
            }
        }
    }

    if quote.is_some() {
        return Err(CommandError::UnclosedQuote { command: command.to_owned() });
    }
    if started {
        argv.push(current);
    }
    if argv.is_empty() {
        return Err(CommandError::Empty);
    }

    Ok(argv)
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
    use std::path::PathBuf;

    use super::*;
    use crate::config::{Cli, Environment};

    fn config_with(command: Option<&str>) -> Config {
        let environment = Environment {
            runtime_dir: Some("/run/user/1000".to_owned()),
            data_home: None,
            home: Some(PathBuf::from("/home/alguem")),
            uid: 1000,
        };
        let cli = Cli { runtime_cmd: command.map(str::to_owned), ..Cli::default() };
        Config::resolve(cli, &environment).expect("configuração válida")
    }

    #[test]
    fn split_separa_por_espaco() {
        assert_eq!(
            split_command("pnpm --silent --filter @elara/agent-runtime start").unwrap(),
            vec!["pnpm", "--silent", "--filter", "@elara/agent-runtime", "start"]
        );
    }

    #[test]
    fn split_colapsa_espacos_repetidos_e_bordas() {
        assert_eq!(split_command("  node   servidor.js  ").unwrap(), vec!["node", "servidor.js"]);
    }

    #[test]
    fn split_respeita_aspas_duplas_e_simples() {
        assert_eq!(
            split_command(r#""/opt/meus programas/rt" --nome 'agent runtime'"#).unwrap(),
            vec!["/opt/meus programas/rt", "--nome", "agent runtime"]
        );
    }

    #[test]
    fn split_permite_argumento_vazio_entre_aspas() {
        assert_eq!(split_command(r#"prog "" fim"#).unwrap(), vec!["prog", "", "fim"]);
    }

    #[test]
    fn split_nao_expande_nada() {
        let argv = split_command("prog $HOME ~/x *.gguf | rm -rf /").unwrap();
        assert_eq!(argv, vec!["prog", "$HOME", "~/x", "*.gguf", "|", "rm", "-rf", "/"]);
    }

    #[test]
    fn barra_invertida_protege_o_proximo_caractere() {
        assert_eq!(split_command(r"prog um\ argumento").unwrap(), vec!["prog", "um argumento"]);
        assert_eq!(split_command(r"prog a\'b").unwrap(), vec!["prog", "a'b"]);
    }

    #[test]
    fn dentro_de_aspas_a_barra_e_literal() {
        assert_eq!(split_command(r"prog 'a\b'").unwrap(), vec!["prog", r"a\b"]);
        assert_eq!(split_command(r#"prog "C:\tmp""#).unwrap(), vec!["prog", r"C:\tmp"]);
    }

    #[test]
    fn aspas_nao_fechadas_sao_erro_em_vez_de_palpite() {
        assert!(matches!(
            split_command(r#"prog "sem fim"#).unwrap_err(),
            CommandError::UnclosedQuote { .. }
        ));
        assert!(matches!(
            split_command(r"prog fim\").unwrap_err(),
            CommandError::TrailingEscape { .. }
        ));
    }

    #[test]
    fn comando_em_branco_e_erro() {
        assert!(matches!(split_command("   ").unwrap_err(), CommandError::Empty));
        assert!(matches!(split_command("").unwrap_err(), CommandError::Empty));
    }

    #[tokio::test]
    async fn sem_variavel_o_componente_fica_indisponivel_e_nao_falha() {
        let bus = Arc::new(EventBus::new(16));
        let supervision = RuntimeSupervision::start(&config_with(None), bus);

        let status = supervision.status();
        assert!(!supervision.is_supervised());
        assert_eq!(status.name, RUNTIME_COMPONENT);
        assert_eq!(status.state, ComponentState::Unavailable);
        assert!(status.detail.unwrap_or_default().contains("ELARA_RUNTIME_CMD"));

        // Desligar uma supervisão que nunca existiu precisa ser um no-op.
        supervision.shutdown().await;
    }

    #[tokio::test]
    async fn executavel_inexistente_fica_indisponivel_e_o_daemon_segue() {
        let bus = Arc::new(EventBus::new(16));
        let supervision =
            RuntimeSupervision::start(&config_with(Some("/nao/existe/agent-runtime")), bus);

        let status = supervision.status();
        assert!(!supervision.is_supervised());
        assert_eq!(status.state, ComponentState::Unavailable);
        assert!(status.detail.unwrap_or_default().contains("/nao/existe/agent-runtime"));
    }

    #[tokio::test]
    async fn comando_mal_formado_vira_falha_com_detalhe() {
        let bus = Arc::new(EventBus::new(16));
        let supervision = RuntimeSupervision::start(&config_with(Some(r#"prog "sem fim"#)), bus);

        let status = supervision.status();
        assert_eq!(status.state, ComponentState::Failed);
        assert!(status.detail.unwrap_or_default().contains("aspas"));
    }

    #[tokio::test]
    async fn processo_de_verdade_e_supervisionado_e_desligado() {
        let bus = Arc::new(EventBus::new(16));
        let supervision = RuntimeSupervision::start(&config_with(Some("/bin/sleep 120")), bus);

        assert!(supervision.is_supervised());
        assert_eq!(supervision.status().state, ComponentState::Ready);

        supervision.shutdown().await;
        assert_eq!(supervision.status().state, ComponentState::Stopped);
    }

    #[tokio::test]
    async fn o_diretorio_de_trabalho_configurado_e_usado() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        let environment = Environment {
            runtime_dir: Some("/run/user/1000".to_owned()),
            data_home: None,
            home: Some(PathBuf::from("/home/alguem")),
            uid: 1000,
        };
        let cli = Cli {
            runtime_cmd: Some("/bin/sleep 120".to_owned()),
            runtime_cwd: Some(dir.path().to_path_buf()),
            ..Cli::default()
        };
        let config = Config::resolve(cli, &environment).expect("configuração válida");

        let bus = Arc::new(EventBus::new(16));
        let supervision = RuntimeSupervision::start(&config, bus);
        assert!(supervision.is_supervised());
        supervision.shutdown().await;
    }
}
