//! Leitura de `/proc/meminfo`.
//!
//! Todos os valores do arquivo estão em KiB. `MemAvailable` é a única linha que
//! pode faltar (kernels anteriores ao 3.14) e por isso é a única opcional: ela
//! não pode ser substituída por `MemFree`, que ignora o cache recuperável e
//! subestima a memória utilizável em várias dezenas de por cento.

use super::{ParseError, split_key_value};

/// Campos do `/proc/meminfo` que o Elara usa, em KiB.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MemInfo {
    /// `MemTotal`: RAM física vista pelo kernel.
    pub total_kb: u64,
    /// `MemFree`: memória sem uso nenhum.
    pub free_kb: u64,
    /// `MemAvailable`: estimativa do kernel do que dá para alocar sem swap.
    pub available_kb: Option<u64>,
    /// `Buffers`.
    pub buffers_kb: u64,
    /// `Cached`.
    pub cached_kb: u64,
    /// `SwapTotal`.
    pub swap_total_kb: u64,
    /// `SwapFree`.
    pub swap_free_kb: u64,
}

impl MemInfo {
    /// Memória utilizável, com degradação explícita quando `MemAvailable` falta.
    ///
    /// O fallback `free + buffers + cached` é a mesma conta que o kernel fazia
    /// antes de expor `MemAvailable`; é uma aproximação declarada, não um
    /// número inventado.
    pub fn usable_kb(&self) -> u64 {
        self.available_kb.unwrap_or_else(|| {
            self.free_kb.saturating_add(self.buffers_kb).saturating_add(self.cached_kb)
        })
    }
}

/// Interpreta o conteúdo de `/proc/meminfo`.
///
/// Só `MemTotal` é obrigatório: sem ele não existe perfil de memória possível.
pub fn parse_meminfo(text: &str) -> Result<MemInfo, ParseError> {
    let mut info = MemInfo::default();
    let mut saw_total = false;

    for line in text.lines() {
        let Some((key, value)) = split_key_value(line, ':') else { continue };
        let Some(kb) = parse_kb(value) else { continue };

        match key {
            "MemTotal" => {
                info.total_kb = kb;
                saw_total = true;
            }
            "MemFree" => info.free_kb = kb,
            "MemAvailable" => info.available_kb = Some(kb),
            "Buffers" => info.buffers_kb = kb,
            "Cached" => info.cached_kb = kb,
            "SwapTotal" => info.swap_total_kb = kb,
            "SwapFree" => info.swap_free_kb = kb,
            _ => {}
        }
    }

    if saw_total { Ok(info) } else { Err(ParseError::missing("MemTotal")) }
}

/// Extrai o número de um valor como `32815700 kB`.
///
/// Linhas sem unidade existem (`HugePages_Total`), e por isso a unidade é
/// opcional — mas quando presente só `kB` é aceito, para não confundir escalas.
fn parse_kb(value: &str) -> Option<u64> {
    let mut parts = value.split_ascii_whitespace();
    let number: u64 = parts.next()?.parse().ok()?;
    match parts.next() {
        None => Some(number),
        Some(unit) if unit.eq_ignore_ascii_case("kb") => Some(number),
        Some(_) => None,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
MemTotal:       32815700 kB
MemFree:         3528484 kB
MemAvailable:   13473464 kB
Buffers:         1601972 kB
Cached:          9574576 kB
SwapCached:            0 kB
SwapTotal:       8388604 kB
SwapFree:        8388604 kB
HugePages_Total:       0
";

    #[test]
    fn le_campos_conhecidos() {
        let info = parse_meminfo(SAMPLE).unwrap();
        assert_eq!(info.total_kb, 32_815_700);
        assert_eq!(info.free_kb, 3_528_484);
        assert_eq!(info.available_kb, Some(13_473_464));
        assert_eq!(info.cached_kb, 9_574_576);
        assert_eq!(info.swap_total_kb, 8_388_604);
        assert_eq!(info.swap_free_kb, 8_388_604);
        assert_eq!(info.usable_kb(), 13_473_464);
    }

    #[test]
    fn sem_mem_available_usa_fallback_declarado() {
        let sem = SAMPLE
            .lines()
            .filter(|l| !l.starts_with("MemAvailable"))
            .collect::<Vec<_>>()
            .join("\n");
        let info = parse_meminfo(&sem).unwrap();
        assert_eq!(info.available_kb, None);
        assert_eq!(info.usable_kb(), 3_528_484 + 1_601_972 + 9_574_576);
    }

    #[test]
    fn sem_mem_total_e_erro() {
        assert_eq!(
            parse_meminfo("MemFree: 100 kB\n").unwrap_err(),
            ParseError::missing("MemTotal")
        );
    }

    #[test]
    fn unidade_inesperada_e_ignorada() {
        let info = parse_meminfo("MemTotal: 1024 kB\nCached: 5 MB\n").unwrap();
        assert_eq!(info.total_kb, 1024);
        assert_eq!(info.cached_kb, 0);
    }
}
