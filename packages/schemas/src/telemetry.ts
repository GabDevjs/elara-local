/**
 * Schemas zod da telemetria: eventos do bus e snapshots de recursos.
 *
 * Espelham `crates/telemetry/src/types.rs`. O `Event` é o ponto mais delicado:
 * no Rust ele é `#[serde(tag = "type")]`, então em zod tem de ser
 * `discriminatedUnion` — uma `union` comum aceitaria um payload de
 * `hardware.detected` rotulado como `daemon.started` e só falharia depois.
 */
import { z } from 'zod';

import type { ComponentState } from './generated/ComponentState.js';
import type { ComponentStatus } from './generated/ComponentStatus.js';
import type { CpuUsage } from './generated/CpuUsage.js';
import type { Event } from './generated/Event.js';
import type { EventEnvelope } from './generated/EventEnvelope.js';
import type { GpuUsage } from './generated/GpuUsage.js';
import type { MemoryUsage } from './generated/MemoryUsage.js';
import type { ResourceSnapshot } from './generated/ResourceSnapshot.js';
import { f64, loadAverage, timestamp, u32, u64 } from './primitives.js';
import { assertSchemaMatches } from './type-assert.js';

// ---------------------------------------------------------------------------
// Estado de componentes
// ---------------------------------------------------------------------------

/** Ciclo de vida de um componente supervisionado pelo daemon. */
export const ComponentStateSchema = z.enum([
  'stopped',
  'starting',
  'ready',
  'degraded',
  'failed',
  'unavailable',
]);
assertSchemaMatches<ComponentState, z.infer<typeof ComponentStateSchema>>();

/** Estado reportado de um componente. */
export const ComponentStatusSchema = z.object({
  name: z.string(),
  state: ComponentStateSchema,
  detail: z.string().nullable(),
  since: timestamp,
  restarts: u32,
});
assertSchemaMatches<ComponentStatus, z.infer<typeof ComponentStatusSchema>>();

// ---------------------------------------------------------------------------
// Snapshot de recursos
// ---------------------------------------------------------------------------

/** Uso de CPU no intervalo. */
export const CpuUsageSchema = z.object({
  totalPercent: f64,
  perCorePercent: z.array(f64),
  loadAverage,
});
assertSchemaMatches<CpuUsage, z.infer<typeof CpuUsageSchema>>();

/** Uso de memória no instante da amostra. */
export const MemoryUsageSchema = z.object({
  totalMb: u64,
  usedMb: u64,
  availableMb: u64,
  cachedMb: u64,
  usedPercent: f64,
  swapUsedMb: u64,
  swapTotalMb: u64,
});
assertSchemaMatches<MemoryUsage, z.infer<typeof MemoryUsageSchema>>();

/** Uso de uma GPU no instante da amostra. */
export const GpuUsageSchema = z.object({
  index: u32,
  name: z.string(),
  utilizationPercent: f64.nullable(),
  vramUsedMb: u64.nullable(),
  vramTotalMb: u64.nullable(),
  vramUsedPercent: f64.nullable(),
  temperatureC: f64.nullable(),
  powerWatts: f64.nullable(),
});
assertSchemaMatches<GpuUsage, z.infer<typeof GpuUsageSchema>>();

/** Uso de recursos já convertido em percentuais. */
export const ResourceSnapshotSchema = z.object({
  takenAt: timestamp,
  intervalMs: u64,
  cpu: CpuUsageSchema,
  memory: MemoryUsageSchema,
  gpus: z.array(GpuUsageSchema),
});
assertSchemaMatches<ResourceSnapshot, z.infer<typeof ResourceSnapshotSchema>>();

// ---------------------------------------------------------------------------
// Eventos
// ---------------------------------------------------------------------------

/** Evento de domínio, discriminado pelo campo `type`. */
export const EventSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('daemon.started'),
    version: z.string(),
    pid: u32,
    socketPath: z.string(),
  }),
  z.object({
    type: z.literal('daemon.stopping'),
    reason: z.string(),
  }),
  z.object({
    type: z.literal('hardware.detected'),
    fingerprint: z.string(),
    probeDurationMs: u64,
    warningCount: u32,
  }),
  z.object({
    type: z.literal('hardware.probe-failed'),
    error: z.string(),
  }),
  z.object({
    type: z.literal('resources.sampled'),
    snapshot: ResourceSnapshotSchema,
  }),
  z.object({
    type: z.literal('component.state-changed'),
    component: z.string(),
    from: ComponentStateSchema,
    to: ComponentStateSchema,
    detail: z.string().nullable(),
  }),
]);
assertSchemaMatches<Event, z.infer<typeof EventSchema>>();

/** Envelope de um evento publicado no bus e retransmitido por SSE. */
export const EventEnvelopeSchema = z.object({
  sequence: u64,
  emittedAt: timestamp,
  payload: EventSchema,
});
assertSchemaMatches<EventEnvelope, z.infer<typeof EventEnvelopeSchema>>();
