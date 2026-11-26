/**
 * Schemas zod do contrato HTTP do daemon.
 *
 * Espelham `crates/agentd/src/api/types.rs`. É esta camada que transforma
 * "a UI nova está falando com um daemon velho" em uma mensagem citando o campo
 * exato, em vez de um `undefined` propagando até a tela.
 */
import { z } from 'zod';

import type { ApiError } from './generated/ApiError.js';
import type { DaemonStatus } from './generated/DaemonStatus.js';
import type { HardwareHistoryEntry } from './generated/HardwareHistoryEntry.js';
import type { HardwareResponse } from './generated/HardwareResponse.js';
import type { HealthResponse } from './generated/HealthResponse.js';
import type { ResourcesResponse } from './generated/ResourcesResponse.js';
import type { RuntimeHeartbeatRequest } from './generated/RuntimeHeartbeatRequest.js';
import type { RuntimeHeartbeatResponse } from './generated/RuntimeHeartbeatResponse.js';
import type { RuntimeRegisterRequest } from './generated/RuntimeRegisterRequest.js';
import type { RuntimeRegisterResponse } from './generated/RuntimeRegisterResponse.js';
import { HardwareProfileSchema } from './hardware.js';
import { timestamp, u32, u64 } from './primitives.js';
import { ComponentStatusSchema, ResourceSnapshotSchema } from './telemetry.js';
import { assertSchemaMatches } from './type-assert.js';

/**
 * Versão do contrato HTTP falada por este pacote.
 *
 * Espelha `PROTOCOL_VERSION` do Rust. Está aqui, e não no SDK, porque o
 * desktop também precisa dela para exibir o motivo de uma recusa.
 */
export const PROTOCOL_VERSION = 1;

/** Header em que cliente e servidor anunciam a versão do protocolo. */
export const PROTOCOL_HEADER = 'x-elara-protocol';

// ---------------------------------------------------------------------------
// Erros
// ---------------------------------------------------------------------------

/** Corpo de toda resposta de erro do daemon. */
export const ApiErrorSchema = z.object({
  code: z.string(),
  message: z.string(),
  details: z.string().nullable(),
});
assertSchemaMatches<ApiError, z.infer<typeof ApiErrorSchema>>();

// ---------------------------------------------------------------------------
// Saúde e status
// ---------------------------------------------------------------------------

/** Resposta de `GET /v1/health`. */
export const HealthResponseSchema = z.object({
  ok: z.boolean(),
  version: z.string(),
  protocolVersion: u32,
  uptimeSeconds: u64,
});
assertSchemaMatches<HealthResponse, z.infer<typeof HealthResponseSchema>>();

/** Resposta de `GET /v1/status`. */
export const DaemonStatusSchema = z.object({
  version: z.string(),
  protocolVersion: u32,
  pid: u32,
  startedAt: timestamp,
  uptimeSeconds: u64,
  socketPath: z.string(),
  dataDir: z.string(),
  components: z.array(ComponentStatusSchema),
});
assertSchemaMatches<DaemonStatus, z.infer<typeof DaemonStatusSchema>>();

// ---------------------------------------------------------------------------
// Hardware
// ---------------------------------------------------------------------------

/** Resposta de `GET /v1/hardware` e `POST /v1/hardware/refresh`. */
export const HardwareResponseSchema = z.object({
  profile: HardwareProfileSchema,
  cached: z.boolean(),
  firstSeenAt: timestamp,
});
assertSchemaMatches<HardwareResponse, z.infer<typeof HardwareResponseSchema>>();

/** Item de `GET /v1/hardware/history`. */
export const HardwareHistoryEntrySchema = z.object({
  fingerprint: z.string(),
  firstSeenAt: timestamp,
  lastSeenAt: timestamp,
  cpuModel: z.string(),
  ramTotalMb: u64,
  gpuSummary: z.string(),
});
assertSchemaMatches<HardwareHistoryEntry, z.infer<typeof HardwareHistoryEntrySchema>>();

/** Corpo de `GET /v1/hardware/history`: a lista inteira. */
export const HardwareHistorySchema = z.array(HardwareHistoryEntrySchema);

// ---------------------------------------------------------------------------
// Recursos
// ---------------------------------------------------------------------------

/** Resposta de `GET /v1/resources`. */
export const ResourcesResponseSchema = z.object({
  snapshot: ResourceSnapshotSchema.nullable(),
  sampleIntervalMs: u64,
});
assertSchemaMatches<ResourcesResponse, z.infer<typeof ResourcesResponseSchema>>();

// ---------------------------------------------------------------------------
// Registro do agent-runtime
// ---------------------------------------------------------------------------

/**
 * Corpo de `POST /v1/runtime/register`.
 *
 * `pid` e `protocolVersion` são `u32`, não `u64`: o corpo é serializado com
 * `JSON.stringify`, que rejeita `bigint`. Vale conferir isto antes de mudar
 * qualquer campo de requisição para inteiro largo.
 */
export const RuntimeRegisterRequestSchema = z.object({
  name: z.string(),
  version: z.string(),
  pid: u32,
  protocolVersion: u32,
  capabilities: z.array(z.string()),
});
assertSchemaMatches<RuntimeRegisterRequest, z.infer<typeof RuntimeRegisterRequestSchema>>();

/** Resposta de `POST /v1/runtime/register`. */
export const RuntimeRegisterResponseSchema = z.object({
  sessionId: z.string(),
  heartbeatIntervalMs: u64,
  daemonVersion: z.string(),
});
assertSchemaMatches<RuntimeRegisterResponse, z.infer<typeof RuntimeRegisterResponseSchema>>();

/** Corpo de `POST /v1/runtime/heartbeat`. */
export const RuntimeHeartbeatRequestSchema = z.object({
  sessionId: z.string(),
  healthy: z.boolean(),
  detail: z.string().nullable(),
});
assertSchemaMatches<RuntimeHeartbeatRequest, z.infer<typeof RuntimeHeartbeatRequestSchema>>();

/** Resposta de `POST /v1/runtime/heartbeat`. */
export const RuntimeHeartbeatResponseSchema = z.object({
  acknowledged: z.boolean(),
  shutdownRequested: z.boolean(),
});
assertSchemaMatches<RuntimeHeartbeatResponse, z.infer<typeof RuntimeHeartbeatResponseSchema>>();
