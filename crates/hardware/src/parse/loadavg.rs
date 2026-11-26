//! Leitura de `/proc/loadavg`.

use super::ParseError;

/// Interpreta as três médias de carga (1, 5 e 15 minutos).
///
/// O restante da linha (processos executáveis e último PID) é descartado: não
/// há consumidor para ele e mantê-lo obrigaria a inventar um tipo.
pub fn parse_loadavg(text: &str) -> Result<[f64; 3], ParseError> {
    let mut fields = text.split_ascii_whitespace();
    let mut next = |name: &str| -> Result<f64, ParseError> {
        let raw = fields.next().ok_or_else(|| ParseError::missing(name))?;
        raw.parse::<f64>()
            .ok()
            .filter(|value| value.is_finite() && *value >= 0.0)
            .ok_or_else(|| ParseError::invalid(name, raw))
    };

    Ok([next("load1")?, next("load5")?, next("load15")?])
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;

    #[test]
    fn le_as_tres_medias() {
        let load = parse_loadavg("2.73 2.83 2.31 2/2634 2118998\n").unwrap();
        assert!((load[0] - 2.73).abs() < f64::EPSILON);
        assert!((load[1] - 2.83).abs() < f64::EPSILON);
        assert!((load[2] - 2.31).abs() < f64::EPSILON);
    }

    #[test]
    fn linha_curta_e_erro() {
        assert_eq!(parse_loadavg("1.0 2.0\n").unwrap_err(), ParseError::missing("load15"));
    }

    #[test]
    fn valor_nao_numerico_e_erro() {
        assert_eq!(
            parse_loadavg("1.0 x 3.0 1/2 3").unwrap_err(),
            ParseError::invalid("load5", "x")
        );
    }

    #[test]
    fn valor_negativo_e_recusado() {
        assert!(parse_loadavg("-1.0 0.0 0.0 1/2 3").is_err());
    }
}
