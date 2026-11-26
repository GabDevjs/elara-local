//! Distribuição interna de eventos.
//!
//! Um único produtor lógico (o daemon) e vários consumidores independentes: o
//! stream SSE da UI, o log estruturado e, mais tarde, a persistência. O canal é
//! `broadcast` justamente para que um consumidor lento nunca aplique
//! contrapressão em quem publica — perder eventos é aceitável, travar o daemon
//! não é.
//!
//! A lacuna causada por um consumidor lento é detectável: cada envelope carrega
//! um [`EventEnvelope::sequence`] monotônico, então o cliente compara o número
//! recebido com o anterior e sabe exatamente quantos eventos perdeu.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};

use tokio::sync::broadcast;
use tokio_stream::wrappers::BroadcastStream;

use crate::types::{Event, EventEnvelope};

/// Barramento de eventos do processo.
///
/// Barato de clonar por trás de um `Arc`; todos os métodos usam `&self` para
/// que o daemon possa compartilhá-lo com os handlers HTTP sem lock externo.
#[derive(Debug)]
pub struct EventBus {
    sender: broadcast::Sender<EventEnvelope>,
    sequence: AtomicU64,
    recent: Mutex<VecDeque<EventEnvelope>>,
    capacity: usize,
}

impl EventBus {
    /// Cria um bus que retém `capacity` eventos, tanto no canal quanto no
    /// histórico recente.
    ///
    /// `capacity` zero é elevado para 1: um canal `broadcast` sem capacidade não
    /// existe, e recusar a construção só transferiria o problema para o chamador.
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            sequence: AtomicU64::new(0),
            recent: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Publica um evento e devolve o número de sequência atribuído.
    ///
    /// Nunca bloqueia e nunca falha: sem assinantes, o envio é descartado em
    /// silêncio; com um assinante lento, o `broadcast` sobrescreve as posições
    /// mais antigas e avisa o assinante via `RecvError::Lagged`.
    pub fn publish(&self, event: Event) -> u64 {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst).saturating_add(1);
        let envelope =
            EventEnvelope { sequence, emitted_at: jiff::Timestamp::now(), payload: event };

        // Eventos de alta frequência (amostras de recurso) existem só para quem
        // está olhando ao vivo: guardá-los no histórico expulsaria em segundos
        // tudo que interessa a um diagnóstico posterior.
        if !envelope.payload.is_high_frequency() {
            tracing::info!(
                event = envelope.payload.name(),
                sequence = envelope.sequence,
                "evento publicado"
            );
            let mut recent = self.lock_recent();
            recent.push_front(envelope.clone());
            while recent.len() > self.capacity {
                recent.pop_back();
            }
        }

        let _ = self.sender.send(envelope);
        sequence
    }

    /// Assina o fluxo ao vivo. O assinante só recebe eventos publicados depois
    /// desta chamada — o histórico vem de [`EventBus::recent`].
    pub fn subscribe(&self) -> broadcast::Receiver<EventEnvelope> {
        self.sender.subscribe()
    }

    /// Mesma assinatura, embrulhada como `Stream` para quem serve SSE.
    ///
    /// Cada item é um `Result`: o `Err` indica quantos eventos foram perdidos,
    /// informação que o cliente precisa para decidir se recarrega o estado.
    pub fn stream(&self) -> BroadcastStream<EventEnvelope> {
        BroadcastStream::new(self.subscribe())
    }

    /// Últimos eventos retidos, do mais novo para o mais antigo.
    ///
    /// Serve a UI que acabou de conectar e precisa de contexto antes do primeiro
    /// evento ao vivo. Eventos de alta frequência não aparecem aqui.
    pub fn recent(&self, limit: usize) -> Vec<EventEnvelope> {
        self.lock_recent().iter().take(limit).cloned().collect()
    }

    /// Quantos assinantes ativos existem. Usado pelo daemon para decidir se vale
    /// manter o sampler rodando.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// Ignora envenenamento do mutex: o histórico é um cache de diagnóstico, e
    /// derrubar o bus porque uma thread entrou em pânico só piora o diagnóstico.
    fn lock_recent(&self) -> MutexGuard<'_, VecDeque<EventEnvelope>> {
        self.recent.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;
    use crate::types::{CpuUsage, MemoryUsage, ResourceSnapshot};

    fn snapshot() -> ResourceSnapshot {
        ResourceSnapshot {
            taken_at: jiff::Timestamp::UNIX_EPOCH,
            interval_ms: 1000,
            cpu: CpuUsage {
                total_percent: 12.5,
                per_core_percent: vec![12.5],
                load_average: [0.0, 0.0, 0.0],
            },
            memory: MemoryUsage {
                total_mb: 1024,
                used_mb: 512,
                available_mb: 512,
                cached_mb: 128,
                used_percent: 50.0,
                swap_used_mb: 0,
                swap_total_mb: 0,
            },
            gpus: Vec::new(),
        }
    }

    fn stopping(reason: &str) -> Event {
        Event::DaemonStopping { reason: reason.to_owned() }
    }

    #[test]
    fn sequencia_e_monotonica_e_comeca_em_um() {
        let bus = EventBus::new(8);
        assert_eq!(bus.publish(stopping("a")), 1);
        assert_eq!(bus.publish(stopping("b")), 2);
        assert_eq!(bus.publish(stopping("c")), 3);

        let recent = bus.recent(10);
        let sequences: Vec<u64> = recent.iter().map(|envelope| envelope.sequence).collect();
        assert_eq!(sequences, vec![3, 2, 1], "recent deve vir do mais novo para o mais antigo");
    }

    #[test]
    fn publicar_sem_assinante_nao_falha() {
        let bus = EventBus::new(4);
        assert_eq!(bus.subscriber_count(), 0);
        assert_eq!(bus.publish(stopping("ninguém ouvindo")), 1);
    }

    #[test]
    fn recent_ignora_eventos_de_alta_frequencia() {
        let bus = EventBus::new(8);
        let sampled = bus.publish(Event::ResourcesSampled { snapshot: snapshot() });
        let stopped = bus.publish(stopping("desligando"));

        assert_eq!(sampled, 1, "eventos de alta frequência também consomem sequência");
        assert_eq!(stopped, 2);

        let recent = bus.recent(10);
        assert_eq!(recent.len(), 1, "só o evento não-high-frequency deve ser retido");
        assert_eq!(recent[0].sequence, 2);
        assert!(!recent[0].payload.is_high_frequency());
    }

    #[test]
    fn recent_respeita_a_capacidade_e_o_limite() {
        let bus = EventBus::new(2);
        for index in 0..5 {
            bus.publish(stopping(&format!("evento {index}")));
        }

        let recent = bus.recent(10);
        assert_eq!(recent.len(), 2, "o histórico não pode crescer além da capacidade");
        assert_eq!(recent[0].sequence, 5);
        assert_eq!(recent[1].sequence, 4);

        assert_eq!(bus.recent(1).len(), 1);
        assert_eq!(bus.recent(0).len(), 0);
    }

    #[tokio::test]
    async fn assinante_lento_perde_eventos_mas_nao_derruba_o_bus() {
        let bus = EventBus::new(2);
        let mut slow = bus.subscribe();

        for index in 0..5 {
            bus.publish(stopping(&format!("evento {index}")));
        }

        let lag = match slow.recv().await {
            Err(broadcast::error::RecvError::Lagged(missed)) => missed,
            other => panic!("esperava Lagged, veio {other:?}"),
        };
        assert_eq!(lag, 3, "três eventos foram sobrescritos");

        // O assinante continua funcionando e a lacuna é visível pelo sequence.
        let recovered = slow.recv().await.expect("assinante continua vivo depois do lag");
        assert_eq!(recovered.sequence, 4);
        assert!(recovered.sequence > 1, "a lacuna é detectável pelo salto no sequence");

        // O que ainda estava no buffer continua chegando, em ordem.
        assert_eq!(slow.recv().await.expect("resto do buffer").sequence, 5);

        // E o bus continua entregando para todo mundo.
        let mut fresh = bus.subscribe();
        let sequence = bus.publish(stopping("depois do lag"));
        assert_eq!(sequence, 6);
        assert_eq!(fresh.recv().await.expect("novo assinante recebe").sequence, sequence);
        assert_eq!(slow.recv().await.expect("assinante antigo também").sequence, sequence);
    }

    #[tokio::test]
    async fn assinantes_recebem_eventos_de_alta_frequencia() {
        let bus = EventBus::new(8);
        let mut receiver = bus.subscribe();
        bus.publish(Event::ResourcesSampled { snapshot: snapshot() });

        let envelope = receiver.recv().await.expect("assinante ao vivo recebe amostras");
        assert!(envelope.payload.is_high_frequency());
        assert!(bus.recent(10).is_empty(), "mas elas não entram no histórico");
    }
}
