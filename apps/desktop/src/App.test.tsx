/**
 * Testes de integração da janela.
 *
 * Renderizam o `App` inteiro com `invoke` e `listen` dublados, de modo que o
 * caminho exercitado é o real: comando da ponte → validação zod → estado →
 * tela. Testar componentes com dados já validados deixaria de fora justamente
 * a fronteira que mais quebra quando o daemon muda de versão.
 */
import { fireEvent, render, screen, within } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.hoisted(() => vi.fn());
const listenMock = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }));
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));

import { App } from './App.js';
import {
  hardwarePayload,
  historyPayload,
  resourcesPayload,
  socketPayload,
  statusPayload,
  unreachableFailure,
} from './test-fixtures.js';

/** Dublê de `invoke` que responde com os payloads de exemplo. */
function respondWithFixtures(): void {
  invokeMock.mockImplementation((command: string) => {
    switch (command) {
      case 'daemon_socket_path':
        return Promise.resolve(socketPayload);
      case 'daemon_status':
        return Promise.resolve(statusPayload);
      case 'resources_latest':
        return Promise.resolve(resourcesPayload);
      case 'hardware_profile':
        return Promise.resolve(hardwarePayload);
      case 'hardware_history':
        return Promise.resolve(historyPayload);
      default:
        return Promise.reject(new Error(`comando inesperado no teste: ${command}`));
    }
  });
}

/**
 * Rejeita com um objeto simples, como o `invoke` do Tauri faz.
 *
 * O `BridgeError` do Rust vira `{ code, message }` na travessia do IPC — não
 * um `Error`. Rejeitar com um `Error` aqui testaria um caminho que a aplicação
 * nunca vê.
 */
function rejectLikeBridge(failure: { code: string; message: string }): Promise<never> {
  // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors -- ver acima
  return Promise.reject(failure);
}

beforeEach(() => {
  invokeMock.mockReset();
  listenMock.mockReset();
  listenMock.mockResolvedValue(() => undefined);
});

describe('painel com dados do daemon', () => {
  it('mostra uso de CPU, memória e o estado de cada componente', async () => {
    respondWithFixtures();
    render(<App />);

    // Uso agregado de CPU, no formato brasileiro.
    expect(await screen.findByText('42,5')).toBeInTheDocument();

    // Memória comprometida e o orçamento efetivo do Elara, lado a lado.
    expect(screen.getByText('37,6')).toBeInTheDocument();
    expect(screen.getAllByText('orçamento Elara').length).toBeGreaterThan(0);

    // Componentes supervisionados, com o motivo de quem não está pronto.
    expect(screen.getByText('hardware')).toBeInTheDocument();
    expect(screen.getByText('agent-runtime')).toBeInTheDocument();
    expect(screen.getByText('degradado')).toBeInTheDocument();
    expect(
      screen.getByText('o runtime respondeu ao heartbeat com healthy=false'),
    ).toBeInTheDocument();

    // A faixa de estado confirma que os instrumentos estão ligados.
    expect(screen.getByText('online')).toBeInTheDocument();
    expect(screen.getByText('48213')).toBeInTheDocument();
  });

  it('mostra travessão, e não zero, quando a sonda não entregou o campo', async () => {
    respondWithFixtures();
    render(<App />);

    // Espera o painel montar antes de procurar cartões: a navegação lateral
    // também é uma lista e responderia ao `findAllByRole` imediatamente.
    await screen.findByText('Intel UHD Graphics 770');

    const cards = screen.getAllByRole('listitem');
    const igpu = cards.find((card) => within(card).queryByText('Intel UHD Graphics 770') !== null);
    expect(igpu).toBeDefined();

    // Utilização, VRAM, temperatura e potência: quatro campos nulos, quatro
    // travessões. Nenhum `0` em lugar nenhum do cartão.
    const missing = within(igpu!).getAllByLabelText('valor não disponível');
    expect(missing.length).toBeGreaterThanOrEqual(4);
    for (const element of missing) {
      expect(element).toHaveTextContent('—');
    }

    // E o motivo vem junto, vindo do ProbeWarning da sonda.
    expect(
      within(igpu!).getByText(
        'NVML não expõe métricas da GPU 1: iGPU Intel só tem dados de sysfs.',
      ),
    ).toBeInTheDocument();
  });
});

describe('tela de hardware', () => {
  it('mostra o perfil detectado e nomeia o que ainda não foi medido', async () => {
    respondWithFixtures();
    render(<App />);
    await screen.findByText('42,5');

    fireEvent.click(screen.getByRole('button', { name: 'Hardware' }));

    // Identidade da máquina: no cabeçalho e na tabela de perfis já vistos.
    expect((await screen.findAllByText('b3:7f41c9a2d5e80613')).length).toBeGreaterThan(0);
    expect(screen.getAllByText('AMD Ryzen 5 5600X 6-Core Processor').length).toBeGreaterThan(0);

    // Extensões presentes e ausentes aparecem as duas, com estado explícito.
    expect(screen.getByTitle('AVX2: disponível')).toBeInTheDocument();
    expect(screen.getByTitle('AVX-512F: ausente nesta CPU')).toBeInTheDocument();

    // Nada de estimativa teórica no lugar de uma medição que não existe.
    expect(
      screen.getAllByText('não medido (Fase 7) — o Calibration Engine ainda não existe').length,
    ).toBeGreaterThanOrEqual(2);

    // O orçamento efetivo vem acompanhado da justificativa.
    expect(
      screen.getByText('CPU: 4 threads online menos 1 reservada ao desktop e ao daemon.'),
    ).toBeInTheDocument();

    expect(screen.getByRole('button', { name: 're-detectar' })).toBeInTheDocument();
  });
});

describe('navegação', () => {
  it('anuncia as telas de fases seguintes sem oferecer link quebrado', () => {
    respondWithFixtures();
    render(<App />);

    expect(screen.getByText('Modelos')).toBeInTheDocument();
    expect(screen.getByText('Fase 3')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Modelos' })).toBeNull();
  });
});

describe('daemon offline', () => {
  it('explica onde está o socket e como subir o daemon', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'daemon_socket_path') return Promise.resolve(socketPayload);
      return rejectLikeBridge(unreachableFailure);
    });

    render(<App />);

    expect(await screen.findByText('daemon offline')).toBeInTheDocument();
    expect(screen.getAllByText('/run/user/1000/elara/agentd.sock').length).toBeGreaterThan(0);
    expect(screen.getByText('derivado de XDG_RUNTIME_DIR')).toBeInTheDocument();
    expect(screen.getByText('pnpm dev:daemon')).toBeInTheDocument();
    expect(screen.getByText('daemon.unreachable')).toBeInTheDocument();
    expect(screen.getByText(unreachableFailure.message)).toBeInTheDocument();
  });

  it('não mostra nenhum número de recurso enquanto está offline', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'daemon_socket_path') return Promise.resolve(socketPayload);
      return rejectLikeBridge(unreachableFailure);
    });

    render(<App />);
    await screen.findByText('daemon offline');

    expect(screen.queryByText('42,5')).toBeNull();
    expect(screen.queryByText('uso agregado')).toBeNull();
  });
});

describe('divergência de contrato', () => {
  it('vira estado de erro visível, com o campo culpado', async () => {
    invokeMock.mockImplementation((command: string) => {
      if (command === 'daemon_socket_path') return Promise.resolve(socketPayload);
      if (command === 'daemon_status') {
        // Um daemon de outra versão: `components` deixou de ser lista.
        return Promise.resolve({ ...statusPayload, components: 'nenhum' });
      }
      return Promise.resolve(resourcesPayload);
    });

    render(<App />);

    expect(await screen.findByText('contract.mismatch')).toBeInTheDocument();
    expect(screen.getByText(/components/u)).toBeInTheDocument();
  });
});
