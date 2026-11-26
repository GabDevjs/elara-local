/**
 * Cliente TypeScript do `agentd`.
 *
 * Ponto único de contato com o daemon: quem importa daqui não precisa saber
 * que existe um Unix socket, nem escrever validação de payload — os dois já
 * estão embutidos.
 */

export {
  AgentdClient,
  SchemaMismatchError,
  type AgentdClientOptions,
  type EventStreamItem,
  type EventStreamOptions,
  type SequenceContinuity,
} from './client.js';

export { computeBackoffDelay, delay, type BackoffOptions } from './backoff.js';

export { SseParser, type SseFrame } from './sse.js';

// Reexportados para que o consumidor trate erros sem depender diretamente de
// `@elara/shared` — todo erro lançado por este pacote é um destes.
export {
  DaemonResponseError,
  DaemonTimeoutError,
  DaemonUnavailableError,
  ElaraError,
  isElaraError,
  ProtocolMismatchError,
} from '@elara/shared';
