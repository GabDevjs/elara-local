import { describe, expect, it } from 'vitest';

import {
  ApiErrorSchema,
  DaemonStatusSchema,
  HardwareHistorySchema,
  HealthResponseSchema,
  ResourcesResponseSchema,
  RuntimeHeartbeatResponseSchema,
  RuntimeRegisterRequestSchema,
  RuntimeRegisterResponseSchema,
} from '../src/api.js';
import { formatPath, parseOrThrow, SchemaMismatchError } from '../src/parse.js';

describe('respostas do daemon', () => {
  it('valida GET /v1/health', () => {
    const health = parseOrThrow(
      HealthResponseSchema,
      { ok: true, version: '0.1.0', protocolVersion: 1, uptimeSeconds: 91 },
      'GET /v1/health',
    );

    expect(health.uptimeSeconds).toBe(91n);
  });

  it('valida GET /v1/status com componentes', () => {
    const status = parseOrThrow(
      DaemonStatusSchema,
      {
        version: '0.1.0',
        protocolVersion: 1,
        pid: 4242,
        startedAt: '2026-07-27T09:00:00Z',
        uptimeSeconds: 900,
        socketPath: '/run/user/1000/elara/agentd.sock',
        dataDir: '/home/example/.local/share/elara',
        components: [
          {
            name: 'hardware',
            state: 'ready',
            detail: null,
            since: '2026-07-27T09:00:01Z',
            restarts: 0,
          },
          {
            name: 'agent-runtime',
            state: 'degraded',
            detail: 'heartbeat atrasado',
            since: '2026-07-27T09:10:00Z',
            restarts: 2,
          },
        ],
      },
      'GET /v1/status',
    );

    expect(status.components).toHaveLength(2);
    expect(status.components[1]?.state).toBe('degraded');
  });

  it('valida GET /v1/resources com snapshot ausente', () => {
    const resources = parseOrThrow(
      ResourcesResponseSchema,
      { snapshot: null, sampleIntervalMs: 1000 },
      'GET /v1/resources',
    );

    expect(resources.snapshot).toBeNull();
    expect(resources.sampleIntervalMs).toBe(1000n);
  });

  it('valida GET /v1/hardware/history vazio', () => {
    expect(parseOrThrow(HardwareHistorySchema, [], 'GET /v1/hardware/history')).toEqual([]);
  });

  it('valida o corpo de erro do daemon', () => {
    const apiError = parseOrThrow(
      ApiErrorSchema,
      { code: 'hardware.unavailable', message: 'sonda falhou', details: null },
      'erro',
    );

    expect(apiError.code).toBe('hardware.unavailable');
  });

  it('valida o ciclo de registro do runtime', () => {
    const registered = parseOrThrow(
      RuntimeRegisterResponseSchema,
      { sessionId: 'sess-1', heartbeatIntervalMs: 5000, daemonVersion: '0.1.0' },
      'POST /v1/runtime/register',
    );
    expect(registered.heartbeatIntervalMs).toBe(5000n);

    const beat = parseOrThrow(
      RuntimeHeartbeatResponseSchema,
      { acknowledged: false, shutdownRequested: false },
      'POST /v1/runtime/heartbeat',
    );
    expect(beat.acknowledged).toBe(false);
  });

  it('mantém o corpo de registro serializável por JSON.stringify', () => {
    // Se algum campo virasse u64, ele seria bigint e o stringify lançaria em
    // produção — o teste existe para essa regressão, não para o schema.
    const body = parseOrThrow(
      RuntimeRegisterRequestSchema,
      { name: 'agent-runtime', version: '0.1.0', pid: 99, protocolVersion: 1, capabilities: [] },
      'POST /v1/runtime/register',
    );

    expect(() => JSON.stringify(body)).not.toThrow();
  });
});

describe('mensagem de erro', () => {
  it('cita contexto, contagem e caminho de cada campo', () => {
    try {
      parseOrThrow(
        HealthResponseSchema,
        { ok: 'sim', version: '0.1.0', protocolVersion: 1 },
        'GET /v1/health',
      );
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      const mismatch = error as SchemaMismatchError;
      expect(mismatch).toBeInstanceOf(SchemaMismatchError);
      expect(mismatch.context).toBe('GET /v1/health');
      expect(mismatch.message).toContain('2 problemas');
      expect(mismatch.message).toContain('  - ok:');
      expect(mismatch.message).toContain('  - uptimeSeconds:');
      expect(mismatch.message).toContain('recebido: "sim"');
    }
  });

  it('nomeia a raiz quando o payload inteiro está errado', () => {
    expect(formatPath([])).toBe('(raiz)');
    expect(formatPath(['gpus', 0, 'vramTotalMb'])).toBe('gpus[0].vramTotalMb');

    try {
      parseOrThrow(HardwareHistorySchema, { nada: true }, 'GET /v1/hardware/history');
      expect.unreachable('deveria ter lançado');
    } catch (error) {
      expect((error as SchemaMismatchError).message).toContain('(raiz)');
    }
  });
});
