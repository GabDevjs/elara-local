/**
 * Sessão do runtime junto ao daemon: registrar, bater o coração, encerrar.
 *
 * É tudo o que o agent-runtime faz na Fase 1 — e é o que precisa funcionar
 * bem, porque é a base sobre a qual o supervisor do daemon vai decidir se o
 * processo está vivo. Os casos que importam não são o caminho feliz:
 *
 * - o socket ainda não existe quando o runtime sobe (o daemon é quem o cria);
 * - o daemon morre no meio e volta com outra sessão;
 * - o daemon esquece a sessão (`acknowledged: false`) e o registro precisa
 *   ser refeito sem reiniciar o processo;
 * - o daemon pede desligamento (`shutdownRequested: true`).
 */
import type {
  RuntimeHeartbeatRequest,
  RuntimeHeartbeatResponse,
  RuntimeRegisterRequest,
  RuntimeRegisterResponse,
} from '@elara/schemas';
import {
  computeBackoffDelay,
  DaemonResponseError,
  delay,
  ProtocolMismatchError,
  type BackoffOptions,
} from '@elara/sdk';
import { isElaraError, type Logger } from '@elara/shared';

/**
 * A parte do daemon de que a sessão depende.
 *
 * Estreito de propósito: o teste consegue simular um daemon amnésico ou que
 * pede desligamento sem subir um servidor, e o `AgentdClient` satisfaz esta
 * forma sem adaptador nenhum.
 */
export interface RuntimeControlPlane {
  registerRuntime(body: RuntimeRegisterRequest): Promise<RuntimeRegisterResponse>;
  heartbeat(body: RuntimeHeartbeatRequest): Promise<RuntimeHeartbeatResponse>;
}

/** Por que a sessão terminou. */
export type SessionOutcome =
  /** O processo recebeu SIGTERM/SIGINT. */
  | 'aborted'
  /** O daemon pediu que o runtime encerrasse. */
  | 'shutdown-requested';

/** Configuração da sessão. */
export interface RuntimeSessionOptions {
  readonly client: RuntimeControlPlane;
  /** Nome com que o runtime se identifica, ex.: `agent-runtime`. */
  readonly name: string;
  readonly version: string;
  readonly pid: number;
  /** Capacidades anunciadas. Vazio na Fase 1 — não há agentes ainda. */
  readonly capabilities?: readonly string[];
  readonly logger: Logger;
  /** Encerra a sessão quando abortado. */
  readonly signal: AbortSignal;
  /** Curva de espera entre tentativas de registro. */
  readonly backoff?: BackoffOptions;
  /**
   * Fração do intervalo devolvido pelo daemon usada entre heartbeats.
   *
   * Padrão 0,5. O `heartbeatIntervalMs` do contrato é o tempo **máximo** que o
   * daemon tolera sem notícias; bater exatamente nele faria qualquer atraso de
   * agendamento virar um falso óbito.
   */
  readonly intervalRatio?: number;
  /** Espera injetável, para o teste não gastar tempo real. */
  readonly sleep?: (ms: number, signal?: AbortSignal) => Promise<void>;
}

/** Piso do intervalo entre heartbeats, para não martelar o daemon. */
const MIN_INTERVAL_MS = 500;

/** Teto do intervalo, para um valor absurdo do daemon não silenciar o runtime. */
const MAX_INTERVAL_MS = 300_000;

/**
 * Mantém a sessão viva até o sinal abortar ou o daemon pedir parada.
 *
 * Só lança em situação de que repetir não adianta: contrato divergente ou
 * recusa definitiva do daemon (4xx). Qualquer outra falha vira nova tentativa
 * de registro com backoff.
 */
export async function runSession(options: RuntimeSessionOptions): Promise<SessionOutcome> {
  const { client, logger, signal } = options;
  const sleep = options.sleep ?? delay;
  const aborted = (): boolean => signal.aborted;

  let attempt = 0;
  let session: RuntimeRegisterResponse | null = null;

  try {
    while (!aborted()) {
      // --- registro ------------------------------------------------------
      try {
        session = await client.registerRuntime({
          name: options.name,
          version: options.version,
          pid: options.pid,
          protocolVersion: PROTOCOL,
          capabilities: [...(options.capabilities ?? [])],
        });
      } catch (error) {
        if (aborted()) return 'aborted';
        rethrowIfFatal(error);

        const wait = computeBackoffDelay(attempt, options.backoff);
        attempt += 1;
        logger.warn('daemon indisponível; nova tentativa de registro', {
          error,
          delayMs: wait,
          attempt,
        });
        await sleep(wait, signal);
        continue;
      }

      attempt = 0;
      const intervalMs = heartbeatIntervalOf(session, options.intervalRatio);
      logger.info('registrado no daemon', {
        sessionId: session.sessionId,
        daemonVersion: session.daemonVersion,
        intervalMs,
      });

      // --- heartbeats ----------------------------------------------------
      const outcome = await beatUntilLost(client, session, intervalMs, logger, signal, sleep);
      if (outcome === 'shutdown-requested') return 'shutdown-requested';
      if (outcome === 'aborted') return 'aborted';
      // `lost`: o daemon sumiu ou esqueceu a sessão — o laço volta ao registro.
      session = null;
    }

    return 'aborted';
  } finally {
    if (session !== null) await announceShutdown(client, session, logger);
  }
}

/** Versão do protocolo falada pelo runtime. */
const PROTOCOL = 1;

/** Resultado do laço de heartbeats. */
type BeatOutcome = 'aborted' | 'shutdown-requested' | 'lost';

/** Bate o coração até perder a sessão, ser abortado ou receber ordem de parada. */
async function beatUntilLost(
  client: RuntimeControlPlane,
  session: RuntimeRegisterResponse,
  intervalMs: number,
  logger: Logger,
  signal: AbortSignal,
  sleep: (ms: number, signal?: AbortSignal) => Promise<void>,
): Promise<BeatOutcome> {
  // Ver a nota em `runSession`: `signal.aborted` muda por fora do fluxo, e o
  // compilador estreitaria o tipo a partir da condição do `while`.
  const aborted = (): boolean => signal.aborted;

  while (!aborted()) {
    await sleep(intervalMs, signal);
    if (aborted()) return 'aborted';

    let response: RuntimeHeartbeatResponse;
    try {
      response = await client.heartbeat({
        sessionId: session.sessionId,
        healthy: true,
        detail: null,
      });
    } catch (error) {
      if (aborted()) return 'aborted';
      rethrowIfFatal(error);
      logger.warn('heartbeat falhou; vou me registrar de novo', {
        error,
        sessionId: session.sessionId,
      });
      return 'lost';
    }

    if (response.shutdownRequested) {
      logger.info('daemon pediu desligamento', { sessionId: session.sessionId });
      return 'shutdown-requested';
    }

    if (!response.acknowledged) {
      // O daemon reiniciou ou expirou a sessão: insistir com o mesmo id só
      // produziria heartbeats recusados para sempre.
      logger.warn('daemon não reconhece a sessão; registrando de novo', {
        sessionId: session.sessionId,
      });
      return 'lost';
    }

    logger.trace('heartbeat confirmado', { sessionId: session.sessionId });
  }

  return 'aborted';
}

/**
 * Avisa o daemon de que este runtime está saindo.
 *
 * A Fase 1 não tem rota de desregistro; o mais próximo disso é um heartbeat
 * declarando-se não saudável, que faz o supervisor marcar o componente como
 * caído em vez de esperar o intervalo expirar. Falhar aqui é irrelevante — o
 * processo está morrendo de qualquer forma —, então o erro só vira log.
 */
async function announceShutdown(
  client: RuntimeControlPlane,
  session: RuntimeRegisterResponse,
  logger: Logger,
): Promise<void> {
  try {
    await client.heartbeat({
      sessionId: session.sessionId,
      healthy: false,
      detail: 'runtime encerrando',
    });
    logger.info('desligamento anunciado ao daemon', { sessionId: session.sessionId });
  } catch (error) {
    logger.debug('não deu para anunciar o desligamento', { error });
  }
}

/**
 * Converte o intervalo do daemon no intervalo realmente usado.
 *
 * O `bigint` do contrato vira `number` aqui porque `setTimeout` não aceita
 * `bigint`; a conversão é segura depois do teto de [`MAX_INTERVAL_MS`].
 */
function heartbeatIntervalOf(session: RuntimeRegisterResponse, ratio = 0.5): number {
  const announced = session.heartbeatIntervalMs;
  const capped = announced > BigInt(MAX_INTERVAL_MS) ? MAX_INTERVAL_MS : Number(announced);
  const safeRatio = Math.min(1, Math.max(0.1, ratio));

  return Math.max(MIN_INTERVAL_MS, Math.floor(capped * safeRatio));
}

/**
 * Relança o que não melhora com repetição.
 *
 * Contrato divergente e recusa 4xx do daemon são erros de configuração ou de
 * versão: repetir só transformaria a falha num loop silencioso.
 */
function rethrowIfFatal(error: unknown): void {
  if (error instanceof ProtocolMismatchError) throw error;
  if (error instanceof DaemonResponseError && error.status >= 400 && error.status < 500) {
    throw error;
  }
  // Erro que não é do Elara não foi previsto por este código; deixar passar
  // esconderia um bug nosso atrás de uma tentativa de reconexão.
  if (!isElaraError(error)) throw error;
}
