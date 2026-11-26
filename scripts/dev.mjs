#!/usr/bin/env node
/**
 * Orquestrador de desenvolvimento.
 *
 *   pnpm dev                  # daemon + desktop
 *   pnpm dev --only daemon
 *   pnpm dev --only desktop
 *
 * Não usamos `concurrently` de propósito: o ciclo de vida aqui tem ordem
 * (pacotes → daemon pronto → desktop) e precisa derrubar tudo junto no Ctrl-C.
 * Um wrapper genérico não faz nenhuma das duas coisas.
 */
import { spawn, spawnSync } from 'node:child_process';
import { existsSync, mkdirSync, rmSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';
import { connect } from 'node:net';
import { setTimeout as delay } from 'node:timers/promises';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');

const DIM = '\x1b[2m';
const CYAN = '\x1b[36m';
const RED = '\x1b[31m';
const RESET = '\x1b[0m';

const onlyIndex = process.argv.indexOf('--only');
const only = onlyIndex === -1 ? 'all' : process.argv[onlyIndex + 1];
if (!['all', 'daemon', 'desktop'].includes(only)) {
  console.error(`--only aceita: daemon, desktop. Recebido: ${only}`);
  process.exit(2);
}

// O socket vive no runtime dir do usuário; caminhos de socket Unix têm limite
// rígido de ~107 bytes, então evitamos aninhar dentro do repositório.
const runtimeDir = process.env.XDG_RUNTIME_DIR ?? `/run/user/${process.getuid?.() ?? 1000}`;
const socketDir = join(runtimeDir, 'elara-dev');
const socketPath = process.env.ELARA_SOCKET ?? join(socketDir, 'agentd.sock');
const dataDir = process.env.ELARA_DATA_DIR ?? join(ROOT, 'data', 'dev');

const childEnv = {
  ...process.env,
  ELARA_SOCKET: socketPath,
  ELARA_DATA_DIR: dataDir,
  ELARA_LOG: process.env.ELARA_LOG ?? 'info,agentd=debug,elara_hardware=debug',
  ELARA_LOG_FORMAT: process.env.ELARA_LOG_FORMAT ?? 'pretty',
  // Em dev o daemon supervisiona o runtime rodando direto do TypeScript.
  ELARA_RUNTIME_CMD:
    process.env.ELARA_RUNTIME_CMD ?? `pnpm --silent --filter @elara/agent-runtime start`,
  ELARA_RUNTIME_CWD: process.env.ELARA_RUNTIME_CWD ?? ROOT,
};

/** @type {{ name: string, child: import('node:child_process').ChildProcess }[]} */
const running = [];
let shuttingDown = false;

function step(message) {
  console.log(`${CYAN}▸${RESET} ${message}`);
}

function run(name, command, args, options = {}) {
  const child = spawn(command, args, {
    cwd: options.cwd ?? ROOT,
    env: childEnv,
    stdio: 'inherit',
    ...options,
  });
  running.push({ name, child });
  child.on('exit', (code, signal) => {
    if (shuttingDown) return;
    const why = signal ? `sinal ${signal}` : `código ${code}`;
    console.error(`\n${RED}✗${RESET} ${name} terminou (${why}). Derrubando o resto.`);
    void shutdown(code ?? 1);
  });
  return child;
}

function runBlocking(label, command, args) {
  step(label);
  const result = spawnSync(command, args, { cwd: ROOT, env: childEnv, stdio: 'inherit' });
  if (result.status !== 0) {
    console.error(`${RED}✗${RESET} falhou: ${command} ${args.join(' ')}`);
    process.exit(result.status ?? 1);
  }
}

/** Resolve quando o daemon aceitar conexão no socket, ou rejeita no timeout. */
async function waitForSocket(path, timeoutMs = 30_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const connected = await new Promise((resolve) => {
      const sock = connect(path);
      sock.once('connect', () => {
        sock.destroy();
        resolve(true);
      });
      sock.once('error', () => {
        sock.destroy();
        resolve(false);
      });
    });
    if (connected) return;
    await delay(150);
  }
  throw new Error(`daemon não atendeu em ${path} após ${timeoutMs} ms`);
}

async function shutdown(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;
  for (const { child } of running) {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  }
  // Dá uma janela curta para desligamento ordenado antes do SIGKILL.
  await delay(1500);
  for (const { child } of running) {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  }
  process.exit(code);
}

process.on('SIGINT', () => void shutdown(0));
process.on('SIGTERM', () => void shutdown(0));

// --- sequência --------------------------------------------------------------

runBlocking('verificando ambiente', 'node', ['scripts/check-env.mjs']);

mkdirSync(socketDir, { recursive: true, mode: 0o700 });
mkdirSync(dataDir, { recursive: true });

// Um socket órfão de um crash anterior impede o bind. Só removemos se ninguém
// estiver atendendo nele.
if (existsSync(socketPath)) {
  const alive = await new Promise((resolve) => {
    const sock = connect(socketPath);
    sock.once('connect', () => {
      sock.destroy();
      resolve(true);
    });
    sock.once('error', () => {
      sock.destroy();
      resolve(false);
    });
  });
  if (alive) {
    console.error(`${RED}✗${RESET} já existe um daemon atendendo em ${socketPath}`);
    process.exit(1);
  }
  rmSync(socketPath);
}

runBlocking('compilando pacotes TypeScript', 'pnpm', [
  '--filter',
  './packages/*',
  '--filter',
  '@elara/agent-runtime',
  'build',
]);

if (only === 'all' || only === 'daemon') {
  runBlocking('compilando agentd', 'cargo', ['build', '-p', 'agentd']);
  step(`iniciando agentd  ${DIM}${socketPath}${RESET}`);
  run('agentd', join(ROOT, 'target', 'debug', 'agentd'), []);
  await waitForSocket(socketPath);
  step('agentd pronto');
}

if (only === 'all' || only === 'desktop') {
  step('iniciando desktop (Tauri + Vite)');
  run('desktop', 'pnpm', ['--filter', '@elara/desktop', 'tauri', 'dev'], { cwd: ROOT });
}

step(`tudo no ar. Ctrl-C encerra. ${DIM}logs abaixo${RESET}\n`);
