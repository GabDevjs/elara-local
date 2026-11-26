/**
 * Ponto de entrada do agent-runtime.
 *
 * Na Fase 1 este processo ainda não hospeda agente nenhum: ele existe para o
 * daemon poder supervisionar um runtime de verdade — subir, registrar-se,
 * provar que está vivo e sair limpo. Deliberadamente não há esqueleto de
 * Agent, Workflow, Tool ou Memory aqui; essas peças chegam na Fase 6, junto
 * com o código que as usa.
 */
import { AgentdClient } from '@elara/sdk';
import { createLogger, isElaraError, resolveElaraPaths } from '@elara/shared';

import { runSession } from './core/session.js';
import { readPackageIdentity } from './core/version.js';

/** Sinais que iniciam o desligamento ordenado. */
const SHUTDOWN_SIGNALS = ['SIGTERM', 'SIGINT'] as const;

/** Código de saída quando o runtime desiste por erro irrecuperável. */
const EXIT_FAILURE = 1;

/**
 * Sobe o runtime e mantém a sessão até um sinal ou uma ordem do daemon.
 *
 * Devolve o código de saída em vez de chamar `process.exit`: assim o `finally`
 * consegue fechar o cliente antes de o processo morrer.
 */
async function main(): Promise<number> {
  const logger = createLogger('agent-runtime');
  const { name, version } = readPackageIdentity();
  const paths = resolveElaraPaths();

  const controller = new AbortController();
  const detach = installSignalHandlers(controller, logger);

  const client = new AgentdClient({ socketPath: paths.socket, logger: logger.child('sdk') });

  logger.info('agent-runtime iniciando', {
    name,
    version,
    pid: process.pid,
    socketPath: paths.socket,
    nodeVersion: process.version,
  });

  try {
    const outcome = await runSession({
      client,
      name,
      version,
      pid: process.pid,
      // Vazio de propósito: anunciar capacidade que não existe faria o daemon
      // rotear trabalho para um runtime que não sabe executá-lo.
      capabilities: [],
      logger,
      signal: controller.signal,
    });

    logger.info('agent-runtime encerrando', { outcome });
    return 0;
  } catch (error) {
    logger.error('agent-runtime desistiu', {
      error,
      // Um erro do Elara já traz `code`; qualquer outro é bug nosso e o stack
      // é a única pista útil.
      ...(isElaraError(error) ? {} : { stack: error instanceof Error ? error.stack : null }),
    });
    return EXIT_FAILURE;
  } finally {
    detach();
    await client.close();
  }
}

/**
 * Liga SIGTERM/SIGINT ao aborto da sessão.
 *
 * O segundo sinal mata o processo na hora: se o desligamento ordenado travar,
 * quem apertou Ctrl-C duas vezes quer sair, não esperar.
 */
function installSignalHandlers(
  controller: AbortController,
  logger: ReturnType<typeof createLogger>,
): () => void {
  const handlers = SHUTDOWN_SIGNALS.map((signal) => {
    const handler = (): void => {
      if (controller.signal.aborted) {
        logger.warn('segundo sinal recebido; saindo imediatamente', { signal });
        process.exit(EXIT_FAILURE);
      }
      logger.info('sinal recebido; iniciando desligamento', { signal });
      controller.abort();
    };
    process.on(signal, handler);
    return { signal, handler };
  });

  return () => {
    for (const { signal, handler } of handlers) process.off(signal, handler);
  };
}

process.exitCode = await main();
