//! Identificação do sistema operacional e do ambiente de execução.
//!
//! A detecção de virtualização não é curiosidade: dentro de um contêiner os
//! limites que valem são os do cgroup, e em uma VM a VRAM anunciada pode não
//! existir. Saber onde estamos muda o que o resto do sistema pode assumir.

use std::path::Path;

use super::{exists, read_file, read_trimmed};
use crate::parse::{cpuinfo, os_release};
use crate::types::{HostProfile, ProbeWarning, Virtualization};

/// Nome desta sonda nos avisos.
const SOURCE: &str = "host";

/// Coleta a identificação do host.
///
/// Nunca falha: sem `/etc/os-release` ainda sabemos o kernel; sem nada disso
/// ainda produzimos um perfil com avisos, porque um host desconhecido não
/// impede o Elara de rodar.
pub fn probe_host(root: &Path, warnings: &mut Vec<ProbeWarning>) -> HostProfile {
    let hostname = read_trimmed(root, "/proc/sys/kernel/hostname")
        .or_else(|| read_trimmed(root, "/etc/hostname"));
    if hostname.is_none() {
        warnings.push(ProbeWarning::info(SOURCE, "hostname não encontrado em /proc nem em /etc"));
    }

    let distribution = probe_distribution(root, warnings);

    let kernel_version = read_trimmed(root, "/proc/sys/kernel/osrelease");
    if kernel_version.is_none() {
        warnings.push(ProbeWarning::info(SOURCE, "/proc/sys/kernel/osrelease ausente ou ilegível"));
    }

    HostProfile {
        hostname,
        os: "linux".to_owned(),
        distribution,
        kernel_version,
        architecture: probe_architecture(root, warnings),
        virtualization: detect_virtualization(root),
    }
}

/// Lê a distribuição de `/etc/os-release`, com o caminho alternativo de
/// distribuições que só populam `/usr/lib`.
fn probe_distribution(root: &Path, warnings: &mut Vec<ProbeWarning>) -> Option<String> {
    let text =
        read_file(root, "/etc/os-release").or_else(|_| read_file(root, "/usr/lib/os-release")).ok();

    let Some(text) = text else {
        warnings.push(ProbeWarning::info(SOURCE, "os-release não encontrado"));
        return None;
    };

    let distribution = os_release::parse_os_release(&text).distribution();
    if distribution.is_none() {
        warnings.push(ProbeWarning::info(
            SOURCE,
            "os-release existe mas não traz NAME nem PRETTY_NAME",
        ));
    }
    distribution
}

/// Arquitetura da máquina descrita pela raiz.
///
/// Deriva de `/proc/cpuinfo` em vez de `uname(2)` porque uma syscall ignoraria
/// a raiz injetada: sob uma fixture ARM lida de um host x86, `uname` mentiria.
/// Só quando a raiz é a real caímos para a arquitetura do próprio binário.
fn probe_architecture(root: &Path, warnings: &mut Vec<ProbeWarning>) -> String {
    let from_cpuinfo = read_file(root, "/proc/cpuinfo")
        .ok()
        .and_then(|text| cpuinfo::parse_cpuinfo(&text).ok())
        .and_then(|info| info.architecture());

    if let Some(arch) = from_cpuinfo {
        return arch.to_owned();
    }

    if root == Path::new("/") {
        warnings.push(ProbeWarning::info(
            SOURCE,
            "arquitetura não inferível de /proc/cpuinfo; usando a do binário em execução",
        ));
        return std::env::consts::ARCH.to_owned();
    }

    warnings.push(ProbeWarning::warning(SOURCE, "arquitetura não inferível de /proc/cpuinfo"));
    "unknown".to_owned()
}

/// Detecta contêiner ou hipervisor.
///
/// A ordem importa: marcadores de contêiner vêm antes dos de VM porque um
/// contêiner dentro de uma VM é limitado pelo contêiner, e é esse o dado que
/// muda decisões de recurso.
pub fn detect_virtualization(root: &Path) -> Option<Virtualization> {
    if let Some(container) = detect_container(root) {
        return Some(container);
    }
    if let Some(hypervisor) = detect_hypervisor(root) {
        return Some(hypervisor);
    }
    // Última evidência: a CPU anuncia que está sob um hipervisor, mas nada
    // identifica qual. Melhor dizer "virtualizado, não sei qual" que nada.
    let cpuinfo = read_file(root, "/proc/cpuinfo").ok()?;
    cpuinfo
        .lines()
        .filter(|line| line.starts_with("flags"))
        .any(|line| line.split_ascii_whitespace().any(|flag| flag == "hypervisor"))
        .then_some(Virtualization::Unknown)
}

/// Marcadores de conteinerização.
fn detect_container(root: &Path) -> Option<Virtualization> {
    if exists(root, "/.dockerenv") {
        return Some(Virtualization::Docker);
    }
    if exists(root, "/run/.containerenv") {
        return Some(Virtualization::Podman);
    }

    let cgroup =
        read_file(root, "/proc/1/cgroup").or_else(|_| read_file(root, "/proc/self/cgroup")).ok()?;
    let lower = cgroup.to_ascii_lowercase();
    if lower.contains("libpod") || lower.contains("podman") {
        return Some(Virtualization::Podman);
    }
    if lower.contains("docker") || lower.contains("containerd") {
        return Some(Virtualization::Docker);
    }
    if lower.contains("/lxc") || lower.contains("lxc.payload") {
        return Some(Virtualization::Lxc);
    }
    None
}

/// Marcadores de hipervisor, por DMI e pela assinatura do kernel.
fn detect_hypervisor(root: &Path) -> Option<Virtualization> {
    if let Some(version) = read_trimmed(root, "/proc/version") {
        let lower = version.to_ascii_lowercase();
        if lower.contains("microsoft") || lower.contains("wsl") {
            return Some(Virtualization::Wsl);
        }
    }

    let dmi: String = [
        "/sys/class/dmi/id/sys_vendor",
        "/sys/class/dmi/id/product_name",
        "/sys/class/dmi/id/product_version",
        "/sys/class/dmi/id/board_vendor",
        "/sys/class/dmi/id/bios_vendor",
    ]
    .iter()
    .filter_map(|path| read_trimmed(root, path))
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase();

    if dmi.is_empty() {
        return None;
    }
    if dmi.contains("qemu") || dmi.contains("kvm") || dmi.contains("bochs") {
        return Some(Virtualization::Kvm);
    }
    if dmi.contains("xen") {
        return Some(Virtualization::Xen);
    }
    if dmi.contains("vmware") {
        return Some(Virtualization::VmWare);
    }
    if dmi.contains("virtualbox") || dmi.contains("innotek") {
        return Some(Virtualization::VirtualBox);
    }
    if dmi.contains("microsoft") && dmi.contains("virtual") {
        return Some(Virtualization::HyperV);
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;

    /// Cria uma árvore mínima e devolve a raiz temporária.
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
    fn identifica_host_completo() {
        let dir = tree(&[
            ("/proc/sys/kernel/hostname", "biel-pc\n"),
            ("/proc/sys/kernel/osrelease", "7.0.11-76070011-generic\n"),
            ("/etc/os-release", "PRETTY_NAME=\"Pop!_OS 24.04 LTS\"\nID=pop\n"),
            ("/proc/cpuinfo", "processor\t: 0\nvendor_id\t: GenuineIntel\nflags\t\t: fpu lm\n"),
        ]);
        let mut warnings = Vec::new();
        let host = probe_host(dir.path(), &mut warnings);

        assert_eq!(host.hostname.as_deref(), Some("biel-pc"));
        assert_eq!(host.os, "linux");
        assert_eq!(host.distribution.as_deref(), Some("Pop!_OS 24.04 LTS"));
        assert_eq!(host.kernel_version.as_deref(), Some("7.0.11-76070011-generic"));
        assert_eq!(host.architecture, "x86_64");
        assert_eq!(host.virtualization, None);
        assert!(warnings.is_empty(), "avisos inesperados: {warnings:?}");
    }

    #[test]
    fn arvore_vazia_avisa_em_vez_de_falhar() {
        let dir = tree(&[]);
        let mut warnings = Vec::new();
        let host = probe_host(dir.path(), &mut warnings);

        assert_eq!(host.hostname, None);
        assert_eq!(host.architecture, "unknown");
        assert_eq!(warnings.len(), 4, "hostname, os-release, kernel e arquitetura");
    }

    #[test]
    fn arquitetura_arm_vem_do_cpuinfo_e_nao_do_binario() {
        let dir = tree(&[(
            "/proc/cpuinfo",
            "processor\t: 0\nCPU implementer\t: 0x41\nCPU architecture: 8\nCPU part\t: 0xd0b\n",
        )]);
        let mut warnings = Vec::new();
        assert_eq!(probe_host(dir.path(), &mut warnings).architecture, "aarch64");
    }

    #[test]
    fn docker_por_arquivo_marcador() {
        let dir = tree(&[("/.dockerenv", "")]);
        assert_eq!(detect_virtualization(dir.path()), Some(Virtualization::Docker));
    }

    #[test]
    fn docker_por_cgroup_do_pid_1() {
        let dir = tree(&[("/proc/1/cgroup", "0::/docker/9f0aab12\n")]);
        assert_eq!(detect_virtualization(dir.path()), Some(Virtualization::Docker));
    }

    #[test]
    fn podman_tem_precedencia_sobre_docker() {
        let dir = tree(&[("/run/.containerenv", ""), ("/proc/1/cgroup", "0::/machine.slice\n")]);
        assert_eq!(detect_virtualization(dir.path()), Some(Virtualization::Podman));
    }

    #[test]
    fn lxc_por_cgroup() {
        let dir = tree(&[("/proc/1/cgroup", "0::/lxc.payload.meu-container\n")]);
        assert_eq!(detect_virtualization(dir.path()), Some(Virtualization::Lxc));
    }

    #[test]
    fn wsl_pela_assinatura_do_kernel() {
        let dir = tree(&[("/proc/version", "Linux version 5.15.0-microsoft-standard-WSL2\n")]);
        assert_eq!(detect_virtualization(dir.path()), Some(Virtualization::Wsl));
    }

    #[test]
    fn hipervisores_por_dmi() {
        for (vendor, expected) in [
            ("QEMU", Virtualization::Kvm),
            ("Xen", Virtualization::Xen),
            ("VMware, Inc.", Virtualization::VmWare),
            ("innotek GmbH", Virtualization::VirtualBox),
        ] {
            let dir = tree(&[("/sys/class/dmi/id/sys_vendor", vendor)]);
            assert_eq!(detect_virtualization(dir.path()), Some(expected), "vendor {vendor}");
        }
    }

    #[test]
    fn hyperv_precisa_de_microsoft_e_virtual() {
        let dir = tree(&[
            ("/sys/class/dmi/id/sys_vendor", "Microsoft Corporation"),
            ("/sys/class/dmi/id/product_name", "Virtual Machine"),
        ]);
        assert_eq!(detect_virtualization(dir.path()), Some(Virtualization::HyperV));
    }

    #[test]
    fn flag_hypervisor_sem_identificacao_vira_unknown() {
        let dir = tree(&[
            ("/sys/class/dmi/id/sys_vendor", "To Be Filled By O.E.M."),
            ("/proc/cpuinfo", "processor\t: 0\nflags\t\t: fpu lm hypervisor\n"),
        ]);
        assert_eq!(detect_virtualization(dir.path()), Some(Virtualization::Unknown));
    }

    #[test]
    fn maquina_fisica_nao_reporta_virtualizacao() {
        let dir = tree(&[
            ("/sys/class/dmi/id/sys_vendor", "ASRock"),
            ("/proc/cpuinfo", "processor\t: 0\nflags\t\t: fpu lm avx2\n"),
        ]);
        assert_eq!(detect_virtualization(dir.path()), None);
    }
}
