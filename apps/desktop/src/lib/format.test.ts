/**
 * Testes dos formatadores.
 *
 * O que está sendo protegido aqui é uma regra de produto, não uma função de
 * apresentação: nenhum caminho pode transformar `null` em `0`.
 */
import { describe, expect, it } from 'vitest';

import {
  DASH,
  formatCache,
  formatCelsius,
  formatFrequency,
  formatMib,
  formatPercent,
  formatUptime,
  ratioPercent,
} from './format.js';

describe('ausência de dado', () => {
  it('devolve travessão para todo formatador que recebe null', () => {
    expect(formatMib(null).value).toBe(DASH);
    expect(formatPercent(null).value).toBe(DASH);
    expect(formatCelsius(null).value).toBe(DASH);
    expect(formatFrequency(null).value).toBe(DASH);
    expect(formatCache(null).value).toBe(DASH);
    expect(formatUptime(null)).toBe(DASH);
  });

  it('não confunde zero com ausência', () => {
    expect(formatPercent(0).value).toBe('0,0');
    expect(formatMib(0)).toEqual({ value: '0', unit: 'MiB' });
  });
});

describe('unidades binárias', () => {
  it('promove MiB para GiB e TiB nas fronteiras de 1024', () => {
    expect(formatMib(512)).toEqual({ value: '512', unit: 'MiB' });
    expect(formatMib(1024)).toEqual({ value: '1,0', unit: 'GiB' });
    expect(formatMib(32_022)).toEqual({ value: '31,3', unit: 'GiB' });
    expect(formatMib(1_048_576)).toEqual({ value: '1,00', unit: 'TiB' });
  });

  it('aceita bigint, que é como os u64 chegam do daemon', () => {
    expect(formatMib(8192n)).toEqual({ value: '8,0', unit: 'GiB' });
  });
});

describe('uptime', () => {
  it('usa formato de cronômetro, com dias à parte', () => {
    expect(formatUptime(0)).toBe('00:00:00');
    expect(formatUptime(5296)).toBe('01:28:16');
    expect(formatUptime(180_247)).toBe('2 d 02:04:07');
  });
});

describe('proporção', () => {
  it('devolve null quando o total é zero, em vez de NaN ou Infinity', () => {
    expect(ratioPercent(10, 0)).toBeNull();
    expect(ratioPercent(4096n, 8192n)).toBe(50);
  });
});
