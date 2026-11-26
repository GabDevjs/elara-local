/**
 * Espera exponencial com jitter.
 *
 * Vive no SDK porque quem reconecta ao daemon — o stream de eventos aqui
 * dentro e o agent-runtime lá fora — precisa das mesmas garantias: nunca girar
 * em loop apertado sobre um socket ausente, nunca crescer sem teto, e nunca
 * fazer vários processos baterem no daemon no mesmo milissegundo depois de ele
 * voltar.
 */

/** Parâmetros da curva de espera. */
export interface BackoffOptions {
  /** Espera da primeira tentativa, em ms. Padrão: 250. */
  readonly initialMs?: number;
  /** Teto da espera, em ms. Padrão: 30_000. */
  readonly maxMs?: number;
  /** Multiplicador entre tentativas. Padrão: 2. */
  readonly factor?: number;
  /**
   * Fração de variação aleatória aplicada ao valor final, entre 0 e 1.
   * Padrão: 0.2 (±20%).
   */
  readonly jitter?: number;
}

/** Valores usados quando o chamador não especifica. */
const DEFAULTS = { initialMs: 250, maxMs: 30_000, factor: 2, jitter: 0.2 } as const;

/**
 * Quanto esperar antes da tentativa `attempt` (contada a partir de zero).
 *
 * `random` é injetável para que o teste possa afirmar o valor exato em vez de
 * uma faixa.
 */
export function computeBackoffDelay(
  attempt: number,
  options: BackoffOptions = {},
  random: () => number = Math.random,
): number {
  const initialMs = options.initialMs ?? DEFAULTS.initialMs;
  const maxMs = options.maxMs ?? DEFAULTS.maxMs;
  const factor = options.factor ?? DEFAULTS.factor;
  const jitter = clamp(options.jitter ?? DEFAULTS.jitter, 0, 1);

  const safeAttempt = Math.max(0, Math.trunc(attempt));
  // `factor ** attempt` estoura para Infinity depois de algumas dezenas de
  // tentativas; o `min` antes do jitter garante que a conta continue finita.
  const base = Math.min(maxMs, initialMs * factor ** safeAttempt);
  const spread = base * jitter;

  return Math.round(clamp(base - spread + random() * spread * 2, 0, maxMs));
}

/**
 * Dorme `ms`, acordando imediatamente se `signal` abortar.
 *
 * Resolve — em vez de rejeitar — no aborto: quem espera um backoff já vai
 * checar o sinal logo em seguida, e uma exceção só faria o chamador escrever
 * um `try` para ignorá-la.
 */
export function delay(ms: number, signal?: AbortSignal): Promise<void> {
  if (signal?.aborted === true) return Promise.resolve();

  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      signal?.removeEventListener('abort', onAbort);
      resolve();
    }, ms);

    function onAbort(): void {
      clearTimeout(timer);
      resolve();
    }

    signal?.addEventListener('abort', onAbort, { once: true });
  });
}

/** Prende `value` ao intervalo `[min, max]`. */
function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}
