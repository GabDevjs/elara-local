import { describe, expect, it } from 'vitest';

import { ElaraError } from '../src/errors.js';
import { createLogger, parseLogFilter } from '../src/logger.js';

/** Captura as linhas emitidas em vez de escrevê-las no stderr do teste. */
function capture() {
  const lines: string[] = [];
  return { lines, write: (line: string) => lines.push(line) };
}

const FIXED_NOW = () => new Date('2026-07-27T09:15:00.123Z');

describe('parseLogFilter', () => {
  it('aplica o nível global quando não há diretiva por alvo', () => {
    const filter = parseLogFilter('debug');
    expect(filter.levelFor('sdk')).toBe('debug');
    expect(filter.levelFor('agent-runtime')).toBe('debug');
  });

  it('usa info quando a especificação está vazia', () => {
    expect(parseLogFilter('').levelFor('sdk')).toBe('info');
  });

  it('casa alvo por prefixo, com o mais específico vencendo', () => {
    const filter = parseLogFilter('info,sdk=warn,sdk.events=trace');

    expect(filter.levelFor('sdk')).toBe('warn');
    expect(filter.levelFor('sdk.http')).toBe('warn');
    expect(filter.levelFor('sdk.events')).toBe('trace');
    expect(filter.levelFor('agent-runtime')).toBe('info');
  });

  it('não confunde alvos que apenas começam com o mesmo texto', () => {
    const filter = parseLogFilter('info,sdk=trace');
    expect(filter.levelFor('sdkx')).toBe('info');
  });

  it('aceita nível em maiúsculas, como o tracing', () => {
    expect(parseLogFilter('WARN').levelFor('sdk')).toBe('warn');
  });

  it('coleta diretivas inválidas em vez de descartá-las', () => {
    const filter = parseLogFilter('info,sdk=debgu,=trace,[span]=debug');

    expect(filter.invalid).toEqual(['sdk=debgu', '=trace', '[span]=debug']);
    expect(filter.levelFor('sdk')).toBe('info');
  });
});

describe('createLogger', () => {
  it('emite uma linha JSON no formato do tracing', () => {
    const { lines, write } = capture();
    const log = createLogger('agent-runtime', {
      env: { ELARA_LOG: 'info' },
      write,
      now: FIXED_NOW,
    });

    log.info('registrado no daemon', { sessionId: 'sess-1', attempt: 2 });

    expect(lines).toHaveLength(1);
    expect(JSON.parse(lines[0]!)).toEqual({
      timestamp: '2026-07-27T09:15:00.123Z',
      level: 'INFO',
      target: 'agent-runtime',
      fields: { message: 'registrado no daemon', sessionId: 'sess-1', attempt: 2 },
    });
  });

  it('respeita o filtro por alvo vindo de ELARA_LOG', () => {
    const { lines, write } = capture();
    const env = { ELARA_LOG: 'warn,agent-runtime=debug' };

    createLogger('sdk', { env, write, now: FIXED_NOW }).info('não deve aparecer');
    createLogger('agent-runtime', { env, write, now: FIXED_NOW }).debug('deve aparecer');

    expect(lines).toHaveLength(1);
    expect(JSON.parse(lines[0]!)).toMatchObject({ level: 'DEBUG', target: 'agent-runtime' });
  });

  it('child herda filtro e concatena o alvo', () => {
    const { lines, write } = capture();
    const log = createLogger('sdk', {
      env: { ELARA_LOG: 'warn,sdk.events=trace' },
      write,
      now: FIXED_NOW,
    });

    log.trace('silenciado');
    log.child('events').trace('emitido');

    expect(lines).toHaveLength(1);
    expect(JSON.parse(lines[0]!)).toMatchObject({ target: 'sdk.events', level: 'TRACE' });
  });

  it('isEnabled concorda com o que é realmente emitido', () => {
    const { lines, write } = capture();
    const log = createLogger('sdk', { env: { ELARA_LOG: 'warn' }, write, now: FIXED_NOW });

    expect(log.isEnabled('info')).toBe(false);
    expect(log.isEnabled('error')).toBe(true);

    log.info('nada');
    expect(lines).toHaveLength(0);
  });

  it('avisa sobre diretiva inválida em vez de ignorá-la', () => {
    const { lines, write } = capture();
    createLogger('sdk', { env: { ELARA_LOG: 'info,sdk=debgu' }, write, now: FIXED_NOW });

    expect(lines).toHaveLength(1);
    expect(JSON.parse(lines[0]!)).toMatchObject({
      level: 'WARN',
      target: 'elara.logger',
      fields: { directive: 'sdk=debgu' },
    });
  });

  it('serializa bigint, Error e ciclo sem derrubar o processo', () => {
    const { lines, write } = capture();
    const log = createLogger('sdk', { env: { ELARA_LOG: 'info' }, write, now: FIXED_NOW });

    const circular: Record<string, unknown> = { name: 'nó' };
    circular.self = circular;

    log.error('falhou', {
      vramMb: 8192n,
      cause: new ElaraError('daemon.unavailable', 'socket sumiu'),
      circular,
    });

    expect(lines).toHaveLength(1);
    expect(JSON.parse(lines[0]!)).toMatchObject({
      fields: {
        vramMb: '8192',
        cause: { name: 'ElaraError', code: 'daemon.unavailable', message: 'socket sumiu' },
        circular: { name: 'nó', self: '[circular]' },
      },
    });
  });
});
