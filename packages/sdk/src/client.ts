/**
 * Cliente do `agentd`.
 *
 * Três regras moldam este arquivo:
 *
 * 1. **Só Unix socket.** Nunca há uma porta TCP para o daemon; a origem HTTP
 *    (`http://elara.invalid`) existe só porque o protocolo exige um `Host`.
 * 2. **Toda resposta é validada.** O corpo entra como `unknown` e sai como o
 *    tipo gerado do Rust, ou vira exceção citando o campo — nunca `undefined`
 *    vazando para a camada de cima.
 * 3. **Todo erro é tipado.** Socket ausente, timeout, erro do daemon e
 *    contrato divergente são quatro decisões diferentes para quem chama, então
 *    são quatro classes diferentes.
 */
import {
  ApiErrorSchema,
  DaemonStatusSchema,
  EventEnvelopeSchema,
  HardwareHistorySchema,
  HardwareResponseSchema,
  HealthResponseSchema,
  parseOrThrow,
  PROTOCOL_HEADER,
  PROTOCOL_VERSION,
  ResourcesResponseSchema,
  RuntimeHeartbeatResponseSchema,
  RuntimeRegisterResponseSchema,
  SchemaMismatchError,
  type DaemonStatus,
  type EventEnvelope,
  type HardwareHistoryEntry,
  type HardwareResponse,
  type HealthResponse,
  type ResourcesResponse,
  type RuntimeHeartbeatRequest,
  type RuntimeHeartbeatResponse,
  type RuntimeRegisterRequest,
  type RuntimeRegisterResponse,
} from '@elara/schemas';
import {
  createLogger,
  DaemonResponseError,
  DaemonTimeoutError,
  DaemonUnavailableError,
  ElaraError,
  ProtocolMismatchError,
  resolveElaraPaths,
  type Logger,
} from '@elara/shared';
import { Pool, type Dispatcher } from 'undici';
import type { z } from 'zod';

import { computeBackoffDelay, delay, type BackoffOptions } from './backoff.js';
import { SseParser } from './sse.js';

/**
 * Origem HTTP nominal.
 *
 * O domínio é reservado pela RFC 6761 justamente para nunca resolver: se
 * alguma configuração errada desviar o cliente para TCP, ele falha em vez de
 * conversar com um servidor qualquer.
 */
const ORIGIN = 'http://elara.invalid';

/** Timeout padrão das requisições curtas, em ms. */
const DEFAULT_TIMEOUT_MS = 5_000;

/** Quantos caracteres de um corpo inesperado entram na mensagem de erro. */
const MAX_BODY_EXCERPT = 400;

/** Códigos de erro de sistema que significam "o daemon não está lá". */
const UNAVAILABLE_CODES = new Set([
  'ENOENT',
  'ECONNREFUSED',
  'EACCES',
  'EPERM',
  'ECONNRESET',
  'EPIPE',
  'UND_ERR_SOCKET',
  'UND_ERR_CLOSED',
  'UND_ERR_DESTROYED',
  'UND_ERR_CONNECT_TIMEOUT',
]);

/** Configuração do cliente. */
export interface AgentdClientOptions {
  /** Caminho do socket. Padrão: o mesmo que o daemon resolve via XDG. */
  readonly socketPath?: string;
  /** Limite por requisição curta, em ms. Padrão: 5000. O SSE não usa. */
  readonly timeoutMs?: number;
  /** Logger a usar. Padrão: um logger de alvo `sdk`. */
  readonly logger?: Logger;
}

/** Como a sequência deste evento se liga à do anterior. */
export type SequenceContinuity =
  /** Primeiro evento desta iteração: não há anterior com que comparar. */
  | { readonly kind: 'first' }
  /** O número seguiu o anterior sem buraco. */
  | { readonly kind: 'continuous' }
  /** Eventos foram perdidos — tipicamente durante uma reconexão. */
  | { readonly kind: 'gap'; readonly missed: bigint }
  /** A sequência recuou: o daemon reiniciou e recomeçou a contar. */
  | { readonly kind: 'reset'; readonly previous: bigint };

/** Um evento entregue pelo stream, com o contexto de continuidade. */
export interface EventStreamItem {
  /** Envelope validado. */
  readonly envelope: EventEnvelope;
  /** Nome vindo do campo `event:` do SSE, quando o daemon o enviou. */
  readonly eventName: string | null;
  /** O que aconteceu com a numeração entre o evento anterior e este. */
  readonly continuity: SequenceContinuity;
  /** Quantas reconexões ocorreram desde o início desta iteração. */
  readonly reconnects: number;
}

/** Configuração de [`AgentdClient.streamEvents`]. */
export interface EventStreamOptions {
  /** Encerra o stream quando abortado. */
  readonly signal?: AbortSignal;
  /** Retoma a partir deste número de sequência (exclusivo). */
  readonly lastEventId?: bigint | number | string;
  /** Reconectar quando a conexão cair. Padrão: `true`. */
  readonly reconnect?: boolean;
  /** Curva de espera entre reconexões. */
  readonly backoff?: BackoffOptions;
}

/**
 * Cliente HTTP tipado do daemon.
 *
 * Uma instância mantém um pool de conexões: o stream de eventos ocupa uma
 * conexão indefinidamente, e sem pool os heartbeats ficariam enfileirados
 * atrás dele para sempre.
 */
export class AgentdClient {
  /** Socket em uso, exposto para mensagens de erro e diagnóstico. */
  readonly socketPath: string;

  /** Limite das requisições curtas, em ms. */
  readonly timeoutMs: number;

  readonly #pool: Pool;
  readonly #log: Logger;

  constructor(options: AgentdClientOptions = {}) {
    this.socketPath = options.socketPath ?? resolveElaraPaths().socket;
    this.timeoutMs = options.timeoutMs ?? DEFAULT_TIMEOUT_MS;
    this.#log = options.logger ?? createLogger('sdk');
    this.#pool = new Pool(ORIGIN, {
      socketPath: this.socketPath,
      // Sem timeout de corpo: o `/v1/events` fica aberto por horas sem tráfego
      // e o padrão de 300 s o mataria. As requisições curtas usam `AbortSignal`.
      bodyTimeout: 0,
      headersTimeout: 0,
      connections: 4,
    });
  }

  /** `GET /v1/health` — barato o bastante para polling. */
  async health(): Promise<HealthResponse> {
    return this.#validate(
      HealthResponseSchema,
      await this.#requestJson('GET', '/v1/health'),
      'GET /v1/health',
    );
  }

  /** `GET /v1/status` — visão completa do daemon e seus componentes. */
  async status(): Promise<DaemonStatus> {
    return this.#validate(
      DaemonStatusSchema,
      await this.#requestJson('GET', '/v1/status'),
      'GET /v1/status',
    );
  }

  /** `GET /v1/hardware` — perfil corrente, possivelmente vindo do cache. */
  async hardware(): Promise<HardwareResponse> {
    return this.#validate(
      HardwareResponseSchema,
      await this.#requestJson('GET', '/v1/hardware'),
      'GET /v1/hardware',
    );
  }

  /** `POST /v1/hardware/refresh` — força nova detecção. */
  async refreshHardware(): Promise<HardwareResponse> {
    return this.#validate(
      HardwareResponseSchema,
      await this.#requestJson('POST', '/v1/hardware/refresh'),
      'POST /v1/hardware/refresh',
    );
  }

  /**
   * `GET /v1/hardware/history` — perfis já vistos nesta máquina.
   *
   * `limit` é validado aqui: mandar `limit=-1` ao daemon só para receber um
   * 400 de volta gasta um round-trip e produz um erro pior.
   */
  async hardwareHistory(limit?: number): Promise<HardwareHistoryEntry[]> {
    let path = '/v1/hardware/history';

    if (limit !== undefined) {
      if (!Number.isInteger(limit) || limit < 1) {
        throw new ElaraError(
          'sdk.invalid-argument',
          `limit precisa ser inteiro positivo; recebido ${limit}`,
        );
      }
      path += `?limit=${limit}`;
    }

    return this.#validate(
      HardwareHistorySchema,
      await this.#requestJson('GET', path),
      `GET ${path}`,
    );
  }

  /** `GET /v1/resources` — última amostra calculada. */
  async resources(): Promise<ResourcesResponse> {
    return this.#validate(
      ResourcesResponseSchema,
      await this.#requestJson('GET', '/v1/resources'),
      'GET /v1/resources',
    );
  }

  /** `POST /v1/runtime/register` — anuncia um runtime ao daemon. */
  async registerRuntime(body: RuntimeRegisterRequest): Promise<RuntimeRegisterResponse> {
    return this.#validate(
      RuntimeRegisterResponseSchema,
      await this.#requestJson('POST', '/v1/runtime/register', body),
      'POST /v1/runtime/register',
    );
  }

  /** `POST /v1/runtime/heartbeat` — confirma que o runtime segue vivo. */
  async heartbeat(body: RuntimeHeartbeatRequest): Promise<RuntimeHeartbeatResponse> {
    return this.#validate(
      RuntimeHeartbeatResponseSchema,
      await this.#requestJson('POST', '/v1/runtime/heartbeat', body),
      'POST /v1/runtime/heartbeat',
    );
  }

  /**
   * `GET /v1/events` — stream SSE, como iterador assíncrono.
   *
   * Reconecta sozinho com backoff quando a conexão cai, retomando pelo header
   * `Last-Event-ID`. **Não** finge que o stream foi contínuo: cada item diz,
   * em `continuity`, se houve lacuna de sequência ou se o daemon reiniciou.
   *
   * Erros de contrato (`ProtocolMismatchError`) e recusas do daemon na faixa
   * 4xx interrompem a iteração: nenhum dos dois melhora com repetição.
   */
  async *streamEvents(options: EventStreamOptions = {}): AsyncGenerator<EventStreamItem> {
    const { signal, reconnect = true } = options;
    const log = this.#log.child('events');

    // Lido por função e não direto: `signal.aborted` muda por fora do fluxo e
    // o compilador, ao estreitar o tipo na condição do `while`, concluiria que
    // as verificações seguintes são inalcançáveis.
    const aborted = (): boolean => signal?.aborted ?? false;

    let lastSequence = toSequence(options.lastEventId);
    let attempt = 0;
    let reconnects = 0;

    while (!aborted()) {
      let delivered = false;

      try {
        for await (const frame of this.#openEventStream(lastSequence, signal)) {
          const envelope = this.#validate(
            EventEnvelopeSchema,
            parseJson(frame.data, 'GET /v1/events'),
            'GET /v1/events',
          );

          const continuity = classify(lastSequence, envelope.sequence);
          lastSequence = envelope.sequence;
          delivered = true;
          attempt = 0;

          yield { envelope, eventName: frame.event, continuity, reconnects };
        }
      } catch (error) {
        if (aborted()) return;
        if (error instanceof ProtocolMismatchError) throw error;
        if (error instanceof DaemonResponseError && error.status < 500) throw error;

        log.warn('stream de eventos caiu', { error, socketPath: this.socketPath });
      }

      if (!reconnect) return;
      if (aborted()) return;

      // Uma conexão que entregou algo antes de cair não é sintoma de daemon
      // doente: reinicia a contagem para não punir um stream longevo.
      if (delivered) attempt = 0;

      const wait = computeBackoffDelay(attempt, options.backoff);
      attempt += 1;
      reconnects += 1;
      log.info('reconectando ao stream de eventos', {
        delayMs: wait,
        attempt,
        lastSequence: lastSequence ?? null,
      });
      await delay(wait, signal);
    }
  }

  /**
   * Encerra o pool.
   *
   * Usa `destroy` e não `close`: o `close` espera as requisições pendentes
   * terminarem, e um stream de eventos aberto nunca termina — o desligamento
   * ficaria pendurado para sempre.
   */
  async close(): Promise<void> {
    await this.#pool.destroy();
  }

  // -------------------------------------------------------------------------
  // Interno
  // -------------------------------------------------------------------------

  /**
   * Valida um corpo já convertido de JSON.
   *
   * Traduz a `SchemaMismatchError` do pacote de schemas para a hierarquia do
   * Elara: quem chama o SDK trata `ElaraError` e não deveria precisar
   * conhecer o zod para descobrir que o daemon está numa versão incompatível.
   */
  #validate<Schema extends z.ZodType>(
    schema: Schema,
    data: unknown,
    context: string,
  ): z.output<Schema> {
    try {
      return parseOrThrow(schema, data, context);
    } catch (error) {
      if (error instanceof SchemaMismatchError) {
        throw new ProtocolMismatchError(error.message, {
          cause: error,
          details: `${error.issues.length} campo(s) fora do contrato`,
        });
      }
      throw error;
    }
  }

  /** Abre a conexão SSE e entrega os frames já montados. */
  async *#openEventStream(
    lastSequence: bigint | null,
    signal: AbortSignal | undefined,
  ): AsyncGenerator<{ event: string | null; data: string }> {
    const headers: Record<string, string> = {
      [PROTOCOL_HEADER]: String(PROTOCOL_VERSION),
      accept: 'text/event-stream',
    };
    if (lastSequence !== null) headers['last-event-id'] = lastSequence.toString();

    const response = await this.#dispatch('GET', '/v1/events', headers, undefined, signal);
    this.#assertProtocol(response, 'GET /v1/events');

    if (response.statusCode >= 400) {
      throw toDaemonError(response.statusCode, await response.body.text(), 'GET /v1/events');
    }

    const parser = new SseParser();
    const decoder = new TextDecoder();

    // O `BodyReadable` do undici herda a tipagem `any` do `Readable` do Node;
    // fixar `Uint8Array` aqui evita espalhar `any` pelo resto do arquivo.
    for await (const chunk of response.body as AsyncIterable<Uint8Array>) {
      for (const frame of parser.feed(decoder.decode(chunk, { stream: true }))) {
        yield { event: frame.event, data: frame.data };
      }
    }
  }

  /** Executa uma requisição curta e devolve o corpo já convertido de JSON. */
  async #requestJson(method: 'GET' | 'POST', path: string, body?: unknown): Promise<unknown> {
    const context = `${method} ${path}`;
    const headers: Record<string, string> = {
      [PROTOCOL_HEADER]: String(PROTOCOL_VERSION),
      accept: 'application/json',
    };
    if (body !== undefined) headers['content-type'] = 'application/json';

    const timeout = AbortSignal.timeout(this.timeoutMs);
    const response = await this.#dispatch(
      method,
      path,
      headers,
      body === undefined ? undefined : JSON.stringify(body),
      timeout,
    );

    let text: string;
    try {
      text = await response.body.text();
    } catch (error) {
      throw this.#connectionError(error, context, timeout);
    }

    this.#assertProtocol(response, context);

    if (response.statusCode >= 400) throw toDaemonError(response.statusCode, text, context);

    return parseJson(text, context);
  }

  /** Envia a requisição, traduzindo falhas de transporte em erros tipados. */
  async #dispatch(
    method: 'GET' | 'POST',
    path: string,
    headers: Record<string, string>,
    body: string | undefined,
    signal: AbortSignal | undefined,
  ): Promise<Dispatcher.ResponseData> {
    try {
      return await this.#pool.request({
        method,
        path,
        headers,
        ...(body === undefined ? {} : { body }),
        ...(signal === undefined ? {} : { signal }),
      });
    } catch (error) {
      throw this.#connectionError(error, `${method} ${path}`, signal);
    }
  }

  /**
   * Classifica uma falha de transporte.
   *
   * O `code` pode estar em qualquer nível da cadeia de causas — o undici
   * embrulha o erro do socket —, por isso a busca desce até achar um.
   */
  #connectionError(error: unknown, context: string, signal: AbortSignal | undefined): ElaraError {
    if (signal?.aborted === true && signal.reason instanceof Error) {
      // `AbortSignal.timeout` aborta com `TimeoutError`; um aborto do chamador
      // vem com outra razão e não deve ser confundido com lentidão do daemon.
      if (signal.reason.name === 'TimeoutError') {
        return new DaemonTimeoutError(
          this.timeoutMs,
          `${context}: o daemon não respondeu em ${this.timeoutMs} ms`,
          { cause: error, details: this.socketPath },
        );
      }
    }

    const code = findErrorCode(error);
    if (code !== null && UNAVAILABLE_CODES.has(code)) {
      return new DaemonUnavailableError(
        this.socketPath,
        `${context}: daemon indisponível em ${this.socketPath} (${code})`,
        { cause: error, details: code },
      );
    }

    return new DaemonUnavailableError(
      this.socketPath,
      `${context}: falha ao falar com o daemon em ${this.socketPath}`,
      { cause: error, ...(code === null ? {} : { details: code }) },
    );
  }

  /**
   * Recusa um daemon que fala outra versão do contrato.
   *
   * A ausência do header não é tratada como erro: um proxy pode removê-lo, e
   * a validação de schema pegaria a divergência real logo em seguida.
   */
  #assertProtocol(response: Dispatcher.ResponseData, context: string): void {
    const raw = response.headers[PROTOCOL_HEADER];
    const announced = Array.isArray(raw) ? raw[0] : raw;
    if (announced === undefined) return;

    if (announced !== String(PROTOCOL_VERSION)) {
      throw new ProtocolMismatchError(
        `${context}: daemon fala o protocolo ${announced}, este cliente fala ${PROTOCOL_VERSION}`,
        { details: `header ${PROTOCOL_HEADER}` },
      );
    }
  }
}

// ---------------------------------------------------------------------------
// Auxiliares
// ---------------------------------------------------------------------------

/** Converte o corpo de erro do daemon preservando o `code` que ele mandou. */
function toDaemonError(status: number, text: string, context: string): DaemonResponseError {
  let payload: unknown;
  try {
    payload = JSON.parse(text);
  } catch {
    payload = undefined;
  }

  const parsed = ApiErrorSchema.safeParse(payload);
  if (parsed.success) {
    return new DaemonResponseError(status, parsed.data.code, `${context}: ${parsed.data.message}`, {
      ...(parsed.data.details === null ? {} : { details: parsed.data.details }),
    });
  }

  // Corpo fora do contrato: o status ainda é informação útil, e o trecho do
  // corpo é o que permite descobrir quem respondeu no lugar do daemon.
  return new DaemonResponseError(
    status,
    'daemon.unexpected-error',
    `${context}: HTTP ${status} com corpo fora do contrato`,
    { details: excerpt(text) },
  );
}

/** `JSON.parse` que falha como violação de contrato, não como `SyntaxError`. */
function parseJson(text: string, context: string): unknown {
  if (text === '') {
    throw new ProtocolMismatchError(`${context}: o daemon respondeu com corpo vazio`);
  }
  try {
    return JSON.parse(text);
  } catch (error) {
    throw new ProtocolMismatchError(`${context}: o corpo não é JSON válido`, {
      cause: error,
      details: excerpt(text),
    });
  }
}

/** Recorta um corpo grande para caber numa mensagem de erro. */
function excerpt(text: string): string {
  return text.length > MAX_BODY_EXCERPT ? `${text.slice(0, MAX_BODY_EXCERPT)}…` : text;
}

/** Desce a cadeia de `cause` até achar um `code` de erro de sistema. */
function findErrorCode(error: unknown): string | null {
  let current: unknown = error;

  for (let depth = 0; depth < 8 && current instanceof Error; depth += 1) {
    const code = (current as { code?: unknown }).code;
    if (typeof code === 'string') return code;
    current = current.cause;
  }

  return null;
}

/** Normaliza o `lastEventId` recebido do chamador. */
function toSequence(value: bigint | number | string | undefined): bigint | null {
  if (value === undefined) return null;
  try {
    return BigInt(value);
  } catch (error) {
    throw new ElaraError('sdk.invalid-argument', `lastEventId inválido: ${String(value)}`, {
      cause: error,
    });
  }
}

/** Compara a sequência recebida com a anterior. */
function classify(previous: bigint | null, current: bigint): SequenceContinuity {
  if (previous === null) return { kind: 'first' };
  if (current <= previous) return { kind: 'reset', previous };

  const missed = current - previous - 1n;
  return missed === 0n ? { kind: 'continuous' } : { kind: 'gap', missed };
}

/** Reexportado para quem trata erros do stream sem importar `@elara/shared`. */
export { SchemaMismatchError };
