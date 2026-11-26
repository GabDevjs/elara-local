import { afterEach, describe, expect, it } from 'vitest';

import { AgentdClient, ProtocolMismatchError } from '@elara/sdk';
import { createLogger, type Logger } from '@elara/shared';

import { runSession } from '../src/core/session.js';
import { createFakeAgentd, type FakeAgentd } from './fake-agentd.js';

const cleanups: (() => Promise<void>)[] = [];

afterEach(async () => {
  for (const cleanup of cleanups.splice(0)) await cleanup();
});

/** Logger silencioso: o teste afirma comportamento, não linhas de log. */
function silentLogger(): Logger {
  return createLogger('agent-runtime', { env: { ELARA_LOG: 'off' } });
}

/**
 * Espera injetável.
 *
 * Registra o intervalo pedido — é o que prova que há backoff — mas dorme no
 * máximo 5 ms, para o teste não gastar o tempo real da curva exponencial.
 */
function recordingSleep(): { delays: number[]; sleep: (ms: number) => Promise<void> } {
  const delays: number[] = [];
  return {
    delays,
    sleep: async (ms: number) => {
      delays.push(ms);
      await new Promise<void>((resolve) => setTimeout(resolve, Math.min(ms, 5)));
    },
  };
}

/** Resolve quando `condition` for verdadeira, ou falha por timeout. */
async function waitFor(condition: () => boolean, label: string, timeoutMs = 3000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`tempo esgotado esperando: ${label}`);
    await new Promise<void>((resolve) => setTimeout(resolve, 5));
  }
}

/** Sobe daemon falso e cliente, já registrados para limpeza. */
async function bootstrap(): Promise<{ daemon: FakeAgentd; client: AgentdClient }> {
  const daemon = await createFakeAgentd();
  const client = new AgentdClient({
    socketPath: daemon.socketPath,
    timeoutMs: 2000,
    logger: silentLogger(),
  });

  cleanups.push(async () => {
    await client.close();
    await daemon.dispose();
  });

  return { daemon, client };
}

describe('ciclo de vida da sessão', () => {
  it('registra, bate o coração e anuncia o desligamento no sinal', async () => {
    const { daemon, client } = await bootstrap();
    const controller = new AbortController();
    const { delays, sleep } = recordingSleep();

    daemon.heartbeatIntervalMs = 4000;
    daemon.onHeartbeat = (_body, index) => {
      if (index >= 2) controller.abort();
    };

    const outcome = await runSession({
      client,
      name: 'agent-runtime',
      version: '0.1.0',
      pid: 4242,
      capabilities: [],
      logger: silentLogger(),
      signal: controller.signal,
      sleep,
    });

    expect(outcome).toBe('aborted');
    expect(daemon.registrations).toEqual([
      {
        name: 'agent-runtime',
        version: '0.1.0',
        pid: 4242,
        protocolVersion: 1,
        capabilities: [],
      },
    ]);

    // Três heartbeats saudáveis e o último declarando o desligamento.
    expect(daemon.heartbeats.slice(0, 3).every((beat) => beat.healthy)).toBe(true);
    expect(daemon.heartbeats.at(-1)).toEqual({
      sessionId: 'sess-1',
      healthy: false,
      detail: 'runtime encerrando',
    });

    // Metade do intervalo anunciado: bater no limite transformaria qualquer
    // atraso de agendamento em falso óbito.
    expect(delays.slice(0, 3)).toEqual([2000, 2000, 2000]);
  });

  it('respeita o piso do intervalo quando o daemon pede algo curto demais', async () => {
    const { daemon, client } = await bootstrap();
    const controller = new AbortController();
    const { delays, sleep } = recordingSleep();

    daemon.heartbeatIntervalMs = 100;
    daemon.onHeartbeat = () => {
      controller.abort();
    };

    await runSession({
      client,
      name: 'agent-runtime',
      version: '0.1.0',
      pid: 1,
      logger: silentLogger(),
      signal: controller.signal,
      sleep,
    });

    expect(delays[0]).toBe(500);
  });

  it('registra-se de novo quando o daemon não reconhece a sessão', async () => {
    const { daemon, client } = await bootstrap();
    const controller = new AbortController();
    const { sleep } = recordingSleep();

    daemon.reply = { acknowledged: false, shutdownRequested: false };
    daemon.onHeartbeat = () => {
      if (daemon.registrations.length >= 3) controller.abort();
    };

    await runSession({
      client,
      name: 'agent-runtime',
      version: '0.1.0',
      pid: 1,
      logger: silentLogger(),
      signal: controller.signal,
      backoff: { initialMs: 1, jitter: 0 },
      sleep,
    });

    expect(daemon.registrations.length).toBeGreaterThanOrEqual(3);
    // Cada registro devolve um id novo, e o runtime passa a usá-lo.
    expect(daemon.heartbeats.map((beat) => beat.sessionId).slice(0, 3)).toEqual([
      'sess-1',
      'sess-2',
      'sess-3',
    ]);
  });

  it('encerra quando o daemon pede desligamento', async () => {
    const { daemon, client } = await bootstrap();
    const { sleep } = recordingSleep();

    daemon.reply = { acknowledged: true, shutdownRequested: true };

    const outcome = await runSession({
      client,
      name: 'agent-runtime',
      version: '0.1.0',
      pid: 1,
      logger: silentLogger(),
      signal: new AbortController().signal,
      sleep,
    });

    expect(outcome).toBe('shutdown-requested');
    expect(daemon.heartbeats).toHaveLength(2);
    expect(daemon.heartbeats[1]?.healthy).toBe(false);
  });
});

describe('daemon indisponível', () => {
  it('socket ausente na subida vira backoff crescente, não loop apertado', async () => {
    const controller = new AbortController();
    const { delays, sleep } = recordingSleep();

    const client = new AgentdClient({
      socketPath: '/tmp/elara-sem-daemon-3f21/agentd.sock',
      timeoutMs: 500,
      logger: silentLogger(),
    });
    cleanups.push(() => client.close());

    const session = runSession({
      client,
      name: 'agent-runtime',
      version: '0.1.0',
      pid: 1,
      logger: silentLogger(),
      signal: controller.signal,
      backoff: { initialMs: 10, factor: 2, jitter: 0, maxMs: 1000 },
      sleep,
    });

    await waitFor(() => delays.length >= 4, 'quatro tentativas de registro');
    controller.abort();
    await expect(session).resolves.toBe('aborted');

    expect(delays.slice(0, 4)).toEqual([10, 20, 40, 80]);
  });

  it('some no meio e volta: o runtime se registra de novo sozinho', async () => {
    const { daemon, client } = await bootstrap();
    const controller = new AbortController();
    const { delays, sleep } = recordingSleep();

    daemon.heartbeatIntervalMs = 1000;

    const session = runSession({
      client,
      name: 'agent-runtime',
      version: '0.1.0',
      pid: 1,
      logger: silentLogger(),
      signal: controller.signal,
      backoff: { initialMs: 10, factor: 2, jitter: 0, maxMs: 100 },
      sleep,
    });

    await waitFor(() => daemon.heartbeats.length >= 1, 'primeiro heartbeat');
    await daemon.stop();

    // Sem daemon, o registro precisa falhar e esperar — não girar.
    await waitFor(() => delays.length >= 3, 'tentativas durante a ausência');

    await daemon.start();
    await waitFor(() => daemon.registrations.length >= 2, 'novo registro após a volta');
    await waitFor(
      () => daemon.heartbeats.some((beat) => beat.sessionId === 'sess-2'),
      'heartbeat da nova sessão',
    );

    controller.abort();
    await expect(session).resolves.toBe('aborted');
  });

  it('desiste quando o daemon fala outra versão do protocolo', async () => {
    const { daemon, client } = await bootstrap();
    const { sleep } = recordingSleep();

    daemon.protocolVersion = '2';

    await expect(
      runSession({
        client,
        name: 'agent-runtime',
        version: '0.1.0',
        pid: 1,
        logger: silentLogger(),
        signal: new AbortController().signal,
        sleep,
      }),
    ).rejects.toBeInstanceOf(ProtocolMismatchError);

    expect(daemon.registrations).toHaveLength(1);
  });

  it('desiste quando o daemon recusa o registro com 4xx', async () => {
    const { daemon, client } = await bootstrap();
    const { delays, sleep } = recordingSleep();

    daemon.registerStatus = 403;

    const error = await runSession({
      client,
      name: 'agent-runtime',
      version: '0.1.0',
      pid: 1,
      logger: silentLogger(),
      signal: new AbortController().signal,
      backoff: { initialMs: 1, jitter: 0 },
      sleep,
    }).catch((cause: unknown) => cause);

    expect(error).toMatchObject({ code: 'runtime.rejected', status: 403 });
    // Uma recusa definitiva não vira tentativa: nenhum backoff foi esperado.
    expect(daemon.registrations).toHaveLength(1);
    expect(delays).toHaveLength(0);
  });
});
