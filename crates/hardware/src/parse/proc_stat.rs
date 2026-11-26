//! Leitura das linhas `cpu` do `/proc/stat`.
//!
//! Os valores são contadores acumulados desde o boot, em jiffies. Nunca são
//! convertidos em percentual aqui: percentual só existe entre duas amostras, e
//! quem tem as duas é o sampler de telemetria.

use super::ParseError;
use crate::types::CpuTimes;

/// Contadores de CPU de um instante.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcStat {
    /// Linha `cpu`, soma de todas as CPUs.
    pub total: CpuTimes,
    /// Linhas `cpuN`, na ordem em que o kernel as escreve.
    pub per_cpu: Vec<CpuTimes>,
}

/// Interpreta o conteúdo de `/proc/stat`.
///
/// Ignora tudo que não começa com `cpu`: `intr` tem centenas de colunas e
/// nenhuma utilidade para o Elara.
pub fn parse_proc_stat(text: &str) -> Result<ProcStat, ParseError> {
    let mut total = None;
    let mut per_cpu = Vec::new();

    for line in text.lines() {
        let Some(rest) = line.strip_prefix("cpu") else { continue };
        let Some((label, values)) = rest.split_once(char::is_whitespace) else { continue };

        let times = parse_times(values)?;
        if label.is_empty() {
            total = Some(times);
        } else if label.chars().all(|c| c.is_ascii_digit()) {
            per_cpu.push(times);
        }
    }

    match total {
        Some(total) => Ok(ProcStat { total, per_cpu }),
        None => Err(ParseError::missing("cpu")),
    }
}

/// Lê as colunas de uma linha `cpu`.
///
/// Kernels antigos têm menos colunas (sem `steal`, sem `guest`); as ausentes
/// ficam em zero, que é o valor correto — o contador simplesmente não existe.
fn parse_times(values: &str) -> Result<CpuTimes, ParseError> {
    let mut fields = values.split_ascii_whitespace();
    let mut next = |name: &str| -> Result<u64, ParseError> {
        match fields.next() {
            None => Ok(0),
            Some(raw) => raw.parse().map_err(|_| ParseError::invalid(name, raw)),
        }
    };

    Ok(CpuTimes {
        user: next("user")?,
        nice: next("nice")?,
        system: next("system")?,
        idle: next("idle")?,
        iowait: next("iowait")?,
        irq: next("irq")?,
        softirq: next("softirq")?,
        steal: next("steal")?,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
cpu  122626 3250285 1503586 14655283 38352 0 20016 0 0 0
cpu0 30193 798406 373296 3676828 10349 0 3011 0 0 0
cpu1 30135 802259 380900 3657794 9386 0 14167 0 0 0
intr 209134065 57 0 0 0
ctxt 1234
btime 1700000000
";

    #[test]
    fn le_total_e_por_cpu() {
        let stat = parse_proc_stat(SAMPLE).unwrap();
        assert_eq!(stat.total.user, 122_626);
        assert_eq!(stat.total.idle, 14_655_283);
        assert_eq!(stat.total.softirq, 20_016);
        assert_eq!(stat.per_cpu.len(), 2);
        assert_eq!(stat.per_cpu[1].user, 30_135);
    }

    #[test]
    fn idle_total_soma_iowait() {
        let stat = parse_proc_stat(SAMPLE).unwrap();
        assert_eq!(stat.total.idle_total(), 14_655_283 + 38_352);
    }

    #[test]
    fn kernel_antigo_com_menos_colunas() {
        let stat = parse_proc_stat("cpu  1 2 3 4\ncpu0 1 2 3 4\n").unwrap();
        assert_eq!(stat.total.iowait, 0);
        assert_eq!(stat.total.steal, 0);
        assert_eq!(stat.total.total(), 10);
    }

    #[test]
    fn sem_linha_agregada_e_erro() {
        assert_eq!(parse_proc_stat("intr 1 2 3\n").unwrap_err(), ParseError::missing("cpu"));
    }

    #[test]
    fn coluna_nao_numerica_e_erro() {
        assert!(parse_proc_stat("cpu  1 2 x 4\n").is_err());
    }
}
