//! Sonda de topologia: nós NUMA, SMT e limites de cgroup.
//!
//! O cgroup é a parte que não pode faltar. Dentro de um contêiner,
//! `/proc/meminfo` continua mostrando a RAM do host — quem lê só isso planeja
//! usar 32 GiB em um contêiner de 4 GiB e é morto pelo OOM killer no meio do
//! carregamento do modelo.

use std::path::Path;

use super::{bytes_to_mib, exists, list_dir, read_trimmed};
use crate::parse::{cgroup, cpulist};
use crate::types::{CgroupLimits, CgroupVersion, HardwareTopology, NumaNode, ProbeWarning};

/// Nome desta sonda nos avisos.
const SOURCE: &str = "topology";

/// Ponto de montagem padrão da hierarquia de cgroups.
const CGROUP_ROOT: &str = "/sys/fs/cgroup";

/// Coleta topologia e limites.
pub fn probe_topology(root: &Path, warnings: &mut Vec<ProbeWarning>) -> HardwareTopology {
    HardwareTopology {
        numa_nodes: numa_nodes(root, warnings),
        simultaneous_multithreading: smt_active(root, warnings),
        cgroup: cgroup_limits(root, warnings),
    }
}

/// Enumera os nós NUMA e o que está ligado a cada um.
fn numa_nodes(root: &Path, warnings: &mut Vec<ProbeWarning>) -> Vec<NumaNode> {
    let base = "/sys/devices/system/node";
    let mut nodes: Vec<NumaNode> = list_dir(root, base)
        .into_iter()
        .filter_map(|name| {
            let id: u32 = name.strip_prefix("node")?.parse().ok()?;
            let cpus = read_trimmed(root, &format!("{base}/{name}/cpulist"))
                .and_then(|text| cpulist::parse_cpu_list(&text).ok())
                .unwrap_or_default();
            let memory_mb = read_trimmed(root, &format!("{base}/{name}/meminfo"))
                .and_then(|text| node_memory_kb(&text))
                .map(|kb| kb / 1024);
            Some(NumaNode { id, cpus, memory_mb })
        })
        .collect();
    nodes.sort_by_key(|node| node.id);

    if nodes.is_empty() {
        warnings.push(ProbeWarning::info(
            SOURCE,
            "kernel sem /sys/devices/system/node; topologia NUMA desconhecida",
        ));
    }
    nodes
}

/// Extrai `MemTotal` do `meminfo` de um nó NUMA (`Node 0 MemTotal: 32815700 kB`).
fn node_memory_kb(text: &str) -> Option<u64> {
    text.lines().find(|line| line.contains("MemTotal")).and_then(|line| {
        let (_, value) = line.split_once("MemTotal:")?;
        value.split_ascii_whitespace().next()?.parse().ok()
    })
}

/// `true` quando SMT/Hyper-Threading está ativo.
///
/// O arquivo `smt/active` é a resposta direta; onde ele não existe, duas CPUs
/// irmãs no mesmo núcleo provam o mesmo fato.
fn smt_active(root: &Path, warnings: &mut Vec<ProbeWarning>) -> bool {
    if let Some(value) = read_trimmed(root, "/sys/devices/system/cpu/smt/active") {
        return value == "1";
    }
    if let Some(siblings) =
        read_trimmed(root, "/sys/devices/system/cpu/cpu0/topology/thread_siblings_list")
    {
        return cpulist::count_cpus(&siblings).is_ok_and(|count| count > 1);
    }
    warnings.push(ProbeWarning::info(
        SOURCE,
        "estado do SMT não exposto pelo kernel; assumindo inativo",
    ));
    false
}

/// Lê os limites do cgroup que contém este processo.
fn cgroup_limits(root: &Path, warnings: &mut Vec<ProbeWarning>) -> CgroupLimits {
    if exists(root, &format!("{CGROUP_ROOT}/cgroup.controllers")) {
        return cgroup_v2(root);
    }
    if exists(root, &format!("{CGROUP_ROOT}/memory")) || exists(root, &format!("{CGROUP_ROOT}/cpu"))
    {
        return cgroup_v1(root);
    }
    warnings.push(ProbeWarning::info(
        SOURCE,
        "sem hierarquia de cgroups montada; sem limites a respeitar",
    ));
    CgroupLimits {
        version: CgroupVersion::None,
        cpu_quota_cores: None,
        memory_limit_mb: None,
        memory_usage_mb: None,
    }
}

/// Limites na hierarquia unificada (v2).
///
/// Percorre do cgroup do processo até a raiz e fica com o **menor** limite
/// encontrado: um limite no pai vale para o filho mesmo que o filho não o
/// declare.
fn cgroup_v2(root: &Path) -> CgroupLimits {
    let mut quota: Option<f64> = None;
    let mut limit: Option<u64> = None;
    let mut usage: Option<u64> = None;

    for dir in ancestor_paths(&process_cgroup_path(root)) {
        let base = format!("{CGROUP_ROOT}{dir}");

        if let Some(text) = read_trimmed(root, &format!("{base}/cpu.max"))
            && let Ok(Some(value)) = cgroup::parse_cpu_max(&text)
        {
            quota = Some(quota.map_or(value, |current: f64| current.min(value)));
        }
        if let Some(text) = read_trimmed(root, &format!("{base}/memory.max"))
            && let Ok(Some(value)) = cgroup::parse_memory_max(&text)
        {
            limit = Some(limit.map_or(value, |current: u64| current.min(value)));
        }
        if usage.is_none()
            && let Some(text) = read_trimmed(root, &format!("{base}/memory.current"))
            && let Ok(value) = cgroup::parse_bytes("memory.current", &text)
        {
            usage = Some(value);
        }
    }

    CgroupLimits {
        version: CgroupVersion::V2,
        cpu_quota_cores: quota,
        memory_limit_mb: limit.map(bytes_to_mib),
        memory_usage_mb: usage.map(bytes_to_mib),
    }
}

/// Limites na hierarquia legada (v1).
fn cgroup_v1(root: &Path) -> CgroupLimits {
    let cpu_dir = controller_dir(root, "cpu");
    let memory_dir = controller_dir(root, "memory");

    let quota = read_trimmed(root, &format!("{cpu_dir}/cpu.cfs_quota_us")).and_then(|quota| {
        let period = read_trimmed(root, &format!("{cpu_dir}/cpu.cfs_period_us"))
            .unwrap_or_else(|| "100000".to_owned());
        cgroup::parse_cpu_quota_v1(&quota, &period).ok().flatten()
    });

    let limit = read_trimmed(root, &format!("{memory_dir}/memory.limit_in_bytes"))
        .and_then(|text| cgroup::parse_memory_limit_v1(&text).ok().flatten());
    let usage = read_trimmed(root, &format!("{memory_dir}/memory.usage_in_bytes"))
        .and_then(|text| cgroup::parse_bytes("memory.usage_in_bytes", &text).ok());

    CgroupLimits {
        version: CgroupVersion::V1,
        cpu_quota_cores: quota,
        memory_limit_mb: limit.map(bytes_to_mib),
        memory_usage_mb: usage.map(bytes_to_mib),
    }
}

/// Diretório do controlador v1 do processo, com fallback para a raiz do
/// controlador (que é o que se enxerga dentro de um contêiner).
fn controller_dir(root: &Path, controller: &str) -> String {
    let base = format!("{CGROUP_ROOT}/{controller}");
    let path = read_trimmed(root, "/proc/self/cgroup")
        .map(|text| cgroup::parse_proc_cgroup(&text))
        .and_then(|paths| {
            paths
                .into_iter()
                .find(|entry| entry.controllers.iter().any(|c| c == controller))
                .map(|entry| entry.path)
        })
        .unwrap_or_default();

    let candidate = format!("{base}{path}");
    if path.len() > 1 && exists(root, &candidate) { candidate } else { base }
}

/// Caminho do cgroup unificado deste processo, ex.: `/user.slice/app.scope`.
fn process_cgroup_path(root: &Path) -> String {
    read_trimmed(root, "/proc/self/cgroup")
        .map(|text| cgroup::parse_proc_cgroup(&text))
        .and_then(|paths| paths.into_iter().find(cgroup::CgroupPath::is_unified))
        .map(|entry| entry.path)
        .unwrap_or_default()
}

/// Do cgroup mais interno até a raiz, para procurar o limite efetivo.
fn ancestor_paths(path: &str) -> Vec<String> {
    let mut accumulated = String::new();
    let mut paths = vec![String::new()];
    for segment in path.split('/').filter(|segment| !segment.is_empty()) {
        accumulated.push('/');
        accumulated.push_str(segment);
        paths.push(accumulated.clone());
    }
    paths.reverse();
    paths
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, content) in files {
            let full = dir.path().join(path.trim_start_matches('/'));
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(full, content).unwrap();
        }
        dir
    }

    #[test]
    fn le_nos_numa() {
        let dir = tree(&[
            ("/sys/devices/system/node/node0/cpulist", "0-3\n"),
            (
                "/sys/devices/system/node/node0/meminfo",
                "Node 0 MemTotal:       32815700 kB\nNode 0 MemFree: 100 kB\n",
            ),
            ("/sys/devices/system/node/node1/cpulist", "4-7\n"),
            ("/sys/devices/system/node/node1/meminfo", "Node 1 MemTotal:       16777216 kB\n"),
        ]);
        let mut warnings = Vec::new();
        let topology = probe_topology(dir.path(), &mut warnings);

        assert_eq!(topology.numa_nodes.len(), 2);
        assert_eq!(topology.numa_nodes[0].id, 0);
        assert_eq!(topology.numa_nodes[0].cpus, vec![0, 1, 2, 3]);
        assert_eq!(topology.numa_nodes[0].memory_mb, Some(32_046));
        assert_eq!(topology.numa_nodes[1].cpus, vec![4, 5, 6, 7]);
    }

    #[test]
    fn smt_pelo_arquivo_dedicado() {
        let ativo = tree(&[("/sys/devices/system/cpu/smt/active", "1\n")]);
        let mut warnings = Vec::new();
        assert!(probe_topology(ativo.path(), &mut warnings).simultaneous_multithreading);

        let inativo = tree(&[("/sys/devices/system/cpu/smt/active", "0\n")]);
        assert!(!probe_topology(inativo.path(), &mut warnings).simultaneous_multithreading);
    }

    #[test]
    fn smt_pelos_irmaos_quando_nao_ha_arquivo() {
        let dir = tree(&[("/sys/devices/system/cpu/cpu0/topology/thread_siblings_list", "0,4\n")]);
        let mut warnings = Vec::new();
        assert!(probe_topology(dir.path(), &mut warnings).simultaneous_multithreading);
    }

    #[test]
    fn cgroup_v2_no_proprio_diretorio() {
        let dir = tree(&[
            ("/sys/fs/cgroup/cgroup.controllers", "cpuset cpu memory\n"),
            ("/sys/fs/cgroup/cpu.max", "200000 100000\n"),
            ("/sys/fs/cgroup/memory.max", "4294967296\n"),
            ("/sys/fs/cgroup/memory.current", "1073741824\n"),
            ("/proc/self/cgroup", "0::/\n"),
        ]);
        let mut warnings = Vec::new();
        let limits = probe_topology(dir.path(), &mut warnings).cgroup;

        assert_eq!(limits.version, CgroupVersion::V2);
        assert_eq!(limits.cpu_quota_cores, Some(2.0));
        assert_eq!(limits.memory_limit_mb, Some(4096));
        assert_eq!(limits.memory_usage_mb, Some(1024));
    }

    #[test]
    fn cgroup_v2_herda_limite_do_pai() {
        let dir = tree(&[
            ("/sys/fs/cgroup/cgroup.controllers", "cpu memory\n"),
            ("/sys/fs/cgroup/user.slice/memory.max", "8589934592\n"),
            ("/sys/fs/cgroup/user.slice/app.scope/cpu.max", "max 100000\n"),
            ("/sys/fs/cgroup/user.slice/app.scope/memory.current", "536870912\n"),
            ("/proc/self/cgroup", "0::/user.slice/app.scope\n"),
        ]);
        let mut warnings = Vec::new();
        let limits = probe_topology(dir.path(), &mut warnings).cgroup;

        assert_eq!(limits.cpu_quota_cores, None, "cpu.max do próprio cgroup é ilimitado");
        assert_eq!(limits.memory_limit_mb, Some(8192), "o limite do pai continua valendo");
        assert_eq!(limits.memory_usage_mb, Some(512));
    }

    #[test]
    fn cgroup_v2_pega_o_limite_mais_restritivo() {
        let dir = tree(&[
            ("/sys/fs/cgroup/cgroup.controllers", "cpu memory\n"),
            ("/sys/fs/cgroup/pai/memory.max", "8589934592\n"),
            ("/sys/fs/cgroup/pai/filho/memory.max", "2147483648\n"),
            ("/proc/self/cgroup", "0::/pai/filho\n"),
        ]);
        let mut warnings = Vec::new();
        let limits = probe_topology(dir.path(), &mut warnings).cgroup;
        assert_eq!(limits.memory_limit_mb, Some(2048));
    }

    #[test]
    fn cgroup_v1_como_fallback() {
        let dir = tree(&[
            ("/sys/fs/cgroup/cpu/cpu.cfs_quota_us", "150000\n"),
            ("/sys/fs/cgroup/cpu/cpu.cfs_period_us", "100000\n"),
            ("/sys/fs/cgroup/memory/memory.limit_in_bytes", "2147483648\n"),
            ("/sys/fs/cgroup/memory/memory.usage_in_bytes", "536870912\n"),
            ("/proc/self/cgroup", "5:memory:/docker/abc\n4:cpu,cpuacct:/docker/abc\n"),
        ]);
        let mut warnings = Vec::new();
        let limits = probe_topology(dir.path(), &mut warnings).cgroup;

        assert_eq!(limits.version, CgroupVersion::V1);
        assert_eq!(limits.cpu_quota_cores, Some(1.5));
        assert_eq!(limits.memory_limit_mb, Some(2048));
        assert_eq!(limits.memory_usage_mb, Some(512));
    }

    #[test]
    fn cgroup_v1_com_caminho_do_processo() {
        let dir = tree(&[
            ("/sys/fs/cgroup/memory/docker/abc/memory.limit_in_bytes", "1073741824\n"),
            ("/sys/fs/cgroup/memory/memory.limit_in_bytes", "9223372036854771712\n"),
            ("/proc/self/cgroup", "5:memory:/docker/abc\n"),
        ]);
        let mut warnings = Vec::new();
        let limits = probe_topology(dir.path(), &mut warnings).cgroup;
        assert_eq!(limits.memory_limit_mb, Some(1024));
    }

    #[test]
    fn sem_cgroup_montado() {
        let dir = tree(&[("/proc/self/cgroup", "0::/\n")]);
        let mut warnings = Vec::new();
        let limits = probe_topology(dir.path(), &mut warnings).cgroup;

        assert_eq!(limits.version, CgroupVersion::None);
        assert_eq!(limits.cpu_quota_cores, None);
        assert_eq!(limits.memory_limit_mb, None);
    }

    #[test]
    fn ancestrais_do_mais_interno_para_a_raiz() {
        assert_eq!(ancestor_paths("/a/b"), vec!["/a/b".to_owned(), "/a".to_owned(), String::new()]);
        assert_eq!(ancestor_paths("/"), vec![String::new()]);
        assert_eq!(ancestor_paths(""), vec![String::new()]);
    }
}
