/**
 * Avisos das sondas de hardware.
 *
 * Ficam em posição fixa e visível, não atrás de um "ver detalhes". Um perfil
 * com avisos é um perfil que decide de forma mais conservadora, e quem lê o
 * painel precisa saber disso antes de concluir que a máquina rende pouco.
 */
import type { ProbeWarning, WarningSeverity } from '@elara/schemas';
import type { ReactNode } from 'react';

/** Aparência e nome de cada severidade. */
const SEVERITY: Record<WarningSeverity, { readonly label: string; readonly tone: string }> = {
  info: { label: 'info', tone: 'text-ink-dim' },
  warning: { label: 'atenção', tone: 'text-warn' },
  error: { label: 'erro', tone: 'text-fail' },
};

/** Propriedades de [`WarningList`]. */
export interface WarningListProps {
  /** Avisos do perfil, na ordem em que as sondas os produziram. */
  readonly warnings: readonly ProbeWarning[];
}

/** Renderiza a lista de avisos, ou a confirmação de que não houve nenhum. */
export function WarningList({ warnings }: WarningListProps): ReactNode {
  if (warnings.length === 0) {
    return (
      <p className="font-sans text-[12px] text-ink-dim">
        Nenhuma sonda reportou problema. Todos os campos do perfil vieram de leitura direta.
      </p>
    );
  }

  return (
    <ul className="flex flex-col gap-1.5">
      {warnings.map((warning, index) => {
        const severity = SEVERITY[warning.severity];
        return (
          <li
            key={`${warning.source}-${index}`}
            className="grid grid-cols-[auto_auto_1fr] items-baseline gap-x-3 border-b border-line pb-1.5 last:border-b-0"
          >
            <span className={`label ${severity.tone}`}>{severity.label}</span>
            <span className="num text-[11px] text-ink-dim">{warning.source}</span>
            <span className="font-sans text-[12px] leading-snug text-ink">{warning.message}</span>
          </li>
        );
      })}
    </ul>
  );
}
