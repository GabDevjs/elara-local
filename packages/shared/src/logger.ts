/**
 * Logging estruturado compatível com o `tracing` do daemon.
 *
 * O Elara é um processo Rust e um processo Node escrevendo no mesmo terminal e,
 * em produção, no mesmo coletor. Se os dois usarem formatos diferentes, ler o
 * log de um incidente vira arqueologia. Por isso a linha emitida aqui tem
 * exatamente a forma que a camada JSON do `tracing-subscriber` produz:
 *
 * ```json
 * {"timestamp":"2026-07-27T09:15:00.123Z","level":"INFO","target":"agent-runtime","fields":{"message":"registrado","sessionId":"…"}}
 * ```
 *
 * Note que `message` e os campos extras ficam dentro de `fields` — é onde o
 * `tracing` os coloca, e é onde um `jq '.fields.message'` vai procurar.
 *
 * Este é o **único** lugar do TypeScript que escreve no console.
 */

/** Níveis aceitos, do mais verboso ao mais grave. */
export const LOG_LEVELS = ['trace', 'debug', 'info', 'warn', 'error'] as const;

/** Nível de um evento de log. */
export type LogLevel = (typeof LOG_LEVELS)[number];

/** Nível de um filtro, que além dos níveis aceita silenciar por completo. */
export type FilterLevel = LogLevel | 'off';

/** Campos extras de um evento. Valores viram JSON pelo [`replacer`]. */
export type LogFields = Readonly<Record<string, unknown>>;

/** Ordem numérica dos níveis; `off` fica acima de tudo para nunca passar. */
const SEVERITY: Readonly<Record<FilterLevel, number>> = {
  trace: 10,
  debug: 20,
  info: 30,
  warn: 40,
  error: 50,
  off: 100,
};

/** Nível usado quando `ELARA_LOG` não está definida. */
const DEFAULT_LEVEL: FilterLevel = 'info';

/** Nome da variável que controla a verbosidade, igual à do daemon. */
export const LOG_ENV_VAR = 'ELARA_LOG';

// ---------------------------------------------------------------------------
// Filtro
// ---------------------------------------------------------------------------

/**
 * Alvos aceitos: caminho de módulo, nada mais.
 *
 * O `EnvFilter` do Rust também entende `alvo[span{campo=valor}]=nível`. Este
 * logger não tem spans, então essas diretivas são recusadas explicitamente —
 * aceitá-las como se fossem nome de alvo criaria um filtro que nunca casa e
 * um usuário convencido de que o log está quebrado.
 */
const TARGET_PATTERN = /^[A-Za-z0-9_.:-]+$/u;

/** Uma diretiva `alvo=nível` já interpretada. */
interface Directive {
  readonly target: string | null;
  readonly level: FilterLevel;
}

/** Filtro compilado a partir de `ELARA_LOG`. */
export interface LogFilter {
  /** Nível mínimo que um evento daquele alvo precisa ter para ser emitido. */
  levelFor(target: string): FilterLevel;
  /** Diretivas que não foram entendidas, na ordem em que apareceram. */
  readonly invalid: readonly string[];
}

/**
 * Interpreta uma especificação no estilo `EnvFilter` do `tracing`.
 *
 * Suporta o subconjunto que o Elara realmente usa: um nível global e diretivas
 * `alvo=nível` (ex.: `info,sdk=debug`). Diretivas com sintaxe de span ou de
 * campo — que o `EnvFilter` aceita e este logger não implementa — entram em
 * `invalid` em vez de serem ignoradas em silêncio.
 *
 * O alvo casa por prefixo, como no `tracing`: `sdk=debug` vale para
 * `sdk.events`. Vence sempre o prefixo mais longo.
 */
export function parseLogFilter(spec: string): LogFilter {
  const directives: Directive[] = [];
  const invalid: string[] = [];

  for (const raw of spec.split(',')) {
    const entry = raw.trim();
    if (entry === '') continue;

    const separator = entry.indexOf('=');
    if (separator === -1) {
      const level = toFilterLevel(entry);
      if (level === null) {
        invalid.push(entry);
        continue;
      }
      directives.push({ target: null, level });
      continue;
    }

    const target = entry.slice(0, separator).trim();
    const level = toFilterLevel(entry.slice(separator + 1));
    if (!TARGET_PATTERN.test(target) || level === null) {
      invalid.push(entry);
      continue;
    }
    directives.push({ target, level });
  }

  const global = directives.findLast((directive) => directive.target === null)?.level;
  const targeted = directives
    .filter((directive): directive is Directive & { target: string } => directive.target !== null)
    .sort((a, b) => b.target.length - a.target.length);

  return {
    invalid,
    levelFor(target: string): FilterLevel {
      const match = targeted.find(
        (directive) => target === directive.target || target.startsWith(`${directive.target}.`),
      );
      return match?.level ?? global ?? DEFAULT_LEVEL;
    },
  };
}

/** Converte o texto de um nível, aceitando maiúsculas como o `tracing` faz. */
function toFilterLevel(value: string): FilterLevel | null {
  const normalized = value.trim().toLowerCase();
  return normalized in SEVERITY ? (normalized as FilterLevel) : null;
}

// ---------------------------------------------------------------------------
// Logger
// ---------------------------------------------------------------------------

/** Emissor de eventos de log para um alvo. */
export interface Logger {
  /** Alvo deste logger, ex.: `agent-runtime.registration`. */
  readonly target: string;
  trace(message: string, fields?: LogFields): void;
  debug(message: string, fields?: LogFields): void;
  info(message: string, fields?: LogFields): void;
  warn(message: string, fields?: LogFields): void;
  error(message: string, fields?: LogFields): void;
  /** `true` quando um evento daquele nível seria emitido. */
  isEnabled(level: LogLevel): boolean;
  /** Logger com alvo `${target}.${suffix}`, herdando filtro e saída. */
  child(suffix: string): Logger;
}

/** Dependências injetáveis do logger. */
export interface LoggerOptions {
  /** Ambiente de onde sai `ELARA_LOG`. Padrão: `process.env`. */
  readonly env?: Readonly<Record<string, string | undefined>>;
  /** Filtro já compilado; tem precedência sobre `env`. */
  readonly filter?: LogFilter;
  /** Destino de cada linha. Padrão: `stderr`, como o daemon. */
  readonly write?: (line: string) => void;
  /** Relógio, para testes determinísticos. Padrão: `Date`. */
  readonly now?: () => Date;
}

/**
 * Cria um logger para `target`.
 *
 * O alvo é o que aparece no campo `target` da linha e o que as diretivas de
 * `ELARA_LOG` casam — então use nomes de módulo (`sdk.events`), não frases.
 */
export function createLogger(target: string, options: LoggerOptions = {}): Logger {
  const write = options.write ?? writeToStderr;
  const now = options.now ?? (() => new Date());
  const filter = options.filter ?? parseLogFilter((options.env ?? process.env)[LOG_ENV_VAR] ?? '');

  // Diretiva errada não pode sumir: se `ELARA_LOG=debgu` passasse batido, o
  // usuário concluiria que o log está quebrado, não que digitou errado.
  for (const entry of filter.invalid) {
    write(
      render(now(), 'WARN', 'elara.logger', `diretiva de ${LOG_ENV_VAR} ignorada`, {
        directive: entry,
      }),
    );
  }

  return build(target, filter, write, now);
}

/** Monta o objeto `Logger`, compartilhado entre o logger raiz e os filhos. */
function build(
  target: string,
  filter: LogFilter,
  write: (line: string) => void,
  now: () => Date,
): Logger {
  const threshold = SEVERITY[filter.levelFor(target)];

  const emit = (level: LogLevel, message: string, fields?: LogFields): void => {
    if (SEVERITY[level] < threshold) return;
    write(render(now(), level.toUpperCase(), target, message, fields));
  };

  return {
    target,
    trace: (message, fields) => {
      emit('trace', message, fields);
    },
    debug: (message, fields) => {
      emit('debug', message, fields);
    },
    info: (message, fields) => {
      emit('info', message, fields);
    },
    warn: (message, fields) => {
      emit('warn', message, fields);
    },
    error: (message, fields) => {
      emit('error', message, fields);
    },
    isEnabled: (level) => SEVERITY[level] >= threshold,
    child: (suffix) => build(`${target}.${suffix}`, filter, write, now),
  };
}

/** Serializa um evento na linha JSON que o `tracing` produziria. */
function render(
  at: Date,
  level: string,
  target: string,
  message: string,
  fields?: LogFields,
): string {
  return JSON.stringify(
    {
      timestamp: at.toISOString(),
      level,
      target,
      fields: { message, ...fields },
    },
    makeReplacer(),
  );
}

/**
 * Torna serializável o que o `JSON.stringify` recusa ou esvazia.
 *
 * Os três casos aparecem de verdade: `bigint` vem dos campos `u64` do
 * contrato e faria o `stringify` lançar; `Error` vira `{}` sem ajuda; e uma
 * referência circular derrubaria o processo justamente na hora de registrar a
 * falha. O `WeakSet` é criado por chamada porque o mesmo objeto pode aparecer
 * legitimamente em dois eventos diferentes.
 */
function makeReplacer(): (this: unknown, key: string, value: unknown) => unknown {
  const seen = new WeakSet<object>();

  return function replace(this: unknown, _key: string, value: unknown): unknown {
    if (typeof value === 'bigint') return value.toString();

    if (value instanceof Error) return describeError(value);

    if (typeof value === 'object' && value !== null) {
      if (seen.has(value)) return '[circular]';
      seen.add(value);
    }

    return value;
  };
}

/** Achata um `Error` no que interessa depois do incidente. */
function describeError(error: Error): Record<string, unknown> {
  const described: Record<string, unknown> = { name: error.name, message: error.message };

  const code = (error as { code?: unknown }).code;
  if (typeof code === 'string') described.code = code;

  const cause: unknown = error.cause;
  if (cause !== undefined) {
    described.cause = cause instanceof Error ? `${cause.name}: ${cause.message}` : cause;
  }

  return described;
}

/**
 * Escreve a linha em `stderr`.
 *
 * `stderr` e não `stdout` porque é onde o daemon escreve os dele e porque o
 * `stdout` de um runtime supervisionado pode um dia carregar dados.
 */
function writeToStderr(line: string): void {
  // Único `console` do código TypeScript; ver a nota no topo do arquivo.
  console.error(line);
}
