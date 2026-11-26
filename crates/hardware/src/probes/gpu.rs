//! Sonda de GPU, em três fontes correlacionadas pelo endereço PCI.
//!
//! 1. **NVML** — a biblioteca da NVIDIA, carregada dinamicamente. Dá VRAM livre,
//!    clock, temperatura e potência. Não estar instalada é o caso normal em uma
//!    máquina AMD ou sem driver proprietário: vira aviso, nunca erro.
//! 2. **`nvidia-smi`** — mesmo dado, um processo de distância. Só é chamado se o
//!    NVML falhar, e sempre com argumentos em vetor (nunca uma linha de comando
//!    montada por concatenação).
//! 3. **sysfs/PCI** — sempre roda. Identifica o dispositivo mesmo sem driver, e
//!    é a única fonte para AMD e Intel.
//!
//! O que nenhuma fonte souber fica `None`. Em particular, uma NVIDIA sem driver
//! aparece com `vram_total_mb: None` — não com zero, que seria lido como "GPU
//! sem memória".

use std::path::Path;
use std::process::Command;

use nvml_wrapper::Nvml;
use nvml_wrapper::enum_wrappers::device::{Clock, TemperatureSensor};

use super::{bytes_to_mib, exists, list_dir, read_trimmed};
use crate::parse::nvidia_smi;
use crate::types::{GpuBackend, GpuProbeSource, GpuProfile, GpuSample, GpuVendor, ProbeWarning};

/// Nome desta sonda nos avisos.
const SOURCE: &str = "gpu";

/// Diretório do sysfs com todos os dispositivos PCI.
const PCI_DEVICES: &str = "/sys/bus/pci/devices";

/// Diretórios em que um ICD Vulkan pode estar registrado.
const VULKAN_ICD_DIRS: [&str; 3] =
    ["/usr/share/vulkan/icd.d", "/usr/local/share/vulkan/icd.d", "/etc/vulkan/icd.d"];

/// De onde a sonda pode buscar dados ao vivo.
#[derive(Debug, Default)]
pub struct GpuProbeOptions<'a> {
    /// Handle NVML já inicializado, mantido vivo entre chamadas pelo provider.
    pub nvml: Option<&'a Nvml>,
    /// Permite executar `nvidia-smi`.
    ///
    /// Só pode ser `true` quando a raiz é a real: um binário do host descreve o
    /// host, e usá-lo com uma raiz de fixture misturaria duas máquinas no mesmo
    /// perfil.
    pub allow_host_tools: bool,
}

/// Enumera as GPUs da máquina descrita pela raiz.
pub fn probe_gpus(
    root: &Path,
    options: &GpuProbeOptions<'_>,
    warnings: &mut Vec<ProbeWarning>,
) -> Vec<GpuProfile> {
    let mut gpus = sysfs_gpus(root, warnings);

    let live = match options.nvml {
        Some(nvml) => nvml_gpus(nvml, warnings),
        None if options.allow_host_tools => nvidia_smi_gpus(warnings),
        None => Vec::new(),
    };

    merge_live_into_sysfs(&mut gpus, live);
    let driver_present = nvidia_driver_present(root, &gpus);
    for gpu in &mut gpus {
        gpu.backends = backends_for(root, gpu, driver_present);
    }
    gpus
}

/// Lê as métricas instantâneas de cada GPU conhecida pelo NVML.
///
/// Só o NVML serve aqui: chamar `nvidia-smi` uma vez por segundo custaria mais
/// que tudo o que o daemon faz no mesmo intervalo.
pub fn sample_gpus(nvml: Option<&Nvml>) -> Vec<GpuSample> {
    let Some(nvml) = nvml else { return Vec::new() };
    let Ok(count) = nvml.device_count() else { return Vec::new() };

    (0..count)
        .filter_map(|index| {
            let device = nvml.device_by_index(index).ok()?;
            let memory = device.memory_info().ok();
            let utilization = device.utilization_rates().ok();
            Some(GpuSample {
                index,
                utilization_percent: utilization.as_ref().map(|u| to_f32(u.gpu)),
                memory_utilization_percent: utilization.as_ref().map(|u| to_f32(u.memory)),
                vram_used_mb: memory.as_ref().map(|m| bytes_to_mib(m.used)),
                vram_total_mb: memory.as_ref().map(|m| bytes_to_mib(m.total)),
                temperature_c: device.temperature(TemperatureSensor::Gpu).ok().map(to_f32),
                power_watts: device.power_usage().ok().map(milliwatts_to_watts),
                // O contrato pede o clock do SM; em GPUs em que o domínio não
                // é exposto, o de gráficos é o mesmo valor.
                clock_mhz: device
                    .clock_info(Clock::SM)
                    .or_else(|_| device.clock_info(Clock::Graphics))
                    .ok(),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------
// sysfs
// ---------------------------------------------------------------------------

/// Enumera os dispositivos PCI de classe VGA (`0x0300`) e 3D (`0x0380`).
fn sysfs_gpus(root: &Path, warnings: &mut Vec<ProbeWarning>) -> Vec<GpuProfile> {
    let mut gpus = Vec::new();

    for address in list_dir(root, PCI_DEVICES) {
        let device_dir = format!("{PCI_DEVICES}/{address}");
        let Some(class) = read_trimmed(root, &format!("{device_dir}/class")) else { continue };
        if !is_display_class(&class) {
            continue;
        }

        let vendor_id = read_trimmed(root, &format!("{device_dir}/vendor")).unwrap_or_default();
        let device_id = read_trimmed(root, &format!("{device_dir}/device")).unwrap_or_default();
        let vendor = vendor_from_pci_id(&vendor_id);
        let integrated = is_integrated(root, &device_dir, vendor);

        let (total, used) = vram_from_sysfs(root, &device_dir, integrated);
        gpus.push(GpuProfile {
            index: 0,
            vendor,
            model: sysfs_model_label(vendor, &vendor_id, &device_id),
            pci_address: Some(normalize_pci_address(&address)),
            vram_total_mb: total,
            vram_available_mb: total.and_then(|t| used.map(|u| t.saturating_sub(u))),
            driver_version: read_trimmed(root, &format!("{device_dir}/driver/module/version")),
            runtime_version: None,
            compute_capability: None,
            backends: Vec::new(),
            is_integrated: integrated,
            power_limit_watts: None,
            source: GpuProbeSource::Sysfs,
        });
    }

    if gpus.is_empty() {
        warnings.push(ProbeWarning::info(
            SOURCE,
            "nenhum dispositivo PCI de classe gráfica encontrado em /sys/bus/pci/devices",
        ));
    }
    gpus
}

/// `true` para as classes PCI de vídeo: `0x030000` (VGA) e `0x038000` (3D).
fn is_display_class(class: &str) -> bool {
    let digits = class.trim_start_matches("0x");
    digits.starts_with("0300") || digits.starts_with("0380")
}

/// Traduz o ID de fabricante PCI.
fn vendor_from_pci_id(vendor_id: &str) -> GpuVendor {
    match vendor_id.trim_start_matches("0x").to_ascii_lowercase().as_str() {
        "10de" => GpuVendor::Nvidia,
        "1002" | "1022" => GpuVendor::Amd,
        "8086" => GpuVendor::Intel,
        "106b" => GpuVendor::Apple,
        _ => GpuVendor::Unknown,
    }
}

/// Rótulo do dispositivo quando só temos os IDs PCI.
///
/// Não existe tabela de nomes comerciais no kernel; inventar um a partir do ID
/// daria "RTX 4070" para uma 4070 Ti. Mostramos os IDs crus, que ao menos são
/// verificáveis com `lspci`.
fn sysfs_model_label(vendor: GpuVendor, vendor_id: &str, device_id: &str) -> String {
    let vendor_name = match vendor {
        GpuVendor::Nvidia => "NVIDIA",
        GpuVendor::Amd => "AMD",
        GpuVendor::Intel => "Intel",
        GpuVendor::Apple => "Apple",
        GpuVendor::Unknown => "GPU",
    };
    let ids =
        format!("{}:{}", vendor_id.trim_start_matches("0x"), device_id.trim_start_matches("0x"));
    format!("{vendor_name} {ids}")
}

/// GPU integrada compartilha a RAM do sistema e não tem VRAM dedicada.
///
/// Uma Intel só é considerada dedicada quando expõe `lmem_total_bytes` (as Arc
/// discretas expõem); as demais são integradas.
fn is_integrated(root: &Path, device_dir: &str, vendor: GpuVendor) -> bool {
    if vendor != GpuVendor::Intel {
        return false;
    }
    !list_dir(root, &format!("{device_dir}/drm"))
        .iter()
        .filter(|name| name.starts_with("card"))
        .any(|card| exists(root, &format!("{device_dir}/drm/{card}/lmem_total_bytes")))
}

/// VRAM total e em uso, quando o driver as expõe (`amdgpu`).
fn vram_from_sysfs(root: &Path, device_dir: &str, integrated: bool) -> (Option<u64>, Option<u64>) {
    if integrated {
        // Memória "de vídeo" de uma integrada é RAM do sistema; reportá-la como
        // VRAM faria o orçamento contar a mesma memória duas vezes.
        return (None, None);
    }
    let total =
        super::read_u64(root, &format!("{device_dir}/mem_info_vram_total")).map(bytes_to_mib);
    let used = super::read_u64(root, &format!("{device_dir}/mem_info_vram_used")).map(bytes_to_mib);
    (total, used)
}

/// Normaliza `00000000:01:00.0` e `0000:01:00.0` para a mesma forma.
///
/// NVML escreve o domínio com oito dígitos e o sysfs com quatro; sem normalizar,
/// a correlação entre as duas fontes falharia sempre.
pub fn normalize_pci_address(address: &str) -> String {
    let lower = address.trim().to_ascii_lowercase();
    let parts: Vec<&str> = lower.split(':').collect();
    match parts.as_slice() {
        [domain, bus, rest] => {
            let domain = domain.trim_start_matches('0');
            let domain = if domain.is_empty() { "0" } else { domain };
            format!("{domain:0>4}:{bus}:{rest}")
        }
        _ => lower,
    }
}

// ---------------------------------------------------------------------------
// NVML
// ---------------------------------------------------------------------------

/// Lê todas as GPUs visíveis ao NVML.
fn nvml_gpus(nvml: &Nvml, warnings: &mut Vec<ProbeWarning>) -> Vec<GpuProfile> {
    let driver_version = nvml.sys_driver_version().ok();
    let runtime_version = nvml.sys_cuda_driver_version().ok().map(format_cuda_version);

    let count = match nvml.device_count() {
        Ok(count) => count,
        Err(error) => {
            warnings.push(ProbeWarning::warning(
                SOURCE,
                format!("NVML carregou mas não listou dispositivos: {error}"),
            ));
            return Vec::new();
        }
    };

    (0..count)
        .filter_map(|index| {
            let device = match nvml.device_by_index(index) {
                Ok(device) => device,
                Err(error) => {
                    warnings.push(ProbeWarning::warning(
                        SOURCE,
                        format!("NVML não abriu a GPU {index}: {error}"),
                    ));
                    return None;
                }
            };
            let memory = device.memory_info().ok();
            Some(GpuProfile {
                index,
                vendor: GpuVendor::Nvidia,
                model: device.name().unwrap_or_else(|_| "NVIDIA (nome indisponível)".to_owned()),
                pci_address: device.pci_info().ok().map(|pci| normalize_pci_address(&pci.bus_id)),
                vram_total_mb: memory.as_ref().map(|m| bytes_to_mib(m.total)),
                vram_available_mb: memory.as_ref().map(|m| bytes_to_mib(m.free)),
                driver_version: driver_version.clone(),
                runtime_version: runtime_version.clone(),
                compute_capability: device
                    .cuda_compute_capability()
                    .ok()
                    .map(|cc| format!("{}.{}", cc.major, cc.minor)),
                backends: Vec::new(),
                is_integrated: false,
                power_limit_watts: device.power_management_limit().ok().map(milliwatts_to_watts),
                source: GpuProbeSource::Nvml,
            })
        })
        .collect()
}

/// `12040` → `12.4`, como a NVIDIA documenta a versão do runtime CUDA.
fn format_cuda_version(version: i32) -> String {
    let major = version / 1000;
    let minor = (version % 1000) / 10;
    format!("{major}.{minor}")
}

// ---------------------------------------------------------------------------
// nvidia-smi
// ---------------------------------------------------------------------------

/// Executa `nvidia-smi` e converte a saída CSV.
///
/// Os argumentos vão em vetor: nada aqui passa por um shell, então não há como
/// um nome de dispositivo virar comando.
fn nvidia_smi_gpus(warnings: &mut Vec<ProbeWarning>) -> Vec<GpuProfile> {
    let query = format!("--query-gpu={}", nvidia_smi::QUERY_FIELDS);
    let output =
        Command::new("nvidia-smi").args([query.as_str(), nvidia_smi::FORMAT_FLAG]).output();

    let output = match output {
        Ok(output) if output.status.success() => output,
        Ok(output) => {
            warnings.push(ProbeWarning::info(
                SOURCE,
                format!("nvidia-smi terminou com {}; sem dados ao vivo de GPU", output.status),
            ));
            return Vec::new();
        }
        Err(error) => {
            warnings.push(ProbeWarning::info(
                SOURCE,
                format!("nvidia-smi indisponível ({error}); usando apenas sysfs"),
            ));
            return Vec::new();
        }
    };

    let text = String::from_utf8_lossy(&output.stdout);
    match nvidia_smi::parse_nvidia_smi_csv(&text) {
        Ok(entries) => entries.into_iter().map(from_nvidia_smi).collect(),
        Err(error) => {
            warnings.push(ProbeWarning::warning(
                SOURCE,
                format!("saída do nvidia-smi não reconhecida: {error}"),
            ));
            Vec::new()
        }
    }
}

/// Converte uma linha do `nvidia-smi` no tipo do contrato.
fn from_nvidia_smi(entry: nvidia_smi::NvidiaSmiGpu) -> GpuProfile {
    let available =
        entry.memory_total_mb.and_then(|t| entry.memory_used_mb.map(|u| t.saturating_sub(u)));
    GpuProfile {
        index: entry.index,
        vendor: GpuVendor::Nvidia,
        model: entry.name,
        pci_address: entry.pci_bus_id.as_deref().map(normalize_pci_address),
        vram_total_mb: entry.memory_total_mb,
        vram_available_mb: available,
        driver_version: entry.driver_version,
        runtime_version: None,
        compute_capability: entry.compute_capability,
        backends: Vec::new(),
        is_integrated: false,
        power_limit_watts: entry.power_limit_watts,
        source: GpuProbeSource::NvidiaSmi,
    }
}

// ---------------------------------------------------------------------------
// Correlação e backends
// ---------------------------------------------------------------------------

/// Substitui as entradas de sysfs pelas equivalentes ao vivo, casando pelo
/// endereço PCI.
///
/// Uma GPU vista pelo NVML e não pelo sysfs (acontece com `CUDA_VISIBLE_DEVICES`
/// dentro de contêiner) é acrescentada em vez de descartada.
fn merge_live_into_sysfs(gpus: &mut Vec<GpuProfile>, live: Vec<GpuProfile>) {
    let mut used_indices: Vec<u32> = Vec::new();

    for entry in live {
        let matching = entry.pci_address.as_ref().and_then(|address| {
            gpus.iter().position(|gpu| gpu.pci_address.as_ref() == Some(address))
        });
        used_indices.push(entry.index);
        match matching {
            Some(position) => {
                if let Some(slot) = gpus.get_mut(position) {
                    *slot = entry;
                }
            }
            None => gpus.push(entry),
        }
    }

    // Quem não veio de uma fonte ao vivo recebe o primeiro índice livre, para
    // que `GpuSample.index` continue casando com o índice do NVML.
    let mut next = 0;
    for gpu in gpus.iter_mut().filter(|gpu| gpu.source == GpuProbeSource::Sysfs) {
        while used_indices.contains(&next) {
            next += 1;
        }
        gpu.index = next;
        used_indices.push(next);
    }
}

/// `true` quando o driver proprietário da NVIDIA está carregado.
fn nvidia_driver_present(root: &Path, gpus: &[GpuProfile]) -> bool {
    if gpus.iter().any(|gpu| gpu.source != GpuProbeSource::Sysfs) {
        // NVML ou nvidia-smi responderam: o driver está lá, por definição.
        return true;
    }
    exists(root, "/sys/module/nvidia/version") || exists(root, "/dev/nvidiactl")
}

/// Backends que esta GPU realmente pode servir.
///
/// Cada item exige evidência: CUDA exige driver NVIDIA carregado, `ROCm` exige
/// `/dev/kfd`, Vulkan exige um ICD do fabricante instalado. Sem evidência, o
/// backend não entra — uma lista otimista faria o planejador escolher um
/// caminho que falha na primeira inferência.
fn backends_for(root: &Path, gpu: &GpuProfile, nvidia_driver: bool) -> Vec<GpuBackend> {
    let mut backends = Vec::new();

    match gpu.vendor {
        GpuVendor::Nvidia if nvidia_driver => backends.push(GpuBackend::Cuda),
        GpuVendor::Amd if exists(root, "/dev/kfd") => backends.push(GpuBackend::Rocm),
        _ => {}
    }

    if has_vulkan_icd(root, gpu.vendor) {
        backends.push(GpuBackend::Vulkan);
    }
    backends
}

/// Procura um ICD Vulkan do fabricante nos diretórios padrão.
fn has_vulkan_icd(root: &Path, vendor: GpuVendor) -> bool {
    let prefixes: &[&str] = match vendor {
        GpuVendor::Nvidia => &["nvidia_icd", "nouveau_icd"],
        GpuVendor::Amd => &["radeon_icd", "amd_icd"],
        GpuVendor::Intel => &["intel_icd", "intel_hasvk_icd"],
        GpuVendor::Apple => &["asahi_icd"],
        GpuVendor::Unknown => return false,
    };

    VULKAN_ICD_DIRS.iter().any(|dir| {
        list_dir(root, dir)
            .iter()
            .any(|file| prefixes.iter().any(|prefix| file.starts_with(prefix)))
    })
}

/// Converte inteiros pequenos do NVML para o `f32` do contrato.
#[allow(
    clippy::cast_precision_loss,
    reason = "percentuais, temperaturas e clocks do NVML cabem exatamente em f32"
)]
fn to_f32(value: u32) -> f32 {
    value as f32
}

/// Converte miliwatts (unidade do NVML) para watts.
#[allow(clippy::cast_precision_loss, reason = "potência em mW cabe exatamente em f32")]
fn milliwatts_to_watts(milliwatts: u32) -> f32 {
    milliwatts as f32 / 1000.0
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

    /// Sem ferramentas do host: é assim que os testes rodam em qualquer máquina.
    fn offline() -> GpuProbeOptions<'static> {
        GpuProbeOptions { nvml: None, allow_host_tools: false }
    }

    #[test]
    fn nvidia_sem_driver_nao_promete_cuda_nem_vram() {
        let dir = tree(&[
            ("/sys/bus/pci/devices/0000:01:00.0/class", "0x030000\n"),
            ("/sys/bus/pci/devices/0000:01:00.0/vendor", "0x10de\n"),
            ("/sys/bus/pci/devices/0000:01:00.0/device", "0x2d04\n"),
        ]);
        let mut warnings = Vec::new();
        let gpus = probe_gpus(dir.path(), &offline(), &mut warnings);

        assert_eq!(gpus.len(), 1);
        assert_eq!(gpus[0].vendor, GpuVendor::Nvidia);
        assert_eq!(gpus[0].model, "NVIDIA 10de:2d04");
        assert_eq!(gpus[0].pci_address.as_deref(), Some("0000:01:00.0"));
        assert_eq!(gpus[0].vram_total_mb, None, "sem NVML não há VRAM conhecida");
        assert!(gpus[0].backends.is_empty(), "sem driver não há backend");
        assert_eq!(gpus[0].source, GpuProbeSource::Sysfs);
    }

    #[test]
    fn cuda_so_com_driver_presente() {
        let dir = tree(&[
            ("/sys/bus/pci/devices/0000:01:00.0/class", "0x030000\n"),
            ("/sys/bus/pci/devices/0000:01:00.0/vendor", "0x10de\n"),
            ("/sys/bus/pci/devices/0000:01:00.0/device", "0x2d04\n"),
            ("/sys/module/nvidia/version", "580.173.02\n"),
            ("/usr/share/vulkan/icd.d/nvidia_icd.json", "{}"),
        ]);
        let mut warnings = Vec::new();
        let gpus = probe_gpus(dir.path(), &offline(), &mut warnings);
        assert_eq!(gpus[0].backends, vec![GpuBackend::Cuda, GpuBackend::Vulkan]);
    }

    #[test]
    fn amd_reporta_vram_e_rocm_apenas_com_kfd() {
        let files = [
            ("/sys/bus/pci/devices/0000:03:00.0/class", "0x030000\n"),
            ("/sys/bus/pci/devices/0000:03:00.0/vendor", "0x1002\n"),
            ("/sys/bus/pci/devices/0000:03:00.0/device", "0x744c\n"),
            ("/sys/bus/pci/devices/0000:03:00.0/mem_info_vram_total", "25757220864\n"),
            ("/sys/bus/pci/devices/0000:03:00.0/mem_info_vram_used", "1073741824\n"),
        ];
        let mut warnings = Vec::new();

        let sem_kfd = tree(&files);
        let gpus = probe_gpus(sem_kfd.path(), &offline(), &mut warnings);
        assert_eq!(gpus[0].vram_total_mb, Some(24_564));
        assert_eq!(gpus[0].vram_available_mb, Some(24_564 - 1024));
        assert!(gpus[0].backends.is_empty());

        let mut com_kfd = files.to_vec();
        com_kfd.push(("/dev/kfd", ""));
        let dir = tree(&com_kfd);
        let gpus = probe_gpus(dir.path(), &offline(), &mut warnings);
        assert_eq!(gpus[0].backends, vec![GpuBackend::Rocm]);
    }

    #[test]
    fn intel_integrada_nao_reporta_vram_dedicada() {
        let dir = tree(&[
            ("/sys/bus/pci/devices/0000:00:02.0/class", "0x030000\n"),
            ("/sys/bus/pci/devices/0000:00:02.0/vendor", "0x8086\n"),
            ("/sys/bus/pci/devices/0000:00:02.0/device", "0x3e92\n"),
            ("/usr/share/vulkan/icd.d/intel_icd.x86_64.json", "{}"),
        ]);
        let mut warnings = Vec::new();
        let gpus = probe_gpus(dir.path(), &offline(), &mut warnings);

        assert!(gpus[0].is_integrated);
        assert_eq!(gpus[0].vram_total_mb, None);
        assert_eq!(gpus[0].backends, vec![GpuBackend::Vulkan]);
    }

    #[test]
    fn intel_discreta_com_lmem_nao_e_integrada() {
        let dir = tree(&[
            ("/sys/bus/pci/devices/0000:03:00.0/class", "0x030000\n"),
            ("/sys/bus/pci/devices/0000:03:00.0/vendor", "0x8086\n"),
            ("/sys/bus/pci/devices/0000:03:00.0/device", "0x56a0\n"),
            ("/sys/bus/pci/devices/0000:03:00.0/drm/card0/lmem_total_bytes", "17179869184\n"),
        ]);
        let mut warnings = Vec::new();
        let gpus = probe_gpus(dir.path(), &offline(), &mut warnings);
        assert!(!gpus[0].is_integrated);
    }

    #[test]
    fn dispositivo_que_nao_e_video_e_ignorado() {
        let dir = tree(&[
            ("/sys/bus/pci/devices/0000:01:00.1/class", "0x040300\n"),
            ("/sys/bus/pci/devices/0000:01:00.1/vendor", "0x10de\n"),
            ("/sys/bus/pci/devices/0000:01:00.1/device", "0x22e9\n"),
        ]);
        let mut warnings = Vec::new();
        let gpus = probe_gpus(dir.path(), &offline(), &mut warnings);
        assert!(gpus.is_empty());
        assert_eq!(warnings.len(), 1, "ausência de GPU é informação, não erro");
    }

    #[test]
    fn classe_3d_tambem_conta() {
        assert!(is_display_class("0x030000"));
        assert!(is_display_class("0x038000"));
        assert!(!is_display_class("0x040300"));
    }

    #[test]
    fn endereco_pci_normalizado() {
        assert_eq!(normalize_pci_address("00000000:01:00.0"), "0000:01:00.0");
        assert_eq!(normalize_pci_address("0000:01:00.0"), "0000:01:00.0");
        assert_eq!(normalize_pci_address("0000:0A:00.0"), "0000:0a:00.0");
    }

    #[test]
    fn dado_ao_vivo_substitui_o_de_sysfs_pelo_pci() {
        let mut gpus = vec![GpuProfile {
            index: 0,
            vendor: GpuVendor::Nvidia,
            model: "NVIDIA 10de:2d04".to_owned(),
            pci_address: Some("0000:01:00.0".to_owned()),
            vram_total_mb: None,
            vram_available_mb: None,
            driver_version: None,
            runtime_version: None,
            compute_capability: None,
            backends: Vec::new(),
            is_integrated: false,
            power_limit_watts: None,
            source: GpuProbeSource::Sysfs,
        }];
        let live = vec![GpuProfile {
            index: 0,
            vendor: GpuVendor::Nvidia,
            model: "NVIDIA GeForce RTX 5060 Ti".to_owned(),
            pci_address: Some("0000:01:00.0".to_owned()),
            vram_total_mb: Some(8151),
            vram_available_mb: Some(7778),
            driver_version: Some("580.173.02".to_owned()),
            runtime_version: Some("12.0".to_owned()),
            compute_capability: Some("12.0".to_owned()),
            backends: Vec::new(),
            is_integrated: false,
            power_limit_watts: Some(180.0),
            source: GpuProbeSource::Nvml,
        }];

        merge_live_into_sysfs(&mut gpus, live);
        assert_eq!(gpus.len(), 1, "mesma GPU, não duas");
        assert_eq!(gpus[0].model, "NVIDIA GeForce RTX 5060 Ti");
        assert_eq!(gpus[0].vram_total_mb, Some(8151));
    }

    #[test]
    fn gpu_integrada_e_dedicada_convivem_com_indices_distintos() {
        let mut gpus = vec![
            GpuProfile {
                index: 0,
                vendor: GpuVendor::Intel,
                model: "Intel 8086:3e92".to_owned(),
                pci_address: Some("0000:00:02.0".to_owned()),
                vram_total_mb: None,
                vram_available_mb: None,
                driver_version: None,
                runtime_version: None,
                compute_capability: None,
                backends: Vec::new(),
                is_integrated: true,
                power_limit_watts: None,
                source: GpuProbeSource::Sysfs,
            },
            GpuProfile {
                index: 0,
                vendor: GpuVendor::Nvidia,
                model: "NVIDIA 10de:2d04".to_owned(),
                pci_address: Some("0000:01:00.0".to_owned()),
                vram_total_mb: None,
                vram_available_mb: None,
                driver_version: None,
                runtime_version: None,
                compute_capability: None,
                backends: Vec::new(),
                is_integrated: false,
                power_limit_watts: None,
                source: GpuProbeSource::Sysfs,
            },
        ];
        let live = vec![GpuProfile {
            index: 0,
            vendor: GpuVendor::Nvidia,
            model: "NVIDIA GeForce RTX 5060 Ti".to_owned(),
            pci_address: Some("0000:01:00.0".to_owned()),
            vram_total_mb: Some(8151),
            vram_available_mb: Some(7778),
            driver_version: None,
            runtime_version: None,
            compute_capability: None,
            backends: Vec::new(),
            is_integrated: false,
            power_limit_watts: None,
            source: GpuProbeSource::Nvml,
        }];

        merge_live_into_sysfs(&mut gpus, live);
        assert_eq!(gpus.len(), 2);
        let nvidia = gpus.iter().find(|g| g.vendor == GpuVendor::Nvidia).unwrap();
        let intel = gpus.iter().find(|g| g.vendor == GpuVendor::Intel).unwrap();
        assert_eq!(nvidia.index, 0, "índice do NVML é preservado");
        assert_eq!(intel.index, 1, "a integrada recebe o primeiro índice livre");
    }

    #[test]
    fn versao_cuda_formatada() {
        assert_eq!(format_cuda_version(12040), "12.4");
        assert_eq!(format_cuda_version(11080), "11.8");
    }

    #[test]
    fn sample_sem_nvml_e_vazio() {
        assert!(sample_gpus(None).is_empty());
    }
}
