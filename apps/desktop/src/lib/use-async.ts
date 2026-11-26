/**
 * Carregamento de um recurso do daemon com recarga sob demanda.
 *
 * Existe para que cada tela não reescreva o mesmo `useEffect` com guarda de
 * desmontagem — o bug clássico aqui é o `setState` de uma resposta que chegou
 * depois de o componente sair, e ele fica invisível até virar um vazamento.
 */
import { useCallback, useEffect, useRef, useState } from 'react';

import { asFailure } from './bridge.js';
import { LOADING, type Loadable } from './loadable.js';

/** Recurso carregado, com controle de recarga. */
export interface AsyncResource<T> {
  /** Estado atual do dado. */
  readonly value: Loadable<T>;
  /**
   * `true` durante uma recarga que ainda mostra o dado anterior.
   *
   * Distinto de `state === 'loading'`: numa re-detecção de hardware a tela
   * segue exibindo o perfil antigo enquanto o novo não chega.
   */
  readonly refreshing: boolean;
  /** Dispara uma nova execução do carregador. */
  readonly reload: () => void;
}

/**
 * Executa `load` na montagem e a cada `reload`.
 *
 * `load` precisa ser estável (`useCallback`): mudá-la a cada render faria o
 * recurso recarregar em laço.
 */
export function useAsync<T>(load: () => Promise<T>): AsyncResource<T> {
  const [value, setValue] = useState<Loadable<T>>(LOADING);
  const [refreshing, setRefreshing] = useState(true);
  const [token, setToken] = useState(0);

  // Um contador de execuções descarta respostas fora de ordem: sem ele, uma
  // recarga rápida seguida de uma lenta terminaria mostrando a lenta.
  const runId = useRef(0);

  useEffect(() => {
    runId.current += 1;
    const current = runId.current;
    let alive = true;
    setRefreshing(true);

    void load().then(
      (data) => {
        if (!alive || current !== runId.current) return;
        setValue({ state: 'ready', data });
        setRefreshing(false);
      },
      (error: unknown) => {
        if (!alive || current !== runId.current) return;
        setValue({ state: 'error', failure: asFailure(error) });
        setRefreshing(false);
      },
    );

    return () => {
      alive = false;
    };
  }, [load, token]);

  const reload = useCallback(() => {
    setToken((previous) => previous + 1);
  }, []);

  return { value, refreshing, reload };
}

/**
 * Recarrega quando um contador externo avança — e só então.
 *
 * Serve para reagir a `hardware.detected` sem recarregar na montagem: o
 * `useAsync` já buscou o dado uma vez, e disparar de novo faria duas
 * requisições a cada abertura da tela.
 */
export function useReloadOnEpoch(epoch: number, reload: () => void): void {
  const seen = useRef(epoch);

  useEffect(() => {
    if (seen.current !== epoch) {
      seen.current = epoch;
      reload();
    }
  }, [epoch, reload]);
}
