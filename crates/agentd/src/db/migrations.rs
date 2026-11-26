//! Migrações versionadas do banco.
//!
//! O esquema é identificado por `PRAGMA user_version`, o contador que o próprio
//! `SQLite` guarda no cabeçalho do arquivo. Não há tabela de controle porque não
//! é preciso: o pragma já é atômico com a transação que o atualiza, então ou a
//! migração inteira entrou ou nada entrou.
//!
//! Regras que este módulo mantém:
//!
//! - **Uma transação por migração.** Um `CREATE TABLE` que falha no meio não
//!   pode deixar o banco entre duas versões.
//! - **Idempotência por construção.** Aplicar de novo é um no-op porque o
//!   pragma já está na versão final, não porque os `CREATE` têm `IF NOT EXISTS`
//!   — que esconderiam uma tabela divergente em vez de acusar.
//! - **Banco mais novo é erro, não aviso.** Um daemon velho abrindo um banco de
//!   uma versão futura corromperia dados que não sabe interpretar.
//!
//! Só existem aqui as tabelas das Fases 1 e 2. Modelos, tasks e workflows
//! entram junto com o código que os escreve.

use rusqlite::Connection;

use super::DbError;

/// Uma migração: o número que ela produz e o SQL que a aplica.
#[derive(Debug, Clone, Copy)]
struct Migration {
    /// Valor de `user_version` **depois** de aplicada.
    version: u32,
    /// SQL completo, executado como um lote dentro de uma transação.
    sql: &'static str,
}

/// Todas as migrações conhecidas, em ordem crescente de versão.
///
/// Acrescentar sempre no fim e nunca editar uma entrada existente: um banco já
/// migrado não reexecuta o SQL alterado, então editar produz duas máquinas com
/// esquemas diferentes e a mesma `user_version`.
const MIGRATIONS: &[Migration] = &[Migration { version: 1, sql: MIGRATION_1 }];

/// Versão de esquema que este binário sabe operar.
pub const SCHEMA_VERSION: u32 = 1;

/// Fase 1/2: perfis de hardware, configurações e histórico de eventos.
///
/// As tabelas são `STRICT` para que um `INTEGER` guardado como texto vire erro
/// na escrita, e não um `FromSql` misterioso meses depois.
const MIGRATION_1: &str = r"
CREATE TABLE hardware_profiles (
    fingerprint    TEXT    PRIMARY KEY,
    schema_version INTEGER NOT NULL,
    profile_json   TEXT    NOT NULL,
    first_seen_at  TEXT    NOT NULL,
    last_seen_at   TEXT    NOT NULL,
    cpu_model      TEXT    NOT NULL,
    ram_total_mb   INTEGER NOT NULL,
    gpu_summary    TEXT    NOT NULL
) STRICT;

CREATE INDEX hardware_profiles_last_seen ON hardware_profiles (last_seen_at DESC);

CREATE TABLE settings (
    key        TEXT PRIMARY KEY,
    value      TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

CREATE TABLE events (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    sequence     INTEGER NOT NULL,
    emitted_at   TEXT    NOT NULL,
    kind         TEXT    NOT NULL,
    payload_json TEXT    NOT NULL
) STRICT;

CREATE INDEX events_id_desc ON events (id DESC);
";

/// Leva o banco até [`SCHEMA_VERSION`], aplicando só o que falta.
pub fn apply(connection: &mut Connection) -> Result<(), DbError> {
    let current = user_version(connection)?;

    if current > SCHEMA_VERSION {
        return Err(DbError::UnsupportedVersion { found: current, supported: SCHEMA_VERSION });
    }

    for migration in MIGRATIONS.iter().filter(|migration| migration.version > current) {
        let transaction = connection
            .transaction()
            .map_err(|source| DbError::Migration { version: migration.version, source })?;

        transaction
            .execute_batch(migration.sql)
            .map_err(|source| DbError::Migration { version: migration.version, source })?;

        // `PRAGMA user_version` não aceita parâmetro vinculado; o `rusqlite`
        // interpola o inteiro, e o valor aqui é uma constante do binário.
        transaction
            .pragma_update(None, "user_version", migration.version)
            .map_err(|source| DbError::Migration { version: migration.version, source })?;

        transaction
            .commit()
            .map_err(|source| DbError::Migration { version: migration.version, source })?;

        tracing::info!(version = migration.version, "migração aplicada");
    }

    Ok(())
}

/// Versão atual do esquema gravada no arquivo.
pub fn user_version(connection: &Connection) -> Result<u32, DbError> {
    let version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|source| DbError::Query { operation: "user_version", source })?;

    u32::try_from(version).map_err(|_| DbError::InvalidValue {
        field: "pragma.user_version",
        detail: format!("valor negativo ou grande demais: {version}"),
    })
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

    fn memory() -> Connection {
        Connection::open_in_memory().expect("banco em memória")
    }

    fn table_names(connection: &Connection) -> Vec<String> {
        let mut statement = connection
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .expect("consulta ao catálogo");
        statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("linhas do catálogo")
            .collect::<Result<Vec<_>, _>>()
            .expect("nomes de tabela")
    }

    #[test]
    fn migracao_do_zero_cria_as_tabelas_da_fase_1() {
        let mut connection = memory();
        assert_eq!(user_version(&connection).unwrap(), 0);

        apply(&mut connection).unwrap();

        assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
        let tables = table_names(&connection);
        assert!(tables.contains(&"hardware_profiles".to_owned()));
        assert!(tables.contains(&"settings".to_owned()));
        assert!(tables.contains(&"events".to_owned()));
    }

    #[test]
    fn nao_cria_tabela_de_fase_futura() {
        let mut connection = memory();
        apply(&mut connection).unwrap();

        for proibida in ["models", "tasks", "workflows", "agents", "benchmarks"] {
            assert!(
                !table_names(&connection).contains(&proibida.to_owned()),
                "{proibida} é de uma fase que ainda não existe"
            );
        }
    }

    #[test]
    fn migracao_e_idempotente() {
        let mut connection = memory();
        apply(&mut connection).unwrap();
        let antes = table_names(&connection);

        apply(&mut connection).unwrap();
        apply(&mut connection).unwrap();

        assert_eq!(user_version(&connection).unwrap(), SCHEMA_VERSION);
        assert_eq!(table_names(&connection), antes);
    }

    #[test]
    fn banco_de_versao_futura_e_recusado() {
        let mut connection = memory();
        connection.pragma_update(None, "user_version", SCHEMA_VERSION + 7).unwrap();

        let error = apply(&mut connection).unwrap_err();
        assert!(matches!(error, DbError::UnsupportedVersion { found, supported }
                if found == SCHEMA_VERSION + 7 && supported == SCHEMA_VERSION));
    }

    #[test]
    fn a_migracao_que_falha_nao_deixa_o_banco_pela_metade() {
        let mut connection = memory();
        // Uma tabela pré-existente com o mesmo nome faz o `CREATE TABLE` falhar
        // no meio do lote — exatamente o cenário que a transação protege.
        connection.execute_batch("CREATE TABLE settings (x INTEGER);").unwrap();

        let error = apply(&mut connection).unwrap_err();
        assert!(matches!(error, DbError::Migration { version: 1, .. }));

        assert_eq!(user_version(&connection).unwrap(), 0, "a versão não pode ter avançado");
        assert!(
            !table_names(&connection).contains(&"hardware_profiles".to_owned()),
            "nada da migração pode ter sobrado"
        );
    }

    #[test]
    fn as_versoes_sao_crescentes_e_unicas() {
        let mut anterior = 0;
        for migration in MIGRATIONS {
            assert!(migration.version > anterior, "versões precisam crescer");
            anterior = migration.version;
        }
        assert_eq!(anterior, SCHEMA_VERSION, "SCHEMA_VERSION tem de ser a última migração");
    }
}
