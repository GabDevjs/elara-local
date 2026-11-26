/**
 * Uso por núcleo lógico.
 *
 * O agregado esconde o caso que mais importa aqui: um único núcleo saturado
 * enquanto os outros dormem indica trabalho serial, e a decisão de quantas
 * threads dar a um modelo depende disso. Por isso a lista é por núcleo, com o
 * índice do `/proc/stat` visível — é ele que aparece no `taskset`.
 */
import type { ReactNode } from 'react';

import { formatPercent } from '../lib/format.js';

/** A partir daqui o núcleo é considerado saturado. */
const HOT_PERCENT = 85;

/** A partir daqui ele está sob carga relevante. */
const BUSY_PERCENT = 45;

/** Propriedades de [`CoreBars`]. */
export interface CoreBarsProps {
  /** Percentual por CPU lógica, na ordem do `/proc/stat`. */
  readonly perCorePercent: readonly number[];
}

/** Cor da barra conforme a faixa de carga. */
function toneFor(percent: number): string {
  if (percent >= HOT_PERCENT) return 'var(--elara-warn)';
  if (percent >= BUSY_PERCENT) return 'var(--elara-live)';
  return 'var(--elara-compute)';
}

/** Renderiza a grade de núcleos. */
export function CoreBars({ perCorePercent }: CoreBarsProps): ReactNode {
  if (perCorePercent.length === 0) {
    return (
      <p className="font-sans text-[11px] text-ink-faint">
        o daemon não reportou uso por núcleo nesta amostra
      </p>
    );
  }

  return (
    <ul className="grid grid-cols-1 gap-x-5 gap-y-1 sm:grid-cols-2 2xl:grid-cols-4">
      {perCorePercent.map((percent, index) => {
        const clamped = Math.min(Math.max(percent, 0), 100);
        const measurement = formatPercent(percent);
        return (
          // O índice É a identidade do núcleo: a ordem do `/proc/stat` é estável.
          <li key={index} className="flex items-center gap-2">
            <span className="num w-6 shrink-0 text-right text-[10px] text-ink-faint">
              {String(index).padStart(2, '0')}
            </span>
            <span className="h-2 flex-1 bg-raised">
              <span
                className="block h-full transition-[width] duration-300 ease-out"
                style={{ width: `${clamped.toFixed(1)}%`, backgroundColor: toneFor(clamped) }}
              />
            </span>
            <span className="num w-12 shrink-0 text-right text-[11px] text-ink-dim">
              {measurement.value}
              {measurement.unit}
            </span>
          </li>
        );
      })}
    </ul>
  );
}
