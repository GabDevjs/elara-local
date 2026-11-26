/**
 * Hierarquia de erros do Elara.
 *
 * O objetivo é que quem captura consiga **decidir** — repetir, reconectar,
 * avisar o usuário — sem inspecionar strings de mensagem. Por isso todo erro
 * carrega um `code` estável, e os códigos que vêm do daemon são preservados
 * exatamente como ele os enviou em vez de traduzidos para outra taxonomia.
 */

/** Opções comuns aos construtores da hierarquia. */
export interface ElaraErrorOptions {
  /** Contexto adicional para diagnóstico, tipicamente vindo do daemon. */
  readonly details?: string | undefined;
  /** Erro subjacente, preservado para o stack trace completo. */
  readonly cause?: unknown;
}

/**
 * Base de todos os erros do Elara.
 *
 * `code` é a chave de decisão programática; `message` é para humanos. Nunca
 * derive comportamento da `message`.
 */
export class ElaraError extends Error {
  /** Código estável, ex.: `daemon.unavailable`. */
  readonly code: string;

  /** Contexto adicional, ou `null` quando não há. */
  readonly details: string | null;

  constructor(code: string, message: string, options: ElaraErrorOptions = {}) {
    super(message, 'cause' in options ? { cause: options.cause } : undefined);
    this.name = new.target.name;
    this.code = code;
    this.details = options.details ?? null;
  }

  /**
   * Forma serializável do erro.
   *
   * Existe para o logger: `JSON.stringify(new Error(...))` produz `{}`, o que
   * transforma uma falha em uma linha de log vazia.
   */
  toJSON(): { name: string; code: string; message: string; details: string | null } {
    return { name: this.name, code: this.code, message: this.message, details: this.details };
  }
}

/**
 * O daemon não está acessível: socket inexistente, recusado ou derrubado no
 * meio da requisição.
 *
 * É o único erro em que reconectar com backoff é a resposta certa — os demais
 * não melhoram com repetição.
 */
export class DaemonUnavailableError extends ElaraError {
  /** Caminho do socket que se tentou usar. */
  readonly socketPath: string;

  constructor(socketPath: string, message: string, options: ElaraErrorOptions = {}) {
    super('daemon.unavailable', message, options);
    this.socketPath = socketPath;
  }
}

/** A requisição excedeu o tempo limite configurado no cliente. */
export class DaemonTimeoutError extends ElaraError {
  /** Limite que foi estourado, em milissegundos. */
  readonly timeoutMs: number;

  constructor(timeoutMs: number, message: string, options: ElaraErrorOptions = {}) {
    super('daemon.timeout', message, options);
    this.timeoutMs = timeoutMs;
  }
}

/**
 * O daemon respondeu, mas com um status de erro.
 *
 * O `code` é o que veio no corpo `ApiError`, não um código nosso: quem trata
 * `hardware.unavailable` continua tratando `hardware.unavailable` mesmo que o
 * daemon passe a devolvê-lo com outro status HTTP.
 */
export class DaemonResponseError extends ElaraError {
  /** Status HTTP da resposta. */
  readonly status: number;

  constructor(status: number, code: string, message: string, options: ElaraErrorOptions = {}) {
    super(code, message, options);
    this.status = status;
  }
}

/**
 * Cliente e daemon não falam o mesmo contrato.
 *
 * Cobre tanto a divergência de `x-elara-protocol` quanto um payload que não
 * passou na validação do schema — nos dois casos a ação é a mesma: atualizar
 * um dos dois lados. Repetir a requisição não resolve.
 */
export class ProtocolMismatchError extends ElaraError {
  constructor(message: string, options: ElaraErrorOptions = {}) {
    super('protocol.mismatch', message, options);
  }
}

/**
 * Discrimina erros do Elara de qualquer outro `unknown` capturado.
 *
 * Verifica a marca estrutural em vez de `instanceof` porque duas cópias do
 * pacote em `node_modules` produziriam classes diferentes e o `instanceof`
 * falharia silenciosamente.
 */
export function isElaraError(value: unknown): value is ElaraError {
  return (
    value instanceof Error &&
    typeof (value as { code?: unknown }).code === 'string' &&
    'details' in value
  );
}
