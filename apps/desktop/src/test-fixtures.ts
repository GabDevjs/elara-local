/**
 * Payloads de exemplo para os testes.
 *
 * São o JSON **cru**, como o daemon o entrega — números onde o tipo gerado diz
 * `bigint`, chaves em camelCase, `null` explícito em todo campo opcional. Usar
 * o objeto já validado aqui esconderia justamente o que os testes precisam
 * exercitar: a passagem pelo zod.
 *
 * A segunda GPU tem todos os campos de métrica nulos de propósito. É o caso
 * que a interface mais erra: uma iGPU sem sonda de utilização não é uma GPU a
 * 0%.
 */

/** Resposta de `daemon_socket_path`. */
export const socketPayload = {
  socketPath: '/run/user/1000/elara/agentd.sock',
  source: 'xdg-runtime-dir',
};

/** Resposta de `GET /v1/status`. */
export const statusPayload = {
  version: '0.1.0',
  protocolVersion: 1,
  pid: 48213,
  startedAt: '2026-07-27T09:15:00Z',
  uptimeSeconds: 5296,
  socketPath: '/run/user/1000/elara/agentd.sock',
  dataDir: '/home/dev/.local/share/elara',
  components: [
    {
      name: 'hardware',
      state: 'ready',
      detail: null,
      since: '2026-07-27T09:15:01Z',
      restarts: 0,
    },
    {
      name: 'telemetry',
      state: 'ready',
      detail: null,
      since: '2026-07-27T09:15:01Z',
      restarts: 0,
    },
    {
      name: 'agent-runtime',
      state: 'degraded',
      detail: 'o runtime respondeu ao heartbeat com healthy=false',
      since: '2026-07-27T10:02:44Z',
      restarts: 2,
    },
  ],
};

/** Resposta de `GET /v1/resources`. */
export const resourcesPayload = {
  snapshot: {
    takenAt: '2026-07-27T10:43:00Z',
    intervalMs: 1000,
    cpu: {
      totalPercent: 42.5,
      perCorePercent: [12.5, 91.25, 30, 36.25],
      loadAverage: [1.24, 0.98, 0.71],
    },
    memory: {
      totalMb: 32_022,
      usedMb: 12_048,
      availableMb: 19_974,
      cachedMb: 6144,
      usedPercent: 37.6,
      swapUsedMb: 0,
      swapTotalMb: 8192,
    },
    gpus: [
      {
        index: 0,
        name: 'NVIDIA GeForce RTX 5060 Ti',
        utilizationPercent: 63,
        vramUsedMb: 4096,
        vramTotalMb: 8192,
        vramUsedPercent: 50,
        temperatureC: 61,
        powerWatts: 118.5,
      },
      {
        index: 1,
        name: 'Intel UHD Graphics 770',
        utilizationPercent: null,
        vramUsedMb: null,
        vramTotalMb: null,
        vramUsedPercent: null,
        temperatureC: null,
        powerWatts: null,
      },
    ],
  },
  sampleIntervalMs: 1000,
};

/** Extensões de CPU de uma máquina x86 moderna sem AVX-512. */
const cpuFeatures = {
  sse3: true,
  ssse3: true,
  sse42: true,
  avx: true,
  avx2: true,
  fma: true,
  f16c: true,
  avx512f: false,
  avx512Vnni: false,
  avx512Bf16: false,
  amxInt8: false,
  amxBf16: false,
  neon: false,
  dotprod: false,
  i8mm: false,
  sve: false,
};

/** Resposta de `GET /v1/hardware`. */
export const hardwarePayload = {
  profile: {
    schemaVersion: 1,
    fingerprint: 'b3:7f41c9a2d5e80613',
    collectedAt: '2026-07-27T09:15:01Z',
    probeDurationMs: 137,
    host: {
      hostname: 'workstation',
      os: 'linux',
      distribution: 'Pop!_OS 24.04 LTS',
      kernelVersion: '7.0.11-76070011-generic',
      architecture: 'x86_64',
      virtualization: null,
    },
    cpu: {
      model: 'AMD Ryzen 5 5600X 6-Core Processor',
      vendor: 'amd',
      architecture: 'x86_64',
      physicalCores: 2,
      logicalThreads: 4,
      onlineThreads: 4,
      sockets: 1,
      baseFrequencyMhz: 3700,
      maxFrequencyMhz: 4650,
      cache: { l1dKb: 192, l1iKb: 192, l2Kb: 3072, l3Kb: null },
      features: cpuFeatures,
      hybrid: null,
    },
    ram: {
      totalMb: 32_022,
      availableMb: 19_974,
      freeMb: 13_830,
      swapTotalMb: 8192,
      swapFreeMb: 8192,
      measuredBandwidthGbps: null,
      bandwidthSource: 'not-measured',
    },
    gpus: [
      {
        index: 0,
        vendor: 'nvidia',
        model: 'NVIDIA GeForce RTX 5060 Ti',
        pciAddress: '0000:01:00.0',
        vramTotalMb: 8192,
        vramAvailableMb: 4096,
        driverVersion: '580.173.02',
        runtimeVersion: '12.4',
        computeCapability: '12.0',
        backends: ['cuda', 'vulkan'],
        isIntegrated: false,
        powerLimitWatts: 180,
        source: 'nvml',
      },
      {
        index: 1,
        vendor: 'intel',
        model: 'Intel UHD Graphics 770',
        pciAddress: '0000:00:02.0',
        vramTotalMb: null,
        vramAvailableMb: null,
        driverVersion: null,
        runtimeVersion: null,
        computeCapability: null,
        backends: [],
        isIntegrated: true,
        powerLimitWatts: null,
        source: 'sysfs',
      },
    ],
    storage: [
      {
        device: 'nvme0n1',
        model: 'Samsung SSD 990 PRO 2TB',
        kind: 'nvme',
        sizeMb: 1_907_729,
        rotational: false,
        mounts: [
          {
            path: '/',
            filesystem: 'ext4',
            totalMb: 1_845_120,
            availableMb: 1_204_224,
            readOnly: false,
          },
        ],
        measuredReadMbps: null,
      },
    ],
    topology: {
      numaNodes: [{ id: 0, cpus: [0, 1, 2, 3], memoryMb: 32_022 }],
      simultaneousMultithreading: true,
      cgroup: {
        version: 'v2',
        cpuQuotaCores: null,
        memoryLimitMb: null,
        memoryUsageMb: 2048,
      },
    },
    effective: {
      cpuThreads: 3,
      ramMb: 24_576,
      vramMb: 6963,
      vramHeadroomRatio: 0.15,
      rationale: [
        'CPU: 4 threads online menos 1 reservada ao desktop e ao daemon.',
        'RAM: 32.022 MiB totais menos 20% de reserva para o sistema.',
        'VRAM: 8.192 MiB da GPU 0 menos 15% de folga para o compositor.',
      ],
    },
    warnings: [
      {
        source: 'gpu.nvml',
        severity: 'warning',
        message: 'NVML não expõe métricas da GPU 1: iGPU Intel só tem dados de sysfs.',
      },
      {
        source: 'cpu.cache',
        severity: 'info',
        message: 'L3 não exposto em /sys/devices/system/cpu/cpu0/cache.',
      },
    ],
  },
  cached: true,
  firstSeenAt: '2026-06-02T18:04:12Z',
};

/** Resposta de `GET /v1/hardware/history`. */
export const historyPayload = [
  {
    fingerprint: 'b3:7f41c9a2d5e80613',
    firstSeenAt: '2026-06-02T18:04:12Z',
    lastSeenAt: '2026-07-27T09:15:01Z',
    cpuModel: 'AMD Ryzen 5 5600X 6-Core Processor',
    ramTotalMb: 32_022,
    gpuSummary: 'NVIDIA GeForce RTX 5060 Ti, Intel UHD Graphics 770',
  },
];

/** Falha que a ponte devolve quando o daemon não está atendendo. */
export const unreachableFailure = {
  code: 'daemon.unreachable',
  message:
    'daemon não está atendendo em /run/user/1000/elara/agentd.sock (No such file or directory)',
};
