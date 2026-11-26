/**
 * Rotas da janela.
 *
 * Sem `react-router`: são duas telas, e um estado no componente raiz descreve
 * isso melhor do que uma tabela de rotas. As telas de fases seguintes já
 * aparecem na navegação — desabilitadas e com a fase em que chegam — porque
 * esconder o mapa do produto faz o usuário procurar o que não existe.
 */

/** Telas que existem hoje. */
export type RouteId = 'dashboard' | 'hardware';

/** Entrada da navegação lateral. */
export interface NavEntry {
  /** Rota alvo, ou `null` quando a tela ainda não existe. */
  readonly route: RouteId | null;
  /** Rótulo visível. */
  readonly label: string;
  /** O que a tela faz, em uma linha. */
  readonly hint: string;
  /** Fase em que a tela chega; `null` para as que já existem. */
  readonly phase: string | null;
}

/**
 * Navegação completa, na ordem em que as telas são construídas.
 *
 * Os números de fase das telas futuras seguem a ordem do roteiro do produto;
 * o único fixado por contrato é a Fase 7, do Calibration Engine, citada em
 * `crates/hardware/src/types.rs`.
 */
export const NAV_ENTRIES: readonly NavEntry[] = [
  {
    route: 'dashboard',
    label: 'Painel',
    hint: 'CPU, memória e GPU ao vivo',
    phase: null,
  },
  {
    route: 'hardware',
    label: 'Hardware',
    hint: 'perfil detectado desta máquina',
    phase: null,
  },
  {
    route: null,
    label: 'Modelos',
    hint: 'catálogo local e download',
    phase: 'Fase 3',
  },
  {
    route: null,
    label: 'Tarefa',
    hint: 'execução de uma tarefa única',
    phase: 'Fase 4',
  },
  {
    route: null,
    label: 'Agentes',
    hint: 'definição e memória dos agentes',
    phase: 'Fase 5',
  },
  {
    route: null,
    label: 'Workflows',
    hint: 'composição de múltiplos agentes',
    phase: 'Fase 6',
  },
  {
    route: null,
    label: 'Ajustes',
    hint: 'orçamento de recursos e preferências',
    phase: 'Fase 8',
  },
];
