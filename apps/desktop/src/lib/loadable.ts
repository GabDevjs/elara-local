/**
 * Estado de um dado que vem do daemon.
 *
 * Um tipo único para os quatro estados obrigatórios de toda tela — carregando,
 * pronto, erro e (via [`BridgeFailure`]) daemon offline — porque separá-los em
 * `data`, `loading` e `error` soltos permite a combinação impossível
 * "carregando com erro e dado ao mesmo tempo".
 */
import type { BridgeFailure } from './bridge.js';

/** Dado do daemon em um dos seus três estados possíveis. */
export type Loadable<T> =
  | { readonly state: 'loading' }
  | { readonly state: 'ready'; readonly data: T }
  | { readonly state: 'error'; readonly failure: BridgeFailure };

/** Constante reutilizável para o estado inicial. */
export const LOADING: Loadable<never> = { state: 'loading' };

/** Dado já carregado, ou `null` enquanto não houver. */
export function loadedValue<T>(loadable: Loadable<T>): T | null {
  return loadable.state === 'ready' ? loadable.data : null;
}

/** Falha atual, ou `null` quando não há erro. */
export function failureOf<T>(loadable: Loadable<T>): BridgeFailure | null {
  return loadable.state === 'error' ? loadable.failure : null;
}
