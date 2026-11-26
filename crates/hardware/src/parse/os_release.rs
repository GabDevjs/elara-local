//! Leitura de `/etc/os-release` (formato `KEY="valor"`).

use super::split_key_value;

/// Campos do `os-release` que identificam a distribuição.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    /// `ID`, ex.: `pop`, `ubuntu`, `arch`.
    pub id: Option<String>,
    /// `NAME`, ex.: `Pop!_OS`.
    pub name: Option<String>,
    /// `VERSION`, ex.: `24.04 LTS`.
    pub version: Option<String>,
    /// `VERSION_ID`, ex.: `24.04`.
    pub version_id: Option<String>,
    /// `PRETTY_NAME`, ex.: `Pop!_OS 24.04 LTS`.
    pub pretty_name: Option<String>,
}

impl OsRelease {
    /// Melhor rótulo disponível da distribuição.
    ///
    /// `PRETTY_NAME` é o campo que a própria distribuição escolheu para se
    /// apresentar; só quando ele falta montamos um a partir de `NAME` e
    /// `VERSION`.
    pub fn distribution(&self) -> Option<String> {
        if let Some(pretty) = &self.pretty_name {
            return Some(pretty.clone());
        }
        let name = self.name.clone()?;
        match self.version.as_ref().or(self.version_id.as_ref()) {
            Some(version) => Some(format!("{name} {version}")),
            None => Some(name),
        }
    }
}

/// Interpreta um `os-release`.
///
/// Nunca falha: um arquivo ausente ou estranho apenas devolve campos vazios, e
/// a sonda decide se isso merece um aviso — a distribuição é informação de
/// diagnóstico, não uma dependência de decisão.
pub fn parse_os_release(text: &str) -> OsRelease {
    let mut release = OsRelease::default();

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = split_key_value(line, '=') else { continue };
        let value = unquote(value);
        match key {
            "ID" => release.id = Some(value),
            "NAME" => release.name = Some(value),
            "VERSION" => release.version = Some(value),
            "VERSION_ID" => release.version_id = Some(value),
            "PRETTY_NAME" => release.pretty_name = Some(value),
            _ => {}
        }
    }

    release
}

/// Remove aspas e as sequências de escape que a especificação do formato
/// permite dentro delas.
fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    let inner = trimmed
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| trimmed.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(trimmed);

    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some(escaped) => out.push(escaped),
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
NAME="Pop!_OS"
VERSION="24.04 LTS"
ID=pop
ID_LIKE="ubuntu debian"
PRETTY_NAME="Pop!_OS 24.04 LTS"
VERSION_ID="24.04"
HOME_URL="https://pop.system76.com"
"#;

    #[test]
    fn le_campos_com_aspas() {
        let release = parse_os_release(SAMPLE);
        assert_eq!(release.name.as_deref(), Some("Pop!_OS"));
        assert_eq!(release.id.as_deref(), Some("pop"));
        assert_eq!(release.version_id.as_deref(), Some("24.04"));
        assert_eq!(release.distribution().as_deref(), Some("Pop!_OS 24.04 LTS"));
    }

    #[test]
    fn sem_pretty_name_monta_a_partir_de_name_e_version() {
        let release = parse_os_release("NAME=\"Alpine Linux\"\nVERSION_ID=3.20.0\n");
        assert_eq!(release.distribution().as_deref(), Some("Alpine Linux 3.20.0"));
    }

    #[test]
    fn arquivo_vazio_nao_falha() {
        let release = parse_os_release("");
        assert_eq!(release, OsRelease::default());
        assert_eq!(release.distribution(), None);
    }

    #[test]
    fn comentarios_e_escapes() {
        let release = parse_os_release(
            "# comentário\nPRETTY_NAME=\"Debian GNU/Linux 12 (\\\"bookworm\\\")\"\n",
        );
        assert_eq!(release.pretty_name.as_deref(), Some(r#"Debian GNU/Linux 12 ("bookworm")"#));
    }
}
