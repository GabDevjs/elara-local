//! Persistência em `SQLite`.
//!
//! Duas regras moldam o módulo:
//!
//! 1. **Nenhuma chamada ao `SQLite` acontece dentro de um handler async.** O
//!    driver é bloqueante; uma consulta lenta em cima de uma thread do executor
//!    trava todas as conexões do daemon, inclusive o stream de eventos. Toda
//!    operação assíncrona daqui passa por [`tokio::task::spawn_blocking`].
//! 2. **Uma conexão só, atrás de um `Mutex`.** O banco é local, o volume é de
//!    dezenas de escritas por minuto e um pool traria o problema de lock do
//!    `SQLite` sem nenhum ganho mensurável. O `busy_timeout` continua ligado
//!    para o caso de outro processo (um `sqlite3` de diagnóstico) abrir o mesmo
//!    arquivo.
//!
//! O SQL propriamente dito mora em [`queries`], que é síncrono e testável sem
//! runtime; aqui fica só a ponte para o mundo assíncrono.

pub mod migrations;
pub mod queries;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use elara_hardware::HardwareProfile;
use elara_telemetry::types::EventEnvelope;
use jiff::Timestamp;
use rusqlite::Connection;

use crate::api::types::HardwareHistoryEntry;

/// Quanto tempo esperar por um lock do arquivo antes de desistir.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Quantos eventos ficam guardados.
///
/// Retenção por contagem, e não por idade: o que importa para diagnóstico é
/// "os últimos N eventos antes do problema", e uma máquina que ficou uma semana
/// desligada não pode acordar com a tabela vazia.
pub const EVENT_RETENTION: u32 = 2_000;

/// Falhas de persistência.
#[derive(Debug, thiserror::Error)]
pub enum DbError {
    /// O diretório do banco não pôde ser criado.
    #[error("não foi possível criar o diretório {path}: {source}")]
    Directory {
        path: String,
        #[source]
        source: std::io::Error,
    },

    /// O arquivo não pôde ser aberto ou configurado.
    #[error("não foi possível abrir o banco em {path}: {source}")]
    Open {
        path: String,
        #[source]
        source: rusqlite::Error,
    },

    /// Uma migração falhou; o banco continua na versão anterior.
    #[error("a migração {version} falhou: {source}")]
    Migration {
        version: u32,
        #[source]
        source: rusqlite::Error,
    },

    /// O arquivo foi escrito por um daemon mais novo.
    #[error("banco na versão {found}, mais nova que a suportada ({supported})")]
    UnsupportedVersion { found: u32, supported: u32 },

    /// Uma consulta falhou.
    #[error("a operação {operation} falhou no banco: {source}")]
    Query {
        operation: &'static str,
        #[source]
        source: rusqlite::Error,
    },

    /// Um valor não pôde ser serializado para JSON.
    #[error("não foi possível serializar o dado de {operation}: {source}")]
    Encode {
        operation: &'static str,
        #[source]
        source: serde_json::Error,
    },

    /// Um valor gravado não cabe no domínio, nas duas direções.
    #[error("valor inválido em {field}: {detail}")]
    InvalidValue { field: &'static str, detail: String },

    /// A thread bloqueante morreu antes de terminar a operação.
    #[error("a task de banco de {operation} não terminou")]
    TaskFailed { operation: &'static str },
}

/// Handle do banco, barato de clonar e compartilhável entre handlers.
#[derive(Debug, Clone)]
pub struct Database {
    connection: Arc<Mutex<Connection>>,
    path: PathBuf,
}

impl Database {
    /// Abre (ou cria) o banco em `path`, com os pragmas e as migrações
    /// aplicados.
    ///
    /// Síncrona porque só é chamada na subida do daemon, antes de existir
    /// qualquer requisição para atrasar.
    pub fn open(path: &Path) -> Result<Self, DbError> {
        if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|source| DbError::Directory {
                path: parent.display().to_string(),
                source,
            })?;
        }

        let connection = Connection::open(path)
            .map_err(|source| DbError::Open { path: path.display().to_string(), source })?;

        Self::prepare(connection, path.to_path_buf())
    }

    /// Banco em memória, para testes que só precisam do esquema.
    ///
    /// Não é um mock: é o mesmo `SQLite`, com as mesmas migrações e as mesmas
    /// consultas — só o arquivo é que não existe. O daemon sempre abre um
    /// arquivo de verdade, por isso o construtor é exclusivo dos testes.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self, DbError> {
        let connection = Connection::open_in_memory()
            .map_err(|source| DbError::Open { path: ":memory:".to_owned(), source })?;

        Self::prepare(connection, PathBuf::from(":memory:"))
    }

    /// Aplica pragmas e migrações a uma conexão recém-aberta.
    fn prepare(mut connection: Connection, path: PathBuf) -> Result<Self, DbError> {
        let open_error =
            |source: rusqlite::Error| DbError::Open { path: path.display().to_string(), source };

        // WAL deixa o leitor (o handler de `/v1/hardware/history`) e o escritor
        // (o gravador de eventos) trabalharem ao mesmo tempo. Em banco de
        // memória o pragma é aceito e ignorado pelo `SQLite`, então não há
        // caminho especial aqui.
        connection
            .query_row("PRAGMA journal_mode = WAL", [], |row| row.get::<_, String>(0))
            .map_err(open_error)?;

        connection.busy_timeout(BUSY_TIMEOUT).map_err(open_error)?;
        connection.pragma_update(None, "foreign_keys", "ON").map_err(open_error)?;
        // `NORMAL` com WAL só arrisca perder as últimas transações num corte de
        // energia — nenhuma delas é dado que o usuário digitou.
        connection.pragma_update(None, "synchronous", "NORMAL").map_err(open_error)?;

        migrations::apply(&mut connection)?;

        Ok(Self { connection: Arc::new(Mutex::new(connection)), path })
    }

    /// Caminho do arquivo, para mensagens de log.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Executa uma operação bloqueante fora do executor async.
    ///
    /// O `operation` é um rótulo estático que entra no erro: sem ele, um
    /// `rusqlite::Error` genérico não diz qual consulta falhou.
    async fn call<T, F>(&self, operation: &'static str, work: F) -> Result<T, DbError>
    where
        F: FnOnce(&mut Connection) -> Result<T, DbError> + Send + 'static,
        T: Send + 'static,
    {
        let connection = Arc::clone(&self.connection);

        let joined = tokio::task::spawn_blocking(move || {
            // Envenenamento do mutex significa que uma consulta anterior entrou
            // em pânico. O banco em si continua íntegro (o `SQLite` faz rollback
            // da transação aberta), então recuperar é melhor que derrubar o
            // daemon inteiro.
            let mut guard: MutexGuard<'_, Connection> =
                connection.lock().unwrap_or_else(PoisonError::into_inner);
            work(&mut guard)
        })
        .await;

        match joined {
            Ok(result) => result,
            Err(error) => {
                tracing::error!(operation, %error, "task de banco não terminou");
                Err(DbError::TaskFailed { operation })
            }
        }
    }

    /// Grava o perfil detectado e devolve quando ele foi visto pela primeira vez.
    pub async fn upsert_hardware_profile(
        &self,
        profile: &HardwareProfile,
        now: Timestamp,
    ) -> Result<Timestamp, DbError> {
        let profile = profile.clone();
        self.call("upsert_hardware_profile", move |connection| {
            queries::upsert_hardware_profile(connection, &profile, now)
        })
        .await
    }

    /// Perfis já vistos nesta máquina, do mais recente para o mais antigo.
    pub async fn hardware_history(&self, limit: u32) -> Result<Vec<HardwareHistoryEntry>, DbError> {
        self.call("hardware_history", move |connection| {
            queries::hardware_history(connection, limit)
        })
        .await
    }

    /// Guarda um evento, se ele não for de alta frequência, e apara a tabela.
    pub async fn record_event(&self, envelope: &EventEnvelope) -> Result<bool, DbError> {
        let envelope = envelope.clone();
        self.call("record_event", move |connection| {
            queries::record_event(connection, &envelope, EVENT_RETENTION)
        })
        .await
    }

    /// Valor de uma configuração persistida.
    pub async fn setting(&self, key: &str) -> Result<Option<String>, DbError> {
        let key = key.to_owned();
        self.call("setting", move |connection| queries::setting(connection, &key)).await
    }

    /// Escreve uma configuração persistida.
    pub async fn set_setting(&self, key: &str, value: &str, now: Timestamp) -> Result<(), DbError> {
        let (key, value) = (key.to_owned(), value.to_owned());
        self.call("set_setting", move |connection| {
            queries::set_setting(connection, &key, &value, now)
        })
        .await
    }
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
    use elara_hardware::MockHardwareProvider;
    use elara_telemetry::types::Event;

    use super::*;

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("instante válido")
    }

    #[tokio::test]
    async fn abre_um_arquivo_novo_ja_migrado() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        let path = dir.path().join("sub").join("agentd.sqlite3");

        let database = Database::open(&path).unwrap();
        assert!(path.exists(), "o diretório intermediário tem de ser criado");
        assert_eq!(database.path(), path);

        // Uma consulta prova que o esquema está de pé.
        assert!(database.hardware_history(10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn reabrir_o_mesmo_arquivo_preserva_os_dados() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        let path = dir.path().join("agentd.sqlite3");
        let profile = MockHardwareProvider::reference_profile();

        {
            let database = Database::open(&path).unwrap();
            database.upsert_hardware_profile(&profile, at(1_000)).await.unwrap();
        }

        let database = Database::open(&path).unwrap();
        let history = database.hardware_history(10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].first_seen_at, at(1_000));
    }

    #[tokio::test]
    async fn upsert_repetido_atualiza_sem_duplicar() {
        let database = Database::open_in_memory().unwrap();
        let profile = MockHardwareProvider::reference_profile();

        let primeiro = database.upsert_hardware_profile(&profile, at(1_000)).await.unwrap();
        let segundo = database.upsert_hardware_profile(&profile, at(9_000)).await.unwrap();

        assert_eq!(primeiro, at(1_000));
        assert_eq!(segundo, at(1_000), "o primeiro avistamento não muda");

        let history = database.hardware_history(10).await.unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].last_seen_at, at(9_000));
    }

    #[tokio::test]
    async fn eventos_sao_gravados_e_amostras_nao() {
        let database = Database::open_in_memory().unwrap();

        let stopping = EventEnvelope {
            sequence: 1,
            emitted_at: at(1_000),
            payload: Event::DaemonStopping { reason: "teste".to_owned() },
        };
        assert!(database.record_event(&stopping).await.unwrap());

        let started = EventEnvelope {
            sequence: 2,
            emitted_at: at(1_001),
            payload: Event::DaemonStarted {
                version: "0.1.0".to_owned(),
                pid: 1,
                socket_path: "/tmp/x.sock".to_owned(),
            },
        };
        assert!(database.record_event(&started).await.unwrap());
    }

    #[tokio::test]
    async fn configuracoes_sobrevivem_ao_fechamento() {
        let dir = tempfile::tempdir().expect("diretório temporário");
        let path = dir.path().join("agentd.sqlite3");

        {
            let database = Database::open(&path).unwrap();
            database.set_setting("daemon.version", "0.1.0", at(1_000)).await.unwrap();
        }

        let database = Database::open(&path).unwrap();
        assert_eq!(database.setting("daemon.version").await.unwrap(), Some("0.1.0".to_owned()));
        assert_eq!(database.setting("inexistente").await.unwrap(), None);
    }

    #[tokio::test]
    async fn o_handle_e_compartilhavel_entre_tasks() {
        let database = Database::open_in_memory().unwrap();
        let mut tasks = Vec::new();

        for index in 0..8u32 {
            let database = database.clone();
            tasks.push(tokio::spawn(async move {
                let mut profile = MockHardwareProvider::reference_profile();
                profile.fingerprint = format!("{index:032}");
                database.upsert_hardware_profile(&profile, at(1_000 + i64::from(index))).await
            }));
        }

        for task in tasks {
            task.await.expect("task").expect("upsert");
        }

        assert_eq!(database.hardware_history(50).await.unwrap().len(), 8);
    }
}
