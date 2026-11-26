import { describe, expect, it } from 'vitest';

import { parseOrThrow, SchemaMismatchError } from '../src/parse.js';
import { EventEnvelopeSchema, EventSchema, ResourceSnapshotSchema } from '../src/telemetry.js';

const snapshot = {
  takenAt: '2026-07-27T09:15:00Z',
  intervalMs: 1000,
  cpu: {
    totalPercent: 23.5,
    perCorePercent: [20, 25, 22, 27],
    loadAverage: [0.8, 0.6, 0.4],
  },
  memory: {
    totalMb: 32000,
    usedMb: 8000,
    availableMb: 24000,
    cachedMb: 9000,
    usedPercent: 25,
    swapUsedMb: 0,
    swapTotalMb: 8192,
  },
  gpus: [
    {
      index: 0,
      name: 'NVIDIA GeForce RTX 5060 Ti',
      utilizationPercent: 12.5,
      vramUsedMb: 1200,
      vramTotalMb: 8192,
      vramUsedPercent: 14.6,
      temperatureC: 41,
      powerWatts: 35.5,
    },
  ],
};

describe('ResourceSnapshotSchema', () => {
  it('aceita um snapshot completo', () => {
    const parsed = parseOrThrow(ResourceSnapshotSchema, snapshot, 'GET /v1/resources');
    expect(parsed.intervalMs).toBe(1000n);
    expect(parsed.gpus[0]?.vramUsedMb).toBe(1200n);
  });

  it('rejeita percentual como texto citando o caminho', () => {
    const broken = { ...snapshot, cpu: { ...snapshot.cpu, totalPercent: '23.5' } };

    try {
      parseOrThrow(ResourceSnapshotSchema, broken, 'GET /v1/resources');
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      expect((error as SchemaMismatchError).message).toContain('cpu.totalPercent');
    }
  });
});

describe('EventSchema', () => {
  it('aceita cada variante do catálogo atual', () => {
    const variants: unknown[] = [
      {
        type: 'daemon.started',
        version: '0.1.0',
        pid: 1234,
        socketPath: '/run/user/1000/elara/agentd.sock',
      },
      { type: 'daemon.stopping', reason: 'SIGTERM' },
      {
        type: 'hardware.detected',
        fingerprint: 'b3:abc',
        probeDurationMs: 42,
        warningCount: 1,
      },
      { type: 'hardware.probe-failed', error: '/proc não montado' },
      { type: 'resources.sampled', snapshot },
      {
        type: 'component.state-changed',
        component: 'agent-runtime',
        from: 'starting',
        to: 'ready',
        detail: null,
      },
    ];

    for (const variant of variants) {
      expect(() => parseOrThrow(EventSchema, variant, 'SSE')).not.toThrow();
    }
  });

  it('discrimina pelo campo type: payload da variante errada é recusado', () => {
    // `reason` pertence a daemon.stopping; sob a tag daemon.started falta
    // `version`, e é isso que a mensagem tem de dizer.
    const misrouted = { type: 'daemon.started', reason: 'SIGTERM' };

    try {
      parseOrThrow(EventSchema, misrouted, 'SSE');
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      expect((error as SchemaMismatchError).message).toContain('version');
    }
  });

  it('recusa um tipo de evento desconhecido', () => {
    expect(() => parseOrThrow(EventSchema, { type: 'model.loaded' }, 'SSE')).toThrow(
      SchemaMismatchError,
    );
  });
});

describe('EventEnvelopeSchema', () => {
  it('converte sequence para bigint', () => {
    const envelope = parseOrThrow(
      EventEnvelopeSchema,
      {
        sequence: 17,
        emittedAt: '2026-07-27T09:15:00Z',
        payload: { type: 'daemon.stopping', reason: 'SIGTERM' },
      },
      'SSE',
    );

    expect(envelope.sequence).toBe(17n);
  });

  it('recusa sequence acima do inteiro seguro em vez de arredondar', () => {
    // Depois do JSON.parse o dígito já se perdeu; converter aqui produziria um
    // número plausível e errado.
    expect(() =>
      parseOrThrow(
        EventEnvelopeSchema,
        {
          sequence: Number.MAX_SAFE_INTEGER + 1,
          emittedAt: '2026-07-27T09:15:00Z',
          payload: { type: 'daemon.stopping', reason: 'SIGTERM' },
        },
        'SSE',
      ),
    ).toThrow(SchemaMismatchError);
  });

  it('aceita sequence em texto decimal, que preserva a precisão', () => {
    const envelope = parseOrThrow(
      EventEnvelopeSchema,
      {
        sequence: '9007199254740993',
        emittedAt: '2026-07-27T09:15:00Z',
        payload: { type: 'daemon.stopping', reason: 'SIGTERM' },
      },
      'SSE',
    );

    expect(envelope.sequence).toBe(9_007_199_254_740_993n);
  });
});
