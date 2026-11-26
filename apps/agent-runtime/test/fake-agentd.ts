/**
 * Um `agentd` mínimo, de verdade, em um Unix socket de verdade.
 *
 * Implementa só as duas rotas de que a sessão depende. O ponto de ser um
 * servidor real — e não um mock do cliente — é que os casos interessantes do
 * agent-runtime são de transporte: o socket que ainda não existe, o daemon que
 * some no meio, o daemon que volta com outra sessão.
 */
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer, type Server, type ServerResponse } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/** Corpo de registro, como chega do runtime. */
export interface RecordedRegister {
  readonly name: string;
  readonly version: string;
  readonly pid: number;
  readonly protocolVersion: number;
  readonly capabilities: string[];
}

/** Corpo de heartbeat, como chega do runtime. */
export interface RecordedHeartbeat {
  readonly sessionId: string;
  readonly healthy: boolean;
  readonly detail: string | null;
}

/** Daemon falso controlável pelo teste. */
export interface FakeAgentd {
  /** Caminho estável do socket: sobrevive a `stop()` + `start()`. */
  readonly socketPath: string;
  readonly registrations: RecordedRegister[];
  readonly heartbeats: RecordedHeartbeat[];
  /** Resposta devolvida ao próximo heartbeat; o teste altera à vontade. */
  reply: { acknowledged: boolean; shutdownRequested: boolean };
  /** Versão anunciada no header; mudar simula skew de protocolo. */
  protocolVersion: string;
  /** Intervalo devolvido no registro, em ms. */
  heartbeatIntervalMs: number;
  /** Status devolvido no registro; != 200 responde um `ApiError`. */
  registerStatus: number;
  /** Chamado depois de responder cada heartbeat. */
  onHeartbeat?: (body: RecordedHeartbeat, index: number) => void;
  start(): Promise<void>;
  stop(): Promise<void>;
  dispose(): Promise<void>;
}

/** Cria o daemon falso já no ar. */
export async function createFakeAgentd(): Promise<FakeAgentd> {
  const directory = await mkdtemp(join(tmpdir(), 'elara-runtime-'));
  const socketPath = join(directory, 'agentd.sock');

  let server: Server | null = null;
  let sessionCounter = 0;

  const daemon: FakeAgentd = {
    socketPath,
    registrations: [],
    heartbeats: [],
    reply: { acknowledged: true, shutdownRequested: false },
    protocolVersion: '1',
    heartbeatIntervalMs: 1000,
    registerStatus: 200,

    start: async () => {
      if (server !== null) return;

      const created = createServer((request, response) => {
        const chunks: Buffer[] = [];
        request.on('data', (chunk: Buffer) => chunks.push(chunk));
        request.on('end', () => {
          const body: unknown =
            chunks.length === 0 ? null : JSON.parse(Buffer.concat(chunks).toString('utf8'));
          route(daemon, request.url ?? '', body, response, () => {
            sessionCounter += 1;
            return `sess-${sessionCounter}`;
          });
        });
      });

      await new Promise<void>((resolve) => {
        created.listen(socketPath, resolve);
      });
      server = created;
    },

    stop: async () => {
      const current = server;
      if (current === null) return;
      server = null;
      current.closeAllConnections();
      await new Promise<void>((resolve) => {
        current.close(() => {
          resolve();
        });
      });
    },

    dispose: async () => {
      await daemon.stop();
      await rm(directory, { recursive: true, force: true });
    },
  };

  await daemon.start();
  return daemon;
}

/** Despacha a requisição para a rota correspondente. */
function route(
  daemon: FakeAgentd,
  url: string,
  body: unknown,
  response: ServerResponse,
  nextSessionId: () => string,
): void {
  if (url === '/v1/runtime/register') {
    daemon.registrations.push(body as RecordedRegister);
    if (daemon.registerStatus !== 200) {
      send(daemon, response, daemon.registerStatus, {
        code: 'runtime.rejected',
        message: 'este runtime não é aceito',
        details: null,
      });
      return;
    }
    send(daemon, response, 200, {
      sessionId: nextSessionId(),
      heartbeatIntervalMs: daemon.heartbeatIntervalMs,
      daemonVersion: '0.1.0-fake',
    });
    return;
  }

  if (url === '/v1/runtime/heartbeat') {
    const recorded = body as RecordedHeartbeat;
    const index = daemon.heartbeats.length;
    daemon.heartbeats.push(recorded);
    send(daemon, response, 200, {
      acknowledged: daemon.reply.acknowledged,
      shutdownRequested: daemon.reply.shutdownRequested,
    });
    daemon.onHeartbeat?.(recorded, index);
    return;
  }

  send(daemon, response, 404, {
    code: 'route.not-found',
    message: `rota desconhecida: ${url}`,
    details: null,
  });
}

/** Escreve a resposta com o header de protocolo, como o daemon real faz. */
function send(
  daemon: FakeAgentd,
  response: ServerResponse,
  status: number,
  payload: unknown,
): void {
  response.writeHead(status, {
    'content-type': 'application/json',
    'x-elara-protocol': daemon.protocolVersion,
  });
  response.end(JSON.stringify(payload));
}
