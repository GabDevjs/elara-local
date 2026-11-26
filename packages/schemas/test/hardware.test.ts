import { describe, expect, it } from 'vitest';

import { HardwareProfileSchema, LiveSampleSchema } from '../src/hardware.js';
import { parseOrThrow, SchemaMismatchError } from '../src/parse.js';

/**
 * Payload equivalente ao que o daemon serializa: `u64` chegam como números
 * JSON, `Option<T>` como `null` com a chave presente.
 */
function validProfile(): unknown {
  return {
    schemaVersion: 1,
    fingerprint: 'b3:0123456789abcdef',
    collectedAt: '2026-07-27T09:15:00Z',
    probeDurationMs: 42,
    host: {
      hostname: 'pop-os',
      os: 'linux',
      distribution: 'Pop!_OS 22.04 LTS',
      kernelVersion: '7.0.11-76070011-generic',
      architecture: 'x86_64',
      virtualization: null,
    },
    cpu: {
      model: 'AMD Ryzen 5 3600',
      vendor: 'amd',
      architecture: 'x86_64',
      physicalCores: 4,
      logicalThreads: 8,
      onlineThreads: 8,
      sockets: 1,
      baseFrequencyMhz: 3600,
      maxFrequencyMhz: null,
      cache: { l1dKb: 128, l1iKb: 128, l2Kb: 2048, l3Kb: 32768 },
      features: {
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
      },
      hybrid: null,
    },
    ram: {
      totalMb: 32000,
      availableMb: 24000,
      freeMb: 12000,
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
        vramAvailableMb: 7600,
        driverVersion: '580.173.02',
        runtimeVersion: '12.4',
        computeCapability: '8.9',
        backends: ['cuda', 'vulkan'],
        isIntegrated: false,
        powerLimitWatts: 180,
        source: 'nvml',
      },
    ],
    storage: [
      {
        device: 'nvme0n1',
        model: 'Samsung SSD 990 PRO',
        kind: 'nvme',
        sizeMb: 1_000_000,
        rotational: false,
        mounts: [
          {
            path: '/',
            filesystem: 'ext4',
            totalMb: 900_000,
            availableMb: 400_000,
            readOnly: false,
          },
        ],
        measuredReadMbps: null,
      },
    ],
    topology: {
      numaNodes: [{ id: 0, cpus: [0, 1, 2, 3, 4, 5, 6, 7], memoryMb: 32000 }],
      simultaneousMultithreading: true,
      cgroup: {
        version: 'v2',
        cpuQuotaCores: null,
        memoryLimitMb: null,
        memoryUsageMb: 1200,
      },
    },
    effective: {
      cpuThreads: 6,
      ramMb: 24000,
      vramMb: 6800,
      vramHeadroomRatio: 0.15,
      rationale: ['6 de 8 threads: 2 reservadas para o desktop'],
    },
    warnings: [
      {
        source: 'storage.smart',
        severity: 'info',
        message: 'smartctl não encontrado; modelo do disco veio do sysfs',
      },
    ],
  };
}

describe('HardwareProfileSchema', () => {
  it('aceita o perfil completo que o daemon emite', () => {
    const profile = parseOrThrow(HardwareProfileSchema, validProfile(), 'GET /v1/hardware');

    expect(profile.cpu.model).toBe('AMD Ryzen 5 3600');
    expect(profile.gpus[0]?.backends).toEqual(['cuda', 'vulkan']);
    expect(profile.warnings).toHaveLength(1);
  });

  it('converte u64 de número JSON para bigint', () => {
    const profile = parseOrThrow(HardwareProfileSchema, validProfile(), 'GET /v1/hardware');

    expect(profile.ram.totalMb).toBe(32_000n);
    expect(profile.probeDurationMs).toBe(42n);
    expect(profile.gpus[0]?.vramTotalMb).toBe(8192n);
  });

  it('descarta chaves desconhecidas em vez de recusar o payload', () => {
    // Um daemon mais novo pode acrescentar campos; recusar por isso deixaria a
    // UI inutilizável sem ganho de segurança nenhum.
    const payload = { ...(validProfile() as Record<string, unknown>), campoDoFuturo: 7 };
    const profile = parseOrThrow(HardwareProfileSchema, payload, 'GET /v1/hardware');

    expect(profile).not.toHaveProperty('campoDoFuturo');
  });

  it('rejeita campo faltando citando o caminho', () => {
    const payload = validProfile() as Record<string, Record<string, unknown>>;
    delete payload.cpu!.physicalCores;

    expect(() => parseOrThrow(HardwareProfileSchema, payload, 'GET /v1/hardware')).toThrow(
      SchemaMismatchError,
    );

    try {
      parseOrThrow(HardwareProfileSchema, payload, 'GET /v1/hardware');
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      const mismatch = error as SchemaMismatchError;
      expect(mismatch.issues.map((issue) => issue.path.join('.'))).toContain('cpu.physicalCores');
      expect(mismatch.message).toContain('GET /v1/hardware');
      expect(mismatch.message).toContain('cpu.physicalCores');
    }
  });

  it('rejeita tipo errado dentro de um array citando o índice', () => {
    const payload = validProfile() as { gpus: { vramTotalMb: unknown }[] };
    payload.gpus[0]!.vramTotalMb = 'oito gigas';

    try {
      parseOrThrow(HardwareProfileSchema, payload, 'GET /v1/hardware');
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      expect((error as SchemaMismatchError).message).toContain('gpus[0].vramTotalMb');
    }
  });

  it('rejeita `Option<T>` ausente — null é obrigatório, não a chave opcional', () => {
    const payload = validProfile() as { host: Record<string, unknown> };
    delete payload.host.virtualization;

    try {
      parseOrThrow(HardwareProfileSchema, payload, 'GET /v1/hardware');
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      expect((error as SchemaMismatchError).message).toContain('host.virtualization');
    }
  });

  it('rejeita enum fora do catálogo', () => {
    const payload = validProfile() as { cpu: Record<string, unknown> };
    payload.cpu.vendor = 'risc-v';

    try {
      parseOrThrow(HardwareProfileSchema, payload, 'GET /v1/hardware');
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      expect((error as SchemaMismatchError).message).toContain('cpu.vendor');
    }
  });
});

describe('LiveSampleSchema', () => {
  const sample = {
    takenAt: '2026-07-27T09:15:00.123456789Z',
    cpuTotal: {
      user: 1,
      nice: 0,
      system: 2,
      idle: 100,
      iowait: 3,
      irq: 0,
      softirq: 0,
      steal: 0,
    },
    cpuPerCore: [],
    loadAverage: [0.5, 0.4, 0.3],
    memory: {
      totalMb: 32000,
      availableMb: 24000,
      freeMb: 12000,
      cachedMb: 9000,
      swapTotalMb: 8192,
      swapFreeMb: 8192,
    },
    gpus: [],
  };

  it('aceita timestamp com precisão de nanossegundos', () => {
    const parsed = parseOrThrow(LiveSampleSchema, sample, 'sampler');
    expect(parsed.takenAt).toBe('2026-07-27T09:15:00.123456789Z');
  });

  it('recusa loadAverage com aridade errada', () => {
    const broken = { ...sample, loadAverage: [0.5, 0.4] };

    try {
      parseOrThrow(LiveSampleSchema, broken, 'sampler');
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      expect((error as SchemaMismatchError).message).toContain('loadAverage');
    }
  });
});
