/**
 * Estado de um componente supervisionado.
 *
 * O contrato diz que `detail` é obrigatório quando o estado não é `Ready`.
 * Esta é a tela que cobra esse contrato: quando o motivo falta, ela diz que
 * falta em vez de exibir um estado vermelho sem explicação.
 */
import type { ComponentState, ComponentStatus } from '@elara/schemas';
import type { ReactNode } from 'react';

/** Aparência e texto de cada estado do ciclo de vida. */
const PRESENTATION: Record<ComponentState, { readonly label: string; readonly tone: string }> = {
  stopped: { label: 'parado', tone: 'text-ink-faint' },
  starting: { label: 'iniciando', tone: 'text-live' },
  ready: { label: 'pronto', tone: 'text-ok' },
  degraded: { label: 'degradado', tone: 'text-warn' },
  failed: { label: 'falhou', tone: 'text-fail' },
  unavailable: { label: 'indisponível', tone: 'text-ink-faint' },
};

/** Cor de fundo do ponto, por estado. */
const DOT: Record<ComponentState, string> = {
  stopped: 'bg-ink-faint',
  starting: 'bg-live',
  ready: 'bg-ok',
  degraded: 'bg-warn',
  failed: 'bg-fail',
  unavailable: 'bg-ink-faint',
};

/** Ponto colorido do estado, sem texto. */
export function StateDot({ state }: { state: ComponentState }): ReactNode {
  return <span aria-hidden="true" className={`block size-1.5 shrink-0 ${DOT[state]}`} />;
}

/** Nome legível do estado, em português. */
export function stateLabel(state: ComponentState): string {
  return PRESENTATION[state].label;
}

/** Propriedades de [`ComponentCard`]. */
export interface ComponentCardProps {
  /** Estado reportado pelo daemon. */
  readonly status: ComponentStatus;
}

/** Cartão de um componente, com o motivo quando ele não está pronto. */
export function ComponentCard({ status }: ComponentCardProps): ReactNode {
  const presentation = PRESENTATION[status.state];
  const needsReason = status.state !== 'ready';

  return (
    <li className="flex min-w-0 flex-col gap-1.5 border border-line bg-raised p-2.5 rounded-hair">
      <div className="flex items-center gap-2">
        <StateDot state={status.state} />
        <span className="truncate text-[12px] text-ink">{status.name}</span>
        <span className={`label ml-auto ${presentation.tone}`}>{presentation.label}</span>
      </div>

      {needsReason ? (
        <p className="font-sans text-[11px] leading-snug text-ink-dim">
          {status.detail ?? (
            <span className="text-warn">
              o daemon não informou o motivo — o contrato exige `detail` fora de `ready`
            </span>
          )}
        </p>
      ) : null}

      {status.restarts > 0 ? (
        <p className="label">
          {status.restarts} {status.restarts === 1 ? 'reinício' : 'reinícios'}
        </p>
      ) : null}
    </li>
  );
}
