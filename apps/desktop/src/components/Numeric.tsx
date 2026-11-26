/**
 * Valor numérico do painel.
 *
 * Todo número da interface passa por aqui, por dois motivos que não são
 * estéticos: `tabular-nums` impede a coluna de tremer quando `100,0` vira
 * `9,4`, e o pisca em ciano é o único jeito de distinguir "o número não mudou"
 * de "o painel congelou".
 */
import { useEffect, useRef, useState, type ReactNode } from 'react';

/** Quanto tempo o valor fica em ciano antes de voltar à cor de repouso. */
const TICK_MS = 90;

/** Escalas de tamanho, do rodapé de painel ao número protagonista. */
const SIZES = {
  xs: 'text-[11px]',
  sm: 'text-[13px]',
  md: 'text-[17px]',
  lg: 'text-[26px] leading-none',
  xl: 'text-[40px] leading-none',
} as const;

/** Cores possíveis do valor, todas semânticas. */
const TONES = {
  ink: 'text-ink',
  dim: 'text-ink-dim',
  faint: 'text-ink-faint',
  live: 'text-live',
  compute: 'text-compute',
  ok: 'text-ok',
  warn: 'text-warn',
  fail: 'text-fail',
} as const;

/** Propriedades de [`Numeric`]. */
export interface NumericProps {
  /** Número já formatado. Nunca receba um `number` cru aqui. */
  readonly value: string;
  /** Unidade exibida ao lado, em tamanho menor. */
  readonly unit?: string;
  /** Escala tipográfica. */
  readonly size?: keyof typeof SIZES;
  /** Cor de repouso. */
  readonly tone?: keyof typeof TONES;
  /** Ativa o pisca a cada mudança de valor. */
  readonly live?: boolean;
  /** Texto de apoio no `title`, ex.: o valor exato sem arredondamento. */
  readonly title?: string;
}

/** Renderiza um número com unidade. */
export function Numeric({
  value,
  unit,
  size = 'md',
  tone = 'ink',
  live = false,
  title,
}: NumericProps): ReactNode {
  const [ticking, setTicking] = useState(false);
  const previous = useRef(value);

  useEffect(() => {
    let timer = 0;
    if (live && previous.current !== value) {
      previous.current = value;
      setTicking(true);
      timer = window.setTimeout(() => {
        setTicking(false);
      }, TICK_MS);
    }
    return () => {
      if (timer !== 0) window.clearTimeout(timer);
    };
  }, [live, value]);

  return (
    <span className="inline-flex items-baseline gap-1" title={title}>
      <span
        className={`num ${SIZES[size]} ${TONES[tone]} ${live ? 'num-live' : ''}`}
        data-tick={live && ticking ? 'true' : 'false'}
      >
        {value}
      </span>
      {unit !== undefined && unit !== '' ? (
        <span className="text-[10px] text-ink-faint">{unit}</span>
      ) : null}
    </span>
  );
}
