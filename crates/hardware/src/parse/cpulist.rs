//! Listas de CPU do sysfs, no formato `0-3,8,10-11`.

use super::ParseError;

/// Limite de CPUs aceito em uma lista.
///
/// Existe para que um arquivo corrompido com `0-4294967295` vire erro em vez de
/// uma alocação de 16 GiB.
const MAX_CPUS: usize = 65_536;

/// Expande uma lista de CPUs do sysfs.
///
/// Uma lista vazia é válida e devolve um vetor vazio — é o que um `cpuset`
/// sem CPUs ou um nó NUMA sem processadores realmente informa.
pub fn parse_cpu_list(text: &str) -> Result<Vec<u32>, ParseError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut cpus = Vec::new();
    for part in trimmed.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (start, end) = if let Some((start, end)) = part.split_once('-') {
            (parse_id(start, text)?, parse_id(end, text)?)
        } else {
            let id = parse_id(part, text)?;
            (id, id)
        };
        if end < start {
            return Err(ParseError::invalid("cpu list", text));
        }
        let span = (end - start) as usize + 1;
        if cpus.len().saturating_add(span) > MAX_CPUS {
            return Err(ParseError::invalid("cpu list", text));
        }
        cpus.extend(start..=end);
    }

    cpus.sort_unstable();
    cpus.dedup();
    Ok(cpus)
}

/// Conta as CPUs de uma lista sem materializar o vetor.
pub fn count_cpus(text: &str) -> Result<u32, ParseError> {
    let cpus = parse_cpu_list(text)?;
    u32::try_from(cpus.len()).map_err(|_| ParseError::invalid("cpu list", text))
}

fn parse_id(raw: &str, original: &str) -> Result<u32, ParseError> {
    raw.trim().parse().map_err(|_| ParseError::invalid("cpu list", original))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;

    #[test]
    fn expande_faixas_e_valores_soltos() {
        assert_eq!(parse_cpu_list("0-3,8,10-11").unwrap(), vec![0, 1, 2, 3, 8, 10, 11]);
        assert_eq!(parse_cpu_list("0").unwrap(), vec![0]);
        assert_eq!(parse_cpu_list("0-7\n").unwrap(), (0..=7).collect::<Vec<_>>());
    }

    #[test]
    fn lista_vazia_e_valida() {
        assert_eq!(parse_cpu_list("").unwrap(), Vec::<u32>::new());
        assert_eq!(parse_cpu_list("\n").unwrap(), Vec::<u32>::new());
    }

    #[test]
    fn ordena_e_remove_duplicatas() {
        assert_eq!(parse_cpu_list("4,0-1,1").unwrap(), vec![0, 1, 4]);
    }

    #[test]
    fn conta_sem_materializar() {
        assert_eq!(count_cpus("0-3,8,10-11").unwrap(), 7);
        assert_eq!(count_cpus("").unwrap(), 0);
    }

    #[test]
    fn faixa_invertida_e_erro() {
        assert!(parse_cpu_list("5-1").is_err());
    }

    #[test]
    fn conteudo_nao_numerico_e_erro() {
        assert!(parse_cpu_list("0-a").is_err());
        assert!(parse_cpu_list("cpu0").is_err());
    }

    #[test]
    fn faixa_absurda_e_erro_em_vez_de_alocacao_gigante() {
        assert!(parse_cpu_list("0-4294967295").is_err());
    }
}
