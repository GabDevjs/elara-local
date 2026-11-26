/**
 * Histórico recente em SVG, desenhado à mão.
 *
 * Nenhuma biblioteca de gráficos: o que precisamos é uma linha, uma área e
 * três guias — e uma dependência de gráfico traria seu próprio sistema de
 * cores, tipografia e animação para brigar com o resto do painel.
 */
import { useId, type ReactNode } from 'react';

/** Largura do sistema de coordenadas interno. Não é pixel; o SVG estica. */
const VIEW_WIDTH = 240;

/** Altura do sistema de coordenadas interno. */
const VIEW_HEIGHT = 56;

/** Guias horizontais, em fração da altura. */
const GUIDES = [0.25, 0.5, 0.75];

/** Propriedades de [`Sparkline`]. */
export interface SparklineProps {
  /** Série do mais antigo ao mais recente. */
  readonly values: readonly number[];
  /** Topo da escala. Fixo, para que a altura signifique sempre a mesma coisa. */
  readonly max: number;
  /** Cor da linha e da área. */
  readonly tone: string;
  /** Descrição para leitores de tela. */
  readonly label: string;
}

/** Converte a série em coordenadas do `viewBox`. */
function toPoints(values: readonly number[], max: number): { x: number; y: number }[] {
  const span = Math.max(values.length - 1, 1);
  return values.map((value, index) => {
    const ratio = max > 0 ? Math.min(Math.max(value / max, 0), 1) : 0;
    return {
      x: (index / span) * VIEW_WIDTH,
      y: VIEW_HEIGHT - ratio * VIEW_HEIGHT,
    };
  });
}

/** Renderiza a sparkline; devolve `null` quando não há o que desenhar. */
export function Sparkline({ values, max, tone, label }: SparklineProps): ReactNode {
  // Os dois-pontos que o React gera não são aceitos em `url(#…)` por todos os
  // motores; removê-los é mais barato do que descobrir isso em produção.
  const gradientId = `spark-${useId().replace(/:/gu, '')}`;

  if (values.length === 0) return null;

  const points = toPoints(values, max);
  const line = points.map((point, index) => `${index === 0 ? 'M' : 'L'}${point.x} ${point.y}`);
  const area = [`M0 ${VIEW_HEIGHT}`, ...line, `L${VIEW_WIDTH} ${VIEW_HEIGHT}`, 'Z'];
  const last = points.at(-1);

  return (
    <div className="relative h-14 w-full">
      <svg
        className="h-full w-full"
        viewBox={`0 0 ${VIEW_WIDTH} ${VIEW_HEIGHT}`}
        preserveAspectRatio="none"
        role="img"
        aria-label={label}
      >
        <defs>
          <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stopColor={tone} stopOpacity="0.28" />
            <stop offset="100%" stopColor={tone} stopOpacity="0" />
          </linearGradient>
        </defs>

        {GUIDES.map((fraction) => (
          <line
            key={fraction}
            x1="0"
            x2={VIEW_WIDTH}
            y1={VIEW_HEIGHT * fraction}
            y2={VIEW_HEIGHT * fraction}
            stroke="var(--elara-line)"
            strokeWidth="1"
            vectorEffect="non-scaling-stroke"
          />
        ))}

        <path d={area.join(' ')} fill={`url(#${gradientId})`} />
        <path
          d={line.join(' ')}
          fill="none"
          stroke={tone}
          strokeWidth="1.25"
          strokeLinejoin="round"
          vectorEffect="non-scaling-stroke"
        />
      </svg>

      {/*
        O marcador do instante atual fica em HTML, não no SVG: com
        `preserveAspectRatio="none"` um círculo viraria uma elipse achatada.
      */}
      {last !== undefined ? (
        <span
          className="pointer-events-none absolute right-0 block size-[5px] -translate-y-1/2 translate-x-1/2 rounded-full"
          style={{
            top: `${((last.y / VIEW_HEIGHT) * 100).toFixed(2)}%`,
            backgroundColor: tone,
          }}
          aria-hidden="true"
        />
      ) : null}
    </div>
  );
}
