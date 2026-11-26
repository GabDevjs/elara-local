/**
 * Barra de capacidade com o orçamento do Elara marcado.
 *
 * É o gráfico central do produto. Um medidor comum mostra "26 GiB de 31 GiB
 * usados" e deixa entender que os 5 GiB restantes estão disponíveis para
 * qualquer coisa. Não estão: o Elara só pode contar com o que sobra depois de
 * cgroups e das reservas para o desktop e o driver. O colchete violeta desenha
 * esse orçamento sobre a capacidade física, no mesmo eixo — a comparação é a
 * informação.
 */
import type { ReactNode } from 'react';

/** Largura do sistema de coordenadas interno. */
const VIEW_WIDTH = 1000;

/** Altura da faixa reservada ao colchete do orçamento. */
const BRACKET_BAND = 14;

/** Altura da barra em si. */
const BAR_HEIGHT = 26;

/** Uma fatia da capacidade total. */
export interface CapacitySegment {
  /** Chave estável para a lista. */
  readonly id: string;
  /** Rótulo na legenda. */
  readonly label: string;
  /** Valor na mesma unidade de `total`. */
  readonly value: number;
  /** Valor já formatado, para a legenda. */
  readonly display: string;
  /** Cor do preenchimento. */
  readonly color: string;
  /** Opacidade — é o que separa "comprometido" de "recuperável". */
  readonly opacity: number;
}

/** Orçamento efetivo destacado sobre a barra. */
export interface CapacityBudget {
  /** Valor na mesma unidade de `total`. */
  readonly value: number;
  /** Rótulo na legenda. */
  readonly label: string;
  /** Valor já formatado. */
  readonly display: string;
}

/** Propriedades de [`CapacityBar`]. */
export interface CapacityBarProps {
  /** Capacidade física total. */
  readonly total: number;
  /** Fatias, na ordem em que se empilham a partir da esquerda. */
  readonly segments: readonly CapacitySegment[];
  /** Orçamento a marcar, ou `null` quando não há um. */
  readonly budget: CapacityBudget | null;
  /** Descrição para leitores de tela. */
  readonly label: string;
}

/** Renderiza a barra segmentada e sua legenda. */
export function CapacityBar({ total, segments, budget, label }: CapacityBarProps): ReactNode {
  if (total <= 0) {
    return (
      <p className="font-sans text-[11px] text-ink-faint">
        capacidade total desconhecida — sem escala para desenhar a barra
      </p>
    );
  }

  const bracketTop = budget === null ? 2 : BRACKET_BAND;
  const viewHeight = bracketTop + BAR_HEIGHT;
  const scale = (value: number): number => Math.min(Math.max(value / total, 0), 1) * VIEW_WIDTH;

  let cursor = 0;
  const placed = segments.map((segment) => {
    const width = scale(segment.value);
    const x = Math.min(cursor, VIEW_WIDTH);
    cursor += width;
    return { segment, x, width: Math.min(width, VIEW_WIDTH - x) };
  });

  const budgetX = budget === null ? null : scale(budget.value);

  return (
    <div className="flex flex-col gap-2">
      <svg
        className="w-full"
        style={{ height: `${viewHeight * 1.1}px` }}
        viewBox={`0 0 ${VIEW_WIDTH} ${viewHeight}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={label}
      >
        {budgetX !== null ? (
          <g>
            {/* Colchete: cobre da origem até o orçamento, deixando claro que
                ele é um recorte do total, e não um valor solto. */}
            <line
              x1="0"
              x2={budgetX}
              y1="4"
              y2="4"
              stroke="var(--elara-compute)"
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
            />
            <line
              x1="0.5"
              x2="0.5"
              y1="4"
              y2="9"
              stroke="var(--elara-compute)"
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
            />
            <line
              x1={budgetX}
              x2={budgetX}
              y1="4"
              y2={viewHeight}
              stroke="var(--elara-compute)"
              strokeWidth="1"
              vectorEffect="non-scaling-stroke"
            />
          </g>
        ) : null}

        <rect
          x="0"
          y={bracketTop}
          width={VIEW_WIDTH}
          height={BAR_HEIGHT}
          fill="var(--elara-raised)"
        />

        {placed.map(({ segment, x, width }) => (
          <rect
            key={segment.id}
            x={x}
            y={bracketTop}
            width={Math.max(width, 0)}
            height={BAR_HEIGHT}
            fill={segment.color}
            fillOpacity={segment.opacity}
          />
        ))}

        {placed.slice(0, -1).map(({ segment, x, width }) => (
          <line
            key={`${segment.id}-edge`}
            x1={x + width}
            x2={x + width}
            y1={bracketTop}
            y2={bracketTop + BAR_HEIGHT}
            stroke="var(--elara-void)"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
        ))}

        <rect
          x="0"
          y={bracketTop}
          width={VIEW_WIDTH}
          height={BAR_HEIGHT}
          fill="none"
          stroke="var(--elara-line)"
          strokeWidth="1"
          vectorEffect="non-scaling-stroke"
        />
      </svg>

      <ul className="flex flex-wrap items-center gap-x-4 gap-y-1">
        {segments.map((segment) => (
          <li key={segment.id} className="flex items-center gap-1.5">
            <span
              aria-hidden="true"
              className="block h-2 w-2"
              style={{ backgroundColor: segment.color, opacity: segment.opacity }}
            />
            <span className="label">{segment.label}</span>
            <span className="num text-[11px] text-ink-dim">{segment.display}</span>
          </li>
        ))}
        {budget !== null ? (
          <li className="flex items-center gap-1.5">
            <span aria-hidden="true" className="block h-3 w-px bg-compute" />
            <span className="label text-compute">{budget.label}</span>
            <span className="num text-[11px] text-compute">{budget.display}</span>
          </li>
        ) : null}
      </ul>
    </div>
  );
}
