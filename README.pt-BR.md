# Elara — Local Agent OS

[English](README.md) · Português (Brasil)

Runtime local de agentes de IA. A ideia central é uma só: **o hardware da máquina
é a entrada mais importante do sistema**. Antes de escolher um modelo, dividir um
trabalho entre agentes ou decidir quantas camadas descarregar na GPU, o Elara
mede o que existe — núcleos, RAM realmente disponível, VRAM livre, backends de
inferência utilizáveis, limites de cgroup — e compila a partir disso a estratégia
de execução. Nada roda na nuvem por padrão e nada é estimado: um número que não
pôde ser medido fica ausente, com o motivo anexado.

Este repositório contém a **base do sistema**, não o produto completo. Leia a
seção seguinte antes de qualquer outra coisa.

## O que existe hoje

Estão concluídas as **Fases 1 (base) e 2 (hardware)**.

Funcionando, com testes:

- **`agentd`** — daemon local que atende HTTP/1.1 em um Unix socket `0600`. Nove
  rotas sob `/v1`, stream SSE de eventos, `SQLite` com migrações versionadas,
  supervisão opcional do agent-runtime, desligamento ordenado.
- **Perfil de hardware** — CPU (núcleos físicos × threads, caches, extensões que
  o llama.cpp explora, híbrido P/E), RAM, GPU (NVML → `nvidia-smi` → sysfs/PCI),
  armazenamento com espaço livre real via `statvfs`, NUMA, SMT, cgroup v1 e v2,
  virtualização. Mais um **fingerprint estável** da máquina e um orçamento
  efetivo (`EffectiveResources`) com a justificativa de cada número.
- **Telemetria** — logging estruturado com rotação diária, event bus com
  sequência monotônica e amostrador de recursos periódico.
- **Contratos tipados** — os tipos nascem em Rust, viram TypeScript por `ts-rs` e
  são validados em runtime por zod na fronteira IPC.
- **Desktop (Tauri 2 + React 19)** — duas telas: painel ao vivo e perfil de
  hardware. Campos não detectados aparecem como travessão com o motivo, nunca
  como zero.
- **agent-runtime (Node)** — registra-se no daemon, mantém heartbeat e encerra
  limpo. É só isso: ele ainda não hospeda agente nenhum.

**Não existe ainda** — e não há esqueleto, stub ou tipo morto esperando por isso:

- llama.cpp, qualquer motor de inferência, qualquer modelo carregado;
- catálogo, download ou gerenciamento de modelos;
- Agent, Workflow, Tool, Memory ou execução de tarefa;
- Calibration Engine (por isso `measuredBandwidthGbps` e `measuredReadMbps` são
  sempre `null`);
- Resource Broker;
- escalation para nuvem.

O [roteiro](docs/roadmap.md) descreve o que ficou de fora e por quê.

## Pré-requisitos

Linux. O daemon usa Unix sockets, `/proc`, `/sys`, `statvfs(3)`, `getuid(2)` e
sinais POSIX; não há caminho alternativo para outros sistemas operacionais.

| Item                            | Versão               | Obrigatório                              |
| ------------------------------- | -------------------- | ---------------------------------------- |
| Node.js                         | ≥ 22.13 (`.nvmrc`)   | sim — o pnpm 11 não roda em versão menor |
| pnpm                            | 11.13.0              | sim                                      |
| Rust                            | stable, edition 2024 | sim                                      |
| `rustfmt`, `clippy`             | do mesmo toolchain   | sim                                      |
| `libwebkit2gtk-4.1`, `libsoup3` | pacotes `-dev`       | sim — o crate do desktop não linka sem   |
| Driver NVIDIA / NVML            | qualquer             | não — sem ele a GPU cai para sysfs/PCI   |

`pnpm check:env` verifica todos eles e diz qual está faltando.

### Node 22 via nvm

O Node padrão de muitas máquinas ainda é a v20, e o `pnpm install` falha nela.
Antes de qualquer comando `pnpm`:

```sh
export NVM_DIR="$HOME/.nvm"; . "$NVM_DIR/nvm.sh"; nvm use
```

O `nvm use` sem argumento lê o `.nvmrc` do repositório.

## Começando

```sh
pnpm install          # também roda check:env em modo aviso
pnpm dev              # daemon + janela desktop
```

O `pnpm dev` não é um `concurrently`: a sequência tem ordem (verificar ambiente →
compilar pacotes TypeScript → compilar e subir o `agentd` → esperar o socket
aceitar conexão → subir o Tauri) e o Ctrl-C derruba tudo junto. Em
desenvolvimento o socket vai para `$XDG_RUNTIME_DIR/elara-dev/agentd.sock` e os
dados para `data/dev/`, de modo que a instalação real do usuário não é tocada.

```sh
pnpm dev:daemon       # só o agentd
pnpm dev:desktop      # só a janela (assume um daemon já no ar)
pnpm dev:runtime      # só o agent-runtime, em watch
```

## Verificação

```sh
pnpm test             # testes TypeScript + cargo test --workspace
pnpm lint             # eslint + clippy --all-targets -D warnings
pnpm typecheck        # tsc -b em todos os pacotes
pnpm format:check     # prettier + cargo fmt --check
pnpm check            # os quatro acima, na ordem em que o CI roda
```

```sh
pnpm bindings         # regenera packages/schemas/src/generated/ a partir do Rust
pnpm bindings:check   # falha se as bindings commitadas estiverem defasadas
```

As bindings TypeScript são **commitadas** para que o build de TypeScript não
exija a toolchain Rust; `bindings:check` é quem garante que continuam
correspondendo às structs. Detalhes em [docs/contracts.md](docs/contracts.md).

## Estrutura

```
elara/
├── apps/
│   ├── agent-runtime/    processo Node supervisionado pelo daemon
│   └── desktop/          janela Tauri 2 + React 19 (src-tauri/ = ponte Rust)
├── crates/
│   ├── agentd/           o daemon: HTTP no socket, SQLite, estado, supervisão
│   ├── hardware/         contrato do perfil, sondas de /proc e /sys, fingerprint
│   ├── process-manager/  supervisão de um filho: backoff, SIGTERM, prazo, SIGKILL
│   └── telemetry/        logging, event bus e amostrador de recursos
├── packages/
│   ├── schemas/          tipos gerados do Rust + schemas zod que os validam
│   ├── sdk/              cliente do agentd sobre Unix socket, com SSE e backoff
│   └── shared/           logger, caminhos XDG e taxonomia de erros
├── scripts/              check-env, dev, check-bindings, generate-icons
├── docs/                 esta documentação
├── data/                 dados gerados em runtime (conteúdo ignorado pelo git)
├── models/               pesos de modelos — vazio; nada os baixa ainda
└── runtime/              runtimes de inferência — vazio; nada os instala ainda
```

`data/` e `runtime/` são versionados só como estrutura — o `.gitignore` descarta
o conteúdo. `models/` é ignorado por inteiro: pesos nunca entram no repositório,
e a pasta é recriada localmente quando a Fase 3 chegar. Em uma instalação real os
dados vivem em `~/.local/share/elara`, não aqui.

## Documentação

| Documento                                         | Assunto                                                              |
| ------------------------------------------------- | -------------------------------------------------------------------- |
| [architecture.md](docs/architecture.md)           | os três processos, quem fala com quem, e as decisões com o porquê    |
| [hardware-profiler.md](docs/hardware-profiler.md) | cada sonda, sua fonte no Linux, o fingerprint e `EffectiveResources` |
| [ipc-protocol.md](docs/ipc-protocol.md)           | o contrato HTTP sobre Unix socket, rota por rota, com exemplos reais |
| [contracts.md](docs/contracts.md)                 | o fluxo Rust → `ts-rs` → zod e o que fazer quando um tipo muda       |
| [roadmap.md](docs/roadmap.md)                     | as fases, o que foi entregue e os riscos técnicos reais              |

## Licença

Apache-2.0. Ver [LICENSE](LICENSE).
