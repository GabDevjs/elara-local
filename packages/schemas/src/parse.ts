/**
 * Validação na fronteira IPC.
 *
 * Todo dado que atravessa o socket entra no TypeScript como `unknown`. Se ele
 * for apenas *convertido* para o tipo esperado, um daemon de outra versão
 * produz `undefined` num campo qualquer e o erro só aparece três camadas
 * acima, sem pista da origem. `parseOrThrow` transforma isso numa exceção que
 * diz qual campo, em que caminho, e o que se esperava dele.
 */
import type { z } from 'zod';

/** Quantas issues aparecem na mensagem antes de o resto virar um contador. */
const MAX_LISTED_ISSUES = 10;

/** Acima disto, o valor recebido é truncado para não inundar o log. */
const MAX_VALUE_CHARS = 40;

/**
 * Falha de validação de um payload trocado com o daemon.
 *
 * Carrega as issues estruturadas além da mensagem legível porque a UI pode
 * querer destacar campos específicos em vez de mostrar o texto inteiro.
 */
export class SchemaMismatchError extends Error {
  /** Onde a validação ocorreu, ex.: `GET /v1/hardware`. */
  readonly context: string;

  /** Issues cruas do zod, na ordem em que foram encontradas. */
  readonly issues: readonly z.core.$ZodIssue[];

  constructor(context: string, issues: readonly z.core.$ZodIssue[]) {
    super(formatIssues(context, issues));
    this.name = 'SchemaMismatchError';
    this.context = context;
    this.issues = issues;
  }
}

/**
 * Valida `data` contra `schema` ou lança [`SchemaMismatchError`].
 *
 * Devolve o valor **de saída** do schema — já com os `u64` convertidos para
 * `bigint` —, não o objeto original.
 */
export function parseOrThrow<Schema extends z.ZodType>(
  schema: Schema,
  data: unknown,
  context: string,
): z.output<Schema> {
  // `reportInput` faz o zod anexar o valor recebido a cada issue. Vale o custo:
  // sem ele a mensagem diz "esperava boolean" sem dizer o que chegou, e o
  // diagnóstico de skew de versão vira adivinhação. Os dados aqui são o perfil
  // da máquina local, não credenciais.
  const result = schema.safeParse(data, { reportInput: true });
  if (!result.success) {
    throw new SchemaMismatchError(context, result.error.issues);
  }
  return result.data;
}

/**
 * Caminho do campo em notação familiar: `gpus[0].vramTotalMb`.
 *
 * A raiz vira `(raiz)` para que a mensagem não fique com um traço solto quando
 * o problema é o payload inteiro, e não um campo dele.
 */
export function formatPath(path: readonly PropertyKey[]): string {
  if (path.length === 0) return '(raiz)';

  let rendered = '';
  for (const segment of path) {
    if (typeof segment === 'number') {
      rendered += `[${segment}]`;
    } else if (rendered === '') {
      rendered += String(segment);
    } else {
      rendered += `.${String(segment)}`;
    }
  }
  return rendered;
}

/**
 * Monta a mensagem multilinha do erro.
 *
 * Uma linha por campo, porque na prática o skew de versão quebra vários campos
 * de uma vez e mostrar só o primeiro esconde o padrão.
 */
function formatIssues(context: string, issues: readonly z.core.$ZodIssue[]): string {
  const plural = issues.length === 1 ? 'problema' : 'problemas';
  const header = `${context}: payload inválido (${issues.length} ${plural})`;

  const listed = issues.slice(0, MAX_LISTED_ISSUES).map((issue) => {
    const suffix = 'input' in issue ? ` (recebido: ${describe(issue.input)})` : '';
    return `  - ${formatPath(issue.path)}: ${issue.message}${suffix}`;
  });

  if (issues.length > MAX_LISTED_ISSUES) {
    listed.push(`  - ... e mais ${issues.length - MAX_LISTED_ISSUES}`);
  }

  return [header, ...listed].join('\n');
}

/**
 * Descreve o valor recebido sem despejar o payload inteiro.
 *
 * Objetos e arrays viram só a forma: quem precisa do conteúdo tem as `issues`.
 */
function describe(value: unknown): string {
  switch (typeof value) {
    case 'string': {
      const clipped =
        value.length > MAX_VALUE_CHARS ? `${value.slice(0, MAX_VALUE_CHARS)}…` : value;
      return JSON.stringify(clipped);
    }
    case 'number':
    case 'boolean':
      return String(value);
    case 'bigint':
      return `${value.toString()}n`;
    case 'undefined':
      return 'undefined';
    case 'object':
      if (value === null) return 'null';
      if (Array.isArray(value)) return `array(${value.length})`;
      return 'object';
    default:
      return typeof value;
  }
}
