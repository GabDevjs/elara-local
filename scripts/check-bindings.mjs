#!/usr/bin/env node
/**
 * Falha se as definições TypeScript geradas estiverem defasadas em relação às
 * structs Rust.
 *
 * As bindings são commitadas para que o TypeScript compile sem exigir a
 * toolchain Rust. Isso só é seguro se alguém verificar que elas continuam
 * correspondendo à fonte — é o que este script faz em CI:
 *
 *   1. copia `packages/schemas/src/generated/` para um diretório temporário;
 *   2. roda `pnpm bindings` (que regenera a partir do Rust);
 *   3. compara.
 */
import { execFileSync } from 'node:child_process';
import { cpSync, mkdtempSync, readdirSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const GENERATED = join(ROOT, 'packages', 'schemas', 'src', 'generated');

const RED = '\x1b[31m';
const GREEN = '\x1b[32m';
const RESET = '\x1b[0m';

const backup = mkdtempSync(join(tmpdir(), 'elara-bindings-'));
cpSync(GENERATED, backup, { recursive: true });

try {
  execFileSync('cargo', ['test', '--workspace', 'export_bindings'], {
    cwd: ROOT,
    stdio: ['ignore', 'ignore', 'inherit'],
  });

  const before = new Map(
    readdirSync(backup).map((f) => [f, readFileSync(join(backup, f), 'utf8')]),
  );
  const after = new Map(
    readdirSync(GENERATED).map((f) => [f, readFileSync(join(GENERATED, f), 'utf8')]),
  );

  const drift = [];
  for (const [name, content] of after) {
    if (!before.has(name)) drift.push(`+ ${name} (novo)`);
    else if (before.get(name) !== content) drift.push(`~ ${name} (mudou)`);
  }
  for (const name of before.keys()) {
    if (!after.has(name)) drift.push(`- ${name} (removido)`);
  }

  if (drift.length > 0) {
    console.error(`\n${RED}✗ bindings defasadas:${RESET}`);
    for (const line of drift) console.error(`    ${line}`);
    console.error(`\n  Rode ${GREEN}pnpm bindings${RESET} e commite o resultado.\n`);
    process.exit(1);
  }

  console.log(`${GREEN}✓${RESET} bindings em dia (${after.size} arquivos)`);
} finally {
  rmSync(backup, { recursive: true, force: true });
}
