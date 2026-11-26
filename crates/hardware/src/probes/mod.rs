//! Sondas: leem `/proc`, `/sys` e o driver, e traduzem para o contrato.
//!
//! Toda sonda recebe uma **raiz** (`&Path`). Em produção é `/`; nos testes é
//! uma árvore de fixtures. Isso é o que permite testar a detecção de um
//! contêiner com cgroup limitado, ou de uma máquina ARM, sem ter nem uma nem
//! outra à mão.
//!
//! Nenhuma sonda aborta a detecção: o que não dá para saber vira `None` mais um
//! [`ProbeWarning`]. O acumulador de avisos é passado por referência mutável
//! justamente para que uma sonda parcial não precise inventar um tipo de
//! retorno próprio.

pub mod cpu;
pub mod gpu;
pub mod host;
pub mod ram;
pub mod storage;
pub mod topology;

use std::path::{Path, PathBuf};

/// Resolve um caminho absoluto do sistema dentro da raiz informada.
///
/// `path_under("/", "/proc/meminfo")` é `/proc/meminfo`;
/// `path_under("/fixtures/arm", "/proc/meminfo")` é
/// `/fixtures/arm/proc/meminfo`.
pub(crate) fn path_under(root: &Path, absolute: &str) -> PathBuf {
    root.join(absolute.trim_start_matches('/'))
}

/// Lê um arquivo sob a raiz, sem aparar nada.
pub(crate) fn read_file(root: &Path, absolute: &str) -> std::io::Result<String> {
    std::fs::read_to_string(path_under(root, absolute))
}

/// Lê um arquivo sob a raiz e apara espaços em branco das pontas.
///
/// Quase todo arquivo de `/sys` é um valor único seguido de `\n`; esquecer o
/// `\n` é o motivo clássico de uma comparação de string falhar sem explicação.
pub(crate) fn read_trimmed(root: &Path, absolute: &str) -> Option<String> {
    let text = read_file(root, absolute).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() { None } else { Some(trimmed.to_owned()) }
}

/// Lê um inteiro de um arquivo de `/sys`.
pub(crate) fn read_u64(root: &Path, absolute: &str) -> Option<u64> {
    read_trimmed(root, absolute)?.parse().ok()
}

/// `true` se o caminho existe sob a raiz.
pub(crate) fn exists(root: &Path, absolute: &str) -> bool {
    path_under(root, absolute).exists()
}

/// Nomes das entradas de um diretório sob a raiz, em ordem estável.
///
/// A ordem importa: sem ela, dois `detect()` seguidos poderiam listar as GPUs
/// em ordens diferentes e mudar o fingerprint da máquina.
pub(crate) fn list_dir(root: &Path, absolute: &str) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(path_under(root, absolute)) else {
        return Vec::new();
    };
    let mut names: Vec<String> =
        entries.filter_map(|entry| Some(entry.ok()?.file_name().to_str()?.to_owned())).collect();
    names.sort_unstable();
    names
}

/// Converte KiB (unidade de `/proc`) para MiB (unidade do contrato).
pub(crate) fn kib_to_mib(kib: u64) -> u64 {
    kib / 1024
}

/// Converte bytes (unidade de `/sys` e do NVML) para MiB.
pub(crate) fn bytes_to_mib(bytes: u64) -> u64 {
    bytes / (1024 * 1024)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "falha de teste deve estourar imediatamente")]
mod tests {
    use super::*;

    #[test]
    fn caminho_relativo_a_raiz() {
        assert_eq!(path_under(Path::new("/"), "/proc/meminfo"), PathBuf::from("/proc/meminfo"));
        assert_eq!(
            path_under(Path::new("/fixtures/arm"), "/proc/meminfo"),
            PathBuf::from("/fixtures/arm/proc/meminfo")
        );
    }

    #[test]
    fn le_e_apara() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sys")).unwrap();
        std::fs::write(dir.path().join("sys/valor"), "  42 \n").unwrap();
        assert_eq!(read_trimmed(dir.path(), "/sys/valor").as_deref(), Some("42"));
        assert_eq!(read_u64(dir.path(), "/sys/valor"), Some(42));
        assert_eq!(read_u64(dir.path(), "/sys/inexistente"), None);
    }

    #[test]
    fn arquivo_vazio_e_none_nao_string_vazia() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("vazio"), "\n").unwrap();
        assert_eq!(read_trimmed(dir.path(), "/vazio"), None);
    }

    #[test]
    fn listagem_e_ordenada() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["cpu10", "cpu2", "cpu1"] {
            std::fs::create_dir(dir.path().join(name)).unwrap();
        }
        assert_eq!(list_dir(dir.path(), "/"), vec!["cpu1", "cpu10", "cpu2"]);
        assert!(list_dir(dir.path(), "/inexistente").is_empty());
    }

    #[test]
    fn conversoes_de_unidade() {
        assert_eq!(kib_to_mib(2048), 2);
        assert_eq!(bytes_to_mib(3 * 1024 * 1024), 3);
    }
}
