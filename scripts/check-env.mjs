#!/usr/bin/env node
/**
 * Verifica os pré-requisitos de desenvolvimento do Elara.
 *
 * `--soft` apenas avisa (usado no preinstall, para não travar `pnpm install`).
 * Sem flag, sai com código != 0 quando algo obrigatório está faltando.
 */
import { execFileSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const soft = process.argv.includes('--soft');
const root = join(dirname(fileURLToPath(import.meta.url)), '..');

const GREEN = '\x1b[32m';
const YELLOW = '\x1b[33m';
const RED = '\x1b[31m';
const DIM = '\x1b[2m';
const RESET = '\x1b[0m';

/** @type {{ ok: boolean; label: string; detail: string; required: boolean }[]} */
const results = [];

function record(label, required, fn) {
  try {
    const detail = fn();
    results.push({ ok: true, label, detail: detail ?? '', required });
  } catch (error) {
    results.push({ ok: false, label, detail: String(error.message ?? error), required });
  }
}

function run(cmd, args) {
  return execFileSync(cmd, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
}

record('Node.js', true, () => {
  const wanted = readFileSync(join(root, '.nvmrc'), 'utf8').trim();
  const [major, minor] = process.versions.node.split('.').map(Number);
  if (major < 22 || (major === 22 && minor < 13)) {
    throw new Error(
      `${process.version} é antigo demais (pnpm 11 exige >= 22.13). Rode: nvm use ${wanted}`,
    );
  }
  return process.version;
});

record('Rust (cargo)', true, () => run('cargo', ['--version']));
record('rustfmt', true, () => run('cargo', ['fmt', '--version']));
record('clippy', true, () => run('cargo', ['clippy', '--version']));

record('webkit2gtk-4.1 (Tauri)', true, () => {
  const v = run('pkg-config', ['--modversion', 'webkit2gtk-4.1']);
  return `v${v}`;
});
record('libsoup-3.0 (Tauri)', true, () => `v${run('pkg-config', ['--modversion', 'libsoup-3.0'])}`);

record('NVIDIA NVML', false, () => {
  const out = run('nvidia-smi', ['--query-gpu=name,memory.total', '--format=csv,noheader']);
  return out.split('\n')[0];
});
record('ripgrep', false, () => run('rg', ['--version']).split('\n')[0]);

let failed = 0;
console.log(`\n  ${DIM}Elara — verificação de ambiente${RESET}\n`);
for (const r of results) {
  if (r.ok) {
    console.log(`  ${GREEN}✓${RESET} ${r.label.padEnd(24)} ${DIM}${r.detail}${RESET}`);
  } else if (r.required) {
    failed += 1;
    console.log(`  ${RED}✗${RESET} ${r.label.padEnd(24)} ${r.detail}`);
  } else {
    console.log(`  ${YELLOW}○${RESET} ${r.label.padEnd(24)} ${DIM}indisponível (opcional)${RESET}`);
  }
}
console.log('');

if (failed > 0 && !soft) {
  console.error(`  ${RED}${failed} pré-requisito(s) obrigatório(s) faltando.${RESET}\n`);
  process.exit(1);
}
if (failed > 0) {
  console.warn(
    `  ${YELLOW}Aviso: ${failed} pré-requisito(s) faltando — o build vai falhar.${RESET}\n`,
  );
}
