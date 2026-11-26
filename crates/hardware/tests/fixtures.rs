//! Detecção completa sobre árvores sintéticas de `/proc` e `/sys`.
//!
//! Cada fixture em `tests/fixtures/` é uma máquina inteira em disco. É assim
//! que dá para verificar o comportamento em um Raspberry Pi ou dentro de um
//! contêiner limitado sem ter nem um nem outro — e é por isso que
//! [`LinuxHardwareProvider::with_root`] existe.
//!
//! - `desktop-x86-nvidia`: Ryzen 8c/16t, 32 GiB, RTX no PCI, `NVMe` + SATA,
//!   cgroup v2 sem limites;
//! - `arm-no-gpu`: quatro Cortex-A72, 8 GiB, sem GPU, sem SMT, sem cgroup;
//! - `container-cgroup2`: o mesmo host x86, porém dentro de um contêiner
//!   limitado a 2 núcleos e 4 GiB.

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]

use std::path::PathBuf;

use elara_hardware::types::{
    CgroupVersion, CpuVendor, GpuBackend, GpuProbeSource, GpuVendor, StorageKind, Virtualization,
    WarningSeverity,
};
use elara_hardware::{HardwareProfile, HardwareProvider, LinuxHardwareProvider};

/// Raiz de uma fixture.
fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Perfil detectado a partir de uma fixture.
fn detect(name: &str) -> HardwareProfile {
    LinuxHardwareProvider::with_root(fixture(name)).detect().unwrap()
}

// ---------------------------------------------------------------------------
// Desktop x86_64 com NVIDIA
// ---------------------------------------------------------------------------

#[test]
fn desktop_identifica_cpu_com_smt() {
    let profile = detect("desktop-x86-nvidia");

    assert_eq!(profile.cpu.model, "AMD Ryzen 7 5800X 8-Core Processor");
    assert_eq!(profile.cpu.vendor, CpuVendor::Amd);
    assert_eq!(profile.cpu.architecture, "x86_64");
    assert_eq!(profile.cpu.physical_cores, 8, "16 threads são 8 núcleos com SMT");
    assert_eq!(profile.cpu.logical_threads, 16);
    assert_eq!(profile.cpu.online_threads, 16);
    assert_eq!(profile.cpu.sockets, 1);
    assert_eq!(profile.cpu.hybrid, None, "Zen 3 não é híbrida");
}

#[test]
fn desktop_le_frequencias_e_caches() {
    let profile = detect("desktop-x86-nvidia");

    assert_eq!(profile.cpu.base_frequency_mhz, Some(3800));
    assert_eq!(profile.cpu.max_frequency_mhz, Some(4850));
    assert_eq!(profile.cpu.cache.l1d_kb, Some(256), "8 instâncias de 32K");
    assert_eq!(profile.cpu.cache.l1i_kb, Some(256));
    assert_eq!(profile.cpu.cache.l2_kb, Some(4096), "8 instâncias de 512K");
    assert_eq!(profile.cpu.cache.l3_kb, Some(32_768), "uma instância compartilhada");
}

#[test]
fn desktop_reconhece_extensoes_relevantes() {
    let features = detect("desktop-x86-nvidia").cpu.features;

    assert!(features.avx2 && features.fma && features.f16c && features.sse4_2);
    assert!(!features.avx512f, "Zen 3 não tem AVX-512");
    assert!(!features.neon, "flags ARM não aparecem em x86");
}

#[test]
fn desktop_le_memoria_sem_estimar_banda() {
    let ram = detect("desktop-x86-nvidia").ram;

    assert_eq!(ram.total_mb, 32_768);
    assert_eq!(ram.available_mb, 24_576);
    assert_eq!(ram.free_mb, 8_192);
    assert_eq!(ram.swap_total_mb, 8_192);
    assert_eq!(ram.measured_bandwidth_gbps, None);
}

#[test]
fn desktop_identifica_gpu_por_pci_sem_inventar_vram() {
    let profile = detect("desktop-x86-nvidia");

    assert_eq!(profile.gpus.len(), 1, "o dispositivo de áudio da placa não é uma GPU");
    let gpu = &profile.gpus[0];
    assert_eq!(gpu.vendor, GpuVendor::Nvidia);
    assert_eq!(gpu.pci_address.as_deref(), Some("0000:01:00.0"));
    assert_eq!(gpu.source, GpuProbeSource::Sysfs);
    assert_eq!(gpu.vram_total_mb, None, "sem NVML não há como saber a VRAM");
    assert!(!gpu.is_integrated);
    assert_eq!(
        gpu.backends,
        vec![GpuBackend::Cuda, GpuBackend::Vulkan],
        "há módulo nvidia carregado e ICD Vulkan instalado"
    );
}

#[test]
fn desktop_lista_discos_fisicos_e_ignora_virtuais() {
    let profile = detect("desktop-x86-nvidia");
    let devices: Vec<&str> = profile.storage.iter().map(|d| d.device.as_str()).collect();

    assert_eq!(devices, vec!["nvme0n1", "sda"], "loop, zram e dm ficam de fora");
    assert_eq!(profile.storage[0].kind, StorageKind::Nvme);
    assert_eq!(profile.storage[0].model.as_deref(), Some("Samsung SSD 990 PRO 1TB"));
    assert_eq!(profile.storage[1].kind, StorageKind::Ssd);
    assert_eq!(profile.storage[0].size_mb, 976_762);
}

#[test]
fn desktop_associa_particoes_ao_disco_pai() {
    let profile = detect("desktop-x86-nvidia");

    let nvme_mounts: Vec<&str> =
        profile.storage[0].mounts.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(nvme_mounts, vec!["/", "/boot/efi"]);
    assert!(profile.storage[0].mounts[1].read_only);

    let sata_mounts: Vec<&str> =
        profile.storage[1].mounts.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(sata_mounts, vec!["/mnt/dados"]);
}

#[test]
fn desktop_le_host_e_topologia() {
    let profile = detect("desktop-x86-nvidia");

    assert_eq!(profile.host.hostname.as_deref(), Some("example-desktop"));
    assert_eq!(profile.host.distribution.as_deref(), Some("Pop!_OS 24.04 LTS"));
    assert_eq!(profile.host.kernel_version.as_deref(), Some("6.12.10-generic"));
    assert_eq!(profile.host.virtualization, None, "máquina física");

    assert!(profile.topology.simultaneous_multithreading);
    assert_eq!(profile.topology.numa_nodes.len(), 1);
    assert_eq!(profile.topology.numa_nodes[0].cpus.len(), 16);
    assert_eq!(profile.topology.numa_nodes[0].memory_mb, Some(32_768));
}

#[test]
fn desktop_sem_limite_de_cgroup_usa_a_maquina_toda() {
    let profile = detect("desktop-x86-nvidia");

    assert_eq!(profile.topology.cgroup.version, CgroupVersion::V2);
    assert_eq!(profile.topology.cgroup.cpu_quota_cores, None);
    assert_eq!(profile.topology.cgroup.memory_limit_mb, None);
    assert_eq!(profile.topology.cgroup.memory_usage_mb, Some(2048));

    assert_eq!(profile.effective.cpu_threads, 15, "16 online menos 1 do sistema");
    assert_eq!(profile.effective.ram_mb, 24_576 - 3_686);
    assert_eq!(profile.effective.vram_mb, 0, "VRAM desconhecida não vira orçamento");
}

#[test]
fn desktop_nao_produz_aviso_de_severidade_error() {
    let profile = detect("desktop-x86-nvidia");
    let graves: Vec<_> =
        profile.warnings.iter().filter(|w| w.severity == WarningSeverity::Error).collect();
    assert!(graves.is_empty(), "avisos graves inesperados: {graves:?}");
}

// ---------------------------------------------------------------------------
// ARM sem GPU
// ---------------------------------------------------------------------------

#[test]
fn arm_identifica_cpu_pelo_dialeto_proprio() {
    let profile = detect("arm-no-gpu");

    assert_eq!(profile.cpu.model, "Raspberry Pi 4 Model B Rev 1.1");
    assert_eq!(profile.cpu.vendor, CpuVendor::Arm);
    assert_eq!(profile.cpu.architecture, "aarch64");
    assert_eq!(profile.host.architecture, "aarch64", "a arquitetura vem da fixture, não do host");
    assert_eq!(profile.cpu.physical_cores, 4, "sem SMT, cada thread é um núcleo");
    assert_eq!(profile.cpu.logical_threads, 4);
    assert_eq!(profile.cpu.sockets, 1);
}

#[test]
fn arm_reconhece_neon_e_nao_reconhece_avx() {
    let features = detect("arm-no-gpu").cpu.features;
    assert!(features.neon, "asimd é NEON");
    assert!(!features.dotprod && !features.i8mm && !features.sve);
    assert!(!features.avx2);
}

#[test]
fn arm_sem_gpu_reporta_lista_vazia_e_avisa() {
    let profile = detect("arm-no-gpu");

    assert!(profile.gpus.is_empty());
    assert_eq!(profile.effective.vram_mb, 0);
    assert!(profile.warnings.iter().any(|w| w.source == "gpu"));
}

#[test]
fn arm_sem_smt_e_sem_cgroup() {
    let profile = detect("arm-no-gpu");

    assert!(!profile.topology.simultaneous_multithreading);
    assert_eq!(profile.topology.cgroup.version, CgroupVersion::None);
    assert_eq!(profile.effective.cpu_threads, 3, "4 online menos 1 do sistema");
}

#[test]
fn arm_le_cartao_sd_e_suas_particoes() {
    let profile = detect("arm-no-gpu");

    assert_eq!(profile.storage.len(), 1);
    assert_eq!(profile.storage[0].device, "mmcblk0");
    let paths: Vec<&str> = profile.storage[0].mounts.iter().map(|m| m.path.as_str()).collect();
    assert_eq!(paths, vec!["/", "/boot/firmware"]);
}

#[test]
fn arm_sem_frequencia_base_declarada_nao_inventa() {
    let cpu = detect("arm-no-gpu").cpu;
    assert_eq!(cpu.base_frequency_mhz, None);
    assert_eq!(cpu.max_frequency_mhz, Some(1800));
}

// ---------------------------------------------------------------------------
// Contêiner com cgroup v2 limitado
// ---------------------------------------------------------------------------

#[test]
fn container_e_detectado_como_docker() {
    let profile = detect("container-cgroup2");
    assert_eq!(profile.host.virtualization, Some(Virtualization::Docker));
}

#[test]
fn container_le_os_limites_do_cgroup() {
    let cgroup = detect("container-cgroup2").topology.cgroup;

    assert_eq!(cgroup.version, CgroupVersion::V2);
    assert_eq!(cgroup.cpu_quota_cores, Some(2.0));
    assert_eq!(cgroup.memory_limit_mb, Some(4096));
    assert_eq!(cgroup.memory_usage_mb, Some(512));
}

#[test]
fn container_respeita_o_cgroup_e_nao_a_maquina_do_host() {
    let profile = detect("container-cgroup2");

    // O host continua visível — é exatamente por isso que confiar nele mata o
    // processo por OOM.
    assert_eq!(profile.cpu.online_threads, 16);
    assert_eq!(profile.ram.total_mb, 32_768);
    assert_eq!(profile.ram.available_mb, 24_576);

    assert_eq!(profile.effective.cpu_threads, 2, "a cota de 2 núcleos manda");
    assert_eq!(profile.effective.ram_mb, 3_047, "4096 MiB de limite, 512 em uso, menos a folga");
    assert!(
        profile.effective.ram_mb < profile.ram.available_mb / 4,
        "o orçamento tem de ficar muito abaixo da RAM do host"
    );
}

#[test]
fn container_explica_cada_numero_do_orcamento() {
    let rationale = detect("container-cgroup2").effective.rationale;

    assert!(rationale.iter().any(|line| line.contains("cota do cgroup")), "{rationale:?}");
    assert!(rationale.iter().any(|line| line.contains("cgroup permite 4096 MiB")), "{rationale:?}");
}

// ---------------------------------------------------------------------------
// Fingerprint
// ---------------------------------------------------------------------------

#[test]
fn fingerprint_e_estavel_para_a_mesma_maquina() {
    let primeiro = detect("desktop-x86-nvidia").fingerprint;
    let segundo = detect("desktop-x86-nvidia").fingerprint;

    assert_eq!(primeiro, segundo);
    assert_eq!(primeiro.len(), 32);
}

#[test]
fn maquinas_diferentes_tem_fingerprints_diferentes() {
    let desktop = detect("desktop-x86-nvidia").fingerprint;
    let arm = detect("arm-no-gpu").fingerprint;
    let container = detect("container-cgroup2").fingerprint;

    assert_ne!(desktop, arm);
    assert_ne!(desktop, container, "o contêiner não enxerga a GPU do host");
    assert_ne!(arm, container);
}

// ---------------------------------------------------------------------------
// Amostragem
// ---------------------------------------------------------------------------

#[test]
fn sample_le_contadores_da_fixture() {
    let provider = LinuxHardwareProvider::with_root(fixture("desktop-x86-nvidia"));
    let sample = provider.sample().unwrap();

    assert_eq!(sample.cpu_per_core.len(), 16);
    assert_eq!(sample.cpu_total.user, 1_200_000);
    assert_eq!(sample.memory.total_mb, 32_768);
    assert!((sample.load_average[0] - 1.42).abs() < f64::EPSILON);
    assert!(sample.gpus.is_empty(), "sem NVML sob raiz de fixture");
}

#[test]
fn provider_serve_como_objeto_de_trait_compartilhado() {
    // O daemon guarda um `Arc<dyn HardwareProvider>` e o consulta de várias
    // tarefas; se o tipo não fosse `Send + Sync`, isto nem compilaria.
    let provider: std::sync::Arc<dyn HardwareProvider> =
        std::sync::Arc::new(LinuxHardwareProvider::with_root(fixture("desktop-x86-nvidia")));
    let outra_thread = std::sync::Arc::clone(&provider);

    let detectado = std::thread::spawn(move || outra_thread.detect().map(|p| p.fingerprint));
    assert_eq!(detectado.join().unwrap().unwrap(), provider.detect().unwrap().fingerprint);
    assert!(provider.sample().is_ok());
}

#[test]
fn sample_de_fixture_incompleta_falha_com_caminho_no_erro() {
    let provider = LinuxHardwareProvider::with_root(fixture("inexistente"));
    let error = provider.sample().unwrap_err().to_string();
    assert!(error.contains("/proc/stat"), "erro pouco informativo: {error}");
}
