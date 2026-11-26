/**
 * Dado que a sonda não conseguiu obter.
 *
 * O ponto do componente é ser impossível de confundir com um zero. Um `0 MiB`
 * de VRAM leva a decidir que não há GPU; um travessão com "NVML não carregou"
 * ao lado leva a instalar o driver. É a diferença entre a interface mentir e a
 * interface admitir o que não sabe.
 */
import type { ProbeWarning } from '@elara/schemas';
import type { ReactNode } from 'react';

import { DASH } from '../lib/format.js';

/** Propriedades de [`Missing`]. */
export interface MissingProps {
  /** Explicação curta do porquê, normalmente vinda de um `ProbeWarning`. */
  readonly reason: string | null;
  /** Mostra o motivo ao lado do travessão, e não só no `title`. */
  readonly inline?: boolean;
}

/** Renderiza o travessão e, quando há, o motivo. */
export function Missing({ reason, inline = false }: MissingProps): ReactNode {
  const title = reason ?? 'não detectado nesta máquina';
  return (
    <span className="inline-flex items-baseline gap-2" title={title}>
      <span className="num text-ink-faint" aria-label="valor não disponível">
        {DASH}
      </span>
      {inline && reason !== null ? (
        <span className="font-sans text-[11px] leading-snug text-ink-faint">{reason}</span>
      ) : null}
    </span>
  );
}

/**
 * Encontra o aviso que explica um campo ausente.
 *
 * A correspondência é por prefixo de `source` (`gpu`, `storage.mounts`) porque
 * é assim que as sondas nomeiam a si mesmas — um `gpu.nvml` responde por
 * qualquer campo de GPU que tenha ficado vazio.
 */
export function reasonFor(warnings: readonly ProbeWarning[], sourcePrefix: string): string | null {
  const match = warnings.find((warning) => warning.source.startsWith(sourcePrefix));
  return match?.message ?? null;
}
