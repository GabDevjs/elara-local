/**
 * Assinatura dos eventos que a ponte Rust reemite para o webview.
 *
 * São dois canais e eles respondem a perguntas diferentes: `elara://event`
 * transporta o que aconteceu no daemon; `elara://connection` diz se ainda há
 * daemon do outro lado. Sem o segundo, uma queda apareceria como "nenhum
 * evento novo", que é indistinguível de "máquina ociosa".
 */
import { EventEnvelopeSchema, type EventEnvelope } from '@elara/schemas';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { z } from 'zod';

import { asFailure, decode, type BridgeFailure } from './bridge.js';

/** Canal com cada `EventEnvelope` do daemon, sem alteração. */
const EVENT_CHANNEL = 'elara://event';

/** Canal com as transições de conectividade. */
const CONNECTION_CHANNEL = 'elara://connection';

/** Payload de `elara://connection`. */
const ConnectionSchema = z.object({ connected: z.boolean() });

/** Reações de quem assina o stream de eventos. */
export interface EventHandlers {
  /** Chamado para cada envelope válido. */
  readonly onEnvelope: (envelope: EventEnvelope) => void;
  /**
   * Chamado quando um envelope não bate com o contrato.
   *
   * Existe para que a divergência vire uma faixa visível na tela em vez de um
   * evento silenciosamente descartado.
   */
  readonly onInvalid: (failure: BridgeFailure) => void;
}

/** Assina `elara://event`. Devolve a função que cancela a assinatura. */
export function subscribeToEvents(handlers: EventHandlers): Promise<UnlistenFn> {
  return listen<unknown>(EVENT_CHANNEL, (event) => {
    try {
      handlers.onEnvelope(decode(EventEnvelopeSchema, event.payload, EVENT_CHANNEL));
    } catch (error) {
      handlers.onInvalid(asFailure(error));
    }
  });
}

/** Assina `elara://connection`. Devolve a função que cancela a assinatura. */
export function subscribeToConnection(onChange: (connected: boolean) => void): Promise<UnlistenFn> {
  return listen<unknown>(CONNECTION_CHANNEL, (event) => {
    const parsed = ConnectionSchema.safeParse(event.payload);
    // Um payload malformado aqui não tem tela própria: a UI descobre o estado
    // real na próxima chamada de comando, que é a fonte autoritativa.
    if (parsed.success) onChange(parsed.data.connected);
  });
}
