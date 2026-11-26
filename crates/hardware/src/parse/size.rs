//! Tamanhos com sufixo, como o kernel os escreve.
//!
//! `/sys/.../cache/index0/size` diz `32K`, `/proc/cpuinfo` diz `6144 KB` e há
//! kernels que escrevem `1M`. São o mesmo dado em três grafias.

use super::ParseError;

/// Converte um tamanho de cache para KiB.
///
/// Aceita `32K`, `8192K`, `1M`, `6144 KB`, `2G` e o número puro. **Número puro
/// é interpretado como KiB**, que é a unidade implícita nos dois arquivos onde
/// isso aparece (`cache size` do `cpuinfo` e `size` do sysfs).
pub fn parse_cache_size_kib(text: &str) -> Result<u32, ParseError> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(ParseError::invalid("cache size", text));
    }

    let split = trimmed.find(|c: char| !c.is_ascii_digit()).unwrap_or(trimmed.len());
    let (digits, suffix) = trimmed.split_at(split);
    let value: u64 = digits.parse().map_err(|_| ParseError::invalid("cache size", text))?;

    let multiplier = match suffix.trim().to_ascii_lowercase().as_str() {
        "" | "k" | "kb" | "kib" => 1,
        "m" | "mb" | "mib" => 1024,
        "g" | "gb" | "gib" => 1024 * 1024,
        _ => return Err(ParseError::invalid("cache size", text)),
    };

    let kib =
        value.checked_mul(multiplier).ok_or_else(|| ParseError::invalid("cache size", text))?;
    u32::try_from(kib).map_err(|_| ParseError::invalid("cache size", text))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;

    #[test]
    fn sufixos_do_sysfs() {
        assert_eq!(parse_cache_size_kib("32K").unwrap(), 32);
        assert_eq!(parse_cache_size_kib("8192K").unwrap(), 8192);
        assert_eq!(parse_cache_size_kib("1M").unwrap(), 1024);
        assert_eq!(parse_cache_size_kib("32768K\n").unwrap(), 32768);
    }

    #[test]
    fn sufixos_do_cpuinfo() {
        assert_eq!(parse_cache_size_kib("6144 KB").unwrap(), 6144);
        assert_eq!(parse_cache_size_kib("512 KB").unwrap(), 512);
    }

    #[test]
    fn numero_puro_e_kib() {
        assert_eq!(parse_cache_size_kib("1024").unwrap(), 1024);
    }

    #[test]
    fn gigabyte_vira_kib() {
        assert_eq!(parse_cache_size_kib("2G").unwrap(), 2 * 1024 * 1024);
    }

    #[test]
    fn recusa_lixo_em_vez_de_chutar() {
        assert!(parse_cache_size_kib("").is_err());
        assert!(parse_cache_size_kib("desconhecido").is_err());
        assert!(parse_cache_size_kib("32T").is_err());
        assert!(parse_cache_size_kib("99999999G").is_err());
    }
}
