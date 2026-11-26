//! Conversão de contadores brutos em percentuais e a task que faz isso em loop.
//!
//! Percentual de CPU não existe em um instante: `/proc/stat` só entrega tempo
//! acumulado. Todo número aqui nasce da diferença entre duas
//! [`LiveSample`](elara_hardware::LiveSample) consecutivas, e é por isso que o
//! primeiro tick do sampler não publica nada — não há intervalo anterior com que
//! comparar.
//!
//! As três armadilhas que os cálculos tratam explicitamente, porque nenhuma é
//! hipotética em uma máquina real:
//!
//! 1. intervalo sem progresso (Δtotal == 0) devolve `0.0`, não `NaN`;
//! 2. contadores que retrocedem (suspend/resume, migração de CPU) devolvem
//!    `0.0` em vez de um número gigante vindo de um `wrapping_sub`;
//! 3. o resultado é sempre grampeado em `0..=100`.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use elara_hardware::{CpuTimes, GpuSample, HardwareProvider, LiveSample, MemorySample};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;

use crate::bus::EventBus;
use crate::types::{CpuUsage, Event, GpuUsage, MemoryUsage, ResourceSnapshot};

/// Calcula o uso de recursos entre duas amostras consecutivas.
///
/// `gpu_names` vem do perfil de hardware (detectado uma vez) porque
/// [`GpuSample`] carrega só o índice: repetir a detecção a cada segundo custaria
/// mais que a amostragem inteira. A posição no slice é o `index` da GPU; um
/// índice sem nome correspondente recebe um rótulo genérico em vez de sumir.
///
/// `previous` precisa ser cronologicamente anterior a `current`; se não for, o
/// intervalo vira `0` e os percentuais derivados de contadores vão a `0.0`.
pub fn compute_snapshot(
    previous: &LiveSample,
    current: &LiveSample,
    gpu_names: &[String],
) -> ResourceSnapshot {
    let elapsed = current.taken_at.as_millisecond() - previous.taken_at.as_millisecond();
    let interval_ms = u64::try_from(elapsed).unwrap_or(0);

    let per_core_percent = previous
        .cpu_per_core
        .iter()
        .zip(current.cpu_per_core.iter())
        .map(|(before, after)| busy_percent(before, after))
        .collect();

    ResourceSnapshot {
        taken_at: current.taken_at,
        interval_ms,
        cpu: CpuUsage {
            total_percent: busy_percent(&previous.cpu_total, &current.cpu_total),
            per_core_percent,
            load_average: current.load_average,
        },
        memory: memory_usage(&current.memory),
        gpus: current.gpus.iter().map(|gpu| gpu_usage(gpu, gpu_names)).collect(),
    }
}

/// Fração de tempo em que a CPU esteve fazendo trabalho útil, em 0..=100.
///
/// Só há um jeito honesto de errar aqui: dividir por um intervalo que não
/// existe. Todos os caminhos degenerados convergem para `0.0`.
fn busy_percent(before: &CpuTimes, after: &CpuTimes) -> f32 {
    let (total_before, total_after) = (before.total(), after.total());
    let (idle_before, idle_after) = (before.idle_total(), after.idle_total());

    // Contador que anda para trás significa que a série foi reiniciada
    // (suspend/resume, contêiner migrado). Não há taxa a calcular.
    if total_after < total_before || idle_after < idle_before {
        return 0.0;
    }

    let total_delta = total_after - total_before;
    if total_delta == 0 {
        return 0.0;
    }

    let idle_delta = (idle_after - idle_before).min(total_delta);
    to_percent(ratio(total_delta - idle_delta, total_delta) * 100.0)
}

/// Uso de memória do instante: aqui não há taxa, só a leitura corrente.
fn memory_usage(sample: &MemorySample) -> MemoryUsage {
    let used_mb = sample.total_mb.saturating_sub(sample.available_mb);
    MemoryUsage {
        total_mb: sample.total_mb,
        used_mb,
        available_mb: sample.available_mb,
        cached_mb: sample.cached_mb,
        used_percent: percent_of(used_mb, sample.total_mb),
        swap_used_mb: sample.swap_total_mb.saturating_sub(sample.swap_free_mb),
        swap_total_mb: sample.swap_total_mb,
    }
}

/// Uso de uma GPU. Todo campo continua `Option`: sem NVML a sonda não tem o que
/// reportar, e um zero seria mentira.
fn gpu_usage(sample: &GpuSample, gpu_names: &[String]) -> GpuUsage {
    let name = gpu_names
        .get(sample.index as usize)
        .cloned()
        // Rótulo de fallback quando o perfil de hardware não pôde ser lido. É um
        // identificador, não uma medição: nenhum número é inventado por isso.
        .unwrap_or_else(|| format!("GPU {}", sample.index));

    let vram_used_percent = match (sample.vram_used_mb, sample.vram_total_mb) {
        (Some(used), Some(total)) if total > 0 => Some(percent_of(used, total)),
        _ => None,
    };

    GpuUsage {
        index: sample.index,
        name,
        utilization_percent: sample.utilization_percent.map(to_percent),
        vram_used_mb: sample.vram_used_mb,
        vram_total_mb: sample.vram_total_mb,
        vram_used_percent,
        temperature_c: sample.temperature_c,
        power_watts: sample.power_watts,
    }
}

/// Percentual de `part` sobre `whole`, com `whole == 0` valendo `0.0`.
fn percent_of(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        return 0.0;
    }
    to_percent(ratio(part, whole) * 100.0)
}

/// Divisão em ponto flutuante de contadores.
///
/// A perda de precisão só apareceria acima de 2^53 jiffies (≈ 2,8 milhões de
/// anos de CPU), então converter para `f64` é seguro.
#[allow(clippy::cast_precision_loss)]
fn ratio(numerator: u64, denominator: u64) -> f64 {
    numerator as f64 / denominator as f64
}

/// Grampeia em 0..=100 e reduz para `f32`, que é o tipo do contrato.
///
/// O truncamento é intencional: percentuais viajam para a UI, onde `f32` já tem
/// muito mais precisão do que qualquer pessoa consegue ler.
#[allow(clippy::cast_possible_truncation)]
fn to_percent<T: Into<f64>>(value: T) -> f32 {
    let value: f64 = value.into();
    if value.is_nan() { 0.0 } else { value.clamp(0.0, 100.0) as f32 }
}

// ---------------------------------------------------------------------------
// Task periódica
// ---------------------------------------------------------------------------

/// Amostrador periódico de recursos.
#[derive(Debug)]
pub struct ResourceSampler {
    provider: Arc<dyn HardwareProvider>,
    interval: Duration,
}

impl ResourceSampler {
    /// Cria o amostrador. `interval` é o período entre leituras e, portanto, o
    /// intervalo coberto por cada [`ResourceSnapshot`].
    pub fn new(provider: Arc<dyn HardwareProvider>, interval: Duration) -> Self {
        Self { provider, interval }
    }

    /// Põe o amostrador para rodar em uma task do `tokio`.
    ///
    /// Precisa ser chamado de dentro de um runtime. O primeiro tick só coleta a
    /// linha de base; o primeiro [`Event::ResourcesSampled`] sai no segundo.
    pub fn spawn(self, bus: Arc<EventBus>) -> SamplerHandle {
        let latest = Arc::new(Mutex::new(None));
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let interval = self.interval;
        let task = tokio::spawn(run(self, bus, Arc::clone(&latest), shutdown_rx));
        SamplerHandle {
            latest,
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
            task: Mutex::new(Some(task)),
            interval,
        }
    }
}

/// Controle sobre um [`ResourceSampler`] em execução.
#[derive(Debug)]
pub struct SamplerHandle {
    latest: Arc<Mutex<Option<ResourceSnapshot>>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
    task: Mutex<Option<JoinHandle<()>>>,
    interval: Duration,
}

impl SamplerHandle {
    /// Último snapshot calculado, ou `None` antes do segundo tick.
    ///
    /// É esta função que responde `GET /v1/resources`: ler o valor já calculado
    /// custa um lock, enquanto amostrar sob demanda custaria dois acessos a
    /// `/proc` mais o intervalo entre eles.
    pub fn latest(&self) -> Option<ResourceSnapshot> {
        lock(&self.latest).clone()
    }

    /// Período configurado entre amostras, para o daemon reportar ao cliente.
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// Encerra a task e espera ela terminar.
    ///
    /// Idempotente: chamar duas vezes (ou depois de a task já ter morrido) é um
    /// no-op, porque desligamento é justamente o momento em que ninguém quer
    /// mais um erro para tratar.
    pub async fn shutdown(&self) {
        if let Some(sender) = lock(&self.shutdown_tx).take() {
            let _ = sender.send(());
        }
        let task = lock(&self.task).take();
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

/// Laço principal do amostrador.
async fn run(
    sampler: ResourceSampler,
    bus: Arc<EventBus>,
    latest: Arc<Mutex<Option<ResourceSnapshot>>>,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    // Nomes das GPUs vêm de uma detecção única: `sample()` só devolve índices.
    // Se a detecção falhar, seguimos amostrando com rótulos genéricos — perder o
    // nome comercial é bem menos grave que perder as métricas.
    //
    // `detect()` é síncrono e custa dezenas de milissegundos; roda uma única vez,
    // no primeiro instante da task, quando o daemon ainda está subindo.
    let gpu_names = match sampler.provider.detect() {
        Ok(profile) => profile.gpus.into_iter().map(|gpu| gpu.model).collect::<Vec<_>>(),
        Err(error) => {
            tracing::warn!(%error, "perfil de hardware indisponível; GPUs sem nome comercial");
            Vec::new()
        }
    };

    let mut ticker = tokio::time::interval(sampler.interval);
    // Depois de uma pausa longa (máquina suspensa) não adianta disparar os ticks
    // atrasados em rajada: os contadores dessa janela já se perderam.
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut previous: Option<LiveSample> = None;

    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = ticker.tick() => {
                match sampler.provider.sample() {
                    Ok(current) => {
                        if let Some(previous) = previous.as_ref() {
                            let snapshot = compute_snapshot(previous, &current, &gpu_names);
                            *lock(&latest) = Some(snapshot.clone());
                            bus.publish(Event::ResourcesSampled { snapshot });
                        }
                        previous = Some(current);
                    }
                    Err(error) => {
                        // Uma leitura perdida não justifica matar a telemetria:
                        // o próximo tick tenta de novo com a mesma referência.
                        tracing::warn!(%error, "falha ao amostrar recursos; tentando no próximo tick");
                    }
                }
            }
        }
    }

    tracing::debug!("amostrador de recursos encerrado");
}

/// Lock que ignora envenenamento: o estado protegido é um cache substituível.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::float_cmp
)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use elara_hardware::{
        BandwidthSource, CgroupLimits, CgroupVersion, CpuCache, CpuFeatures, CpuProfile, CpuVendor,
        EffectiveResources, GpuProbeSource, GpuProfile, GpuVendor, HARDWARE_SCHEMA_VERSION,
        HardwareError, HardwareProfile, HardwareTopology, HostProfile, RamProfile,
    };

    use super::*;

    fn times(user: u64, idle: u64) -> CpuTimes {
        CpuTimes { user, idle, ..CpuTimes::default() }
    }

    fn sample_at(millis: i64, total: CpuTimes) -> LiveSample {
        LiveSample {
            taken_at: jiff::Timestamp::from_millisecond(millis).expect("timestamp válido"),
            cpu_total: total,
            cpu_per_core: vec![total],
            load_average: [0.5, 0.4, 0.3],
            memory: MemorySample {
                total_mb: 1000,
                available_mb: 250,
                free_mb: 100,
                cached_mb: 150,
                swap_total_mb: 2000,
                swap_free_mb: 1500,
            },
            gpus: Vec::new(),
        }
    }

    // -- regras de cálculo --------------------------------------------------

    #[test]
    fn percentual_de_cpu_e_um_menos_delta_idle_sobre_delta_total() {
        // 100 jiffies de trabalho e 100 de ocioso no intervalo → 50%.
        let before = sample_at(0, times(100, 100));
        let after = sample_at(1000, times(200, 200));

        let snapshot = compute_snapshot(&before, &after, &[]);
        assert!((snapshot.cpu.total_percent - 50.0).abs() < f32::EPSILON);
        assert_eq!(snapshot.cpu.per_core_percent, vec![50.0]);
        assert_eq!(snapshot.interval_ms, 1000);
    }

    #[test]
    fn cpu_totalmente_ocupada_satura_em_cem() {
        let before = sample_at(0, times(100, 500));
        let after = sample_at(1000, times(400, 500));

        let snapshot = compute_snapshot(&before, &after, &[]);
        assert!((snapshot.cpu.total_percent - 100.0).abs() < f32::EPSILON);
    }

    #[test]
    fn delta_total_zero_devolve_zero_e_nao_nan() {
        let counters = times(100, 100);
        let before = sample_at(0, counters);
        let after = sample_at(1000, counters);

        let snapshot = compute_snapshot(&before, &after, &[]);
        assert_eq!(snapshot.cpu.total_percent, 0.0);
        assert!(!snapshot.cpu.total_percent.is_nan(), "0/0 não pode virar NaN");
        assert_eq!(snapshot.cpu.per_core_percent, vec![0.0]);
    }

    #[test]
    fn contadores_que_retrocedem_devolvem_zero() {
        // Cenário de suspend/resume: a segunda leitura é menor que a primeira.
        let before = sample_at(0, times(10_000, 90_000));
        let after = sample_at(1000, times(10, 90));

        let snapshot = compute_snapshot(&before, &after, &[]);
        assert_eq!(snapshot.cpu.total_percent, 0.0);
        assert_eq!(snapshot.cpu.per_core_percent, vec![0.0]);
    }

    #[test]
    fn idle_que_retrocede_sozinho_tambem_devolve_zero() {
        let before = sample_at(0, times(100, 900));
        let after = sample_at(1000, times(1000, 800));

        let snapshot = compute_snapshot(&before, &after, &[]);
        assert_eq!(snapshot.cpu.total_percent, 0.0);
    }

    #[test]
    fn relogio_que_anda_para_tras_zera_o_intervalo() {
        let before = sample_at(5000, times(100, 100));
        let after = sample_at(1000, times(200, 200));

        let snapshot = compute_snapshot(&before, &after, &[]);
        assert_eq!(snapshot.interval_ms, 0, "intervalo negativo não é representável");
    }

    #[test]
    fn memoria_usa_total_menos_disponivel() {
        let before = sample_at(0, times(0, 0));
        let after = sample_at(1000, times(0, 0));

        let snapshot = compute_snapshot(&before, &after, &[]);
        assert_eq!(snapshot.memory.used_mb, 750);
        assert_eq!(snapshot.memory.available_mb, 250);
        assert!((snapshot.memory.used_percent - 75.0).abs() < f32::EPSILON);
        assert_eq!(snapshot.memory.swap_used_mb, 500);
    }

    #[test]
    fn gpu_sem_vram_conhecida_nao_ganha_percentual() {
        let mut before = sample_at(0, times(0, 0));
        let mut after = sample_at(1000, times(0, 0));
        let with_vram = GpuSample {
            index: 0,
            utilization_percent: Some(42.0),
            memory_utilization_percent: None,
            vram_used_mb: Some(2048),
            vram_total_mb: Some(8192),
            temperature_c: Some(55.0),
            power_watts: None,
            clock_mhz: None,
        };
        let without_vram = GpuSample {
            index: 1,
            utilization_percent: None,
            memory_utilization_percent: None,
            vram_used_mb: None,
            vram_total_mb: None,
            temperature_c: None,
            power_watts: None,
            clock_mhz: None,
        };
        before.gpus = vec![with_vram.clone(), without_vram.clone()];
        after.gpus = vec![with_vram, without_vram];

        let names = vec!["RTX 5060 Ti".to_owned()];
        let snapshot = compute_snapshot(&before, &after, &names);

        assert_eq!(snapshot.gpus[0].name, "RTX 5060 Ti");
        assert_eq!(snapshot.gpus[0].vram_used_percent, Some(25.0));
        assert_eq!(snapshot.gpus[1].name, "GPU 1", "índice sem nome vira rótulo genérico");
        assert_eq!(snapshot.gpus[1].vram_used_percent, None);
        assert_eq!(snapshot.gpus[1].utilization_percent, None);
    }

    // -- task periódica -----------------------------------------------------

    /// Provider determinístico: devolve amostras pré-definidas e permite injetar
    /// falhas. Um mock local (e não o `MockHardwareProvider` do crate de
    /// hardware) porque só aqui interessa controlar *qual* chamada falha.
    #[derive(Debug)]
    struct ScriptedProvider {
        samples: Vec<Result<LiveSample, ()>>,
        calls: AtomicUsize,
        detect_fails: bool,
    }

    impl ScriptedProvider {
        fn new(samples: Vec<Result<LiveSample, ()>>) -> Self {
            Self { samples, calls: AtomicUsize::new(0), detect_fails: false }
        }

        fn with_failing_detect(mut self) -> Self {
            self.detect_fails = true;
            self
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl HardwareProvider for ScriptedProvider {
        fn detect(&self) -> Result<HardwareProfile, HardwareError> {
            if self.detect_fails {
                return Err(HardwareError::UnsupportedPlatform("mock".to_owned()));
            }
            Ok(HardwareProfile {
                schema_version: HARDWARE_SCHEMA_VERSION,
                fingerprint: "mock".to_owned(),
                collected_at: jiff::Timestamp::UNIX_EPOCH,
                probe_duration_ms: 0,
                host: HostProfile {
                    hostname: None,
                    os: "linux".to_owned(),
                    distribution: None,
                    kernel_version: None,
                    architecture: "x86_64".to_owned(),
                    virtualization: None,
                },
                cpu: CpuProfile {
                    model: "Mock CPU".to_owned(),
                    vendor: CpuVendor::Other,
                    architecture: "x86_64".to_owned(),
                    physical_cores: 1,
                    logical_threads: 1,
                    online_threads: 1,
                    sockets: 1,
                    base_frequency_mhz: None,
                    max_frequency_mhz: None,
                    cache: CpuCache::default(),
                    features: CpuFeatures::default(),
                    hybrid: None,
                },
                ram: RamProfile {
                    total_mb: 1000,
                    available_mb: 250,
                    free_mb: 100,
                    swap_total_mb: 2000,
                    swap_free_mb: 1500,
                    measured_bandwidth_gbps: None,
                    bandwidth_source: BandwidthSource::NotMeasured,
                },
                gpus: vec![GpuProfile {
                    index: 0,
                    vendor: GpuVendor::Nvidia,
                    model: "Mock GPU".to_owned(),
                    pci_address: None,
                    vram_total_mb: None,
                    vram_available_mb: None,
                    driver_version: None,
                    runtime_version: None,
                    compute_capability: None,
                    backends: Vec::new(),
                    is_integrated: false,
                    power_limit_watts: None,
                    source: GpuProbeSource::Sysfs,
                }],
                storage: Vec::new(),
                topology: HardwareTopology {
                    numa_nodes: Vec::new(),
                    simultaneous_multithreading: false,
                    cgroup: CgroupLimits {
                        version: CgroupVersion::None,
                        cpu_quota_cores: None,
                        memory_limit_mb: None,
                        memory_usage_mb: None,
                    },
                },
                effective: EffectiveResources {
                    cpu_threads: 1,
                    ram_mb: 500,
                    vram_mb: 0,
                    vram_headroom_ratio: 0.0,
                    rationale: Vec::new(),
                },
                warnings: Vec::new(),
            })
        }

        fn sample(&self) -> Result<LiveSample, HardwareError> {
            let index = self.calls.fetch_add(1, Ordering::SeqCst);
            match self.samples.get(index) {
                Some(Ok(sample)) => Ok(sample.clone()),
                Some(Err(())) => Err(HardwareError::Parse {
                    path: "/proc/stat".to_owned(),
                    detail: "mock".to_owned(),
                }),
                // Depois do roteiro, repete a última leitura: mantém a task viva
                // sem produzir novos snapshots inesperados.
                None => match self.samples.last() {
                    Some(Ok(sample)) => Ok(sample.clone()),
                    _ => Err(HardwareError::Parse {
                        path: "/proc/stat".to_owned(),
                        detail: "roteiro esgotado".to_owned(),
                    }),
                },
            }
        }
    }

    #[tokio::test(start_paused = true)]
    async fn sampler_publica_snapshot_a_partir_do_segundo_tick() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            Ok(sample_at(0, times(100, 100))),
            Ok(sample_at(1000, times(200, 200))),
        ]));
        let bus = Arc::new(EventBus::new(16));
        let mut events = bus.subscribe();

        let handle = ResourceSampler::new(
            Arc::clone(&provider) as Arc<dyn HardwareProvider>,
            Duration::from_millis(100),
        )
        .spawn(Arc::clone(&bus));

        let envelope = events.recv().await.expect("o sampler deve publicar");
        let Event::ResourcesSampled { snapshot } = envelope.payload else {
            panic!("evento inesperado");
        };
        assert!((snapshot.cpu.total_percent - 50.0).abs() < f32::EPSILON);
        assert_eq!(snapshot.gpus, Vec::new());

        assert_eq!(handle.latest().map(|snapshot| snapshot.interval_ms), Some(1000));
        assert_eq!(handle.interval(), Duration::from_millis(100));

        handle.shutdown().await;
        // Depois do shutdown a task não amostra mais.
        let calls = provider.calls();
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(provider.calls(), calls, "a task deveria estar encerrada");

        // E chamar de novo continua sendo seguro.
        handle.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn falha_de_amostragem_nao_mata_a_task() {
        let provider = Arc::new(ScriptedProvider::new(vec![
            Err(()),
            Err(()),
            Ok(sample_at(0, times(100, 100))),
            Ok(sample_at(2000, times(300, 100))),
        ]));
        let bus = Arc::new(EventBus::new(16));
        let mut events = bus.subscribe();

        let handle = ResourceSampler::new(
            Arc::clone(&provider) as Arc<dyn HardwareProvider>,
            Duration::from_millis(50),
        )
        .spawn(Arc::clone(&bus));

        let envelope = events.recv().await.expect("a task sobrevive às falhas");
        let Event::ResourcesSampled { snapshot } = envelope.payload else {
            panic!("evento inesperado");
        };
        assert!((snapshot.cpu.total_percent - 100.0).abs() < f32::EPSILON);
        assert!(provider.calls() >= 4, "as falhas foram tentadas de novo");

        handle.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn detect_que_falha_nao_impede_a_amostragem() {
        let provider = Arc::new(
            ScriptedProvider::new(vec![
                Ok(sample_at(0, times(100, 100))),
                Ok(sample_at(1000, times(200, 200))),
            ])
            .with_failing_detect(),
        );
        let bus = Arc::new(EventBus::new(16));
        let mut events = bus.subscribe();

        let handle =
            ResourceSampler::new(provider as Arc<dyn HardwareProvider>, Duration::from_millis(50))
                .spawn(Arc::clone(&bus));

        let envelope = events.recv().await.expect("amostragem continua sem o perfil");
        assert!(matches!(envelope.payload, Event::ResourcesSampled { .. }));

        handle.shutdown().await;
    }
}
