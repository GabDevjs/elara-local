/**
 * Sessão com o daemon: conectividade, status e amostras ao vivo.
 *
 * Um único assinante do stream de eventos para a janela inteira. Se cada tela
 * assinasse por conta própria, trocar de tela zeraria o histórico do gráfico e
 * o painel perderia justamente o que ele existe para mostrar.
 */
import type { DaemonStatus, EventEnvelope, ResourceSnapshot } from '@elara/schemas';
import type { UnlistenFn } from '@tauri-apps/api/event';
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from 'react';

import {
  asFailure,
  fetchResources,
  fetchSocketInfo,
  fetchStatus,
  isOfflineFailure,
  type BridgeFailure,
  type SocketInfo,
} from './bridge.js';
import { subscribeToConnection, subscribeToEvents } from './events.js';
import { LOADING, type Loadable } from './loadable.js';

/**
 * Quantas amostras o gráfico guarda.
 *
 * Com o intervalo padrão do daemon isto cobre poucos minutos — o suficiente
 * para ver uma inferência começar e terminar, que é o que o painel serve.
 */
const HISTORY_LENGTH = 90;

/** Intervalo de repescagem quando o daemon está no ar. */
const POLL_ONLINE_MS = 15_000;

/** Intervalo de repescagem quando ele não está. */
const POLL_OFFLINE_MS = 4000;

/** Conectividade com o daemon. */
export type ConnectionState = 'probing' | 'online' | 'offline';

/** Tudo que as telas precisam saber sobre o daemon. */
export interface DaemonSession {
  /** Estado da conexão; `probing` só até a primeira resposta. */
  readonly connection: ConnectionState;
  /** Caminho do socket, disponível mesmo com o daemon fora do ar. */
  readonly socket: Loadable<SocketInfo>;
  /** Status completo do daemon e de seus componentes. */
  readonly status: Loadable<DaemonStatus>;
  /** Última amostra recebida, do stream ou do `GET /v1/resources`. */
  readonly snapshot: ResourceSnapshot | null;
  /** Período de amostragem anunciado pelo daemon, em milissegundos. */
  readonly sampleIntervalMs: number | null;
  /** Percentuais de CPU agregada, do mais antigo ao mais recente. */
  readonly cpuHistory: readonly number[];
  /**
   * Incrementa a cada `hardware.detected`.
   *
   * A tela de hardware observa este número para recarregar sozinha quando o
   * daemon re-detecta — inclusive quando quem pediu foi outra janela.
   */
  readonly hardwareEpoch: number;
  /** Divergência de contrato detectada em um evento, quando houve. */
  readonly contractIssue: BridgeFailure | null;
  /** Refaz `GET /v1/status` e `GET /v1/resources`. */
  readonly reload: () => void;
}

const DaemonContext = createContext<DaemonSession | null>(null);

/** Provedor da sessão. Deve envolver a aplicação inteira. */
export function DaemonProvider({ children }: { children: ReactNode }): ReactNode {
  const [connection, setConnection] = useState<ConnectionState>('probing');
  const [socket, setSocket] = useState<Loadable<SocketInfo>>(LOADING);
  const [status, setStatus] = useState<Loadable<DaemonStatus>>(LOADING);
  const [snapshot, setSnapshot] = useState<ResourceSnapshot | null>(null);
  const [sampleIntervalMs, setSampleIntervalMs] = useState<number | null>(null);
  const [cpuHistory, setCpuHistory] = useState<readonly number[]>([]);
  const [hardwareEpoch, setHardwareEpoch] = useState(0);
  const [contractIssue, setContractIssue] = useState<BridgeFailure | null>(null);

  const applySnapshot = useCallback((next: ResourceSnapshot) => {
    setSnapshot(next);
    setCpuHistory((previous) => {
      const merged = [...previous, next.cpu.totalPercent];
      return merged.length > HISTORY_LENGTH ? merged.slice(merged.length - HISTORY_LENGTH) : merged;
    });
  }, []);

  const refresh = useCallback(async () => {
    try {
      const [nextStatus, nextResources] = await Promise.all([fetchStatus(), fetchResources()]);
      setStatus({ state: 'ready', data: nextStatus });
      setSampleIntervalMs(Number(nextResources.sampleIntervalMs));
      if (nextResources.snapshot !== null) applySnapshot(nextResources.snapshot);
      setConnection('online');
    } catch (error) {
      const failure = asFailure(error);
      setStatus({ state: 'error', failure });
      // Um erro de aplicação não é uma queda: o daemon respondeu, e a tela de
      // offline mandaria o usuário reiniciar algo que está no ar.
      setConnection(isOfflineFailure(failure) ? 'offline' : 'online');
    }
  }, [applySnapshot]);

  const reload = useCallback(() => {
    void refresh();
  }, [refresh]);

  // Caminho do socket: uma leitura de ambiente, sem I/O, e por isso a única
  // informação que a tela de daemon offline pode garantir.
  useEffect(() => {
    let alive = true;
    void fetchSocketInfo().then(
      (info) => {
        if (alive) setSocket({ state: 'ready', data: info });
      },
      (error: unknown) => {
        if (alive) setSocket({ state: 'error', failure: asFailure(error) });
      },
    );
    return () => {
      alive = false;
    };
  }, []);

  const handleEnvelope = useCallback(
    (envelope: EventEnvelope) => {
      const { payload } = envelope;
      switch (payload.type) {
        case 'resources.sampled':
          applySnapshot(payload.snapshot);
          break;
        case 'hardware.detected':
          setHardwareEpoch((previous) => previous + 1);
          break;
        case 'daemon.started':
        case 'daemon.stopping':
        case 'hardware.probe-failed':
        case 'component.state-changed':
          void refresh();
          break;
      }
    },
    [applySnapshot, refresh],
  );

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    void subscribeToEvents({ onEnvelope: handleEnvelope, onInvalid: setContractIssue }).then(
      (fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      },
      () => {
        // Sem stream de eventos a tela ainda funciona: a repescagem periódica
        // abaixo cobre o caso, só com menos resolução temporal.
      },
    );

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [handleEnvelope]);

  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    let cancelled = false;

    void subscribeToConnection((connected) => {
      setConnection(connected ? 'online' : 'offline');
      if (connected) void refresh();
    }).then(
      (fn) => {
        if (cancelled) fn();
        else unlisten = fn;
      },
      () => {
        // Idem: a repescagem detecta a volta do daemon sozinha.
      },
    );

    return () => {
      cancelled = true;
      unlisten?.();
    };
  }, [refresh]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const period = connection === 'online' ? POLL_ONLINE_MS : POLL_OFFLINE_MS;
    const timer = window.setInterval(() => {
      void refresh();
    }, period);
    return () => {
      window.clearInterval(timer);
    };
  }, [connection, refresh]);

  const session = useMemo<DaemonSession>(
    () => ({
      connection,
      socket,
      status,
      snapshot,
      sampleIntervalMs,
      cpuHistory,
      hardwareEpoch,
      contractIssue,
      reload,
    }),
    [
      connection,
      socket,
      status,
      snapshot,
      sampleIntervalMs,
      cpuHistory,
      hardwareEpoch,
      contractIssue,
      reload,
    ],
  );

  return <DaemonContext.Provider value={session}>{children}</DaemonContext.Provider>;
}

/** Acessa a sessão. Falha alto fora do [`DaemonProvider`]. */
export function useDaemon(): DaemonSession {
  const session = useContext(DaemonContext);
  if (session === null) {
    throw new Error('useDaemon precisa estar dentro de <DaemonProvider>');
  }
  return session;
}
