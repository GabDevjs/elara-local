/**
 * Fronteira com a ponte Rust.
 *
 * Tudo que atravessa o `invoke` entra aqui como `unknown` e sai validado pelos
 * schemas de `@elara/schemas`. Nenhum outro módulo pode chamar `invoke`: é o
 * que garante que uma resposta fora do contrato vire uma falha nomeada em vez
 * de um `undefined` aparecendo três telas adiante.
 */
import { invoke } from '@tauri-apps/api/core';
import {
  DaemonStatusSchema,
  HardwareHistorySchema,
  HardwareResponseSchema,
  HealthResponseSchema,
  ResourcesResponseSchema,
  SchemaMismatchError,
  parseOrThrow,
  type DaemonStatus,
  type HardwareHistoryEntry,
  type HardwareResponse,
  type HealthResponse,
  type ResourcesResponse,
} from '@elara/schemas';
import { z } from 'zod';

// ---------------------------------------------------------------------------
// Falhas
// ---------------------------------------------------------------------------

/**
 * Falha vinda da ponte, no formato `{ code, message }` que o `BridgeError` do
 * Rust serializa.
 */
export interface BridgeFailure {
  /** Código estável, ex.: `daemon.unreachable`. */
  readonly code: string;
  /** Mensagem em português, pronta para a tela. */
  readonly message: string;
}

/** Forma exata em que a ponte entrega um erro. */
const BridgeFailureSchema = z.object({ code: z.string(), message: z.string() });

/**
 * Erro lançado por qualquer chamada deste módulo.
 *
 * Carrega o `code` separado da mensagem porque a UI decide a tela pelo código
 * e mostra a mensagem — comparar strings de mensagem seria frágil.
 */
export class BridgeCallError extends Error {
  /** Código e mensagem, prontos para armazenar em estado de React. */
  readonly failure: BridgeFailure;

  constructor(failure: BridgeFailure) {
    super(failure.message);
    this.name = 'BridgeCallError';
    this.failure = failure;
  }
}

/**
 * Códigos em que o daemon simplesmente não está respondendo.
 *
 * `daemon.timeout` entra aqui porque, do ponto de vista de quem olha a tela,
 * um daemon travado e um daemon ausente pedem a mesma ação: verificar se ele
 * está no ar.
 */
const OFFLINE_CODES: ReadonlySet<string> = new Set([
  'daemon.unreachable',
  'daemon.socket-unresolved',
  'daemon.timeout',
]);

/** `true` quando a falha significa "não há daemon do outro lado". */
export function isOfflineFailure(failure: BridgeFailure): boolean {
  return OFFLINE_CODES.has(failure.code);
}

/**
 * Normaliza qualquer coisa capturada em um `catch` para uma [`BridgeFailure`].
 *
 * O caso genérico não repassa a mensagem original de propósito: um erro que
 * não veio da ponte não tem texto revisado para o usuário.
 */
export function asFailure(error: unknown): BridgeFailure {
  if (error instanceof BridgeCallError) return error.failure;
  return {
    code: 'bridge.unexpected',
    message: 'falha inesperada na ponte com o daemon',
  };
}

// ---------------------------------------------------------------------------
// Transporte
// ---------------------------------------------------------------------------

/** Chama um comando Tauri traduzindo a rejeição em [`BridgeCallError`]. */
async function call(command: string, args?: Record<string, unknown>): Promise<unknown> {
  try {
    return await invoke<unknown>(command, args);
  } catch (raw) {
    const parsed = BridgeFailureSchema.safeParse(raw);
    if (parsed.success) throw new BridgeCallError(parsed.data);
    throw new BridgeCallError({
      code: 'bridge.unavailable',
      message: `o comando ${command} não pôde ser executado pela ponte`,
    });
  }
}

/**
 * Valida o payload e converte a divergência de contrato em falha nomeada.
 *
 * Um `SchemaMismatchError` significa que o daemon e esta janela foram
 * compilados de versões diferentes — a UI mostra isso, com o campo culpado,
 * em vez de renderizar dado pela metade.
 */
export function decode<Schema extends z.ZodType>(
  schema: Schema,
  raw: unknown,
  context: string,
): z.output<Schema> {
  try {
    return parseOrThrow(schema, raw, context);
  } catch (error) {
    if (error instanceof SchemaMismatchError) {
      throw new BridgeCallError({ code: 'contract.mismatch', message: error.message });
    }
    throw error;
  }
}

// ---------------------------------------------------------------------------
// Comandos
// ---------------------------------------------------------------------------

/** De onde a ponte tirou o caminho do socket. */
export const SocketSourceSchema = z.enum(['env', 'xdg-runtime-dir']);

/** Caminho do socket e sua procedência. */
export const SocketInfoSchema = z.object({
  socketPath: z.string(),
  source: SocketSourceSchema,
});

/** Resultado de [`fetchSocketInfo`]. */
export type SocketInfo = z.infer<typeof SocketInfoSchema>;

/**
 * Onde a ponte procura o daemon. Não toca no socket, então funciona com o
 * daemon fora do ar — é o que alimenta a tela de daemon offline.
 */
export async function fetchSocketInfo(): Promise<SocketInfo> {
  return decode(SocketInfoSchema, await call('daemon_socket_path'), 'daemon_socket_path');
}

/** `GET /v1/health`. */
export async function fetchHealth(): Promise<HealthResponse> {
  return decode(HealthResponseSchema, await call('daemon_health'), 'GET /v1/health');
}

/** `GET /v1/status`. */
export async function fetchStatus(): Promise<DaemonStatus> {
  return decode(DaemonStatusSchema, await call('daemon_status'), 'GET /v1/status');
}

/**
 * `GET /v1/hardware`, ou `POST /v1/hardware/refresh` quando `refresh`.
 *
 * Re-detectar é caro (todas as sondas rodam de novo), por isso é sempre uma
 * ação explícita do usuário, nunca um efeito de montagem.
 */
export async function fetchHardware(refresh: boolean): Promise<HardwareResponse> {
  const raw = await call('hardware_profile', { refresh });
  return decode(
    HardwareResponseSchema,
    raw,
    refresh ? 'POST /v1/hardware/refresh' : 'GET /v1/hardware',
  );
}

/** `GET /v1/hardware/history`. */
export async function fetchHardwareHistory(limit: number): Promise<HardwareHistoryEntry[]> {
  const raw = await call('hardware_history', { limit });
  return decode(HardwareHistorySchema, raw, 'GET /v1/hardware/history');
}

/** `GET /v1/resources`. */
export async function fetchResources(): Promise<ResourcesResponse> {
  return decode(ResourcesResponseSchema, await call('resources_latest'), 'GET /v1/resources');
}
