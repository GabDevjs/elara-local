/**
 * Resolução dos caminhos do Elara segundo a XDG Base Directory Specification.
 *
 * Precisa concordar **byte a byte** com o que o daemon Rust calcula: se o
 * agent-runtime procurar o socket em um lugar e o daemon criar em outro, o
 * sintoma é um `ENOENT` que não diz nada sobre a causa. Por isso a resolução
 * está num só lugar e é testada com e sem cada variável de ambiente.
 */
import { homedir } from 'node:os';
import { join } from 'node:path';

/** Caminhos que o Elara usa em disco. */
export interface ElaraPaths {
  /** Socket Unix onde o `agentd` atende. */
  readonly socket: string;
  /** Diretório do socket, útil para `mkdir` antes de conectar. */
  readonly runtimeDir: string;
  /** Banco, perfis de hardware e logs persistidos. */
  readonly dataDir: string;
  /** Configuração escrita pelo usuário. */
  readonly configDir: string;
}

/** Entradas da resolução, todas injetáveis para que os testes não dependam da máquina. */
export interface ResolvePathsOptions {
  /** Ambiente a consultar. Padrão: `process.env`. */
  readonly env?: Readonly<Record<string, string | undefined>>;
  /** Diretório do usuário. Padrão: `os.homedir()`. */
  readonly home?: string;
  /**
   * UID usado no fallback de `XDG_RUNTIME_DIR`. Padrão: `process.getuid()`.
   *
   * Injetável porque `process.getuid` não existe no Windows e porque o teste
   * precisa de um valor previsível.
   */
  readonly uid?: number;
}

/**
 * Nome do diretório do Elara dentro de cada base XDG.
 *
 * Uma constante, e não literais espalhados, porque os quatro caminhos têm de
 * mudar juntos se algum dia isto virar outra coisa.
 */
const APP_DIR = 'elara';

/**
 * Resolve todos os caminhos de uma vez.
 *
 * Ordem de precedência, igual à do daemon: variável específica do Elara
 * (`ELARA_SOCKET`, `ELARA_DATA_DIR`) → variável XDG → padrão da especificação.
 */
export function resolveElaraPaths(options: ResolvePathsOptions = {}): ElaraPaths {
  const env = options.env ?? process.env;
  const home = options.home ?? homedir();
  const uid = options.uid ?? currentUid();

  const runtimeBase = absoluteOrNull(env.XDG_RUNTIME_DIR) ?? `/run/user/${uid}`;
  const runtimeDir = join(runtimeBase, APP_DIR);

  // `ELARA_SOCKET` é o caminho do arquivo, não de um diretório: quem o define
  // (o `scripts/dev.mjs`, por exemplo) escolheu o nome inteiro.
  const explicitSocket = nonEmpty(env.ELARA_SOCKET);
  const socket = explicitSocket ?? join(runtimeDir, 'agentd.sock');

  const dataDir =
    nonEmpty(env.ELARA_DATA_DIR) ??
    join(absoluteOrNull(env.XDG_DATA_HOME) ?? join(home, '.local', 'share'), APP_DIR);

  const configDir = join(absoluteOrNull(env.XDG_CONFIG_HOME) ?? join(home, '.config'), APP_DIR);

  return {
    socket,
    // Quando o socket vem de `ELARA_SOCKET`, o diretório dele é o que interessa
    // para criar a pasta antes do bind — não o `XDG_RUNTIME_DIR/elara`.
    runtimeDir: explicitSocket === null ? runtimeDir : dirNameOf(explicitSocket),
    dataDir,
    configDir,
  };
}

/**
 * A especificação XDG manda ignorar valores relativos.
 *
 * Não é preciosismo: um `XDG_DATA_HOME=data` herdado de um script faria o
 * daemon e o runtime resolverem caminhos diferentes conforme o diretório de
 * trabalho de cada processo.
 */
function absoluteOrNull(value: string | undefined): string | null {
  const trimmed = nonEmpty(value);
  if (trimmed === null) return null;
  return trimmed.startsWith('/') ? trimmed : null;
}

/** Variável vazia é tratada como ausente, como faz o shell. */
function nonEmpty(value: string | undefined): string | null {
  return value !== undefined && value !== '' ? value : null;
}

/**
 * Diretório que contém `filePath`.
 *
 * Usa `join(path, '..')` em vez de `dirname` para normalizar barras
 * duplicadas do mesmo jeito que os outros caminhos daqui.
 */
function dirNameOf(filePath: string): string {
  return join(filePath, '..');
}

/**
 * UID do processo, ou `0` onde `getuid` não existe.
 *
 * O `0` não é um chute: em plataforma sem UID a variável `XDG_RUNTIME_DIR`
 * precisa estar definida de qualquer forma, e o caminho gerado aqui serve só
 * para a mensagem de erro dizer onde se procurou.
 */
function currentUid(): number {
  return process.getuid?.() ?? 0;
}
