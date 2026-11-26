/**
 * Daemon falso: um servidor HTTP de verdade em um Unix socket de verdade.
 *
 * Não é um mock do `AgentdClient` — é o outro lado do socket. Mockar o undici
 * testaria o mock; aqui o que se testa é a coisa que vai rodar em produção:
 * conexão por socket, headers, streaming e queda de conexão no meio.
 */
import { mkdtemp, rm } from 'node:fs/promises';
import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

/** O que o daemon falso viu chegar. */
export interface RecordedRequest {
  readonly method: string;
  readonly url: string;
  readonly headers: Readonly<Record<string, string | string[] | undefined>>;
  readonly body: string;
}

/** Servidor em execução. */
export interface FakeDaemon {
  /** Caminho do socket a passar ao cliente. */
  readonly socketPath: string;
  /** Requisições recebidas, em ordem. */
  readonly requests: RecordedRequest[];
  /** Derruba o servidor e apaga o diretório temporário. */
  close(): Promise<void>;
}

/** Assinatura do tratador de requisições do daemon falso. */
export type FakeHandler = (
  request: IncomingMessage,
  response: ServerResponse,
  recorded: RecordedRequest,
) => void | Promise<void>;

/**
 * Sobe o servidor em um socket dentro de um diretório temporário novo.
 *
 * O diretório é criado por `mkdtemp` porque um socket com nome fixo faria dois
 * testes em paralelo brigarem pelo mesmo caminho.
 */
export async function startFakeDaemon(handler: FakeHandler): Promise<FakeDaemon> {
  const directory = await mkdtemp(join(tmpdir(), 'elara-sdk-'));
  const socketPath = join(directory, 'agentd.sock');
  const requests: RecordedRequest[] = [];

  const server = createServer((request, response) => {
    const chunks: Buffer[] = [];
    request.on('data', (chunk: Buffer) => chunks.push(chunk));
    request.on('end', () => {
      const recorded: RecordedRequest = {
        method: request.method ?? '',
        url: request.url ?? '',
        headers: request.headers,
        body: Buffer.concat(chunks).toString('utf8'),
      };
      requests.push(recorded);
      void handler(request, response, recorded);
    });
  });

  await new Promise<void>((resolve) => {
    server.listen(socketPath, resolve);
  });

  return {
    socketPath,
    requests,
    close: async () => {
      // Sem isto, um stream SSE aberto seguraria o `close` para sempre.
      server.closeAllConnections();
      await new Promise<void>((resolve) => {
        server.close(() => {
          resolve();
        });
      });
      await rm(directory, { recursive: true, force: true });
    },
  };
}

/** Responde um JSON já anunciando a versão do protocolo, como o daemon faz. */
export function replyJson(
  response: ServerResponse,
  status: number,
  payload: unknown,
  protocolVersion = '1',
): void {
  response.writeHead(status, {
    'content-type': 'application/json',
    'x-elara-protocol': protocolVersion,
  });
  response.end(JSON.stringify(payload));
}

/** Abre uma resposta `text/event-stream` sem fechá-la. */
export function openEventStream(response: ServerResponse): void {
  response.writeHead(200, {
    'content-type': 'text/event-stream',
    'cache-control': 'no-cache',
    'x-elara-protocol': '1',
  });
}
