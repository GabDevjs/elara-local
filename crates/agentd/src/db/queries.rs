//! Consultas do daemon, todas síncronas e sobre uma [`Connection`] emprestada.
//!
//! Ficam separadas do wrapper assíncrono de propósito: assim um teste abre um
//! banco em memória, chama a função e verifica o efeito, sem runtime `tokio` e
//! sem `spawn_blocking` no meio. Quem cuida de não bloquear o executor é
//! [`crate::db::Database`].
//!
//! Nenhuma função aqui inventa valor: um número fora de faixa ou um instante
//! ilegível gravado no arquivo vira [`DbError::InvalidValue`], nunca um zero
//! silencioso.

use elara_hardware::HardwareProfile;
use elara_telemetry::types::EventEnvelope;
use jiff::Timestamp;
use rusqlite::{Connection, OptionalExtension as _, params};

use super::DbError;
use crate::api::types::HardwareHistoryEntry;

/// Insere o perfil ou atualiza o que já existia, devolvendo quando ele foi
/// visto pela primeira vez.
///
/// O `first_seen_at` é o dado que não pode ser recalculado: ele é a resposta
/// para "há quanto tempo esta máquina é esta máquina", e o `HardwareResponse` o
/// devolve em toda consulta. O `RETURNING` traz o valor já gravado — o do
/// primeiro `INSERT` — sem uma segunda ida ao banco que poderia ver outra coisa.
pub fn upsert_hardware_profile(
    connection: &Connection,
    profile: &HardwareProfile,
    now: Timestamp,
) -> Result<Timestamp, DbError> {
    let json = serde_json::to_string(profile)
        .map_err(|source| DbError::Encode { operation: "upsert_hardware_profile", source })?;
    let ram_total_mb = to_integer("hardware_profiles.ram_total_mb", profile.ram.total_mb)?;

    let first_seen: String = connection
        .query_row(
            "INSERT INTO hardware_profiles (
                 fingerprint, schema_version, profile_json,
                 first_seen_at, last_seen_at,
                 cpu_model, ram_total_mb, gpu_summary
             )
             VALUES (?1, ?2, ?3, ?4, ?4, ?5, ?6, ?7)
             ON CONFLICT (fingerprint) DO UPDATE SET
                 schema_version = excluded.schema_version,
                 profile_json   = excluded.profile_json,
                 last_seen_at   = excluded.last_seen_at,
                 cpu_model      = excluded.cpu_model,
                 ram_total_mb   = excluded.ram_total_mb,
                 gpu_summary    = excluded.gpu_summary
             RETURNING first_seen_at",
            params![
                profile.fingerprint,
                i64::from(profile.schema_version),
                json,
                now.to_string(),
                profile.cpu.model,
                ram_total_mb,
                gpu_summary(profile),
            ],
            |row| row.get(0),
        )
        .map_err(|source| DbError::Query { operation: "upsert_hardware_profile", source })?;

    to_timestamp("hardware_profiles.first_seen_at", &first_seen)
}

/// Perfis já vistos nesta máquina, do mais recente para o mais antigo.
pub fn hardware_history(
    connection: &Connection,
    limit: u32,
) -> Result<Vec<HardwareHistoryEntry>, DbError> {
    let mut statement = connection
        .prepare(
            "SELECT fingerprint, first_seen_at, last_seen_at, cpu_model, ram_total_mb, gpu_summary
             FROM hardware_profiles
             ORDER BY last_seen_at DESC, fingerprint ASC
             LIMIT ?1",
        )
        .map_err(|source| DbError::Query { operation: "hardware_history", source })?;

    let rows = statement
        .query_map(params![i64::from(limit)], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|source| DbError::Query { operation: "hardware_history", source })?;

    let mut entries = Vec::new();
    for row in rows {
        let (fingerprint, first_seen, last_seen, cpu_model, ram_total_mb, gpu_summary) =
            row.map_err(|source| DbError::Query { operation: "hardware_history", source })?;

        entries.push(HardwareHistoryEntry {
            fingerprint,
            first_seen_at: to_timestamp("hardware_profiles.first_seen_at", &first_seen)?,
            last_seen_at: to_timestamp("hardware_profiles.last_seen_at", &last_seen)?,
            cpu_model,
            ram_total_mb: to_unsigned("hardware_profiles.ram_total_mb", ram_total_mb)?,
            gpu_summary,
        });
    }

    Ok(entries)
}

/// Guarda um evento e apara a tabela para no máximo `retention` linhas.
///
/// Devolve `false` — sem gravar nada — para eventos de alta frequência: uma
/// amostra por segundo encheria a retenção em meia hora e expulsaria justamente
/// o que interessa a um diagnóstico posterior. A decisão fica aqui, e não só no
/// chamador, para que nenhum caminho futuro consiga contorná-la por engano.
pub fn record_event(
    connection: &Connection,
    envelope: &EventEnvelope,
    retention: u32,
) -> Result<bool, DbError> {
    if envelope.payload.is_high_frequency() {
        return Ok(false);
    }

    let payload = serde_json::to_string(&envelope.payload)
        .map_err(|source| DbError::Encode { operation: "record_event", source })?;
    let sequence = to_integer("events.sequence", envelope.sequence)?;

    connection
        .execute(
            "INSERT INTO events (sequence, emitted_at, kind, payload_json)
             VALUES (?1, ?2, ?3, ?4)",
            params![sequence, envelope.emitted_at.to_string(), envelope.payload.name(), payload],
        )
        .map_err(|source| DbError::Query { operation: "record_event", source })?;

    // A poda usa `NOT IN (… ORDER BY id DESC LIMIT n)` em vez de `MAX(id) - n`
    // porque a coluna é `AUTOINCREMENT`: depois da primeira poda a numeração tem
    // buracos, e a aritmética apagaria de menos.
    connection
        .execute(
            "DELETE FROM events
             WHERE id NOT IN (SELECT id FROM events ORDER BY id DESC LIMIT ?1)",
            params![i64::from(retention)],
        )
        .map_err(|source| DbError::Query { operation: "prune_events", source })?;

    Ok(true)
}

/// Quantos eventos estão guardados.
///
/// Existe só para a suíte afirmar sobre a retenção: o daemon nunca precisa
/// contar eventos em produção, e uma consulta que roda só em teste é honesto
/// marcar como tal.
#[cfg(test)]
pub fn count_events(connection: &Connection) -> Result<u64, DbError> {
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM events", [], |row| row.get(0))
        .map_err(|source| DbError::Query { operation: "count_events", source })?;

    to_unsigned("events.count", count)
}

/// Valor de uma configuração, ou `None` quando a chave nunca foi escrita.
pub fn setting(connection: &Connection, key: &str) -> Result<Option<String>, DbError> {
    connection
        .query_row("SELECT value FROM settings WHERE key = ?1", params![key], |row| row.get(0))
        .optional()
        .map_err(|source| DbError::Query { operation: "setting", source })
}

/// Escreve uma configuração, sobrescrevendo o valor anterior.
pub fn set_setting(
    connection: &Connection,
    key: &str,
    value: &str,
    now: Timestamp,
) -> Result<(), DbError> {
    connection
        .execute(
            "INSERT INTO settings (key, value, updated_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value,
                                            updated_at = excluded.updated_at",
            params![key, value, now.to_string()],
        )
        .map_err(|source| DbError::Query { operation: "set_setting", source })?;

    Ok(())
}

/// Resumo das GPUs para o histórico.
///
/// A coluna existe para a UI conseguir dizer "o hardware mudou: você tinha uma
/// RTX 4060 Ti e agora tem duas" sem desserializar o perfil inteiro de cada
/// linha. VRAM desconhecida aparece como tal — a alternativa seria escrever
/// `0 MiB` e mentir sobre uma GPU que existe.
fn gpu_summary(profile: &HardwareProfile) -> String {
    if profile.gpus.is_empty() {
        return "sem GPU detectada".to_owned();
    }

    profile
        .gpus
        .iter()
        .map(|gpu| match gpu.vram_total_mb {
            Some(vram) => format!("{} ({vram} MiB)", gpu.model),
            None => format!("{} (VRAM desconhecida)", gpu.model),
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

/// Converte um contador do domínio para o `INTEGER` com sinal do `SQLite`.
fn to_integer(field: &'static str, value: u64) -> Result<i64, DbError> {
    i64::try_from(value).map_err(|_| DbError::InvalidValue {
        field,
        detail: format!("{value} não cabe no INTEGER com sinal do SQLite"),
    })
}

/// Converte de volta, recusando o negativo que só um arquivo adulterado teria.
fn to_unsigned(field: &'static str, value: i64) -> Result<u64, DbError> {
    u64::try_from(value)
        .map_err(|_| DbError::InvalidValue { field, detail: format!("valor negativo: {value}") })
}

/// Converte o texto RFC 3339 gravado na coluna.
fn to_timestamp(field: &'static str, raw: &str) -> Result<Timestamp, DbError> {
    raw.parse::<Timestamp>().map_err(|source| DbError::InvalidValue {
        field,
        detail: format!("instante ilegível {raw:?}: {source}"),
    })
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
    use elara_telemetry::types::{CpuUsage, Event, EventEnvelope, MemoryUsage, ResourceSnapshot};

    use super::*;
    use crate::db::migrations;

    fn migrated() -> Connection {
        let mut connection = Connection::open_in_memory().expect("banco em memória");
        migrations::apply(&mut connection).expect("migração");
        connection
    }

    fn at(seconds: i64) -> Timestamp {
        Timestamp::from_second(seconds).expect("instante válido")
    }

    fn envelope(sequence: u64, event: Event) -> EventEnvelope {
        EventEnvelope { sequence, emitted_at: at(1_700_000_000), payload: event }
    }

    fn stopping(reason: &str) -> Event {
        Event::DaemonStopping { reason: reason.to_owned() }
    }

    fn snapshot() -> ResourceSnapshot {
        ResourceSnapshot {
            taken_at: at(1_700_000_000),
            interval_ms: 1_000,
            cpu: CpuUsage {
                total_percent: 10.0,
                per_core_percent: vec![10.0],
                load_average: [0.1, 0.2, 0.3],
            },
            memory: MemoryUsage {
                total_mb: 1_024,
                used_mb: 512,
                available_mb: 512,
                cached_mb: 64,
                used_percent: 50.0,
                swap_used_mb: 0,
                swap_total_mb: 0,
            },
            gpus: Vec::new(),
        }
    }

    #[test]
    fn upsert_grava_o_perfil_e_devolve_o_primeiro_avistamento() {
        let connection = migrated();
        let profile = MockHardwareProvider::reference_profile();

        let first = upsert_hardware_profile(&connection, &profile, at(1_000)).unwrap();
        assert_eq!(first, at(1_000));

        let history = hardware_history(&connection, 10).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].fingerprint, profile.fingerprint);
        assert_eq!(history[0].cpu_model, profile.cpu.model);
        assert_eq!(history[0].ram_total_mb, profile.ram.total_mb);
        assert!(history[0].gpu_summary.contains("8192 MiB"));
    }

    #[test]
    fn upsert_do_mesmo_fingerprint_atualiza_last_seen_sem_duplicar() {
        let connection = migrated();
        let profile = MockHardwareProvider::reference_profile();

        upsert_hardware_profile(&connection, &profile, at(1_000)).unwrap();
        let first = upsert_hardware_profile(&connection, &profile, at(5_000)).unwrap();

        assert_eq!(first, at(1_000), "o primeiro avistamento não pode ser reescrito");

        let history = hardware_history(&connection, 10).unwrap();
        assert_eq!(history.len(), 1, "o mesmo fingerprint é uma linha só");
        assert_eq!(history[0].first_seen_at, at(1_000));
        assert_eq!(history[0].last_seen_at, at(5_000));
    }

    #[test]
    fn fingerprint_diferente_vira_outra_linha_no_historico() {
        let connection = migrated();
        let mut antigo = MockHardwareProvider::reference_profile();
        antigo.fingerprint = "a".repeat(32);
        let mut novo = MockHardwareProvider::reference_profile();
        novo.fingerprint = "b".repeat(32);
        novo.cpu.model = "Outra CPU".to_owned();

        upsert_hardware_profile(&connection, &antigo, at(1_000)).unwrap();
        upsert_hardware_profile(&connection, &novo, at(2_000)).unwrap();

        let history = hardware_history(&connection, 10).unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].fingerprint, novo.fingerprint, "o mais recente vem primeiro");
        assert_eq!(history[1].fingerprint, antigo.fingerprint);
    }

    #[test]
    fn historico_respeita_o_limite() {
        let connection = migrated();
        for index in 0..5u32 {
            let mut profile = MockHardwareProvider::reference_profile();
            profile.fingerprint = format!("{index:032}");
            upsert_hardware_profile(&connection, &profile, at(1_000 + i64::from(index))).unwrap();
        }

        assert_eq!(hardware_history(&connection, 2).unwrap().len(), 2);
        assert_eq!(hardware_history(&connection, 99).unwrap().len(), 5);
        assert!(hardware_history(&connection, 0).unwrap().is_empty());
    }

    #[test]
    fn perfil_sem_gpu_diz_que_nao_ha_gpu() {
        let connection = migrated();
        let mut profile = MockHardwareProvider::reference_profile();
        profile.gpus.clear();

        upsert_hardware_profile(&connection, &profile, at(1_000)).unwrap();
        let history = hardware_history(&connection, 1).unwrap();
        assert_eq!(history[0].gpu_summary, "sem GPU detectada");
    }

    #[test]
    fn gpu_sem_vram_conhecida_nao_vira_zero() {
        let connection = migrated();
        let mut profile = MockHardwareProvider::reference_profile();
        profile.gpus[0].vram_total_mb = None;

        upsert_hardware_profile(&connection, &profile, at(1_000)).unwrap();
        let history = hardware_history(&connection, 1).unwrap();
        assert!(history[0].gpu_summary.contains("VRAM desconhecida"));
        assert!(!history[0].gpu_summary.contains("0 MiB"));
    }

    #[test]
    fn o_perfil_completo_fica_guardado_em_json() {
        let connection = migrated();
        let profile = MockHardwareProvider::reference_profile();
        upsert_hardware_profile(&connection, &profile, at(1_000)).unwrap();

        let json: String = connection
            .query_row("SELECT profile_json FROM hardware_profiles", [], |row| row.get(0))
            .unwrap();
        let voltou: HardwareProfile = serde_json::from_str(&json).unwrap();
        assert_eq!(voltou, profile, "o perfil tem de sobreviver ao ida e volta");
    }

    #[test]
    fn eventos_de_alta_frequencia_nao_sao_gravados() {
        let connection = migrated();
        let sampled = envelope(1, Event::ResourcesSampled { snapshot: snapshot() });

        assert!(!record_event(&connection, &sampled, 100).unwrap());
        assert_eq!(count_events(&connection).unwrap(), 0);
    }

    #[test]
    fn retencao_por_contagem_mantem_os_mais_novos() {
        let connection = migrated();
        for sequence in 1..=10u64 {
            let event = envelope(sequence, stopping(&format!("evento {sequence}")));
            assert!(record_event(&connection, &event, 3).unwrap());
        }

        assert_eq!(count_events(&connection).unwrap(), 3);

        let mut statement = connection
            .prepare("SELECT sequence FROM events ORDER BY id DESC")
            .expect("consulta dos eventos");
        let sequences: Vec<i64> =
            statement.query_map([], |row| row.get(0)).unwrap().collect::<Result<_, _>>().unwrap();
        assert_eq!(sequences, vec![10, 9, 8], "a poda tem de sobrar os mais recentes");
    }

    #[test]
    fn o_evento_gravado_preserva_tipo_e_carga() {
        let connection = migrated();
        record_event(&connection, &envelope(7, stopping("sinal recebido")), 10).unwrap();

        let (kind, payload): (String, String) = connection
            .query_row("SELECT kind, payload_json FROM events", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .unwrap();

        assert_eq!(kind, "daemon.stopping");
        let voltou: Event = serde_json::from_str(&payload).unwrap();
        assert_eq!(voltou, stopping("sinal recebido"));
    }

    #[test]
    fn configuracao_ausente_e_none_e_a_escrita_sobrescreve() {
        let connection = migrated();
        assert_eq!(setting(&connection, "daemon.version").unwrap(), None);

        set_setting(&connection, "daemon.version", "0.1.0", at(1_000)).unwrap();
        assert_eq!(setting(&connection, "daemon.version").unwrap(), Some("0.1.0".to_owned()));

        set_setting(&connection, "daemon.version", "0.2.0", at(2_000)).unwrap();
        assert_eq!(setting(&connection, "daemon.version").unwrap(), Some("0.2.0".to_owned()));

        let total: i64 =
            connection.query_row("SELECT COUNT(*) FROM settings", [], |row| row.get(0)).unwrap();
        assert_eq!(total, 1, "sobrescrever não pode duplicar a chave");
    }

    #[test]
    fn instante_ilegivel_no_arquivo_vira_erro_e_nao_um_zero() {
        let connection = migrated();
        let profile = MockHardwareProvider::reference_profile();
        upsert_hardware_profile(&connection, &profile, at(1_000)).unwrap();

        connection
            .execute("UPDATE hardware_profiles SET first_seen_at = 'ontem'", [])
            .expect("adulteração");

        let error = hardware_history(&connection, 10).unwrap_err();
        assert!(matches!(error, DbError::InvalidValue { field, .. }
                if field == "hardware_profiles.first_seen_at"));
    }

    #[test]
    fn ram_negativa_no_arquivo_vira_erro() {
        let connection = migrated();
        let profile = MockHardwareProvider::reference_profile();
        upsert_hardware_profile(&connection, &profile, at(1_000)).unwrap();

        connection
            .execute("UPDATE hardware_profiles SET ram_total_mb = -1", [])
            .expect("adulteração");

        let error = hardware_history(&connection, 10).unwrap_err();
        assert!(matches!(error, DbError::InvalidValue { field, .. }
                if field == "hardware_profiles.ram_total_mb"));
    }
}
