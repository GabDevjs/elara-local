//! Sonda de memória do sistema.
//!
//! Não há medição de banda aqui. Um número teórico calculado a partir do tipo
//! de DDR seria uma mentira confortável: a banda real depende de canais
//! populados, XMP e do próprio controlador. Enquanto o Calibration Engine
//! (Fase 7) não medir, o campo fica `None` e a origem é `NotMeasured`.

use std::path::Path;

use super::{kib_to_mib, read_file};
use crate::HardwareError;
use crate::parse::meminfo;
use crate::types::{BandwidthSource, MemorySample, ProbeWarning, RamProfile};

/// Nome desta sonda nos avisos.
const SOURCE: &str = "ram";

/// Coleta o perfil de memória a partir de `/proc/meminfo`.
pub fn probe_ram(
    root: &Path,
    warnings: &mut Vec<ProbeWarning>,
) -> Result<RamProfile, HardwareError> {
    let info = read_meminfo(root)?;

    if info.available_kb.is_none() {
        warnings.push(ProbeWarning::info(
            SOURCE,
            "kernel sem MemAvailable; memória utilizável estimada por free+buffers+cached",
        ));
    }

    Ok(RamProfile {
        total_mb: kib_to_mib(info.total_kb),
        available_mb: kib_to_mib(info.usable_kb()),
        free_mb: kib_to_mib(info.free_kb),
        swap_total_mb: kib_to_mib(info.swap_total_kb),
        swap_free_mb: kib_to_mib(info.swap_free_kb),
        measured_bandwidth_gbps: None,
        bandwidth_source: BandwidthSource::NotMeasured,
    })
}

/// Lê o estado instantâneo da memória para o sampler.
pub fn sample_memory(root: &Path) -> Result<MemorySample, HardwareError> {
    let info = read_meminfo(root)?;
    Ok(MemorySample {
        total_mb: kib_to_mib(info.total_kb),
        available_mb: kib_to_mib(info.usable_kb()),
        free_mb: kib_to_mib(info.free_kb),
        cached_mb: kib_to_mib(info.cached_kb),
        swap_total_mb: kib_to_mib(info.swap_total_kb),
        swap_free_mb: kib_to_mib(info.swap_free_kb),
    })
}

fn read_meminfo(root: &Path) -> Result<meminfo::MemInfo, HardwareError> {
    let text = read_file(root, "/proc/meminfo")
        .map_err(|source| HardwareError::Io { path: "/proc/meminfo".to_owned(), source })?;
    meminfo::parse_meminfo(&text).map_err(|error| HardwareError::Parse {
        path: "/proc/meminfo".to_owned(),
        detail: error.to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;

    fn tree(meminfo: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("proc")).unwrap();
        std::fs::write(dir.path().join("proc/meminfo"), meminfo).unwrap();
        dir
    }

    const SAMPLE: &str = "\
MemTotal:       32815700 kB
MemFree:         3528484 kB
MemAvailable:   13473464 kB
Buffers:         1601972 kB
Cached:          9574576 kB
SwapTotal:       8388604 kB
SwapFree:        8388604 kB
";

    #[test]
    fn converte_kib_para_mib() {
        let dir = tree(SAMPLE);
        let mut warnings = Vec::new();
        let ram = probe_ram(dir.path(), &mut warnings).unwrap();
        assert_eq!(ram.total_mb, 32_046);
        assert_eq!(ram.available_mb, 13_157);
        assert_eq!(ram.free_mb, 3_445);
        assert_eq!(ram.swap_total_mb, 8_191);
        assert!(warnings.is_empty());
    }

    #[test]
    fn banda_nunca_e_estimada() {
        let dir = tree(SAMPLE);
        let mut warnings = Vec::new();
        let ram = probe_ram(dir.path(), &mut warnings).unwrap();
        assert_eq!(ram.measured_bandwidth_gbps, None);
        assert_eq!(ram.bandwidth_source, BandwidthSource::NotMeasured);
    }

    #[test]
    fn sem_mem_available_avisa() {
        let sem: String = SAMPLE
            .lines()
            .filter(|l| !l.starts_with("MemAvailable"))
            .collect::<Vec<_>>()
            .join("\n");
        let dir = tree(&sem);
        let mut warnings = Vec::new();
        let ram = probe_ram(dir.path(), &mut warnings).unwrap();
        assert_eq!(ram.available_mb, kib_to_mib(3_528_484 + 1_601_972 + 9_574_576));
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn sem_meminfo_e_erro() {
        let dir = tempfile::tempdir().unwrap();
        let mut warnings = Vec::new();
        assert!(matches!(
            probe_ram(dir.path(), &mut warnings),
            Err(HardwareError::Io { ref path, .. }) if path == "/proc/meminfo"
        ));
    }

    #[test]
    fn amostra_traz_cache_separado() {
        let dir = tree(SAMPLE);
        let sample = sample_memory(dir.path()).unwrap();
        assert_eq!(sample.cached_mb, 9_350);
        assert_eq!(sample.total_mb, 32_046);
    }
}
