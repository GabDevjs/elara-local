/**
 * Navegação lateral.
 *
 * As telas de fases seguintes aparecem desabilitadas com a fase ao lado. É
 * deliberado: um link quebrado ensina que o produto está torto, enquanto um
 * item apagado com "Fase 5" ensina que ele tem um plano.
 */
import type { ReactNode } from 'react';

import { NAV_ENTRIES, type RouteId } from '../lib/navigation.js';

/** Propriedades de [`NavRail`]. */
export interface NavRailProps {
  /** Rota atualmente exibida. */
  readonly current: RouteId;
  /** Troca de rota. */
  readonly onNavigate: (route: RouteId) => void;
}

/** Renderiza a barra lateral. */
export function NavRail({ current, onNavigate }: NavRailProps): ReactNode {
  return (
    <nav
      aria-label="Seções do Elara"
      className="flex w-52 shrink-0 flex-col border-r border-line bg-surface"
    >
      <div className="flex items-baseline gap-2 border-b border-line px-4 py-3">
        <span className="text-[15px] tracking-[0.2em] text-ink">ELARA</span>
        <span className="label text-live">local agent os</span>
      </div>

      <ul className="flex flex-col py-2">
        {NAV_ENTRIES.map((entry) => {
          const active = entry.route !== null && entry.route === current;

          if (entry.route === null) {
            return (
              <li key={entry.label}>
                <div
                  aria-disabled="true"
                  title={`${entry.label}: ${entry.hint} — chega na ${entry.phase ?? 'próxima fase'}`}
                  className="flex cursor-not-allowed items-baseline justify-between gap-2 px-4 py-1.5"
                >
                  <span className="text-[12px] text-ink-faint">{entry.label}</span>
                  <span className="label">{entry.phase}</span>
                </div>
              </li>
            );
          }

          const route = entry.route;
          return (
            <li key={entry.label}>
              <button
                type="button"
                aria-current={active ? 'page' : undefined}
                onClick={() => {
                  onNavigate(route);
                }}
                title={entry.hint}
                className={`flex w-full items-center gap-2 border-l-2 px-4 py-1.5 text-left text-[12px] transition-colors ${
                  active
                    ? 'border-l-live bg-raised text-ink'
                    : 'border-l-transparent text-ink-dim hover:bg-raised hover:text-ink'
                }`}
              >
                {entry.label}
              </button>
            </li>
          );
        })}
      </ul>

      <p className="mt-auto border-t border-line px-4 py-3 font-sans text-[11px] leading-snug text-ink-faint">
        Fases 1 e 2: base e hardware. As demais telas chegam com os subsistemas que as alimentam.
      </p>
    </nav>
  );
}
