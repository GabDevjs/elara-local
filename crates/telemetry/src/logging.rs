//! Instalação do subscriber global do `tracing`.
//!
//! O daemon precisa de duas saídas com públicos diferentes: o console, lido por
//! humanos durante o desenvolvimento, e um arquivo, lido por máquinas depois do
//! incidente. Por isso o formato do console é configurável ([`LogFormat`]) e o
//! arquivo é sempre JSON — uma linha por evento, sem cores.
//!
//! Não usamos `tracing-appender`: ele não está no workspace e a rotação de que
//! precisamos cabe em algumas dezenas de linhas ([`RotatingFileWriter`]). A troca
//! de arquivo acontece na primeira escrita de cada dia (UTC), sem thread extra e
//! sem buffer intermediário — cada evento vira um `write` no arquivo, de modo que
//! um `SIGKILL` no daemon não perde as últimas linhas.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::time::FormatTime;
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::{EnvFilter, Layer, Registry};

use crate::TelemetryError;

/// Formato do log emitido no console.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    /// Uma linha JSON por evento. É o formato de produção porque sobrevive a
    /// `grep`, `jq` e a qualquer coletor de log.
    Json,
    /// Multilinha, colorido e indentado. Só para desenvolvimento: é ilegível
    /// para máquinas e caro para escrever.
    Pretty,
}

/// Como o processo deve registrar seus eventos.
#[derive(Debug, Clone)]
pub struct LoggingConfig {
    /// Diretiva no formato do `EnvFilter` (ex.: `info,elara_agentd=debug`).
    /// Inválida vira [`TelemetryError::InvalidFilter`], nunca panic.
    pub filter: String,
    /// Formato do console.
    pub format: LogFormat,
    /// Caminho-base do log em arquivo. O arquivo realmente escrito carrega a
    /// data (`elara.log` → `elara.2026-07-27.log`), de modo que a rotação não
    /// depende de renomear arquivo aberto por outro processo.
    pub file: Option<PathBuf>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self { filter: "info".to_owned(), format: LogFormat::Json, file: None }
    }
}

/// Mantém vivos os recursos de escrita do log.
///
/// Precisa ficar vivo enquanto o processo quiser logar: ao ser descartado ele
/// força o flush do arquivo, garantindo que o último evento antes de um
/// desligamento chegue ao disco.
#[derive(Debug)]
#[must_use = "descartar o guard imediatamente perde o flush final do log em arquivo"]
pub struct LoggingGuard {
    file: Option<Arc<RotatingFileWriter>>,
}

impl LoggingGuard {
    /// Força a gravação do que estiver pendente. Útil antes de um `abort`
    /// deliberado, quando o `Drop` não vai rodar.
    pub fn flush(&self) {
        if let Some(writer) = &self.file {
            writer.flush_now();
        }
    }
}

impl Drop for LoggingGuard {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Sinaliza que este processo já instalou um subscriber por esta função.
///
/// O `tracing` também recusa a segunda instalação, mas só na hora de publicar —
/// depois de a chamada perdedora já ter criado um arquivo de log que ninguém vai
/// usar. O flag corta esse efeito colateral antes que ele aconteça.
static INSTALLED: AtomicBool = AtomicBool::new(false);

/// Instala o subscriber global do `tracing` para todo o processo.
///
/// Só pode acontecer uma vez: a segunda chamada devolve
/// [`TelemetryError::LoggingAlreadyInitialized`] em vez de derrubar o processo,
/// porque quem chama costuma ser código de inicialização que também roda em
/// testes.
pub fn init(config: LoggingConfig) -> Result<LoggingGuard, TelemetryError> {
    let LoggingConfig { filter, format, file } = config;

    // Validar o filtro antes de reservar o processo: uma diretiva errada não
    // pode consumir a única chance de inicializar o log.
    let filter = EnvFilter::try_new(&filter)
        .map_err(|source| TelemetryError::InvalidFilter { filter, detail: source.to_string() })?;

    if INSTALLED.swap(true, Ordering::SeqCst) {
        return Err(TelemetryError::LoggingAlreadyInitialized);
    }

    let file_writer = match file {
        Some(path) => match RotatingFileWriter::open(&path) {
            Ok(writer) => Some(Arc::new(writer)),
            Err(source) => {
                // Nada foi instalado ainda: devolver a reserva permite ao
                // chamador corrigir o caminho e tentar de novo.
                INSTALLED.store(false, Ordering::SeqCst);
                return Err(TelemetryError::LogFile { path: path.display().to_string(), source });
            }
        },
        None => None,
    };

    // As camadas têm tipos diferentes (JSON e Pretty não são o mesmo `Layer`),
    // então viram objetos de trait em um vetor — que o `tracing-subscriber`
    // também sabe tratar como uma camada só.
    let mut layers: Vec<Box<dyn Layer<Registry> + Send + Sync>> = Vec::with_capacity(2);

    layers.push(match format {
        LogFormat::Json => Box::new(
            fmt::layer().json().with_timer(JiffTimer).with_span_list(false).with_writer(io::stderr),
        ),
        LogFormat::Pretty => {
            Box::new(fmt::layer().pretty().with_timer(JiffTimer).with_writer(io::stderr))
        }
    });

    if let Some(writer) = file_writer.clone() {
        // O arquivo é sempre JSON, mesmo em modo `Pretty`: cores ANSI e
        // multilinha num arquivo só atrapalham quem for ler depois do incidente.
        layers.push(Box::new(
            fmt::layer()
                .json()
                .with_timer(JiffTimer)
                .with_span_list(false)
                .with_ansi(false)
                .with_writer(RotatingMakeWriter(writer)),
        ));
    }

    let subscriber = Registry::default().with(layers).with(filter);
    tracing::subscriber::set_global_default(subscriber)
        .map_err(|_| TelemetryError::LoggingAlreadyInitialized)?;

    Ok(LoggingGuard { file: file_writer })
}

// ---------------------------------------------------------------------------
// Carimbo de tempo
// ---------------------------------------------------------------------------

/// Carimba os eventos com `jiff`, a mesma fonte de tempo usada nos tipos do
/// contrato — assim o `timestamp` do log e o `emittedAt` de um evento têm
/// exatamente o mesmo formato RFC 3339.
#[derive(Debug, Clone, Copy)]
struct JiffTimer;

impl FormatTime for JiffTimer {
    fn format_time(&self, writer: &mut Writer<'_>) -> std::fmt::Result {
        write!(writer, "{}", jiff::Timestamp::now())
    }
}

// ---------------------------------------------------------------------------
// Arquivo com rotação diária
// ---------------------------------------------------------------------------

/// Arquivo de log que troca de nome quando o dia (UTC) vira.
///
/// UTC e não o fuso local de propósito: o nome do arquivo precisa ser
/// monotônico e reproduzível mesmo que a máquina mude de fuso ou entre em
/// horário de verão no meio da execução.
#[derive(Debug)]
struct RotatingFileWriter {
    state: Mutex<RotationState>,
}

/// Estado mutável protegido pelo mutex do [`RotatingFileWriter`].
#[derive(Debug)]
struct RotationState {
    directory: PathBuf,
    stem: String,
    extension: Option<String>,
    day: jiff::civil::Date,
    file: File,
}

impl RotatingFileWriter {
    /// Abre (ou cria) o arquivo do dia corrente derivado de `base`.
    fn open(base: &Path) -> io::Result<Self> {
        let directory = base.parent().unwrap_or(Path::new(".")).to_path_buf();
        if !directory.as_os_str().is_empty() {
            fs::create_dir_all(&directory)?;
        }
        let stem = base
            .file_stem()
            .map_or_else(|| "elara".to_owned(), |stem| stem.to_string_lossy().into_owned());
        let extension = base.extension().map(|ext| ext.to_string_lossy().into_owned());

        let day = today_utc();
        let file = open_day_file(&directory, &stem, extension.as_deref(), day)?;
        Ok(Self { state: Mutex::new(RotationState { directory, stem, extension, day, file }) })
    }

    /// Toma o lock ignorando envenenamento: um log meio escrito é melhor que um
    /// processo que morre porque outra thread entrou em pânico.
    fn lock(&self) -> MutexGuard<'_, RotationState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Descarrega o buffer do sistema operacional. Erros aqui não têm para onde
    /// ser reportados — quem falharia é justamente o canal de log.
    fn flush_now(&self) {
        let _ = self.lock().file.flush();
    }
}

impl RotationState {
    /// Troca o arquivo quando o dia mudou. Chamado antes de cada escrita: é uma
    /// comparação de inteiros, barata o bastante para não valer uma thread de
    /// timer só para isso.
    fn rotate_if_needed(&mut self) -> io::Result<()> {
        let today = today_utc();
        if today == self.day {
            return Ok(());
        }
        self.file.flush()?;
        self.file = open_day_file(&self.directory, &self.stem, self.extension.as_deref(), today)?;
        self.day = today;
        Ok(())
    }
}

/// Data corrente em UTC, base do nome do arquivo de log.
fn today_utc() -> jiff::civil::Date {
    jiff::Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date()
}

/// Abre em modo append o arquivo daquele dia, criando-o se necessário.
fn open_day_file(
    directory: &Path,
    stem: &str,
    extension: Option<&str>,
    day: jiff::civil::Date,
) -> io::Result<File> {
    let name = match extension {
        Some(extension) => format!("{stem}.{day}.{extension}"),
        None => format!("{stem}.{day}"),
    };
    OpenOptions::new().create(true).append(true).open(directory.join(name))
}

/// Adaptador que entrega o arquivo rotativo ao `tracing-subscriber`.
#[derive(Debug, Clone)]
struct RotatingMakeWriter(Arc<RotatingFileWriter>);

impl<'a> MakeWriter<'a> for RotatingMakeWriter {
    type Writer = RotatingWriterHandle<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        RotatingWriterHandle { state: self.0.lock() }
    }
}

/// Escrita em curso: segura o lock do arquivo até a linha inteira sair.
#[derive(Debug)]
struct RotatingWriterHandle<'a> {
    state: MutexGuard<'a, RotationState>,
}

impl Write for RotatingWriterHandle<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.state.rotate_if_needed()?;
        self.state.file.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.state.file.flush()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    #[test]
    fn filtro_invalido_vira_erro_e_nao_panic() {
        let config = LoggingConfig {
            filter: "elara=verbosissimo".to_owned(),
            format: LogFormat::Json,
            file: None,
        };
        let error = init(config).expect_err("filtro inválido deveria falhar");
        assert!(
            matches!(error, TelemetryError::InvalidFilter { .. }),
            "erro inesperado: {error:?}"
        );
    }

    #[test]
    fn segunda_inicializacao_e_recusada_sem_panic() {
        let directory = tempfile::tempdir().expect("tempdir");
        let config = LoggingConfig {
            filter: "info".to_owned(),
            format: LogFormat::Json,
            file: Some(directory.path().join("elara.log")),
        };

        let guard = init(config.clone()).expect("primeira inicialização");
        tracing::info!(teste = "logging", "linha de verificação");
        guard.flush();

        let error = init(config).expect_err("segunda inicialização deveria falhar");
        assert!(
            matches!(error, TelemetryError::LoggingAlreadyInitialized),
            "erro inesperado: {error:?}"
        );

        let day = today_utc();
        let written = fs::read_to_string(directory.path().join(format!("elara.{day}.log")))
            .expect("arquivo do dia");
        let first = written.lines().next().expect("ao menos uma linha");
        let parsed: serde_json::Value = serde_json::from_str(first).expect("linha JSON válida");
        assert!(parsed.get("timestamp").is_some(), "sem timestamp: {parsed}");
        assert_eq!(parsed.get("level").and_then(serde_json::Value::as_str), Some("INFO"));
        assert!(parsed.get("target").is_some(), "sem target: {parsed}");
        assert!(parsed.get("fields").is_some(), "sem fields: {parsed}");
    }

    #[test]
    fn nome_do_arquivo_carrega_a_data() {
        let directory = tempfile::tempdir().expect("tempdir");
        let writer = RotatingFileWriter::open(&directory.path().join("daemon.log")).expect("abrir");
        {
            let mut handle = RotatingWriterHandle { state: writer.lock() };
            handle.write_all(b"linha\n").expect("escrever");
        }
        writer.flush_now();

        let day = today_utc();
        let expected = directory.path().join(format!("daemon.{day}.log"));
        assert!(expected.exists(), "arquivo {expected:?} não foi criado");
    }

    #[test]
    fn rotacao_troca_o_arquivo_quando_o_dia_muda() {
        let directory = tempfile::tempdir().expect("tempdir");
        let writer = RotatingFileWriter::open(&directory.path().join("daemon.log")).expect("abrir");

        // Simula um processo que atravessou a meia-noite: o estado acha que
        // ainda é ontem, a próxima escrita precisa abrir o arquivo de hoje.
        let yesterday = today_utc().yesterday().expect("data anterior");
        {
            let mut state = writer.lock();
            state.day = yesterday;
        }
        {
            let mut handle = RotatingWriterHandle { state: writer.lock() };
            handle.write_all(b"depois da virada\n").expect("escrever");
        }
        writer.flush_now();

        let today = today_utc();
        assert!(directory.path().join(format!("daemon.{today}.log")).exists());
        assert_eq!(writer.lock().day, today);
    }
}
