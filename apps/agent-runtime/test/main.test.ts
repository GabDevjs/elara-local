import { spawn, type ChildProcessByStdio } from 'node:child_process';
import type { Readable } from 'node:stream';
import { fileURLToPath } from 'node:url';

import { afterEach, describe, expect, it } from 'vitest';

import { readPackageIdentity } from '../src/core/version.js';
import { createFakeAgentd, type FakeAgentd } from './fake-agentd.js';

const PACKAGE_ROOT = fileURLToPath(new URL('..', import.meta.url));
const TSX = fileURLToPath(new URL('../node_modules/.bin/tsx', import.meta.url));

const cleanups: (() => Promise<void>)[] = [];

afterEach(async () => {
  for (const cleanup of cleanups.splice(0)) await cleanup();
});

/** Resolve quando `condition` for verdadeira, ou falha por timeout. */
async function waitFor(condition: () => boolean, label: string, timeoutMs = 15_000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!condition()) {
    if (Date.now() > deadline) throw new Error(`tempo esgotado esperando: ${label}`);
    await new Promise<void>((resolve) => setTimeout(resolve, 20));
  }
}

/**
 * Sobe o `main.ts` como processo separado, apontado ao daemon falso.
 *
 * Roda o fonte via `tsx` de propósito: o teste do ponto de entrada não deve
 * exigir que o `dist` esteja compilado para valer alguma coisa.
 */
function startRuntime(daemon: FakeAgentd): {
  child: ChildProcessByStdio<null, Readable, Readable>;
  logs: string[];
} {
  const child = spawn(TSX, ['src/main.ts'], {
    cwd: PACKAGE_ROOT,
    env: { ...process.env, ELARA_SOCKET: daemon.socketPath, ELARA_LOG: 'info' },
    stdio: ['ignore', 'pipe', 'pipe'],
  });

  const logs: string[] = [];
  child.stderr.setEncoding('utf8');
  child.stderr.on('data', (chunk: string) => {
    logs.push(...chunk.split('\n').filter((line) => line !== ''));
  });

  // A promessa é criada aqui, e não no cleanup: se o processo já tiver
  // fechado quando a limpeza rodar, um listener novo nunca dispararia.
  const closed = new Promise<void>((resolve) => {
    child.once('close', () => {
      resolve();
    });
  });

  cleanups.push(async () => {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
    await closed;
  });

  return { child, logs };
}

describe('readPackageIdentity', () => {
  it('lê nome e versão do manifesto em vez de repeti-los no código', () => {
    expect(readPackageIdentity()).toEqual({ name: '@elara/agent-runtime', version: '0.1.0' });
  });
});

describe('main', () => {
  it('registra-se, bate o coração e sai com 0 no SIGTERM', async () => {
    const daemon = await createFakeAgentd();
    cleanups.push(() => daemon.dispose());
    daemon.heartbeatIntervalMs = 1000;

    const { child, logs } = startRuntime(daemon);

    await waitFor(() => daemon.registrations.length >= 1, 'registro do runtime');
    expect(daemon.registrations[0]).toMatchObject({
      name: '@elara/agent-runtime',
      version: '0.1.0',
      protocolVersion: 1,
      capabilities: [],
    });

    await waitFor(() => daemon.heartbeats.length >= 1, 'primeiro heartbeat');

    child.kill('SIGTERM');
    const code = await new Promise<number | null>((resolve) => {
      child.on('exit', (exitCode) => {
        resolve(exitCode);
      });
    });

    expect(code).toBe(0);
    expect(daemon.heartbeats.at(-1)).toMatchObject({
      healthy: false,
      detail: 'runtime encerrando',
    });

    // O log tem de sair no formato do `tracing`, senão o incidente vira
    // arqueologia quando daemon e runtime escrevem no mesmo lugar.
    const parsed = logs
      .filter((line) => line.startsWith('{'))
      .map((line) => JSON.parse(line) as Record<string, unknown>);
    const startup = parsed.find(
      (entry) =>
        (entry.fields as { message?: unknown } | undefined)?.message === 'agent-runtime iniciando',
    );

    expect(startup).toMatchObject({ level: 'INFO', target: 'agent-runtime' });
    expect(typeof startup?.timestamp).toBe('string');
  }, 30_000);

  it('espera pelo daemon quando o socket ainda não existe', async () => {
    const daemon = await createFakeAgentd();
    cleanups.push(() => daemon.dispose());
    await daemon.stop();

    const { child, logs } = startRuntime(daemon);

    // Sem daemon o processo não pode morrer nem girar: fica avisando e
    // esperando até o socket aparecer.
    await waitFor(
      () => logs.some((line) => line.includes('nova tentativa de registro')),
      'aviso de daemon indisponível',
    );
    expect(child.exitCode).toBeNull();

    await daemon.start();
    await waitFor(() => daemon.registrations.length >= 1, 'registro após o daemon subir');

    child.kill('SIGTERM');
    await new Promise<void>((resolve) => {
      child.on('exit', () => {
        resolve();
      });
    });
  }, 30_000);
});
