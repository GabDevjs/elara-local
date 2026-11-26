import { describe, expect, it } from 'vitest';

import { resolveElaraPaths } from '../src/paths.js';

const HOME = '/home/tester';
const UID = 1000;

/** Resolve com ambiente controlado — nada aqui pode depender da máquina real. */
function resolve(env: Record<string, string | undefined>) {
  return resolveElaraPaths({ env, home: HOME, uid: UID });
}

describe('resolveElaraPaths', () => {
  it('usa os padrões da especificação quando nada está definido', () => {
    const paths = resolve({});

    expect(paths.socket).toBe('/run/user/1000/elara/agentd.sock');
    expect(paths.runtimeDir).toBe('/run/user/1000/elara');
    expect(paths.dataDir).toBe('/home/tester/.local/share/elara');
    expect(paths.configDir).toBe('/home/tester/.config/elara');
  });

  it('respeita as bases XDG quando definidas', () => {
    const paths = resolve({
      XDG_RUNTIME_DIR: '/run/user/4242',
      XDG_DATA_HOME: '/var/lib/xdg-data',
      XDG_CONFIG_HOME: '/etc/xdg-config',
    });

    expect(paths.socket).toBe('/run/user/4242/elara/agentd.sock');
    expect(paths.dataDir).toBe('/var/lib/xdg-data/elara');
    expect(paths.configDir).toBe('/etc/xdg-config/elara');
  });

  it('ELARA_SOCKET vence o XDG e define o próprio diretório', () => {
    const paths = resolve({
      XDG_RUNTIME_DIR: '/run/user/4242',
      ELARA_SOCKET: '/run/user/1000/elara-dev/agentd.sock',
    });

    expect(paths.socket).toBe('/run/user/1000/elara-dev/agentd.sock');
    expect(paths.runtimeDir).toBe('/run/user/1000/elara-dev');
  });

  it('ELARA_DATA_DIR vence XDG_DATA_HOME', () => {
    const paths = resolve({
      XDG_DATA_HOME: '/var/lib/xdg-data',
      ELARA_DATA_DIR: '/srv/elara/data',
    });

    expect(paths.dataDir).toBe('/srv/elara/data');
  });

  it('ignora bases XDG relativas, como manda a especificação', () => {
    // Um `XDG_DATA_HOME=dados` herdado de um script faria daemon e runtime
    // resolverem caminhos diferentes conforme o cwd de cada processo.
    const paths = resolve({ XDG_DATA_HOME: 'dados', XDG_RUNTIME_DIR: 'tmp' });

    expect(paths.dataDir).toBe('/home/tester/.local/share/elara');
    expect(paths.socket).toBe('/run/user/1000/elara/agentd.sock');
  });

  it('trata variável vazia como ausente', () => {
    const paths = resolve({ ELARA_SOCKET: '', ELARA_DATA_DIR: '', XDG_CONFIG_HOME: '' });

    expect(paths.socket).toBe('/run/user/1000/elara/agentd.sock');
    expect(paths.dataDir).toBe('/home/tester/.local/share/elara');
    expect(paths.configDir).toBe('/home/tester/.config/elara');
  });

  it('resolve contra o ambiente real sem lançar', () => {
    // Não afirma valores: o objetivo é garantir que os padrões injetados
    // (`process.env`, `homedir`, `getuid`) existem de fato.
    const paths = resolveElaraPaths();

    expect(paths.socket.endsWith('.sock')).toBe(true);
    expect(paths.configDir.endsWith('/elara')).toBe(true);
  });
});
