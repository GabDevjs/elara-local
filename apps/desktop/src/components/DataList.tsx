/**
 * Lista de campo e valor.
 *
 * A tela de hardware é, em essência, uma sequência de pares. Centralizar o
 * alinhamento aqui é o que mantém os números na mesma coluna entre painéis
 * diferentes — sem isso, cada painel escorregaria alguns pixels e a leitura
 * vertical, que é como se compara hardware, se perderia.
 */
import type { ReactNode } from 'react';

/** Uma linha da lista. */
export interface DataRow {
  /** Rótulo do campo. */
  readonly label: string;
  /** Conteúdo já formatado — texto, [`Numeric`] ou [`Missing`]. */
  readonly value: ReactNode;
  /** Nota em prosa abaixo do valor, para explicar procedência. */
  readonly note?: string;
}

/** Propriedades de [`DataList`]. */
export interface DataListProps {
  /** Linhas na ordem de leitura. */
  readonly rows: readonly DataRow[];
  /** Número de colunas em telas largas. */
  readonly columns?: 1 | 2 | 3;
}

/** Classes de grade por número de colunas. */
const COLUMNS: Record<1 | 2 | 3, string> = {
  1: 'grid-cols-1',
  2: 'grid-cols-1 md:grid-cols-2',
  3: 'grid-cols-1 md:grid-cols-2 xl:grid-cols-3',
};

/** Renderiza os pares campo/valor. */
export function DataList({ rows, columns = 2 }: DataListProps): ReactNode {
  return (
    <dl className={`grid gap-x-6 gap-y-2 ${COLUMNS[columns]}`}>
      {rows.map((row) => (
        <div
          key={row.label}
          className="flex min-w-0 flex-col gap-0.5 border-b border-line pb-1.5 last:border-b-0"
        >
          <dt className="label">{row.label}</dt>
          <dd className="min-w-0 break-words text-[12px] text-ink">{row.value}</dd>
          {row.note !== undefined ? (
            <dd className="font-sans text-[11px] leading-snug text-ink-faint">{row.note}</dd>
          ) : null}
        </div>
      ))}
    </dl>
  );
}
