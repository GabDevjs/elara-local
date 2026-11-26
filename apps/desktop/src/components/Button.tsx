/**
 * Botão do painel.
 *
 * Retangular, de fio de cabelo, com rótulo em caixa alta. Parece um botão de
 * equipamento porque as ações desta interface são operações — re-detectar
 * hardware, tentar de novo —, não conversões.
 */
import type { ReactNode } from 'react';

/** Propriedades de [`Button`]. */
export interface ButtonProps {
  /** Texto do botão. Sempre um verbo no infinitivo. */
  readonly children: ReactNode;
  /** Ação disparada no clique. */
  readonly onClick: () => void;
  /** Desabilita o botão; exige [`ButtonProps.title`] explicando por quê. */
  readonly disabled?: boolean;
  /** Explicação no `title`, obrigatória quando desabilitado. */
  readonly title?: string;
  /** Destaca o botão como ação primária da tela. */
  readonly primary?: boolean;
}

/** Renderiza o botão. */
export function Button({
  children,
  onClick,
  disabled = false,
  title,
  primary = false,
}: ButtonProps): ReactNode {
  const tone = primary
    ? 'border-live/40 text-live hover:border-live hover:bg-live/10'
    : 'border-line-strong text-ink-dim hover:border-ink-faint hover:text-ink';

  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      title={title}
      className={`label border px-2.5 py-1 transition-colors rounded-hair disabled:cursor-not-allowed disabled:border-line disabled:text-ink-faint disabled:hover:bg-transparent ${tone}`}
    >
      {children}
    </button>
  );
}
