//! Configuração do daemon: linha de comando, ambiente e caminhos XDG.
//!
//! A resolução de caminhos é a parte delicada. Ela precisa produzir exatamente
//! os mesmos bytes que `packages/shared/src/paths.ts` e que a ponte do desktop
//! (`apps/desktop/src-tauri/src/daemon.rs`), porque o sintoma de uma divergência
//! é um `ENOENT` num socket — um erro que não diz nada sobre a causa. Por isso a
//! ordem de precedência está escrita aqui de forma explícita e é testada com e
//! sem cada variável, em vez de delegada a um crate que pode mudar de regra.
//!
//! O ambiente entra por [`Environment`], nunca por leitura direta dentro das
//! funções de resolução: é o que permite testar `XDG_RUNTIME_DIR` relativo,
//! `HOME` ausente e `ELARA_SOCKET` vazio sem mexer no processo de teste.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Parser, ValueEnum};
use elara_telemetry::LogFormat;

/// Nome do diretório do Elara dentro de cada base XDG.
///
/// Constante e não literal repetido: os quatro caminhos derivados têm de mudar
/// juntos se um dia isto virar outro nome.
const APP_DIR: &str = "elara";

/// Nome do arquivo de socket dentro do diretório de runtime.
const SOCKET_FILE: &str = "agentd.sock";

/// Nome do banco dentro do diretório de dados.
const DATABASE_FILE: &str = "agentd.sqlite3";

/// Intervalo padrão entre amostras de recurso, em milissegundos.
const DEFAULT_SAMPLE_INTERVAL_MS: u64 = 1_000;

/// Filtro de log padrão, no formato do `EnvFilter`.
const DEFAULT_LOG_FILTER: &str = "info";

/// Falhas ao montar a configuração.
///
/// Só entra aqui o que impede o daemon de decidir onde escrever. Valor ausente
/// com padrão razoável não é erro; valor presente e impossível é.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// Não há como derivar o diretório de dados sem saber onde é a casa do
    /// usuário.
    #[error(
        "não foi possível descobrir o diretório do usuário; \
         defina HOME, XDG_DATA_HOME ou ELARA_DATA_DIR"
    )]
    HomeUnknown,

    /// Um intervalo de zero faria o amostrador rodar em laço fechado.
    #[error("ELARA_SAMPLE_INTERVAL_MS não pode ser 0")]
    ZeroSampleInterval,
}

/// Formato de log aceito na linha de comando e em `ELARA_LOG_FORMAT`.
///
/// Duplica [`LogFormat`] porque o tipo do crate de telemetria não deriva
/// `ValueEnum` — e não deveria: ele não conhece o `clap`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
#[value(rename_all = "lowercase")]
pub enum LogFormatArg {
    /// Uma linha JSON por evento.
    Json,
    /// Multilinha e colorido, para desenvolvimento.
    Pretty,
}

impl From<LogFormatArg> for LogFormat {
    fn from(value: LogFormatArg) -> Self {
        match value {
            LogFormatArg::Json => Self::Json,
            LogFormatArg::Pretty => Self::Pretty,
        }
    }
}

/// Opções cruas do daemon, como vêm da linha de comando ou do ambiente.
///
/// Todos os campos são `Option` de propósito: o padrão de cada um depende de
/// outros valores (o socket depende de `XDG_RUNTIME_DIR`, os dados dependem de
/// `HOME`), e um `default_value` do `clap` não consegue expressar isso.
#[derive(Debug, Clone, Default, Parser)]
#[command(
    name = "agentd",
    version,
    about = "Daemon local do Elara: hardware, recursos, eventos e supervisão de runtimes.",
    long_about = None
)]
pub struct Cli {
    /// Caminho do socket Unix. Padrão: `$XDG_RUNTIME_DIR/elara/agentd.sock`.
    #[arg(long, value_name = "CAMINHO", env = "ELARA_SOCKET")]
    pub socket: Option<PathBuf>,

    /// Diretório de dados. Padrão: `${XDG_DATA_HOME:-~/.local/share}/elara`.
    #[arg(long = "data-dir", value_name = "CAMINHO", env = "ELARA_DATA_DIR")]
    pub data_dir: Option<PathBuf>,

    /// Filtro de log no formato do `EnvFilter`, ex.: `info,agentd=debug`.
    #[arg(long = "log", value_name = "FILTRO", env = "ELARA_LOG")]
    pub log: Option<String>,

    /// Formato do log no console.
    #[arg(long = "log-format", value_name = "FORMATO", env = "ELARA_LOG_FORMAT")]
    pub log_format: Option<LogFormatArg>,

    /// Intervalo entre amostras de recurso, em milissegundos.
    #[arg(long = "sample-interval-ms", value_name = "MS", env = "ELARA_SAMPLE_INTERVAL_MS")]
    pub sample_interval_ms: Option<u64>,

    /// Comando do agent-runtime a supervisionar. Sem ele, o componente fica
    /// indisponível e o daemon segue funcionando.
    #[arg(long = "runtime-cmd", value_name = "COMANDO", env = "ELARA_RUNTIME_CMD")]
    pub runtime_cmd: Option<String>,

    /// Diretório de trabalho do agent-runtime.
    #[arg(long = "runtime-cwd", value_name = "CAMINHO", env = "ELARA_RUNTIME_CWD")]
    pub runtime_cwd: Option<PathBuf>,
}

/// Entradas do ambiente usadas na resolução de caminhos.
///
/// Um struct de campos explícitos, e não um mapa: são exatamente estas quatro
/// coisas que mudam o resultado, e vê-las na assinatura é o que impede que uma
/// quinta apareça sem teste.
#[derive(Debug, Clone)]
pub struct Environment {
    /// `XDG_RUNTIME_DIR`, cru. Valor relativo é descartado, como manda a XDG.
    pub runtime_dir: Option<String>,
    /// `XDG_DATA_HOME`, cru.
    pub data_home: Option<String>,
    /// Diretório do usuário.
    pub home: Option<PathBuf>,
    /// UID, usado só no fallback `/run/user/{uid}` de `XDG_RUNTIME_DIR`.
    pub uid: u32,
}

impl Environment {
    /// Lê o ambiente real do processo.
    pub fn from_process() -> Self {
        Self {
            runtime_dir: std::env::var("XDG_RUNTIME_DIR").ok(),
            data_home: std::env::var("XDG_DATA_HOME").ok(),
            home: directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
            uid: current_uid(),
        }
    }
}

/// UID do processo.
///
/// `getuid(2)` não tem equivalente na biblioteca padrão e é a única forma de
/// reproduzir o fallback `/run/user/{uid}` que o TypeScript já implementa.
#[allow(
    unsafe_code,
    reason = "getuid(2) não recebe ponteiro, não falha e não tem equivalente seguro na std"
)]
fn current_uid() -> u32 {
    // SAFETY: `getuid` não recebe argumentos, não pode falhar e não toca em
    // memória do chamador. É seguro em qualquer contexto, inclusive com threads.
    unsafe { libc::getuid() }
}

/// Configuração já resolvida: nenhum campo aqui depende mais do ambiente.
#[derive(Debug, Clone)]
pub struct Config {
    /// Onde o daemon cria e escuta o socket Unix.
    pub socket_path: PathBuf,
    /// Raiz de tudo que precisa sobreviver a reinícios.
    pub data_dir: PathBuf,
    /// Arquivo `SQLite`, dentro de [`Config::data_dir`].
    pub database_path: PathBuf,
    /// Caminho-base do log em arquivo; a rotação diária acrescenta a data.
    pub log_file: PathBuf,
    /// Diretiva do `EnvFilter`.
    pub log_filter: String,
    /// Formato do log no console.
    pub log_format: LogFormat,
    /// Período entre amostras de recurso.
    pub sample_interval: Duration,
    /// Linha de comando do agent-runtime, ainda não dividida em `argv`.
    pub runtime_command: Option<String>,
    /// Diretório de trabalho do agent-runtime.
    pub runtime_cwd: Option<PathBuf>,
}

impl Config {
    /// Monta a configuração a partir da linha de comando e do ambiente real.
    ///
    /// Sai do processo com a mensagem de uso do `clap` quando os argumentos são
    /// inválidos — comportamento desejado para um binário, e o motivo de a
    /// análise de argumentos não estar em [`Config::resolve`].
    pub fn load() -> Result<Self, ConfigError> {
        Self::resolve(Cli::parse(), &Environment::from_process())
    }

    /// Aplica a precedência `opção explícita → variável XDG → padrão`.
    pub fn resolve(cli: Cli, env: &Environment) -> Result<Self, ConfigError> {
        let socket_path = resolve_socket(cli.socket.as_deref(), env);
        let data_dir = resolve_data_dir(cli.data_dir.as_deref(), env)?;

        let sample_interval_ms = cli.sample_interval_ms.unwrap_or(DEFAULT_SAMPLE_INTERVAL_MS);
        if sample_interval_ms == 0 {
            return Err(ConfigError::ZeroSampleInterval);
        }

        Ok(Self {
            socket_path,
            database_path: data_dir.join(DATABASE_FILE),
            log_file: data_dir.join("logs").join("agentd.log"),
            data_dir,
            log_filter: non_empty_string(cli.log).unwrap_or_else(|| DEFAULT_LOG_FILTER.to_owned()),
            log_format: cli.log_format.unwrap_or(LogFormatArg::Json).into(),
            sample_interval: Duration::from_millis(sample_interval_ms),
            runtime_command: non_empty_string(cli.runtime_cmd),
            runtime_cwd: cli.runtime_cwd.filter(|path| !path.as_os_str().is_empty()),
        })
    }
}

/// Resolve o caminho do socket.
///
/// `ELARA_SOCKET` é o caminho do **arquivo**, não de um diretório: quem a
/// define escolheu o nome inteiro, e é assim que `scripts/dev.mjs` e o
/// `paths.ts` a tratam.
fn resolve_socket(explicit: Option<&Path>, env: &Environment) -> PathBuf {
    if let Some(path) = explicit.filter(|path| !path.as_os_str().is_empty()) {
        return path.to_path_buf();
    }

    let base = absolute_or_none(env.runtime_dir.as_deref())
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", env.uid)));

    base.join(APP_DIR).join(SOCKET_FILE)
}

/// Resolve o diretório de dados.
fn resolve_data_dir(explicit: Option<&Path>, env: &Environment) -> Result<PathBuf, ConfigError> {
    if let Some(path) = explicit.filter(|path| !path.as_os_str().is_empty()) {
        return Ok(path.to_path_buf());
    }

    let base = match absolute_or_none(env.data_home.as_deref()) {
        Some(path) => path,
        None => env.home.clone().ok_or(ConfigError::HomeUnknown)?.join(".local").join("share"),
    };

    Ok(base.join(APP_DIR))
}

/// A especificação XDG manda ignorar valores relativos.
///
/// Não é preciosismo: um `XDG_DATA_HOME=data` herdado de um script faria o
/// daemon e o agent-runtime resolverem caminhos diferentes conforme o diretório
/// de trabalho de cada processo.
fn absolute_or_none(value: Option<&str>) -> Option<PathBuf> {
    let path = PathBuf::from(value.filter(|raw| !raw.is_empty())?);
    path.is_absolute().then_some(path)
}

/// Variável vazia é tratada como ausente, como faz o shell.
fn non_empty_string(value: Option<String>) -> Option<String> {
    value.filter(|raw| !raw.is_empty())
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    reason = "em teste, um panic é o relatório de falha desejado"
)]
mod tests {
    use super::*;

    fn env() -> Environment {
        Environment {
            runtime_dir: Some("/run/user/1000".to_owned()),
            data_home: None,
            home: Some(PathBuf::from("/home/alguem")),
            uid: 1000,
        }
    }

    fn resolve(cli: Cli, env: &Environment) -> Config {
        Config::resolve(cli, env).expect("configuração válida")
    }

    #[test]
    fn padroes_xdg_batem_com_o_paths_ts() {
        let config = resolve(Cli::default(), &env());
        assert_eq!(config.socket_path, PathBuf::from("/run/user/1000/elara/agentd.sock"));
        assert_eq!(config.data_dir, PathBuf::from("/home/alguem/.local/share/elara"));
        assert_eq!(
            config.database_path,
            PathBuf::from("/home/alguem/.local/share/elara/agentd.sqlite3")
        );
    }

    #[test]
    fn sem_xdg_runtime_dir_o_fallback_usa_o_uid() {
        let mut environment = env();
        environment.runtime_dir = None;
        environment.uid = 4242;

        let config = resolve(Cli::default(), &environment);
        assert_eq!(config.socket_path, PathBuf::from("/run/user/4242/elara/agentd.sock"));
    }

    #[test]
    fn xdg_runtime_dir_relativo_e_ignorado() {
        let mut environment = env();
        environment.runtime_dir = Some("runtime".to_owned());

        let config = resolve(Cli::default(), &environment);
        assert_eq!(
            config.socket_path,
            PathBuf::from("/run/user/1000/elara/agentd.sock"),
            "valor relativo tem de cair no fallback, como no paths.ts"
        );
    }

    #[test]
    fn xdg_data_home_absoluto_vence_o_home() {
        let mut environment = env();
        environment.data_home = Some("/srv/dados".to_owned());

        let config = resolve(Cli::default(), &environment);
        assert_eq!(config.data_dir, PathBuf::from("/srv/dados/elara"));
    }

    #[test]
    fn xdg_data_home_relativo_e_ignorado() {
        let mut environment = env();
        environment.data_home = Some("dados".to_owned());

        let config = resolve(Cli::default(), &environment);
        assert_eq!(config.data_dir, PathBuf::from("/home/alguem/.local/share/elara"));
    }

    #[test]
    fn elara_socket_vence_o_xdg_e_define_o_proprio_diretorio() {
        let cli = Cli {
            socket: Some(PathBuf::from("/run/user/1000/elara-dev/agentd.sock")),
            ..Cli::default()
        };

        let config = resolve(cli, &env());
        assert_eq!(config.socket_path, PathBuf::from("/run/user/1000/elara-dev/agentd.sock"));
        assert_eq!(config.socket_path.parent(), Some(Path::new("/run/user/1000/elara-dev")));
    }

    #[test]
    fn elara_data_dir_vence_o_xdg_data_home() {
        let mut environment = env();
        environment.data_home = Some("/srv/dados".to_owned());
        let cli = Cli { data_dir: Some(PathBuf::from("/opt/elara")), ..Cli::default() };

        let config = resolve(cli, &environment);
        assert_eq!(config.data_dir, PathBuf::from("/opt/elara"));
    }

    #[test]
    fn variaveis_vazias_valem_como_ausentes() {
        let cli = Cli {
            socket: Some(PathBuf::new()),
            data_dir: Some(PathBuf::new()),
            log: Some(String::new()),
            runtime_cmd: Some(String::new()),
            runtime_cwd: Some(PathBuf::new()),
            ..Cli::default()
        };

        let config = resolve(cli, &env());
        assert_eq!(config.socket_path, PathBuf::from("/run/user/1000/elara/agentd.sock"));
        assert_eq!(config.data_dir, PathBuf::from("/home/alguem/.local/share/elara"));
        assert_eq!(config.log_filter, "info");
        assert_eq!(config.runtime_command, None);
        assert_eq!(config.runtime_cwd, None);
    }

    #[test]
    fn sem_home_e_sem_data_home_a_resolucao_falha_em_vez_de_chutar() {
        let mut environment = env();
        environment.home = None;

        let error = Config::resolve(Cli::default(), &environment).unwrap_err();
        assert!(matches!(error, ConfigError::HomeUnknown));
    }

    #[test]
    fn sem_home_mas_com_elara_data_dir_a_resolucao_funciona() {
        let mut environment = env();
        environment.home = None;
        let cli = Cli { data_dir: Some(PathBuf::from("/opt/elara")), ..Cli::default() };

        let config = resolve(cli, &environment);
        assert_eq!(config.data_dir, PathBuf::from("/opt/elara"));
    }

    #[test]
    fn intervalo_de_amostragem_zero_e_recusado() {
        let cli = Cli { sample_interval_ms: Some(0), ..Cli::default() };
        let error = Config::resolve(cli, &env()).unwrap_err();
        assert!(matches!(error, ConfigError::ZeroSampleInterval));
    }

    #[test]
    fn intervalo_padrao_e_um_segundo() {
        let config = resolve(Cli::default(), &env());
        assert_eq!(config.sample_interval, Duration::from_secs(1));
    }

    #[test]
    fn formato_de_log_padrao_e_json() {
        let config = resolve(Cli::default(), &env());
        assert_eq!(config.log_format, LogFormat::Json);

        let cli = Cli { log_format: Some(LogFormatArg::Pretty), ..Cli::default() };
        assert_eq!(resolve(cli, &env()).log_format, LogFormat::Pretty);
    }

    #[test]
    fn o_log_em_arquivo_fica_sob_o_diretorio_de_dados() {
        let config = resolve(Cli::default(), &env());
        assert!(config.log_file.starts_with(&config.data_dir));
        assert_eq!(config.log_file.file_name().and_then(|n| n.to_str()), Some("agentd.log"));
    }

    #[test]
    fn a_linha_de_comando_do_clap_e_valida() {
        // `debug_assert` do clap: pega opção duplicada ou nome inválido, que só
        // apareceriam no primeiro `--help` de um usuário.
        use clap::CommandFactory as _;
        Cli::command().debug_assert();
    }

    #[test]
    fn argumentos_longos_sobrescrevem_o_ambiente() {
        let cli = Cli::try_parse_from([
            "agentd",
            "--socket",
            "/tmp/elara-teste/agentd.sock",
            "--sample-interval-ms",
            "250",
        ])
        .expect("argumentos válidos");

        let config = resolve(cli, &env());
        assert_eq!(config.socket_path, PathBuf::from("/tmp/elara-teste/agentd.sock"));
        assert_eq!(config.sample_interval, Duration::from_millis(250));
    }
}
