/**
 * Contrato compartilhado entre o daemon Rust e o TypeScript.
 *
 * Reexporta três coisas que sempre andam juntas: os **tipos** gerados do Rust
 * por `ts-rs`, os **schemas** zod que os validam em runtime, e o
 * [`parseOrThrow`] que aplica os segundos aos primeiros na fronteira IPC.
 */

// --- Tipos gerados (fonte da verdade, não editar à mão) ---------------------

export type { BandwidthSource } from './generated/BandwidthSource.js';
export type { CgroupLimits } from './generated/CgroupLimits.js';
export type { CgroupVersion } from './generated/CgroupVersion.js';
export type { CpuCache } from './generated/CpuCache.js';
export type { CpuFeatures } from './generated/CpuFeatures.js';
export type { CpuProfile } from './generated/CpuProfile.js';
export type { CpuTimes } from './generated/CpuTimes.js';
export type { CpuVendor } from './generated/CpuVendor.js';
export type { EffectiveResources } from './generated/EffectiveResources.js';
export type { GpuBackend } from './generated/GpuBackend.js';
export type { GpuProbeSource } from './generated/GpuProbeSource.js';
export type { GpuProfile } from './generated/GpuProfile.js';
export type { GpuSample } from './generated/GpuSample.js';
export type { GpuVendor } from './generated/GpuVendor.js';
export type { HardwareProfile } from './generated/HardwareProfile.js';
export type { HardwareTopology } from './generated/HardwareTopology.js';
export type { HostProfile } from './generated/HostProfile.js';
export type { HybridTopology } from './generated/HybridTopology.js';
export type { LiveSample } from './generated/LiveSample.js';
export type { MemorySample } from './generated/MemorySample.js';
export type { MountPoint } from './generated/MountPoint.js';
export type { NumaNode } from './generated/NumaNode.js';
export type { ProbeWarning } from './generated/ProbeWarning.js';
export type { RamProfile } from './generated/RamProfile.js';
export type { StorageKind } from './generated/StorageKind.js';
export type { StorageProfile } from './generated/StorageProfile.js';
export type { Virtualization } from './generated/Virtualization.js';
export type { WarningSeverity } from './generated/WarningSeverity.js';

export type { ComponentState } from './generated/ComponentState.js';
export type { ComponentStatus } from './generated/ComponentStatus.js';
export type { CpuUsage } from './generated/CpuUsage.js';
export type { Event } from './generated/Event.js';
export type { EventEnvelope } from './generated/EventEnvelope.js';
export type { GpuUsage } from './generated/GpuUsage.js';
export type { MemoryUsage } from './generated/MemoryUsage.js';
export type { ResourceSnapshot } from './generated/ResourceSnapshot.js';

export type { ApiError } from './generated/ApiError.js';
export type { DaemonStatus } from './generated/DaemonStatus.js';
export type { HardwareHistoryEntry } from './generated/HardwareHistoryEntry.js';
export type { HardwareResponse } from './generated/HardwareResponse.js';
export type { HealthResponse } from './generated/HealthResponse.js';
export type { ResourcesResponse } from './generated/ResourcesResponse.js';
export type { RuntimeHeartbeatRequest } from './generated/RuntimeHeartbeatRequest.js';
export type { RuntimeHeartbeatResponse } from './generated/RuntimeHeartbeatResponse.js';
export type { RuntimeRegisterRequest } from './generated/RuntimeRegisterRequest.js';
export type { RuntimeRegisterResponse } from './generated/RuntimeRegisterResponse.js';

// --- Schemas zod ------------------------------------------------------------

export * from './hardware.js';
export * from './telemetry.js';
export * from './api.js';

// --- Ferramentas de fronteira ----------------------------------------------

export { f64, loadAverage, timestamp, u32, u64 } from './primitives.js';
export { formatPath, parseOrThrow, SchemaMismatchError } from './parse.js';
export { assertSchemaMatches } from './type-assert.js';
export type { ContractMismatch, Equals } from './type-assert.js';
