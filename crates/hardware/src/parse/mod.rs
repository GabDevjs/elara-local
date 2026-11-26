//! Parsers puros dos formatos de `/proc`, `/sys` e das ferramentas de fabricante.
//!
//! Nada aqui toca o disco: toda função recebe `&str` e devolve dados. A
//! separação existe porque é no parsing que os bugs se escondem — um `/proc`
//! com um campo a mais, um kernel antigo sem `MemAvailable`, um `nvidia-smi`
//! que responde `[N/A]`. Sem I/O, cada um desses casos vira um teste de duas
//! linhas com uma fixture inline.
//!
//! As sondas em [`crate::probes`] fazem o I/O e delegam a interpretação para cá.

pub mod cgroup;
pub mod cpuinfo;
pub mod cpulist;
pub mod loadavg;
pub mod meminfo;
pub mod mounts;
pub mod nvidia_smi;
pub mod os_release;
pub mod proc_stat;
pub mod size;

/// Falha ao interpretar um conteúdo já lido.
///
/// Separado de [`crate::HardwareError`] porque um parser não sabe de qual
/// arquivo o texto veio — quem tem essa informação é a sonda, que a acrescenta
/// ao converter o erro.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ParseError {
    /// O conteúdo não tinha nenhuma linha aproveitável.
    #[error("conteúdo vazio ou sem nenhuma linha reconhecível")]
    Empty,

    /// Um campo sem o qual o resultado seria uma invenção não estava presente.
    #[error("campo obrigatório ausente: {field}")]
    MissingField {
        /// Nome do campo tal como aparece no formato de origem.
        field: String,
    },

    /// O campo existe mas o valor não é interpretável.
    #[error("valor inválido em {field}: {value:?}")]
    InvalidValue {
        /// Nome do campo tal como aparece no formato de origem.
        field: String,
        /// Trecho exato recusado, para aparecer no log sem ambiguidade.
        value: String,
    },
}

impl ParseError {
    /// Atalho para [`ParseError::MissingField`].
    pub fn missing(field: impl Into<String>) -> Self {
        Self::MissingField { field: field.into() }
    }

    /// Atalho para [`ParseError::InvalidValue`].
    pub fn invalid(field: impl Into<String>, value: impl Into<String>) -> Self {
        Self::InvalidValue { field: field.into(), value: value.into() }
    }
}

/// Quebra uma linha `chave<sep>valor` nos dois lados, já aparados.
///
/// Os arquivos de `/proc` alinham as chaves com tabulações e às vezes com
/// espaços; comparar a chave sem aparar é a origem clássica de um campo que
/// existe em uma máquina e "some" em outra.
pub(crate) fn split_key_value(line: &str, separator: char) -> Option<(&str, &str)> {
    let (key, value) = line.split_once(separator)?;
    Some((key.trim(), value.trim()))
}
