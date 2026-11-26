//! Limites de cgroup, nas duas hierarquias.
//!
//! Dentro de um contêiner, `/proc/meminfo` continua mostrando a RAM do host: é
//! o cgroup que diz quanto o processo pode realmente usar. Ignorar isso é o
//! caminho mais curto para um OOM kill no meio de uma inferência.

use super::ParseError;

/// Valor a partir do qual um limite do cgroup v1 significa "sem limite".
///
/// O kernel escreve `PAGE_COUNTER_MAX` em páginas (algo como
/// `9223372036854771712` bytes) quando não há limite. Qualquer coisa acima de
/// 4 EiB não descreve uma máquina real.
const V1_UNLIMITED_THRESHOLD: u64 = 1 << 62;

/// Cota de CPU declarada em `cpu.max` (cgroup v2), em núcleos equivalentes.
///
/// `max <período>` significa sem cota e devolve `Ok(None)` — distinto de erro,
/// que significaria arquivo ilegível.
pub fn parse_cpu_max(text: &str) -> Result<Option<f64>, ParseError> {
    let mut fields = text.split_ascii_whitespace();
    let quota = fields.next().ok_or_else(|| ParseError::missing("cpu.max"))?;
    if quota == "max" {
        return Ok(None);
    }
    let period = fields.next().unwrap_or("100000");

    let quota: f64 = quota.parse().map_err(|_| ParseError::invalid("cpu.max", text))?;
    let period: f64 = period.parse().map_err(|_| ParseError::invalid("cpu.max", text))?;
    if period <= 0.0 || quota <= 0.0 {
        return Err(ParseError::invalid("cpu.max", text));
    }
    Ok(Some(quota / period))
}

/// Limite de memória de `memory.max` (cgroup v2), em bytes.
///
/// `max` devolve `Ok(None)`.
pub fn parse_memory_max(text: &str) -> Result<Option<u64>, ParseError> {
    let trimmed = text.trim();
    if trimmed == "max" {
        return Ok(None);
    }
    let bytes: u64 = trimmed.parse().map_err(|_| ParseError::invalid("memory.max", text))?;
    Ok(Some(bytes))
}

/// Valor simples em bytes, como `memory.current` ou `memory.usage_in_bytes`.
pub fn parse_bytes(field: &str, text: &str) -> Result<u64, ParseError> {
    text.trim().parse().map_err(|_| ParseError::invalid(field, text))
}

/// Cota de CPU do cgroup v1, a partir de `cpu.cfs_quota_us` e `cpu.cfs_period_us`.
///
/// Cota `-1` significa sem limite e devolve `Ok(None)`.
pub fn parse_cpu_quota_v1(quota: &str, period: &str) -> Result<Option<f64>, ParseError> {
    let quota: i64 =
        quota.trim().parse().map_err(|_| ParseError::invalid("cpu.cfs_quota_us", quota))?;
    if quota <= 0 {
        return Ok(None);
    }
    let period: i64 =
        period.trim().parse().map_err(|_| ParseError::invalid("cpu.cfs_period_us", period))?;
    if period <= 0 {
        return Err(ParseError::invalid("cpu.cfs_period_us", period.to_string()));
    }
    // Precisão de ponto flutuante é irrelevante nesta faixa (microssegundos).
    #[allow(clippy::cast_precision_loss, reason = "quota e período cabem folgadamente em f64")]
    Ok(Some(quota as f64 / period as f64))
}

/// Limite de memória do cgroup v1 (`memory.limit_in_bytes`), em bytes.
///
/// O sentinela de "sem limite" é um número absurdo, não um texto; tratá-lo como
/// limite real faria o broker acreditar em 8 EiB de RAM.
pub fn parse_memory_limit_v1(text: &str) -> Result<Option<u64>, ParseError> {
    let bytes: u64 =
        text.trim().parse().map_err(|_| ParseError::invalid("memory.limit_in_bytes", text))?;
    if bytes >= V1_UNLIMITED_THRESHOLD { Ok(None) } else { Ok(Some(bytes)) }
}

/// Uma linha de `/proc/<pid>/cgroup`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CgroupPath {
    /// ID da hierarquia; `0` identifica a hierarquia unificada (v2).
    pub hierarchy_id: u32,
    /// Controladores desta linha; vazio na hierarquia unificada.
    pub controllers: Vec<String>,
    /// Caminho do cgroup relativo à raiz da hierarquia.
    pub path: String,
}

impl CgroupPath {
    /// `true` para a linha da hierarquia unificada (`0::/...`).
    pub fn is_unified(&self) -> bool {
        self.hierarchy_id == 0 && self.controllers.is_empty()
    }
}

/// Interpreta `/proc/self/cgroup` ou `/proc/1/cgroup`.
///
/// Além dos limites, este arquivo é a evidência mais confiável de que estamos
/// dentro de um contêiner: o caminho carrega o ID do runtime.
pub fn parse_proc_cgroup(text: &str) -> Vec<CgroupPath> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.splitn(3, ':');
            let hierarchy_id = fields.next()?.parse().ok()?;
            let controllers = fields.next()?;
            let path = fields.next()?.to_owned();
            let controllers = if controllers.is_empty() {
                Vec::new()
            } else {
                controllers.split(',').map(str::to_owned).collect()
            };
            Some(CgroupPath { hierarchy_id, controllers, path })
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;

    #[test]
    fn cpu_max_com_cota() {
        assert_eq!(parse_cpu_max("200000 100000").unwrap(), Some(2.0));
        assert_eq!(parse_cpu_max("50000 100000\n").unwrap(), Some(0.5));
        assert_eq!(parse_cpu_max("250000 100000").unwrap(), Some(2.5));
    }

    #[test]
    fn cpu_max_sem_cota() {
        assert_eq!(parse_cpu_max("max 100000").unwrap(), None);
        assert_eq!(parse_cpu_max("max").unwrap(), None);
    }

    #[test]
    fn cpu_max_invalido() {
        assert!(parse_cpu_max("").is_err());
        assert!(parse_cpu_max("abc 100000").is_err());
        assert!(parse_cpu_max("100000 0").is_err());
    }

    #[test]
    fn memory_max_em_bytes() {
        assert_eq!(parse_memory_max("4294967296").unwrap(), Some(4 * 1024 * 1024 * 1024));
        assert_eq!(parse_memory_max("max\n").unwrap(), None);
        assert!(parse_memory_max("4G").is_err());
    }

    #[test]
    fn cota_v1() {
        assert_eq!(parse_cpu_quota_v1("200000", "100000").unwrap(), Some(2.0));
        assert_eq!(parse_cpu_quota_v1("-1", "100000").unwrap(), None);
        assert!(parse_cpu_quota_v1("x", "100000").is_err());
    }

    #[test]
    fn limite_v1_com_sentinela_nao_e_limite() {
        assert_eq!(parse_memory_limit_v1("9223372036854771712").unwrap(), None);
        assert_eq!(parse_memory_limit_v1("2147483648").unwrap(), Some(2 * 1024 * 1024 * 1024));
    }

    #[test]
    fn proc_cgroup_v2() {
        let paths = parse_proc_cgroup("0::/user.slice/user-1000.slice/session-621.scope\n");
        assert_eq!(paths.len(), 1);
        let first = paths.first().unwrap();
        assert!(first.is_unified());
        assert_eq!(first.path, "/user.slice/user-1000.slice/session-621.scope");
    }

    #[test]
    fn proc_cgroup_v1_com_controladores() {
        let text = "\
12:memory:/docker/9f0a
11:cpu,cpuacct:/docker/9f0a
0::/
";
        let paths = parse_proc_cgroup(text);
        assert_eq!(paths.len(), 3);
        let memory = paths.iter().find(|p| p.controllers.iter().any(|c| c == "memory")).unwrap();
        assert_eq!(memory.hierarchy_id, 12);
        assert_eq!(memory.path, "/docker/9f0a");
        let cpu = paths.iter().find(|p| p.hierarchy_id == 11).unwrap();
        assert_eq!(cpu.controllers, vec!["cpu".to_owned(), "cpuacct".to_owned()]);
    }

    #[test]
    fn linha_malformada_e_ignorada() {
        assert!(parse_proc_cgroup("lixo\n").is_empty());
    }
}
