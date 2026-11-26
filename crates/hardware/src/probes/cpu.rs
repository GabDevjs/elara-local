//! Sonda de CPU: modelo, núcleos, frequências, caches e extensões.
//!
//! O ponto delicado é a diferença entre **thread lógica** e **núcleo físico**.
//! Tratar 16 threads como 16 núcleos faz o planejador prometer o dobro da
//! capacidade real de uma CPU com SMT; por isso a contagem física tem três
//! fontes em cascata e, quando nenhuma responde, um aviso explícito.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use super::{list_dir, read_file, read_trimmed, read_u64};
use crate::HardwareError;
use crate::parse::{cpuinfo, cpulist, loadavg, proc_stat, size};
use crate::types::{CpuCache, CpuProfile, HybridTopology, ProbeWarning};

/// Nome desta sonda nos avisos.
const SOURCE: &str = "cpu";

/// Raiz do sysfs de CPUs.
const CPU_SYSFS: &str = "/sys/devices/system/cpu";

/// Coleta o perfil de CPU.
///
/// É a única sonda que pode falhar de verdade: sem `/proc/cpuinfo` não existe
/// nem modelo nem contagem de threads, e o contrato não admite esses campos
/// vazios.
pub fn probe_cpu(
    root: &Path,
    warnings: &mut Vec<ProbeWarning>,
) -> Result<CpuProfile, HardwareError> {
    let text = read_file(root, "/proc/cpuinfo")
        .map_err(|source| HardwareError::Io { path: "/proc/cpuinfo".to_owned(), source })?;
    let info = cpuinfo::parse_cpuinfo(&text).map_err(|error| HardwareError::Parse {
        path: "/proc/cpuinfo".to_owned(),
        detail: error.to_string(),
    })?;

    let listed = u32::try_from(info.processors.len()).unwrap_or(u32::MAX);
    let logical_threads = count_from_list(root, "present").unwrap_or(listed);
    let online_threads = count_from_list(root, "online").unwrap_or(listed);

    let model = info.model_label().unwrap_or_else(|| {
        warnings
            .push(ProbeWarning::warning(SOURCE, "modelo da CPU não declarado em /proc/cpuinfo"));
        "CPU desconhecida".to_owned()
    });

    let sockets = info.socket_count().or_else(|| sysfs_socket_count(root)).unwrap_or_else(|| {
        warnings.push(ProbeWarning::info(
            SOURCE,
            "sockets não declarados em /proc/cpuinfo nem no sysfs; assumindo 1",
        ));
        1
    });

    let physical_cores = physical_cores(&info, root, logical_threads, warnings);
    let (base_frequency_mhz, max_frequency_mhz) = frequencies(&info, root, warnings);

    Ok(CpuProfile {
        model,
        vendor: info.vendor(),
        architecture: info.architecture().unwrap_or("unknown").to_owned(),
        physical_cores,
        logical_threads,
        online_threads,
        sockets,
        base_frequency_mhz,
        max_frequency_mhz,
        cache: caches(root, sockets, warnings),
        features: info.features(),
        hybrid: hybrid_topology(root),
    })
}

/// Lê os contadores de `/proc/stat`.
///
/// Vive na sonda de CPU, e não em uma sonda separada, porque é a mesma fonte
/// que o `detect()` usa — só que chamada a cada segundo pelo sampler.
pub fn sample_cpu_times(root: &Path) -> Result<proc_stat::ProcStat, HardwareError> {
    let text = read_file(root, "/proc/stat")
        .map_err(|source| HardwareError::Io { path: "/proc/stat".to_owned(), source })?;
    proc_stat::parse_proc_stat(&text).map_err(|error| HardwareError::Parse {
        path: "/proc/stat".to_owned(),
        detail: error.to_string(),
    })
}

/// Lê as médias de carga de `/proc/loadavg`.
pub fn sample_load_average(root: &Path) -> Result<[f64; 3], HardwareError> {
    let text = read_file(root, "/proc/loadavg")
        .map_err(|source| HardwareError::Io { path: "/proc/loadavg".to_owned(), source })?;
    loadavg::parse_loadavg(&text).map_err(|error| HardwareError::Parse {
        path: "/proc/loadavg".to_owned(),
        detail: error.to_string(),
    })
}

/// Conta CPUs em `/sys/devices/system/cpu/{present,online}`.
fn count_from_list(root: &Path, file: &str) -> Option<u32> {
    let text = read_trimmed(root, &format!("{CPU_SYSFS}/{file}"))?;
    cpulist::count_cpus(&text).ok().filter(|count| *count > 0)
}

/// Diretórios `cpuN` presentes no sysfs, em ordem numérica.
fn cpu_dirs(root: &Path) -> Vec<(u32, String)> {
    let mut dirs: Vec<(u32, String)> = list_dir(root, CPU_SYSFS)
        .into_iter()
        .filter_map(|name| {
            let id: u32 = name.strip_prefix("cpu")?.parse().ok()?;
            Some((id, name))
        })
        .collect();
    dirs.sort_unstable();
    dirs
}

/// Lê um campo de `topology/` de uma CPU.
fn topology_field(root: &Path, cpu: &str, field: &str) -> Option<u64> {
    read_u64(root, &format!("{CPU_SYSFS}/{cpu}/topology/{field}"))
}

/// Núcleos físicos, em três fontes de confiança decrescente.
fn physical_cores(
    info: &cpuinfo::Cpuinfo,
    root: &Path,
    logical_threads: u32,
    warnings: &mut Vec<ProbeWarning>,
) -> u32 {
    if let Some(cores) = info.physical_core_count() {
        return cores;
    }
    if let Some(cores) = sysfs_core_count(root) {
        return cores;
    }
    warnings.push(ProbeWarning::warning(
        SOURCE,
        "topologia de núcleos ausente em /proc/cpuinfo e no sysfs; \
         usando o número de threads, que superestima a capacidade se houver SMT",
    ));
    logical_threads.max(1)
}

/// Conta pares `(pacote, núcleo)` distintos no sysfs.
fn sysfs_core_count(root: &Path) -> Option<u32> {
    let pairs: BTreeSet<(u64, u64)> = cpu_dirs(root)
        .iter()
        .filter_map(|(_, cpu)| {
            let package = topology_field(root, cpu, "physical_package_id")?;
            let core = topology_field(root, cpu, "core_id")?;
            Some((package, core))
        })
        .collect();
    u32::try_from(pairs.len()).ok().filter(|count| *count > 0)
}

/// Conta pacotes distintos no sysfs.
fn sysfs_socket_count(root: &Path) -> Option<u32> {
    let packages: BTreeSet<u64> = cpu_dirs(root)
        .iter()
        .filter_map(|(_, cpu)| topology_field(root, cpu, "physical_package_id"))
        .collect();
    u32::try_from(packages.len()).ok().filter(|count| *count > 0)
}

/// Frequência base e máxima, em MHz.
///
/// `cpu MHz` do `/proc/cpuinfo` é deliberadamente ignorado: é a frequência do
/// instante da leitura, que num sistema ocioso é a mínima e num sob carga é a
/// de turbo — nunca a base.
fn frequencies(
    info: &cpuinfo::Cpuinfo,
    root: &Path,
    warnings: &mut Vec<ProbeWarning>,
) -> (Option<u32>, Option<u32>) {
    let base = read_u64(root, &format!("{CPU_SYSFS}/cpu0/cpufreq/base_frequency"))
        .and_then(khz_to_mhz)
        .or_else(|| info.base_frequency_mhz());

    let max = cpu_dirs(root)
        .iter()
        .filter_map(|(_, cpu)| {
            read_u64(root, &format!("{CPU_SYSFS}/{cpu}/cpufreq/cpuinfo_max_freq"))
        })
        .max()
        .and_then(khz_to_mhz);

    if max.is_none() {
        warnings.push(ProbeWarning::info(
            SOURCE,
            "cpufreq indisponível; frequência máxima desconhecida",
        ));
    }
    (base, max)
}

/// Converte o kHz do `cpufreq` para MHz.
fn khz_to_mhz(khz: u64) -> Option<u32> {
    u32::try_from(khz / 1000).ok().filter(|mhz| *mhz > 0)
}

/// Tamanhos de cache por nível, agregados por socket.
///
/// Cada instância de cache aparece em todas as CPUs que a compartilham; sem
/// deduplicar por `shared_cpu_list` um L3 de 32 MiB compartilhado por 8 CPUs
/// viraria 256 MiB.
fn caches(root: &Path, sockets: u32, warnings: &mut Vec<ProbeWarning>) -> CpuCache {
    let mut totals: BTreeMap<(u64, char), u64> = BTreeMap::new();
    let mut seen: BTreeSet<(u64, char, String)> = BTreeSet::new();

    for (_, cpu) in cpu_dirs(root) {
        let base = format!("{CPU_SYSFS}/{cpu}/cache");
        for index in list_dir(root, &base) {
            if !index.starts_with("index") {
                continue;
            }
            let dir = format!("{base}/{index}");
            let Some(level) = read_u64(root, &format!("{dir}/level")) else { continue };
            let kind = cache_kind(read_trimmed(root, &format!("{dir}/type")).as_deref());
            let Some(size_text) = read_trimmed(root, &format!("{dir}/size")) else { continue };
            let Ok(kib) = size::parse_cache_size_kib(&size_text) else { continue };

            // Uma instância é identificada pelo conjunto de CPUs que a usa.
            let shared = read_trimmed(root, &format!("{dir}/shared_cpu_list"))
                .unwrap_or_else(|| cpu.clone());
            if seen.insert((level, kind, shared)) {
                *totals.entry((level, kind)).or_default() += u64::from(kib);
            }
        }
    }

    if totals.is_empty() {
        warnings.push(ProbeWarning::info(
            SOURCE,
            "caches não expostos em /sys; tamanhos desconhecidos",
        ));
        return CpuCache::default();
    }

    let per_socket = |level: u64, kind: char| -> Option<u32> {
        let total = *totals.get(&(level, kind))?;
        u32::try_from(total / u64::from(sockets.max(1))).ok()
    };

    CpuCache {
        l1d_kb: per_socket(1, 'd'),
        l1i_kb: per_socket(1, 'i'),
        l2_kb: per_socket(2, 'u').or_else(|| per_socket(2, 'd')),
        l3_kb: per_socket(3, 'u').or_else(|| per_socket(3, 'd')),
    }
}

/// Classifica o `type` de um cache (`Data`, `Instruction`, `Unified`).
fn cache_kind(kind: Option<&str>) -> char {
    match kind.map(str::to_ascii_lowercase).as_deref() {
        Some("data") => 'd',
        Some("instruction") => 'i',
        _ => 'u',
    }
}

/// Classe de um núcleo em CPUs híbridas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CoreClass {
    /// Núcleo de desempenho (P-core).
    Performance,
    /// Núcleo de eficiência (E-core).
    Efficiency,
}

/// Distribuição P/E, quando a máquina é híbrida.
///
/// Devolve `None` para CPUs homogêneas — o campo existe para dizer "esta
/// máquina tem dois tipos de núcleo", não para repetir a contagem total.
fn hybrid_topology(root: &Path) -> Option<HybridTopology> {
    let classes = classes_by_core_type(root).or_else(|| classes_by_capacity(root))?;

    let mut performance = 0;
    let mut efficiency = 0;
    for (_, class) in &classes {
        match class {
            CoreClass::Performance => performance += 1,
            CoreClass::Efficiency => efficiency += 1,
        }
    }

    if performance > 0 && efficiency > 0 {
        Some(HybridTopology { performance_cores: performance, efficiency_cores: efficiency })
    } else {
        None
    }
}

/// Classifica núcleos por `topology/core_type` (Intel).
fn classes_by_core_type(root: &Path) -> Option<Vec<((u64, u64), CoreClass)>> {
    let mut classes = BTreeMap::new();
    for (id, cpu) in cpu_dirs(root) {
        let raw = read_trimmed(root, &format!("{CPU_SYSFS}/{cpu}/topology/core_type"))?;
        let lower = raw.to_ascii_lowercase();
        let class = if lower.contains("atom") || lower == "0x20" {
            CoreClass::Efficiency
        } else if lower.contains("core") || lower == "0x40" {
            CoreClass::Performance
        } else {
            return None;
        };
        classes.insert(core_key(root, &cpu, id), class);
    }
    (!classes.is_empty()).then(|| classes.into_iter().collect())
}

/// Classifica núcleos por `cpu_capacity` (ARM big.LITTLE).
///
/// A maior capacidade define os núcleos de desempenho; tudo abaixo dela é
/// eficiência. É a mesma regra que o escalonador do kernel usa.
fn classes_by_capacity(root: &Path) -> Option<Vec<((u64, u64), CoreClass)>> {
    let capacities: Vec<((u64, u64), u64)> = cpu_dirs(root)
        .into_iter()
        .filter_map(|(id, cpu)| {
            let capacity = read_u64(root, &format!("{CPU_SYSFS}/{cpu}/cpu_capacity"))?;
            Some((core_key(root, &cpu, id), capacity))
        })
        .collect();

    let max = capacities.iter().map(|(_, capacity)| *capacity).max()?;
    if capacities.iter().all(|(_, capacity)| *capacity == max) {
        return None;
    }

    let mut classes = BTreeMap::new();
    for (key, capacity) in capacities {
        let class = if capacity == max { CoreClass::Performance } else { CoreClass::Efficiency };
        classes.insert(key, class);
    }
    Some(classes.into_iter().collect())
}

/// Identidade do núcleo físico de uma CPU lógica, para não contar duas vezes o
/// mesmo núcleo com SMT.
fn core_key(root: &Path, cpu: &str, id: u32) -> (u64, u64) {
    let package = topology_field(root, cpu, "physical_package_id").unwrap_or(0);
    let core = topology_field(root, cpu, "core_id").unwrap_or(u64::from(id));
    (package, core)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
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

    const CPUINFO_SMT: &str = "\
processor\t: 0
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i7 CPU @ 3.00GHz
physical id\t: 0
core id\t\t: 0
cpu cores\t: 2
siblings\t: 4
flags\t\t: fpu lm avx2

processor\t: 1
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i7 CPU @ 3.00GHz
physical id\t: 0
core id\t\t: 0
cpu cores\t: 2
siblings\t: 4
flags\t\t: fpu lm avx2

processor\t: 2
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i7 CPU @ 3.00GHz
physical id\t: 0
core id\t\t: 1
cpu cores\t: 2
siblings\t: 4
flags\t\t: fpu lm avx2

processor\t: 3
vendor_id\t: GenuineIntel
model name\t: Intel(R) Core(TM) i7 CPU @ 3.00GHz
physical id\t: 0
core id\t\t: 1
cpu cores\t: 2
siblings\t: 4
flags\t\t: fpu lm avx2
";

    #[test]
    fn separa_nucleos_de_threads() {
        let dir = tree(&[
            ("/proc/cpuinfo", CPUINFO_SMT),
            ("/sys/devices/system/cpu/present", "0-3"),
            ("/sys/devices/system/cpu/online", "0-3"),
        ]);
        let mut warnings = Vec::new();
        let cpu = probe_cpu(dir.path(), &mut warnings).unwrap();

        assert_eq!(cpu.physical_cores, 2);
        assert_eq!(cpu.logical_threads, 4);
        assert_eq!(cpu.online_threads, 4);
        assert_eq!(cpu.sockets, 1);
        assert_eq!(cpu.base_frequency_mhz, Some(3000));
        assert!(cpu.features.avx2);
    }

    #[test]
    fn threads_offline_aparecem_na_diferenca() {
        let dir = tree(&[
            ("/proc/cpuinfo", CPUINFO_SMT),
            ("/sys/devices/system/cpu/present", "0-7"),
            ("/sys/devices/system/cpu/online", "0-3"),
        ]);
        let mut warnings = Vec::new();
        let cpu = probe_cpu(dir.path(), &mut warnings).unwrap();
        assert_eq!(cpu.logical_threads, 8);
        assert_eq!(cpu.online_threads, 4);
    }

    #[test]
    fn sem_topologia_no_cpuinfo_usa_sysfs() {
        let dir = tree(&[
            (
                "/proc/cpuinfo",
                "processor\t: 0\nFeatures\t: fp asimd\nCPU implementer\t: 0x41\nCPU architecture: 8\nCPU part\t: 0xd08\n\nprocessor\t: 1\nFeatures\t: fp asimd\nCPU implementer\t: 0x41\nCPU architecture: 8\nCPU part\t: 0xd08\n",
            ),
            ("/sys/devices/system/cpu/cpu0/topology/physical_package_id", "0"),
            ("/sys/devices/system/cpu/cpu0/topology/core_id", "0"),
            ("/sys/devices/system/cpu/cpu1/topology/physical_package_id", "0"),
            ("/sys/devices/system/cpu/cpu1/topology/core_id", "1"),
        ]);
        let mut warnings = Vec::new();
        let cpu = probe_cpu(dir.path(), &mut warnings).unwrap();
        assert_eq!(cpu.physical_cores, 2);
        assert_eq!(cpu.sockets, 1);
        assert_eq!(cpu.architecture, "aarch64");
    }

    #[test]
    fn sem_nenhuma_topologia_avisa_e_usa_threads() {
        let dir =
            tree(&[("/proc/cpuinfo", "processor\t: 0\nmodel name\t: CPU X\nflags\t: fpu lm\n")]);
        let mut warnings = Vec::new();
        let cpu = probe_cpu(dir.path(), &mut warnings).unwrap();
        assert_eq!(cpu.physical_cores, 1);
        assert!(warnings.iter().any(|w| w.message.contains("topologia de núcleos")));
    }

    #[test]
    fn sem_cpuinfo_e_erro() {
        let dir = tree(&[]);
        let mut warnings = Vec::new();
        assert!(matches!(
            probe_cpu(dir.path(), &mut warnings),
            Err(HardwareError::Io { ref path, .. }) if path == "/proc/cpuinfo"
        ));
    }

    #[test]
    fn cache_compartilhado_nao_e_somado_por_cpu() {
        let mut files: Vec<(String, String)> = Vec::new();
        for cpu in 0..4 {
            let base = format!("/sys/devices/system/cpu/cpu{cpu}/cache");
            files.push((format!("{base}/index0/level"), "1".into()));
            files.push((format!("{base}/index0/type"), "Data".into()));
            files.push((format!("{base}/index0/size"), "32K".into()));
            files.push((format!("{base}/index0/shared_cpu_list"), format!("{cpu}")));
            files.push((format!("{base}/index3/level"), "3".into()));
            files.push((format!("{base}/index3/type"), "Unified".into()));
            files.push((format!("{base}/index3/size"), "8192K".into()));
            files.push((format!("{base}/index3/shared_cpu_list"), "0-3".into()));
        }
        files.push(("/proc/cpuinfo".into(), CPUINFO_SMT.into()));
        let borrowed: Vec<(&str, &str)> =
            files.iter().map(|(p, c)| (p.as_str(), c.as_str())).collect();
        let dir = tree(&borrowed);

        let mut warnings = Vec::new();
        let cpu = probe_cpu(dir.path(), &mut warnings).unwrap();
        assert_eq!(cpu.cache.l1d_kb, Some(4 * 32), "quatro instâncias privadas de 32K");
        assert_eq!(cpu.cache.l3_kb, Some(8192), "uma única instância compartilhada");
        assert_eq!(cpu.cache.l1i_kb, None, "não havia cache de instrução na fixture");
    }

    #[test]
    fn hibrido_por_core_type() {
        let dir = tree(&[
            ("/proc/cpuinfo", CPUINFO_SMT),
            ("/sys/devices/system/cpu/cpu0/topology/core_type", "intel_core"),
            ("/sys/devices/system/cpu/cpu0/topology/core_id", "0"),
            ("/sys/devices/system/cpu/cpu1/topology/core_type", "intel_core"),
            ("/sys/devices/system/cpu/cpu1/topology/core_id", "0"),
            ("/sys/devices/system/cpu/cpu2/topology/core_type", "intel_atom"),
            ("/sys/devices/system/cpu/cpu2/topology/core_id", "1"),
            ("/sys/devices/system/cpu/cpu3/topology/core_type", "intel_atom"),
            ("/sys/devices/system/cpu/cpu3/topology/core_id", "2"),
        ]);
        let mut warnings = Vec::new();
        let cpu = probe_cpu(dir.path(), &mut warnings).unwrap();
        assert_eq!(cpu.hybrid, Some(HybridTopology { performance_cores: 1, efficiency_cores: 2 }));
    }

    #[test]
    fn hibrido_por_capacidade_arm() {
        let dir = tree(&[
            ("/proc/cpuinfo", CPUINFO_SMT),
            ("/sys/devices/system/cpu/cpu0/cpu_capacity", "1024"),
            ("/sys/devices/system/cpu/cpu1/cpu_capacity", "1024"),
            ("/sys/devices/system/cpu/cpu2/cpu_capacity", "462"),
            ("/sys/devices/system/cpu/cpu3/cpu_capacity", "462"),
        ]);
        let mut warnings = Vec::new();
        let cpu = probe_cpu(dir.path(), &mut warnings).unwrap();
        assert_eq!(cpu.hybrid, Some(HybridTopology { performance_cores: 2, efficiency_cores: 2 }));
    }

    #[test]
    fn cpu_homogenea_nao_reporta_hibrido() {
        let dir = tree(&[
            ("/proc/cpuinfo", CPUINFO_SMT),
            ("/sys/devices/system/cpu/cpu0/cpu_capacity", "1024"),
            ("/sys/devices/system/cpu/cpu1/cpu_capacity", "1024"),
        ]);
        let mut warnings = Vec::new();
        assert_eq!(probe_cpu(dir.path(), &mut warnings).unwrap().hybrid, None);
    }

    #[test]
    fn frequencia_maxima_vem_do_cpufreq() {
        let dir = tree(&[
            ("/proc/cpuinfo", CPUINFO_SMT),
            ("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq", "4200000"),
            ("/sys/devices/system/cpu/cpu1/cpufreq/cpuinfo_max_freq", "4600000"),
            ("/sys/devices/system/cpu/cpu0/cpufreq/base_frequency", "2500000"),
        ]);
        let mut warnings = Vec::new();
        let cpu = probe_cpu(dir.path(), &mut warnings).unwrap();
        assert_eq!(cpu.max_frequency_mhz, Some(4600));
        assert_eq!(cpu.base_frequency_mhz, Some(2500), "sysfs tem precedência sobre o nome");
    }

    #[test]
    fn amostra_de_contadores() {
        let dir = tree(&[
            ("/proc/stat", "cpu  10 0 5 100 0 0 0 0\ncpu0 10 0 5 100 0 0 0 0\n"),
            ("/proc/loadavg", "0.50 0.25 0.10 1/100 999\n"),
        ]);
        let stat = sample_cpu_times(dir.path()).unwrap();
        assert_eq!(stat.total.user, 10);
        assert_eq!(stat.per_cpu.len(), 1);
        let load = sample_load_average(dir.path()).unwrap();
        assert!((load[0] - 0.5).abs() < f64::EPSILON);
    }
}
