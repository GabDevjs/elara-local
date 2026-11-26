import { describe, expect, it } from 'vitest';

import { SseParser } from '../src/sse.js';

describe('SseParser', () => {
  it('junta linhas data múltiplas com \\n', () => {
    const parser = new SseParser();

    const frames = parser.feed('event: hardware.detected\ndata: {"a":1,\ndata: "b":2}\n\n');

    expect(frames).toHaveLength(1);
    expect(frames[0]).toEqual({
      id: null,
      event: 'hardware.detected',
      data: '{"a":1,\n"b":2}',
    });
  });

  it('monta o evento mesmo quando ele chega picado em vários chunks', () => {
    const parser = new SseParser();

    expect(parser.feed('id: 7\nev')).toHaveLength(0);
    expect(parser.feed('ent: daemon.st')).toHaveLength(0);
    expect(parser.feed('opping\ndata: {"x"')).toHaveLength(0);

    const frames = parser.feed(':1}\n\n');
    expect(frames).toEqual([{ id: '7', event: 'daemon.stopping', data: '{"x":1}' }]);
  });

  it('trata \\r\\n partido entre chunks como um terminador só', () => {
    const parser = new SseParser();

    // O chunk termina em `\r`: só o próximo revela se era `\r` ou `\r\n`.
    expect(parser.feed('data: um\r')).toHaveLength(0);
    const frames = parser.feed('\n\r\n');

    expect(frames).toEqual([{ id: null, event: null, data: 'um' }]);
  });

  it('aceita \\r sozinho como terminador', () => {
    const parser = new SseParser();

    // Um `\r` no fim do buffer fica retido — ainda pode virar `\r\n`. Só o
    // caractere seguinte prova que era terminador sozinho.
    expect(parser.feed('data: um\r\r')).toHaveLength(0);
    expect(parser.feed(': ping\n')).toEqual([{ id: null, event: null, data: 'um' }]);
  });

  it('ignora comentários de keep-alive', () => {
    const parser = new SseParser();

    expect(parser.feed(': ping\n')).toHaveLength(0);
    expect(parser.feed('data: real\n\n')).toEqual([{ id: null, event: null, data: 'real' }]);
  });

  it('mantém o id entre eventos, como manda a especificação', () => {
    const parser = new SseParser();

    const frames = parser.feed('id: 3\ndata: a\n\ndata: b\n\nid: 5\ndata: c\n\n');

    expect(frames.map((frame) => frame.id)).toEqual(['3', '3', '5']);
    expect(parser.lastEventId).toBe('5');
  });

  it('remove exatamente um espaço depois dos dois-pontos', () => {
    const parser = new SseParser();

    expect(parser.feed('data:  dois espaços\n\n')[0]?.data).toBe(' dois espaços');
    expect(parser.feed('data:sem espaço\n\n')[0]?.data).toBe('sem espaço');
  });

  it('não emite evento quando só há separador em branco', () => {
    const parser = new SseParser();
    expect(parser.feed('\n\n\n')).toHaveLength(0);
  });

  it('trata linha sem dois-pontos como campo de valor vazio', () => {
    const parser = new SseParser();
    expect(parser.feed('data\n\n')).toEqual([{ id: null, event: null, data: '' }]);
  });

  it('lê a sugestão de retry', () => {
    const parser = new SseParser();

    parser.feed('retry: 2500\ndata: x\n\n');
    expect(parser.retryMs).toBe(2500);

    parser.feed('retry: nem um pouco\ndata: y\n\n');
    expect(parser.retryMs).toBe(2500);
  });

  it('ignora campos desconhecidos sem descartar o evento', () => {
    const parser = new SseParser();
    expect(parser.feed('inventado: 1\ndata: ok\n\n')).toEqual([
      { id: null, event: null, data: 'ok' },
    ]);
  });
});
