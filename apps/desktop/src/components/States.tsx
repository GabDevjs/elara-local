/**
 * Os quatro estados que toda tela tem de saber mostrar.
 *
 * Daemon offline não é exceção: é o primeiro estado que quem roda só a janela
 * encontra. Por isso ele tem tela própria, com o caminho do socket que a ponte
 * está tentando e o comando que sobe o daemon — a tela responde a pergunta que
 * o usuário faria em seguida.
 */
import type { ReactNode } from 'react';

import type { BridgeFailure, SocketInfo } from '../lib/bridge.js';
import type { Loadable } from '../lib/loadable.js';
import { Button } from './Button.js';

/** Comando que sobe o daemon a partir da raiz do repositório. */
const START_COMMAND = 'pnpm dev:daemon';

/** Moldura comum dos estados vazios: centralizada, contida, sem ilustração. */
function Frame({ children }: { children: ReactNode }): ReactNode {
  return (
    <div className="flex min-h-[280px] w-full items-center justify-center p-8">
      <div className="flex w-full max-w-xl flex-col gap-4">{children}</div>
    </div>
  );
}

/** Carregando: barra indeterminada e o que está sendo esperado. */
export function LoadingState({ what }: { what: string }): ReactNode {
  return (
    <Frame>
      <p className="label text-live">carregando</p>
      <div className="h-px w-full overflow-hidden bg-line">
        <div className="h-full w-1/3 animate-pulse bg-live" />
      </div>
      <p className="font-sans text-[13px] text-ink-dim">{what}</p>
    </Frame>
  );
}

/** Vazio: o daemon respondeu, e a resposta não tem conteúdo. */
export function EmptyState({ title, detail }: { title: string; detail: string }): ReactNode {
  return (
    <Frame>
      <p className="label">{title}</p>
      <p className="font-sans text-[13px] leading-relaxed text-ink-dim">{detail}</p>
    </Frame>
  );
}

/** Erro de aplicação: o daemon está no ar e recusou ou falhou. */
export function ErrorState({
  failure,
  onRetry,
}: {
  failure: BridgeFailure;
  onRetry: () => void;
}): ReactNode {
  return (
    <Frame>
      <p className="label text-fail">falha</p>
      <p className="num text-[12px] text-fail">{failure.code}</p>
      <pre className="max-h-64 overflow-auto border border-line bg-surface p-3 text-[12px] leading-relaxed whitespace-pre-wrap text-ink-dim rounded-hair">
        {failure.message}
      </pre>
      <div>
        <Button onClick={onRetry}>tentar de novo</Button>
      </div>
    </Frame>
  );
}

/** Propriedades de [`OfflineState`]. */
export interface OfflineStateProps {
  /** Onde a ponte procura o daemon. */
  readonly socket: Loadable<SocketInfo>;
  /** Falha que levou ao diagnóstico, quando há uma. */
  readonly failure: BridgeFailure | null;
  /** Nova tentativa de contato. */
  readonly onRetry: () => void;
}

/** Daemon fora do ar, com caminho do socket e o comando para subi-lo. */
export function OfflineState({ socket, failure, onRetry }: OfflineStateProps): ReactNode {
  return (
    <Frame>
      <div className="flex items-center gap-2">
        <span aria-hidden="true" className="block size-1.5 bg-fail" />
        <p className="label text-fail">daemon offline</p>
      </div>

      <p className="font-sans text-[13px] leading-relaxed text-ink-dim">
        O <span className="font-mono text-ink">agentd</span> não está atendendo. A janela continua
        funcionando, mas não há hardware nem recursos para mostrar até ele subir.
      </p>

      <dl className="flex flex-col gap-2 border border-line bg-surface p-3 rounded-hair">
        <div className="flex flex-col gap-0.5">
          <dt className="label">socket</dt>
          <dd className="num break-all text-[12px] text-ink">
            {socket.state === 'ready' ? (
              socket.data.socketPath
            ) : (
              <span className="text-ink-faint">não foi possível determinar</span>
            )}
          </dd>
          <dd className="font-sans text-[11px] text-ink-faint">
            {socket.state === 'ready'
              ? socket.data.source === 'env'
                ? 'definido pela variável ELARA_SOCKET'
                : 'derivado de XDG_RUNTIME_DIR'
              : 'defina ELARA_SOCKET ou garanta XDG_RUNTIME_DIR no ambiente'}
          </dd>
        </div>

        {failure !== null ? (
          <div className="flex flex-col gap-0.5 border-t border-line pt-2">
            <dt className="label">última tentativa</dt>
            <dd className="num text-[11px] text-fail">{failure.code}</dd>
            <dd className="font-sans text-[11px] leading-snug text-ink-dim">{failure.message}</dd>
          </div>
        ) : null}
      </dl>

      <div className="flex flex-col gap-1.5">
        <p className="label">para subir o daemon, na raiz do repositório</p>
        <code className="border border-line-strong bg-raised px-3 py-2 text-[12px] text-live select-all rounded-hair">
          {START_COMMAND}
        </code>
      </div>

      <div className="flex items-center gap-3">
        <Button onClick={onRetry} primary>
          tentar de novo
        </Button>
        <span className="font-sans text-[11px] text-ink-faint">
          a janela também reconecta sozinha assim que o daemon aparecer
        </span>
      </div>
    </Frame>
  );
}
