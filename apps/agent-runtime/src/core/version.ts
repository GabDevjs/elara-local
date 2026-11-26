/**
 * Versão anunciada ao daemon.
 *
 * Lida do `package.json` em tempo de execução em vez de embutida numa
 * constante: uma constante duplicada é uma constante que vai divergir, e o
 * daemon usa este número para decidir compatibilidade. Se o arquivo não puder
 * ser lido, o processo falha — anunciar uma versão inventada seria pior que
 * não subir.
 */
import { readFileSync } from 'node:fs';

/**
 * Caminho do `package.json` do pacote.
 *
 * Sobe um nível a partir deste módulo, o que dá certo tanto compilado
 * (`dist/core/version.js`) quanto rodando direto do fonte com `tsx`
 * (`src/core/version.ts`) — as duas árvores têm a mesma profundidade.
 */
const MANIFEST = new URL('../../package.json', import.meta.url);

/**
 * Lê `name` e `version` do manifesto.
 *
 * Exportada como função, e não como constante de módulo, para que o teste
 * possa exercitá-la sem depender da ordem de importação.
 */
export function readPackageIdentity(): { name: string; version: string } {
  const raw = readFileSync(MANIFEST, 'utf8');
  const parsed: unknown = JSON.parse(raw);

  if (typeof parsed !== 'object' || parsed === null) {
    throw new Error(`package.json inválido em ${MANIFEST.pathname}`);
  }

  const { name, version } = parsed as { name?: unknown; version?: unknown };
  if (typeof name !== 'string' || typeof version !== 'string') {
    throw new Error(`package.json sem name/version legíveis em ${MANIFEST.pathname}`);
  }

  return { name, version };
}
