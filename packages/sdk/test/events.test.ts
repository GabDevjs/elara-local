import { afterEach, describe, expect, it } from 'vitest';

import { createLogger, ProtocolMismatchError } from '@elara/shared';

import { AgentdClient, type EventStreamItem } from '../src/client.js';
import { openEventStream, replyJson, startFakeDaemon, type FakeDaemon } from './fake-daemon.js';

const openResources: (() => Promise<void>)[] = [];

afterEach(async () => {
  for (const close of openResources.splice(0)) await close();
});

/** Serializa um envelope em uma única linha `data:`, o caso comum. */
function frame(sequence: number, name: string, payload: Record<string, unknown>): string {
  const envelope = {
    sequence,
    emittedAt: '2026-07-27T09:15:00Z',
    payload: { type: name, ...payload },
  };
  return `id: ${sequence}\nevent: ${name}\ndata: ${JSON.stringify(envelope)}\n\n`;
}

/**
 * Serializa o mesmo envelope com o JSON indentado, uma linha `data:` por
 * linha do JSON.
 *
 * É o caso que quebra parser ingênuo: o SSE junta as linhas `data:` com `\n`,
 * então quem só olhar a primeira — ou quem esquecer de juntar — recebe JSON
 * truncado. Um daemon que use `serde_json::to_string_pretty` produz
 * exatamente isto.
 */
function multilineFrame(sequence: number): string {
  const envelope = {
    sequence,
    emittedAt: '2026-07-27T09:15:00Z',
    payload: { type: 'daemon.stopping', reason: 'SIGTERM' },
  };
  const lines = JSON.stringify(envelope, null, 2)
    .split('\n')
    .map((line) => `data: ${line}`);
  return `id: ${sequence}\nevent: daemon.stopping\n${lines.join('\n')}\n\n`;
}

async function connect(
  handler: Parameters<typeof startFakeDaemon>[0],
): Promise<{ client: AgentdClient; daemon: FakeDaemon }> {
  const daemon = await startFakeDaemon(handler);
  const client = new AgentdClient({
    socketPath: daemon.socketPath,
    timeoutMs: 2000,
    // Reconexão emite um aviso por tentativa; silenciado para o log do teste
    // mostrar só o que falhou.
    logger: createLogger('sdk', { env: { ELARA_LOG: 'off' } }),
  });
  openResources.push(async () => {
    await client.close();
    await daemon.close();
  });
  return { client, daemon };
}

/** Consome no máximo `count` itens do stream e aborta. */
async function take(
  client: AgentdClient,
  count: number,
  options: { lastEventId?: number; reconnect?: boolean } = {},
): Promise<EventStreamItem[]> {
  const controller = new AbortController();
  const items: EventStreamItem[] = [];

  const stream = client.streamEvents({
    signal: controller.signal,
    backoff: { initialMs: 5, maxMs: 20, jitter: 0 },
    ...(options.lastEventId === undefined ? {} : { lastEventId: options.lastEventId }),
    ...(options.reconnect === undefined ? {} : { reconnect: options.reconnect }),
  });

  try {
    for await (const item of stream) {
      items.push(item);
      if (items.length >= count) break;
    }
  } finally {
    controller.abort();
  }

  return items;
}

describe('streamEvents', () => {
  it('entrega eventos com data multilinha corretamente remontado', async () => {
    const { client } = await connect((_request, response) => {
      openEventStream(response);
      response.write(multilineFrame(1));
    });

    const [item] = await take(client, 1);

    expect(item?.envelope.sequence).toBe(1n);
    expect(item?.envelope.payload).toEqual({ type: 'daemon.stopping', reason: 'SIGTERM' });
    expect(item?.eventName).toBe('daemon.stopping');
    expect(item?.continuity).toEqual({ kind: 'first' });
  });

  it('marca sequência contínua e detecta lacuna', async () => {
    const { client } = await connect((_request, response) => {
      openEventStream(response);
      response.write(frame(10, 'daemon.stopping', { reason: 'a' }));
      response.write(frame(11, 'daemon.stopping', { reason: 'b' }));
      // 12 e 13 nunca chegam: o daemon perdeu assinantes lentos.
      response.write(frame(14, 'daemon.stopping', { reason: 'c' }));
    });

    const items = await take(client, 3);

    expect(items.map((item) => item.continuity)).toEqual([
      { kind: 'first' },
      { kind: 'continuous' },
      { kind: 'gap', missed: 2n },
    ]);
  });

  it('compara com o lastEventId informado pelo chamador', async () => {
    const { client, daemon } = await connect((_request, response) => {
      openEventStream(response);
      response.write(frame(9, 'daemon.stopping', { reason: 'a' }));
    });

    const [item] = await take(client, 1, { lastEventId: 5 });

    expect(daemon.requests[0]?.headers['last-event-id']).toBe('5');
    expect(item?.continuity).toEqual({ kind: 'gap', missed: 3n });
  });

  it('reconhece que o daemon reiniciou quando a sequência recua', async () => {
    const { client } = await connect((_request, response) => {
      openEventStream(response);
      response.write(frame(50, 'daemon.stopping', { reason: 'antes' }));
      response.write(frame(1, 'daemon.started', { version: '0.1.0', pid: 7, socketPath: '/s' }));
    });

    const items = await take(client, 2);

    expect(items[1]?.continuity).toEqual({ kind: 'reset', previous: 50n });
  });

  it('reconecta com backoff quando o daemon cai, retomando pelo Last-Event-ID', async () => {
    let connection = 0;

    const { client, daemon } = await connect((_request, response) => {
      connection += 1;
      openEventStream(response);
      if (connection === 1) {
        response.write(frame(1, 'daemon.stopping', { reason: 'antes da queda' }));
        // Derruba a conexão no meio do stream.
        setTimeout(() => response.destroy(), 20);
        return;
      }
      response.write(frame(4, 'daemon.stopping', { reason: 'depois da volta' }));
    });

    const items = await take(client, 2);

    expect(items).toHaveLength(2);
    expect(items[1]?.reconnects).toBe(1);
    expect(items[1]?.continuity).toEqual({ kind: 'gap', missed: 2n });
    expect(daemon.requests).toHaveLength(2);
    expect(daemon.requests[1]?.headers['last-event-id']).toBe('1');
  });

  it('não reconecta quando reconnect é false', async () => {
    let connection = 0;

    const { client, daemon } = await connect((_request, response) => {
      connection += 1;
      openEventStream(response);
      response.write(frame(connection, 'daemon.stopping', { reason: 'x' }));
      setTimeout(() => response.end(), 10);
    });

    const items: EventStreamItem[] = [];
    for await (const item of client.streamEvents({
      reconnect: false,
      backoff: { initialMs: 5, jitter: 0 },
    })) {
      items.push(item);
    }

    expect(items).toHaveLength(1);
    expect(daemon.requests).toHaveLength(1);
  });

  it('interrompe — sem reconectar — quando o payload viola o contrato', async () => {
    const { client, daemon } = await connect((_request, response) => {
      openEventStream(response);
      response.write('id: 1\nevent: daemon.stopping\ndata: {"sequence":1,"payload":{}}\n\n');
    });

    const stream = client.streamEvents({ backoff: { initialMs: 5, jitter: 0 } });
    const error = await stream.next().catch((cause: unknown) => cause);

    expect(error).toBeInstanceOf(ProtocolMismatchError);
    expect((error as Error).message).toContain('emittedAt');
    expect(daemon.requests).toHaveLength(1);
  });

  it('interrompe quando o daemon recusa o stream com 4xx', async () => {
    const { client, daemon } = await connect((_request, response) => {
      replyJson(response, 403, {
        code: 'events.forbidden',
        message: 'sem permissão',
        details: null,
      });
    });

    const stream = client.streamEvents({ backoff: { initialMs: 5, jitter: 0 } });
    const error = await stream.next().catch((cause: unknown) => cause);

    expect(error).toMatchObject({ code: 'events.forbidden', status: 403 });
    expect(daemon.requests).toHaveLength(1);
  });
});
