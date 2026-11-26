import { describe, expect, it } from 'vitest';

import {
  DaemonResponseError,
  DaemonTimeoutError,
  DaemonUnavailableError,
  ElaraError,
  isElaraError,
  ProtocolMismatchError,
} from '../src/errors.js';

describe('ElaraError', () => {
  it('expõe code e details e continua sendo um Error', () => {
    const error = new ElaraError('hardware.unavailable', 'sonda falhou', {
      details: 'NVML não carregou',
    });

    expect(error).toBeInstanceOf(Error);
    expect(error.name).toBe('ElaraError');
    expect(error.code).toBe('hardware.unavailable');
    expect(error.details).toBe('NVML não carregou');
  });

  it('preserva a causa original', () => {
    const cause = new Error('ENOENT');
    const error = new ElaraError('daemon.unavailable', 'socket ausente', { cause });

    expect(error.cause).toBe(cause);
  });

  it('serializa em JSON com conteúdo, não em objeto vazio', () => {
    const error = new DaemonTimeoutError(2000, 'estourou');

    expect(JSON.parse(JSON.stringify(error))).toEqual({
      name: 'DaemonTimeoutError',
      code: 'daemon.timeout',
      message: 'estourou',
      details: null,
    });
  });
});

describe('subclasses', () => {
  it('cada uma carrega o contexto que a decisão precisa', () => {
    const unavailable = new DaemonUnavailableError('/run/elara.sock', 'sem daemon');
    expect(unavailable.code).toBe('daemon.unavailable');
    expect(unavailable.socketPath).toBe('/run/elara.sock');

    const timeout = new DaemonTimeoutError(5000, 'demorou');
    expect(timeout.code).toBe('daemon.timeout');
    expect(timeout.timeoutMs).toBe(5000);

    const mismatch = new ProtocolMismatchError('versões divergem');
    expect(mismatch.code).toBe('protocol.mismatch');
  });

  it('DaemonResponseError preserva o code do daemon, não um nosso', () => {
    const error = new DaemonResponseError(503, 'hardware.unavailable', 'sonda indisponível');

    expect(error.code).toBe('hardware.unavailable');
    expect(error.status).toBe(503);
    expect(error.name).toBe('DaemonResponseError');
  });
});

describe('isElaraError', () => {
  it('reconhece a hierarquia', () => {
    expect(isElaraError(new ElaraError('x', 'y'))).toBe(true);
    expect(isElaraError(new DaemonTimeoutError(1, 'y'))).toBe(true);
  });

  it('rejeita erros alheios', () => {
    expect(isElaraError(new Error('comum'))).toBe(false);
    expect(isElaraError('daemon.unavailable')).toBe(false);
    expect(isElaraError(null)).toBe(false);
  });
});
