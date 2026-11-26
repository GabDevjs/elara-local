import { afterEach, describe, expect, it } from 'vitest';

import { AgentdClient } from '../src/client.js';
import {
  DaemonResponseError,
  DaemonTimeoutError,
  DaemonUnavailableError,
  ProtocolMismatchError,
} from '@elara/shared';

import { replyJson, startFakeDaemon, type FakeDaemon, type FakeHandler } from './fake-daemon.js';

/** Recursos abertos por cada teste, derrubados no `afterEach`. */
const openResources: (() => Promise<void>)[] = [];

afterEach(async () => {
  for (const close of openResources.splice(0)) await close();
});

/** Sobe daemon falso e cliente já apontados um para o outro. */
async function connect(
  handler: FakeHandler,
  timeoutMs = 2000,
): Promise<{ client: AgentdClient; daemon: FakeDaemon }> {
  const daemon = await startFakeDaemon(handler);
  const client = new AgentdClient({ socketPath: daemon.socketPath, timeoutMs });

  openResources.push(async () => {
    await client.close();
    await daemon.close();
  });

  return { client, daemon };
}

const HEALTH = { ok: true, version: '0.1.0', protocolVersion: 1, uptimeSeconds: 91 };

describe('resposta bem-sucedida', () => {
  it('fala por Unix socket, valida e converte u64', async () => {
    const { client, daemon } = await connect((_request, response) => {
      replyJson(response, 200, HEALTH);
    });

    const health = await client.health();

    expect(health).toEqual({ ok: true, version: '0.1.0', protocolVersion: 1, uptimeSeconds: 91n });
    expect(daemon.requests[0]?.method).toBe('GET');
    expect(daemon.requests[0]?.url).toBe('/v1/health');
  });

  it('anuncia x-elara-protocol em toda requisição', async () => {
    const { client, daemon } = await connect((request, response) => {
      if (request.url === '/v1/runtime/register') {
        replyJson(response, 200, {
          sessionId: 'sess-1',
          heartbeatIntervalMs: 5000,
          daemonVersion: '0.1.0',
        });
        return;
      }
      replyJson(response, 200, HEALTH);
    });

    await client.health();
    await client.registerRuntime({
      name: 'agent-runtime',
      version: '0.1.0',
      pid: 42,
      protocolVersion: 1,
      capabilities: [],
    });

    expect(daemon.requests).toHaveLength(2);
    for (const request of daemon.requests) {
      expect(request.headers['x-elara-protocol']).toBe('1');
    }
    expect(JSON.parse(daemon.requests[1]!.body)).toMatchObject({ name: 'agent-runtime', pid: 42 });
  });

  it('monta a query de hardware/history e recusa limit inválido', async () => {
    const { client, daemon } = await connect((_request, response) => {
      replyJson(response, 200, []);
    });

    await client.hardwareHistory(5);
    expect(daemon.requests[0]?.url).toBe('/v1/hardware/history?limit=5');

    await expect(client.hardwareHistory(0)).rejects.toMatchObject({
      code: 'sdk.invalid-argument',
    });
    // A requisição inválida nem chegou a sair.
    expect(daemon.requests).toHaveLength(1);
  });
});

describe('erro vindo do daemon', () => {
  it('preserva o code do corpo ApiError', async () => {
    const { client } = await connect((_request, response) => {
      replyJson(response, 503, {
        code: 'hardware.unavailable',
        message: 'NVML não carregou',
        details: 'libnvidia-ml.so.1 ausente',
      });
    });

    const error = await client.hardware().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(DaemonResponseError);
    expect(error).toMatchObject({
      code: 'hardware.unavailable',
      status: 503,
      details: 'libnvidia-ml.so.1 ausente',
    });
    expect((error as Error).message).toContain('NVML não carregou');
  });

  it('não engole um corpo de erro fora do contrato', async () => {
    const { client } = await connect((_request, response) => {
      response.writeHead(500, { 'content-type': 'text/html', 'x-elara-protocol': '1' });
      response.end('<html>nginx</html>');
    });

    const error = await client.status().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(DaemonResponseError);
    expect(error).toMatchObject({ code: 'daemon.unexpected-error', status: 500 });
    expect((error as DaemonResponseError).details).toContain('nginx');
  });
});

describe('resposta que viola o contrato', () => {
  it('falha na validação citando o campo, em vez de devolver undefined', async () => {
    const { client } = await connect((_request, response) => {
      // `uptimeSeconds` como texto não numérico: o tipo do Rust é u64.
      replyJson(response, 200, { ...HEALTH, uptimeSeconds: 'noventa e um' });
    });

    const error = await client.health().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(ProtocolMismatchError);
    expect((error as Error).message).toContain('uptimeSeconds');
    expect((error as Error).message).toContain('GET /v1/health');
  });

  it('recusa daemon que anuncia outra versão de protocolo', async () => {
    const { client } = await connect((_request, response) => {
      replyJson(response, 200, HEALTH, '2');
    });

    const error = await client.health().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(ProtocolMismatchError);
    expect((error as Error).message).toContain('protocolo 2');
  });

  it('trata corpo não-JSON como violação de contrato', async () => {
    const { client } = await connect((_request, response) => {
      response.writeHead(200, { 'content-type': 'application/json', 'x-elara-protocol': '1' });
      response.end('isto não é json');
    });

    await expect(client.health()).rejects.toBeInstanceOf(ProtocolMismatchError);
  });
});

describe('daemon ausente ou lento', () => {
  it('socket inexistente vira DaemonUnavailableError', async () => {
    const client = new AgentdClient({
      socketPath: '/tmp/elara-inexistente-9a7f/agentd.sock',
      timeoutMs: 1000,
    });
    openResources.push(() => client.close());

    const error = await client.health().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(DaemonUnavailableError);
    expect(error).toMatchObject({ code: 'daemon.unavailable' });
    expect((error as DaemonUnavailableError).socketPath).toContain('agentd.sock');
  });

  it('daemon que não responde vira DaemonTimeoutError', async () => {
    const { client } = await connect(() => {
      // Nunca responde: é exatamente o caso que o timeout existe para cobrir.
    }, 150);

    const error = await client.health().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(DaemonTimeoutError);
    expect((error as DaemonTimeoutError).timeoutMs).toBe(150);
  });

  it('conexão derrubada no meio da resposta vira DaemonUnavailableError', async () => {
    const { client } = await connect((_request, response) => {
      response.writeHead(200, { 'content-type': 'application/json', 'x-elara-protocol': '1' });
      response.write('{"ok":true,');
      response.destroy();
    });

    const error = await client.health().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(DaemonUnavailableError);
  });
});
