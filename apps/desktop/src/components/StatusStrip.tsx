/**
 * Faixa de estado, sempre visível no topo.
 *
 * Um painel de instrumentos precisa dizer, o tempo todo, se os instrumentos
 * estão ligados. Sem esta faixa, um número parado é ambíguo entre "a máquina
 * está ociosa" e "a leitura morreu há dez minutos".
 */
import type { ReactNode } from 'react';

import { formatUptime } from '../lib/format.js';
import { useDaemon } from '../lib/daemon-session.js';

/** Aparência de cada estado de conexão. */
const CONNECTION = {
  probing: { label: 'sondando', dot: 'bg-live', text: 'text-live' },
  online: { label: 'online', dot: 'bg-ok', text: 'text-ok' },
  offline: { label: 'offline', dot: 'bg-fail', text: 'text-fail' },
} as const;

/** Um campo da faixa: rótulo minúsculo acima, valor monoespaçado abaixo. */
function Field({
  label,
  value,
  wide = false,
  display = 'flex',
}: {
  label: string;
  value: string;
  wide?: boolean;
  /** Classe de exibição; permite esconder campos secundários em janela estreita. */
  display?: string;
}): ReactNode {
  return (
    <div className={`min-w-0 items-baseline gap-2 ${display} ${wide ? 'flex-1' : ''}`}>
      <span className="label">{label}</span>
      <span className="num truncate text-[11px] text-ink-dim" title={value}>
        {value}
      </span>
    </div>
  );
}

/** Renderiza a faixa de estado. */
export function StatusStrip(): ReactNode {
  const { connection, status, socket, sampleIntervalMs } = useDaemon();
  const presentation = CONNECTION[connection];
  const daemon = status.state === 'ready' ? status.data : null;

  return (
    <header className="flex h-11 shrink-0 items-center gap-5 overflow-hidden border-b border-line bg-surface px-4">
      <div className="flex shrink-0 items-center gap-2">
        <span aria-hidden="true" className={`block size-1.5 ${presentation.dot}`} />
        <span className="label text-ink-dim">daemon</span>
        <span className={`label ${presentation.text}`}>{presentation.label}</span>
      </div>

      {daemon !== null ? (
        <>
          <Field label="versão" value={daemon.version} />
          <Field label="uptime" value={formatUptime(daemon.uptimeSeconds)} />
          <Field label="pid" value={String(daemon.pid)} display="hidden lg:flex" />
          <Field
            label="protocolo"
            value={String(daemon.protocolVersion)}
            display="hidden xl:flex"
          />
          {sampleIntervalMs !== null ? (
            <Field
              label="amostragem"
              value={`${String(sampleIntervalMs)} ms`}
              display="hidden xl:flex"
            />
          ) : null}
          <Field label="socket" value={daemon.socketPath} wide />
        </>
      ) : (
        <Field
          label="socket"
          value={socket.state === 'ready' ? socket.data.socketPath : 'indeterminado'}
          wide
        />
      )}
    </header>
  );
}
