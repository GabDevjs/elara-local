import { describe, expect, it } from 'vitest';

import { computeBackoffDelay, delay } from '../src/backoff.js';

describe('computeBackoffDelay', () => {
  it('cresce exponencialmente a partir do valor inicial', () => {
    const options = { initialMs: 100, factor: 2, jitter: 0, maxMs: 10_000 };

    expect(computeBackoffDelay(0, options)).toBe(100);
    expect(computeBackoffDelay(1, options)).toBe(200);
    expect(computeBackoffDelay(2, options)).toBe(400);
    expect(computeBackoffDelay(3, options)).toBe(800);
  });

  it('respeita o teto e não estoura com tentativa absurda', () => {
    const options = { initialMs: 100, factor: 2, jitter: 0, maxMs: 5000 };

    expect(computeBackoffDelay(20, options)).toBe(5000);
    // `factor ** 2000` é Infinity; o resultado ainda tem de ser um número.
    expect(computeBackoffDelay(2000, options)).toBe(5000);
  });

  it('aplica jitter dentro da faixa esperada', () => {
    const options = { initialMs: 1000, factor: 2, jitter: 0.5, maxMs: 10_000 };

    expect(computeBackoffDelay(0, options, () => 0)).toBe(500);
    expect(computeBackoffDelay(0, options, () => 0.5)).toBe(1000);
    expect(computeBackoffDelay(0, options, () => 1)).toBe(1500);
  });

  it('nunca devolve valor negativo nem ultrapassa o teto com jitter', () => {
    const options = { initialMs: 1000, factor: 2, jitter: 1, maxMs: 1200 };

    expect(computeBackoffDelay(0, options, () => 0)).toBe(0);
    expect(computeBackoffDelay(5, options, () => 1)).toBe(1200);
  });

  it('trata tentativa negativa como a primeira', () => {
    const options = { initialMs: 100, factor: 2, jitter: 0 };
    expect(computeBackoffDelay(-3, options)).toBe(100);
  });
});

describe('delay', () => {
  it('resolve depois do tempo pedido', async () => {
    const start = Date.now();
    await delay(30);
    expect(Date.now() - start).toBeGreaterThanOrEqual(20);
  });

  it('acorda imediatamente quando o sinal aborta', async () => {
    const controller = new AbortController();
    const start = Date.now();

    const waiting = delay(5000, controller.signal);
    controller.abort();
    await waiting;

    expect(Date.now() - start).toBeLessThan(1000);
  });

  it('não espera quando o sinal já está abortado', async () => {
    const controller = new AbortController();
    controller.abort();

    const start = Date.now();
    await delay(5000, controller.signal);

    expect(Date.now() - start).toBeLessThan(100);
  });
});
