/**
 * Blocos primitivos compartilhados pelos schemas.
 *
 * Existem porque os tipos escalares do Rust não têm equivalente exato em
 * JavaScript e cada tradução carrega uma decisão que precisa ficar num só
 * lugar: `u64` vira `bigint` (o `ts-rs` decidiu assim), `u32` tem faixa
 * fechada, e `jiff::Timestamp` chega como texto RFC 3339.
 */
import { z } from 'zod';

/** Maior valor representável por um `u32` do Rust. */
const U32_MAX = 4_294_967_295;

/** Maior valor representável por um `u64` do Rust. */
const U64_MAX = 18_446_744_073_709_551_615n;

/**
 * Inteiro sem sinal de 32 bits.
 *
 * A faixa é verificada em runtime porque um `-1` ou um `1e12` chegando num
 * campo destes indica um daemon incompatível, não um dado excêntrico.
 */
export const u32 = z.int().min(0).max(U32_MAX);

/**
 * Inteiro sem sinal de 64 bits, entregue como `bigint`.
 *
 * O `ts-rs` mapeia `u64` para `bigint`, mas `JSON.parse` produz `number` — é
 * aqui que a ponte é feita. Aceitar `string` também é deliberado: é o único
 * jeito de um daemon futuro transmitir valores acima de 2^53 sem perda, e o
 * schema já estará pronto.
 *
 * Números acima de `Number.MAX_SAFE_INTEGER` são **rejeitados** em vez de
 * convertidos: depois do `JSON.parse` o dígito já se perdeu, e converter
 * inventaria um valor plausível porém errado.
 */
export const u64 = z
  .union([z.bigint(), z.int(), z.string().regex(/^\d+$/u)], {
    error: 'esperava um inteiro sem sinal de 64 bits (número, bigint ou texto decimal)',
  })
  .transform((value) => BigInt(value))
  .refine((value) => value >= 0n && value <= U64_MAX, 'fora da faixa de um u64');

/**
 * Ponto flutuante (`f32` ou `f64` do Rust).
 *
 * Sem restrição de faixa: percentuais, temperaturas e watts têm limites
 * físicos diferentes, e apertar aqui só produziria rejeição de dado legítimo.
 */
export const f64 = z.number();

/**
 * Instante em RFC 3339, como o `jiff::Timestamp` serializa (sempre em UTC,
 * com precisão de até nanossegundos).
 *
 * Fica como `string` — e não `Date` — porque o tipo gerado do Rust é `string`
 * e converter aqui faria o schema divergir do contrato.
 */
export const timestamp = z.iso.datetime({ offset: true });

/**
 * Média de carga de 1, 5 e 15 minutos.
 *
 * O `[f64; 3]` do Rust vira tupla de tamanho fixo, não array: perder a aridade
 * transformaria "faltou a média de 15 minutos" em `undefined` silencioso.
 */
export const loadAverage = z.tuple([f64, f64, f64]);
