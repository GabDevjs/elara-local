/**
 * Parser de `text/event-stream`.
 *
 * Escrito à mão porque o `EventSource` do Node não aceita `dispatcher`, e sem
 * isso não há como falar por Unix socket. A implementação segue a seção
 * "interpreting an event stream" do HTML Standard, o que importa em três
 * pontos que quebram parsers ingênuos:
 *
 * - um evento pode ter **várias** linhas `data:`, e elas se juntam com `\n`;
 * - `\r\n`, `\n` e `\r` são todos terminadores válidos, e um `\r` no fim do
 *   chunk pode ser a primeira metade de um `\r\n` que ainda não chegou;
 * - o `id` é um estado do stream, não do evento: uma vez definido, vale para
 *   todos os eventos seguintes até ser redefinido.
 */

/** Um evento completo extraído do stream. */
export interface SseFrame {
  /** Último `id:` visto, ou `null` se o stream nunca enviou um. */
  readonly id: string | null;
  /** Valor de `event:`, ou `null` quando o stream omitiu (o padrão é `message`). */
  readonly event: string | null;
  /** Linhas `data:` já unidas por `\n`. */
  readonly data: string;
}

/**
 * Máquina de estados incremental do stream.
 *
 * Uma instância por conexão: o `id` corrente e o buffer de linha parcial são
 * estado da conexão, e reaproveitar o parser entre reconexões misturaria dados
 * de dois streams.
 */
export class SseParser {
  /** Sobra do último chunk: uma linha que ainda não terminou. */
  #pending = '';

  /** Linhas `data:` acumuladas do evento em construção. */
  #data: string[] = [];

  /** Valor de `event:` do evento em construção. */
  #eventType = '';

  /** Último `id:` visto — estado do stream, preservado entre eventos. */
  #lastId: string | null = null;

  /** Última diretiva `retry:`, em ms, ou `null` se o servidor não sugeriu. */
  #retryMs: number | null = null;

  /** Reconexão sugerida pelo servidor, em ms. */
  get retryMs(): number | null {
    return this.#retryMs;
  }

  /** Último `id:` recebido; é o que deve ir no header `Last-Event-ID`. */
  get lastEventId(): string | null {
    return this.#lastId;
  }

  /**
   * Consome um pedaço do stream e devolve os eventos que se completaram.
   *
   * Um chunk pode conter zero, um ou vários eventos, e pode terminar no meio
   * de uma linha — daí o buffer interno.
   */
  feed(chunk: string): SseFrame[] {
    const text = this.#pending + chunk;
    const frames: SseFrame[] = [];

    let lineStart = 0;
    let cursor = 0;

    while (cursor < text.length) {
      const char = text[cursor];

      if (char === '\n') {
        const frame = this.#consumeLine(text.slice(lineStart, cursor));
        if (frame !== null) frames.push(frame);
        cursor += 1;
        lineStart = cursor;
        continue;
      }

      if (char === '\r') {
        // Um `\r` no fim do texto ainda pode virar `\r\n`: só dá para decidir
        // com o próximo chunk.
        if (cursor + 1 === text.length) break;

        const frame = this.#consumeLine(text.slice(lineStart, cursor));
        if (frame !== null) frames.push(frame);
        cursor += text[cursor + 1] === '\n' ? 2 : 1;
        lineStart = cursor;
        continue;
      }

      cursor += 1;
    }

    this.#pending = text.slice(lineStart);
    return frames;
  }

  /**
   * Processa uma linha completa; devolve o evento quando a linha era o
   * separador em branco que fecha um evento.
   */
  #consumeLine(line: string): SseFrame | null {
    if (line === '') return this.#dispatch();

    // Linha iniciada por `:` é comentário. Serve de keep-alive e nada mais.
    if (line.startsWith(':')) return null;

    const separator = line.indexOf(':');
    const field = separator === -1 ? line : line.slice(0, separator);
    const raw = separator === -1 ? '' : line.slice(separator + 1);
    // Exatamente um espaço depois dos dois-pontos é sintaxe, não conteúdo.
    const value = raw.startsWith(' ') ? raw.slice(1) : raw;

    switch (field) {
      case 'event':
        this.#eventType = value;
        break;
      case 'data':
        this.#data.push(value);
        break;
      case 'id':
        // A especificação manda ignorar ids com NUL, não rejeitar o stream.
        if (!value.includes('\0')) this.#lastId = value;
        break;
      case 'retry':
        if (/^\d+$/u.test(value)) this.#retryMs = Number(value);
        break;
      default:
        // Campo desconhecido é ignorado — é assim que o formato evolui sem
        // quebrar clientes antigos.
        break;
    }

    return null;
  }

  /** Fecha o evento em construção, se houver algum `data:` acumulado. */
  #dispatch(): SseFrame | null {
    if (this.#data.length === 0) {
      // Sem `data`, o separador em branco só zera o tipo — não há evento.
      this.#eventType = '';
      return null;
    }

    const frame: SseFrame = {
      id: this.#lastId,
      event: this.#eventType === '' ? null : this.#eventType,
      data: this.#data.join('\n'),
    };

    this.#data = [];
    this.#eventType = '';
    return frame;
  }
}
