//! Detecção na máquina real, afirmando **apenas invariantes**.
//!
//! Este arquivo roda em qualquer Linux: no desktop com RTX do desenvolvedor, no
//! contêiner sem GPU do CI e em um servidor ARM. Por isso nenhuma asserção cita
//! um modelo, uma contagem ou um tamanho — só relações que precisam valer em
//! toda máquina. Um teste que exigisse 32 GiB de RAM quebraria no CI e seria
//! desativado na semana seguinte, deixando a sonda real sem cobertura nenhuma.

#![allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]

use elara_hardware::types::HARDWARE_SCHEMA_VERSION;
use elara_hardware::{HardwareProvider, LinuxHardwareProvider, fingerprint};

#[test]
fn detecta_a_maquina_real_respeitando_os_invariantes() {
    let profile = LinuxHardwareProvider::new().detect().unwrap();

    assert_eq!(profile.schema_version, HARDWARE_SCHEMA_VERSION);
    assert_eq!(profile.host.os, "linux");
    assert!(!profile.host.architecture.is_empty());

    assert!(profile.cpu.logical_threads >= 1);
    assert!(profile.cpu.online_threads >= 1);
    assert!(profile.cpu.physical_cores >= 1);
    assert!(
        profile.cpu.physical_cores <= profile.cpu.logical_threads,
        "não pode haver mais núcleos físicos que threads"
    );
    assert!(profile.cpu.sockets >= 1);
    assert!(!profile.cpu.model.is_empty());

    assert!(profile.ram.total_mb > 0);
    assert!(
        profile.ram.available_mb <= profile.ram.total_mb,
        "memória disponível não pode exceder a total"
    );
    assert!(profile.ram.free_mb <= profile.ram.total_mb);
    assert!(profile.ram.swap_free_mb <= profile.ram.swap_total_mb);
}

#[test]
fn fingerprint_da_maquina_real_tem_o_formato_esperado() {
    let profile = LinuxHardwareProvider::new().detect().unwrap();

    assert_eq!(profile.fingerprint.len(), fingerprint::FINGERPRINT_LEN);
    assert!(profile.fingerprint.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        profile.fingerprint,
        fingerprint::of_profile(&profile),
        "o fingerprint gravado tem de ser reproduzível a partir do próprio perfil"
    );
}

#[test]
fn duas_deteccoes_seguidas_descrevem_a_mesma_maquina() {
    let provider = LinuxHardwareProvider::new();
    let primeira = provider.detect().unwrap();
    let segunda = provider.detect().unwrap();

    assert_eq!(
        primeira.fingerprint, segunda.fingerprint,
        "o uso de memória muda entre as duas coletas; a identidade não pode mudar"
    );
}

#[test]
fn orcamento_efetivo_cabe_no_que_a_maquina_tem() {
    let profile = LinuxHardwareProvider::new().detect().unwrap();

    assert!(profile.effective.cpu_threads >= 1);
    assert!(
        profile.effective.cpu_threads <= profile.cpu.online_threads.max(1),
        "não dá para usar mais threads do que existem online"
    );
    assert!(
        profile.effective.ram_mb <= profile.ram.available_mb,
        "o orçamento não pode passar da memória disponível"
    );
    assert!((profile.effective.vram_headroom_ratio - 0.10).abs() < f64::EPSILON);
    assert!(
        !profile.effective.rationale.is_empty(),
        "todo número do orçamento precisa vir com explicação"
    );
}

#[test]
fn gpus_reais_nao_reportam_mais_livre_que_total() {
    let profile = LinuxHardwareProvider::new().detect().unwrap();

    for gpu in &profile.gpus {
        if let (Some(total), Some(available)) = (gpu.vram_total_mb, gpu.vram_available_mb) {
            assert!(available <= total, "{}: {available} MiB livres de {total} MiB", gpu.model);
        }
        if gpu.is_integrated {
            assert_eq!(gpu.vram_total_mb, None, "GPU integrada não tem VRAM dedicada");
        }
    }
}

#[test]
fn montagens_reais_nao_reportam_mais_livre_que_total() {
    let profile = LinuxHardwareProvider::new().detect().unwrap();

    for disk in &profile.storage {
        for mount in &disk.mounts {
            assert!(
                mount.available_mb <= mount.total_mb,
                "{}: {} MiB livres de {} MiB",
                mount.path,
                mount.available_mb,
                mount.total_mb
            );
        }
    }
}

#[test]
fn amostra_real_e_barata_e_monotonica() {
    let provider = LinuxHardwareProvider::new();
    let primeira = provider.sample().unwrap();
    let segunda = provider.sample().unwrap();

    assert!(!primeira.cpu_per_core.is_empty());
    assert!(primeira.cpu_total.total() > 0);
    assert!(
        segunda.cpu_total.total() >= primeira.cpu_total.total(),
        "contadores de /proc/stat só crescem"
    );
    assert!(segunda.taken_at >= primeira.taken_at);
    assert!(primeira.memory.available_mb <= primeira.memory.total_mb);
    assert!(primeira.load_average.iter().all(|value| *value >= 0.0));
}

#[test]
fn amostra_de_gpu_casa_com_o_perfil() {
    let provider = LinuxHardwareProvider::new();
    let profile = provider.detect().unwrap();
    let sample = provider.sample().unwrap();

    for gpu_sample in &sample.gpus {
        assert!(
            profile.gpus.iter().any(|gpu| gpu.index == gpu_sample.index),
            "amostra da GPU {} não corresponde a nenhuma GPU do perfil",
            gpu_sample.index
        );
        if let Some(percent) = gpu_sample.utilization_percent {
            assert!((0.0..=100.0).contains(&percent));
        }
    }
}
