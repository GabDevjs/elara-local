//! Saída CSV do `nvidia-smi`.
//!
//! É o plano B quando o NVML não carrega. O formato é estável desde que a
//! consulta seja explícita: por isso as colunas são uma constante deste módulo
//! e o parser posicional casa exatamente com ela.

use super::ParseError;

/// Colunas pedidas ao `nvidia-smi`, na ordem em que este módulo as consome.
///
/// Mudar esta constante sem mudar [`parse_nvidia_smi_csv`] desalinha todas as
/// colunas — por isso as duas moram juntas.
pub const QUERY_FIELDS: &str =
    "index,name,pci.bus_id,memory.total,memory.used,driver_version,compute_cap,power.limit";

/// Formato pedido ao `nvidia-smi`: sem cabeçalho e sem unidades, para que os
/// números não precisem ser desgrudados de um sufixo `MiB`.
pub const FORMAT_FLAG: &str = "--format=csv,noheader,nounits";

/// Uma GPU como o `nvidia-smi` a descreve.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NvidiaSmiGpu {
    /// Índice do dispositivo segundo o driver.
    pub index: u32,
    /// Nome comercial, ex.: `NVIDIA GeForce RTX 5060 Ti`.
    pub name: String,
    /// Endereço PCI como o driver o escreve, ex.: `00000000:01:00.0`.
    pub pci_bus_id: Option<String>,
    /// VRAM total, em MiB.
    pub memory_total_mb: Option<u64>,
    /// VRAM em uso, em MiB.
    pub memory_used_mb: Option<u64>,
    /// Versão do driver NVIDIA.
    pub driver_version: Option<String>,
    /// Compute capability, ex.: `8.9`.
    pub compute_capability: Option<String>,
    /// Limite de potência configurado, em watts.
    pub power_limit_watts: Option<f32>,
}

/// Interpreta a saída de `nvidia-smi --query-gpu=<QUERY_FIELDS> <FORMAT_FLAG>`.
///
/// Campos que o driver não sabe responder (`[N/A]`, `[Not Supported]`) viram
/// `None`, nunca zero: um limite de potência de 0 W seria lido como GPU
/// desligada.
pub fn parse_nvidia_smi_csv(text: &str) -> Result<Vec<NvidiaSmiGpu>, ParseError> {
    let mut gpus = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let field = |position: usize| -> Option<&str> {
            fields.get(position).copied().filter(|value| !is_unavailable(value))
        };

        let index_raw = field(0).ok_or_else(|| ParseError::missing("index"))?;
        let index = index_raw.parse().map_err(|_| ParseError::invalid("index", index_raw))?;
        let name = field(1).ok_or_else(|| ParseError::missing("name"))?.to_owned();

        gpus.push(NvidiaSmiGpu {
            index,
            name,
            pci_bus_id: field(2).map(str::to_owned),
            memory_total_mb: field(3).and_then(|v| v.parse().ok()),
            memory_used_mb: field(4).and_then(|v| v.parse().ok()),
            driver_version: field(5).map(str::to_owned),
            compute_capability: field(6).map(str::to_owned),
            power_limit_watts: field(7).and_then(|v| v.parse().ok()),
        });
    }

    if gpus.is_empty() { Err(ParseError::Empty) } else { Ok(gpus) }
}

/// Marcadores que o `nvidia-smi` usa para "não sei responder".
fn is_unavailable(value: &str) -> bool {
    value.is_empty()
        || value.starts_with('[')
        || value.eq_ignore_ascii_case("n/a")
        || value.eq_ignore_ascii_case("unknown")
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::indexing_slicing,
    reason = "em teste, um índice fora de faixa é exatamente a falha que queremos ver"
)]
mod tests {
    use super::*;

    #[test]
    fn le_linha_completa() {
        let text = "0, NVIDIA GeForce RTX 5060 Ti, 00000000:01:00.0, 8151, 373, 580.173.02, 12.0, 180.00\n";
        let gpus = parse_nvidia_smi_csv(text).unwrap();
        assert_eq!(gpus.len(), 1);
        let gpu = &gpus[0];
        assert_eq!(gpu.index, 0);
        assert_eq!(gpu.name, "NVIDIA GeForce RTX 5060 Ti");
        assert_eq!(gpu.pci_bus_id.as_deref(), Some("00000000:01:00.0"));
        assert_eq!(gpu.memory_total_mb, Some(8151));
        assert_eq!(gpu.memory_used_mb, Some(373));
        assert_eq!(gpu.driver_version.as_deref(), Some("580.173.02"));
        assert_eq!(gpu.compute_capability.as_deref(), Some("12.0"));
        assert_eq!(gpu.power_limit_watts, Some(180.0));
    }

    #[test]
    fn duas_gpus() {
        let text = "\
0, NVIDIA A100-SXM4-40GB, 00000000:07:00.0, 40960, 0, 535.104.05, 8.0, 400.00
1, NVIDIA A100-SXM4-40GB, 00000000:0F:00.0, 40960, 1024, 535.104.05, 8.0, 400.00
";
        let gpus = parse_nvidia_smi_csv(text).unwrap();
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus[1].index, 1);
        assert_eq!(gpus[1].memory_used_mb, Some(1024));
    }

    #[test]
    fn campos_indisponiveis_viram_none_nao_zero() {
        let text =
            "0, NVIDIA T400, 00000000:01:00.0, 2048, 100, 535.104.05, [N/A], [Not Supported]\n";
        let gpus = parse_nvidia_smi_csv(text).unwrap();
        assert_eq!(gpus[0].compute_capability, None);
        assert_eq!(gpus[0].power_limit_watts, None);
        assert_eq!(gpus[0].memory_total_mb, Some(2048));
    }

    #[test]
    fn saida_vazia_e_erro() {
        assert_eq!(parse_nvidia_smi_csv("").unwrap_err(), ParseError::Empty);
        assert_eq!(parse_nvidia_smi_csv("\n\n").unwrap_err(), ParseError::Empty);
    }

    #[test]
    fn indice_nao_numerico_e_erro() {
        assert!(parse_nvidia_smi_csv("x, GPU, 0000:01:00.0, 1, 1, 1, 1, 1\n").is_err());
    }

    #[test]
    fn linha_curta_ainda_produz_o_que_da() {
        let gpus = parse_nvidia_smi_csv("0, NVIDIA T400\n").unwrap();
        assert_eq!(gpus[0].name, "NVIDIA T400");
        assert_eq!(gpus[0].pci_bus_id, None);
        assert_eq!(gpus[0].memory_total_mb, None);
    }
}
