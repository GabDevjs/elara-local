/**
 * Schemas zod do perfil de hardware.
 *
 * Espelham, campo a campo, os tipos gerados de `crates/hardware/src/types.rs`.
 * Cada schema é seguido de um `assertSchemaMatches`: é o que impede as duas
 * descrições de divergirem sem ninguém notar. Se o `pnpm typecheck` quebrar
 * aqui, o Rust mudou — o schema é que precisa acompanhar, nunca o contrário.
 *
 * Detalhe recorrente: `Option<T>` do Rust vira `T | null` com a chave
 * **presente**. Usar `.optional()` deixaria passar um payload em que o daemon
 * simplesmente esqueceu o campo, que é exatamente o bug que queremos pegar.
 */
import { z } from 'zod';

import type { BandwidthSource } from './generated/BandwidthSource.js';
import type { CgroupLimits } from './generated/CgroupLimits.js';
import type { CgroupVersion } from './generated/CgroupVersion.js';
import type { CpuCache } from './generated/CpuCache.js';
import type { CpuFeatures } from './generated/CpuFeatures.js';
import type { CpuProfile } from './generated/CpuProfile.js';
import type { CpuTimes } from './generated/CpuTimes.js';
import type { CpuVendor } from './generated/CpuVendor.js';
import type { EffectiveResources } from './generated/EffectiveResources.js';
import type { GpuBackend } from './generated/GpuBackend.js';
import type { GpuProbeSource } from './generated/GpuProbeSource.js';
import type { GpuProfile } from './generated/GpuProfile.js';
import type { GpuSample } from './generated/GpuSample.js';
import type { GpuVendor } from './generated/GpuVendor.js';
import type { HardwareProfile } from './generated/HardwareProfile.js';
import type { HardwareTopology } from './generated/HardwareTopology.js';
import type { HostProfile } from './generated/HostProfile.js';
import type { HybridTopology } from './generated/HybridTopology.js';
import type { LiveSample } from './generated/LiveSample.js';
import type { MemorySample } from './generated/MemorySample.js';
import type { MountPoint } from './generated/MountPoint.js';
import type { NumaNode } from './generated/NumaNode.js';
import type { ProbeWarning } from './generated/ProbeWarning.js';
import type { RamProfile } from './generated/RamProfile.js';
import type { StorageKind } from './generated/StorageKind.js';
import type { StorageProfile } from './generated/StorageProfile.js';
import type { Virtualization } from './generated/Virtualization.js';
import type { WarningSeverity } from './generated/WarningSeverity.js';
import { f64, loadAverage, timestamp, u32, u64 } from './primitives.js';
import { assertSchemaMatches } from './type-assert.js';

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

/** Ambiente virtualizado ou conteinerizado detectado. */
export const VirtualizationSchema = z.enum([
  'kvm',
  'xen',
  'vm-ware',
  'hyper-v',
  'virtual-box',
  'wsl',
  'docker',
  'podman',
  'lxc',
  'unknown',
]);
assertSchemaMatches<Virtualization, z.infer<typeof VirtualizationSchema>>();

/** Identificação do sistema operacional hospedeiro. */
export const HostProfileSchema = z.object({
  hostname: z.string().nullable(),
  os: z.string(),
  distribution: z.string().nullable(),
  kernelVersion: z.string().nullable(),
  architecture: z.string(),
  virtualization: VirtualizationSchema.nullable(),
});
assertSchemaMatches<HostProfile, z.infer<typeof HostProfileSchema>>();

// ---------------------------------------------------------------------------
// CPU
// ---------------------------------------------------------------------------

/** Fabricante da CPU. */
export const CpuVendorSchema = z.enum(['amd', 'intel', 'arm', 'apple', 'other']);
assertSchemaMatches<CpuVendor, z.infer<typeof CpuVendorSchema>>();

/** Tamanhos de cache por nível, em KiB. */
export const CpuCacheSchema = z.object({
  l1dKb: u32.nullable(),
  l1iKb: u32.nullable(),
  l2Kb: u32.nullable(),
  l3Kb: u32.nullable(),
});
assertSchemaMatches<CpuCache, z.infer<typeof CpuCacheSchema>>();

/** Extensões de instrução relevantes para inferência em CPU. */
export const CpuFeaturesSchema = z.object({
  sse3: z.boolean(),
  ssse3: z.boolean(),
  sse42: z.boolean(),
  avx: z.boolean(),
  avx2: z.boolean(),
  fma: z.boolean(),
  f16c: z.boolean(),
  avx512f: z.boolean(),
  avx512Vnni: z.boolean(),
  avx512Bf16: z.boolean(),
  amxInt8: z.boolean(),
  amxBf16: z.boolean(),
  neon: z.boolean(),
  dotprod: z.boolean(),
  i8mm: z.boolean(),
  sve: z.boolean(),
});
assertSchemaMatches<CpuFeatures, z.infer<typeof CpuFeaturesSchema>>();

/** Distribuição de núcleos em CPUs híbridas. */
export const HybridTopologySchema = z.object({
  performanceCores: u32,
  efficiencyCores: u32,
});
assertSchemaMatches<HybridTopology, z.infer<typeof HybridTopologySchema>>();

/** Descrição da CPU. */
export const CpuProfileSchema = z.object({
  model: z.string(),
  vendor: CpuVendorSchema,
  architecture: z.string(),
  physicalCores: u32,
  logicalThreads: u32,
  onlineThreads: u32,
  sockets: u32,
  baseFrequencyMhz: u32.nullable(),
  maxFrequencyMhz: u32.nullable(),
  cache: CpuCacheSchema,
  features: CpuFeaturesSchema,
  hybrid: HybridTopologySchema.nullable(),
});
assertSchemaMatches<CpuProfile, z.infer<typeof CpuProfileSchema>>();

// ---------------------------------------------------------------------------
// Memória
// ---------------------------------------------------------------------------

/** Origem do número de largura de banda de memória. */
export const BandwidthSourceSchema = z.enum(['not-measured', 'measured']);
assertSchemaMatches<BandwidthSource, z.infer<typeof BandwidthSourceSchema>>();

/** Memória do sistema. */
export const RamProfileSchema = z.object({
  totalMb: u64,
  availableMb: u64,
  freeMb: u64,
  swapTotalMb: u64,
  swapFreeMb: u64,
  measuredBandwidthGbps: f64.nullable(),
  bandwidthSource: BandwidthSourceSchema,
});
assertSchemaMatches<RamProfile, z.infer<typeof RamProfileSchema>>();

// ---------------------------------------------------------------------------
// GPU
// ---------------------------------------------------------------------------

/** Fabricante da GPU. */
export const GpuVendorSchema = z.enum(['nvidia', 'amd', 'intel', 'apple', 'unknown']);
assertSchemaMatches<GpuVendor, z.infer<typeof GpuVendorSchema>>();

/** Backend de aceleração utilizável. */
export const GpuBackendSchema = z.enum(['cuda', 'rocm', 'vulkan', 'metal', 'sycl', 'opencl']);
assertSchemaMatches<GpuBackend, z.infer<typeof GpuBackendSchema>>();

/** Sonda que produziu os dados da GPU. */
export const GpuProbeSourceSchema = z.enum(['nvml', 'nvidia-smi', 'sysfs']);
assertSchemaMatches<GpuProbeSource, z.infer<typeof GpuProbeSourceSchema>>();

/** Uma GPU detectada. */
export const GpuProfileSchema = z.object({
  index: u32,
  vendor: GpuVendorSchema,
  model: z.string(),
  pciAddress: z.string().nullable(),
  vramTotalMb: u64.nullable(),
  vramAvailableMb: u64.nullable(),
  driverVersion: z.string().nullable(),
  runtimeVersion: z.string().nullable(),
  computeCapability: z.string().nullable(),
  backends: z.array(GpuBackendSchema),
  isIntegrated: z.boolean(),
  powerLimitWatts: f64.nullable(),
  source: GpuProbeSourceSchema,
});
assertSchemaMatches<GpuProfile, z.infer<typeof GpuProfileSchema>>();

// ---------------------------------------------------------------------------
// Armazenamento
// ---------------------------------------------------------------------------

/** Classe física do dispositivo de armazenamento. */
export const StorageKindSchema = z.enum(['nvme', 'ssd', 'hdd', 'unknown']);
assertSchemaMatches<StorageKind, z.infer<typeof StorageKindSchema>>();

/** Um ponto de montagem e seu espaço livre. */
export const MountPointSchema = z.object({
  path: z.string(),
  filesystem: z.string(),
  totalMb: u64,
  availableMb: u64,
  readOnly: z.boolean(),
});
assertSchemaMatches<MountPoint, z.infer<typeof MountPointSchema>>();

/** Um dispositivo de bloco e onde ele está montado. */
export const StorageProfileSchema = z.object({
  device: z.string(),
  model: z.string().nullable(),
  kind: StorageKindSchema,
  sizeMb: u64,
  rotational: z.boolean().nullable(),
  mounts: z.array(MountPointSchema),
  measuredReadMbps: f64.nullable(),
});
assertSchemaMatches<StorageProfile, z.infer<typeof StorageProfileSchema>>();

// ---------------------------------------------------------------------------
// Topologia e limites
// ---------------------------------------------------------------------------

/** Versão da hierarquia de cgroups em uso. */
export const CgroupVersionSchema = z.enum(['none', 'v1', 'v2']);
assertSchemaMatches<CgroupVersion, z.infer<typeof CgroupVersionSchema>>();

/** Restrições do cgroup que contém o daemon. */
export const CgroupLimitsSchema = z.object({
  version: CgroupVersionSchema,
  cpuQuotaCores: f64.nullable(),
  memoryLimitMb: u64.nullable(),
  memoryUsageMb: u64.nullable(),
});
assertSchemaMatches<CgroupLimits, z.infer<typeof CgroupLimitsSchema>>();

/** Um nó NUMA e os recursos ligados a ele. */
export const NumaNodeSchema = z.object({
  id: u32,
  cpus: z.array(u32),
  memoryMb: u64.nullable(),
});
assertSchemaMatches<NumaNode, z.infer<typeof NumaNodeSchema>>();

/** Layout NUMA, SMT e limites de cgroup. */
export const HardwareTopologySchema = z.object({
  numaNodes: z.array(NumaNodeSchema),
  simultaneousMultithreading: z.boolean(),
  cgroup: CgroupLimitsSchema,
});
assertSchemaMatches<HardwareTopology, z.infer<typeof HardwareTopologySchema>>();

/** Orçamento efetivamente utilizável pelo Elara. */
export const EffectiveResourcesSchema = z.object({
  cpuThreads: u32,
  ramMb: u64,
  vramMb: u64,
  vramHeadroomRatio: f64,
  rationale: z.array(z.string()),
});
assertSchemaMatches<EffectiveResources, z.infer<typeof EffectiveResourcesSchema>>();

// ---------------------------------------------------------------------------
// Diagnóstico
// ---------------------------------------------------------------------------

/** Gravidade de um aviso de sonda. */
export const WarningSeveritySchema = z.enum(['info', 'warning', 'error']);
assertSchemaMatches<WarningSeverity, z.infer<typeof WarningSeveritySchema>>();

/** Registro de uma sonda que não entregou dados completos. */
export const ProbeWarningSchema = z.object({
  source: z.string(),
  severity: WarningSeveritySchema,
  message: z.string(),
});
assertSchemaMatches<ProbeWarning, z.infer<typeof ProbeWarningSchema>>();

// ---------------------------------------------------------------------------
// Raiz
// ---------------------------------------------------------------------------

/** Retrato completo da máquina em um instante. */
export const HardwareProfileSchema = z.object({
  schemaVersion: u32,
  fingerprint: z.string(),
  collectedAt: timestamp,
  probeDurationMs: u64,
  host: HostProfileSchema,
  cpu: CpuProfileSchema,
  ram: RamProfileSchema,
  gpus: z.array(GpuProfileSchema),
  storage: z.array(StorageProfileSchema),
  topology: HardwareTopologySchema,
  effective: EffectiveResourcesSchema,
  warnings: z.array(ProbeWarningSchema),
});
assertSchemaMatches<HardwareProfile, z.infer<typeof HardwareProfileSchema>>();

// ---------------------------------------------------------------------------
// Amostragem ao vivo
// ---------------------------------------------------------------------------

/** Tempo acumulado de CPU, em jiffies. */
export const CpuTimesSchema = z.object({
  user: u64,
  nice: u64,
  system: u64,
  idle: u64,
  iowait: u64,
  irq: u64,
  softirq: u64,
  steal: u64,
});
assertSchemaMatches<CpuTimes, z.infer<typeof CpuTimesSchema>>();

/** Estado instantâneo da memória do sistema. */
export const MemorySampleSchema = z.object({
  totalMb: u64,
  availableMb: u64,
  freeMb: u64,
  cachedMb: u64,
  swapTotalMb: u64,
  swapFreeMb: u64,
});
assertSchemaMatches<MemorySample, z.infer<typeof MemorySampleSchema>>();

/** Leitura instantânea de uma GPU. */
export const GpuSampleSchema = z.object({
  index: u32,
  utilizationPercent: f64.nullable(),
  memoryUtilizationPercent: f64.nullable(),
  vramUsedMb: u64.nullable(),
  vramTotalMb: u64.nullable(),
  temperatureC: f64.nullable(),
  powerWatts: f64.nullable(),
  clockMhz: u32.nullable(),
});
assertSchemaMatches<GpuSample, z.infer<typeof GpuSampleSchema>>();

/** Contadores brutos de um instante. */
export const LiveSampleSchema = z.object({
  takenAt: timestamp,
  cpuTotal: CpuTimesSchema,
  cpuPerCore: z.array(CpuTimesSchema),
  loadAverage,
  memory: MemorySampleSchema,
  gpus: z.array(GpuSampleSchema),
});
assertSchemaMatches<LiveSample, z.infer<typeof LiveSampleSchema>>();
