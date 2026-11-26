/**
 * Utilidades comuns ao TypeScript do Elara.
 *
 * Só entra aqui o que precisa ser idêntico entre o SDK, o agent-runtime e o
 * desktop: o formato do log, a resolução de caminhos e a taxonomia de erros.
 * Nada que dependa de runtime pesado — o desktop importa este pacote também.
 */

export {
  createLogger,
  LOG_ENV_VAR,
  LOG_LEVELS,
  parseLogFilter,
  type FilterLevel,
  type LogFields,
  type LogFilter,
  type Logger,
  type LoggerOptions,
  type LogLevel,
} from './logger.js';

export { resolveElaraPaths, type ElaraPaths, type ResolvePathsOptions } from './paths.js';

export {
  DaemonResponseError,
  DaemonTimeoutError,
  DaemonUnavailableError,
  ElaraError,
  isElaraError,
  ProtocolMismatchError,
  type ElaraErrorOptions,
} from './errors.js';
