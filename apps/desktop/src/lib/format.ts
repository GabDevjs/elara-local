/**
 * Formatação de valores para o painel.
 *
 * Duas regras governam este módulo. A primeira: `null` nunca vira `0`; vira
 * [`DASH`], e quem chama mostra o motivo ao lado. A segunda: nada de
 * arredondar para cima nem "melhorar" um número — o painel repete o que o
 * daemon mediu.
 */

/** Marcador de dado ausente. Um travessão, nunca um zero. */
export const DASH = '—';

/** Separador decimal e de milhar em português. */
const LOCALE = 'pt-BR';

/** Um MiB tem 1024 KiB; um GiB, 1024 MiB. Nada de 1000 aqui. */
const BINARY_STEP = 1024;

/** Valor numérico já formatado, separado da unidade. */
export interface Measurement {
  /** Número formatado, ex.: `31,3`. */
  readonly value: string;
  /** Unidade correspondente, ex.: `GiB`. */
  readonly unit: string;
}

/** Medida ausente, para quando a sonda não entregou o dado. */
export const MISSING: Measurement = { value: DASH, unit: '' };

/**
 * Converte para `number` os `u64` que chegam como `bigint`.
 *
 * A perda de precisão acima de 2^53 é irrelevante para as grandezas do painel
 * (MiB de RAM, milissegundos de uptime) e o SVG precisa de `number`.
 */
export function toNumber(value: bigint | number): number {
  return typeof value === 'bigint' ? Number(value) : value;
}

/** Formata com casas decimais fixas, no padrão brasileiro. */
function decimals(value: number, digits: number): string {
  return value.toLocaleString(LOCALE, {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  });
}

/**
 * Converte MiB na maior unidade binária que ainda deixa o número legível.
 *
 * Um único dígito decimal: em `31,3 GiB` a segunda casa não muda decisão
 * nenhuma e só faz a coluna balançar.
 */
export function formatMib(mib: bigint | number | null): Measurement {
  if (mib === null) return MISSING;
  const raw = toNumber(mib);
  if (!Number.isFinite(raw)) return MISSING;

  if (raw >= BINARY_STEP * BINARY_STEP) {
    return { value: decimals(raw / (BINARY_STEP * BINARY_STEP), 2), unit: 'TiB' };
  }
  if (raw >= BINARY_STEP) {
    return { value: decimals(raw / BINARY_STEP, 1), unit: 'GiB' };
  }
  return { value: decimals(raw, 0), unit: 'MiB' };
}

/** Percentual com uma casa; `null` vira travessão. */
export function formatPercent(value: number | null, digits = 1): Measurement {
  if (value === null || !Number.isFinite(value)) return MISSING;
  return { value: decimals(value, digits), unit: '%' };
}

/** Inteiro com separador de milhar. */
export function formatCount(value: bigint | number | null): string {
  if (value === null) return DASH;
  const raw = toNumber(value);
  if (!Number.isFinite(raw)) return DASH;
  return raw.toLocaleString(LOCALE, { maximumFractionDigits: 0 });
}

/** Frequência em MHz, promovida a GHz quando passa de mil. */
export function formatFrequency(mhz: number | null): Measurement {
  if (mhz === null || !Number.isFinite(mhz)) return MISSING;
  if (mhz >= 1000) return { value: decimals(mhz / 1000, 2), unit: 'GHz' };
  return { value: decimals(mhz, 0), unit: 'MHz' };
}

/** Cache em KiB, promovido a MiB acima de 1024. */
export function formatCache(kib: number | null): Measurement {
  if (kib === null || !Number.isFinite(kib)) return MISSING;
  if (kib >= BINARY_STEP) return { value: decimals(kib / BINARY_STEP, 1), unit: 'MiB' };
  return { value: decimals(kib, 0), unit: 'KiB' };
}

/** Temperatura em graus Celsius. */
export function formatCelsius(value: number | null): Measurement {
  if (value === null || !Number.isFinite(value)) return MISSING;
  return { value: decimals(value, 0), unit: '°C' };
}

/** Potência em watts. */
export function formatWatts(value: number | null): Measurement {
  if (value === null || !Number.isFinite(value)) return MISSING;
  return { value: decimals(value, 1), unit: 'W' };
}

/** Número solto com uma casa, para médias de carga. */
export function formatDecimal(value: number | null, digits = 2): string {
  if (value === null || !Number.isFinite(value)) return DASH;
  return decimals(value, digits);
}

/**
 * Duração legível a partir de segundos, ex.: `2 d 03:14:07`.
 *
 * Formato de cronômetro em vez de "há 2 dias": o uptime de um daemon é um
 * número que se compara com logs, não uma expressão de tempo social.
 */
export function formatUptime(seconds: bigint | number | null): string {
  if (seconds === null) return DASH;
  const total = Math.max(0, Math.floor(toNumber(seconds)));
  if (!Number.isFinite(total)) return DASH;

  const days = Math.floor(total / 86_400);
  const hours = Math.floor((total % 86_400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const secs = total % 60;

  const clock = [hours, minutes, secs].map((part) => String(part).padStart(2, '0')).join(':');
  return days > 0 ? `${days} d ${clock}` : clock;
}

/** Milissegundos, com `ms` ou `s` conforme a ordem de grandeza. */
export function formatMillis(millis: bigint | number | null): Measurement {
  if (millis === null) return MISSING;
  const raw = toNumber(millis);
  if (!Number.isFinite(raw)) return MISSING;
  if (raw >= 1000) return { value: decimals(raw / 1000, 2), unit: 's' };
  return { value: decimals(raw, 0), unit: 'ms' };
}

/**
 * Instante RFC 3339 em hora local, com segundos.
 *
 * Sem timezone no texto: a janela roda na máquina medida, então o fuso é
 * sempre o do usuário e repeti-lo em cada linha só gasta espaço.
 */
export function formatInstant(iso: string | null): string {
  if (iso === null) return DASH;
  const parsed = new Date(iso);
  if (Number.isNaN(parsed.getTime())) return iso;
  return parsed.toLocaleString(LOCALE, {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
  });
}

/** Razão `parte / total` em percentual, ou `null` quando o total é zero. */
export function ratioPercent(part: bigint | number, total: bigint | number): number | null {
  const partValue = toNumber(part);
  const totalValue = toNumber(total);
  if (!Number.isFinite(partValue) || !Number.isFinite(totalValue) || totalValue <= 0) {
    return null;
  }
  return (partValue / totalValue) * 100;
}
