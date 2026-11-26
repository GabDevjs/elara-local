/**
 * Superfície elevada com cabeçalho.
 *
 * Um painel é uma caixa de fio de cabelo — sem sombra e praticamente sem canto
 * arredondado. A separação entre painéis vem da linha e do fundo, não de
 * profundidade simulada: em tela densa, sombra difusa só embaça o número.
 */
import type { ReactNode } from 'react';

/** Propriedades de [`Panel`]. */
export interface PanelProps {
  /** Rótulo do painel, exibido em caixa alta com tracking largo. */
  readonly title: string;
  /** Informação secundária à direita do título, ex.: a origem da sonda. */
  readonly meta?: ReactNode;
  /** Controles à direita do cabeçalho. */
  readonly actions?: ReactNode;
  /** Classes extras para posicionar o painel na grade. */
  readonly className?: string;
  /** Remove o preenchimento interno, para painéis que só contêm tabela. */
  readonly flush?: boolean;
  readonly children: ReactNode;
}

/** Renderiza um painel do instrumento. */
export function Panel({
  title,
  meta,
  actions,
  className = '',
  flush = false,
  children,
}: PanelProps): ReactNode {
  return (
    <section
      className={`flex min-w-0 flex-col border border-line bg-surface rounded-hair ${className}`}
    >
      <header className="flex min-h-9 items-center justify-between gap-3 border-b border-line px-3 py-2">
        <div className="flex min-w-0 items-baseline gap-3">
          <h2 className="label text-ink-dim">{title}</h2>
          {meta !== undefined ? (
            <span className="truncate text-[11px] text-ink-faint">{meta}</span>
          ) : null}
        </div>
        {actions !== undefined ? <div className="flex items-center gap-2">{actions}</div> : null}
      </header>
      <div className={flush ? 'min-w-0 flex-1' : 'min-w-0 flex-1 p-3'}>{children}</div>
    </section>
  );
}
