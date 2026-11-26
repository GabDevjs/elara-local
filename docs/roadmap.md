# Roteiro

## Sobre a numeração das fases

O documento de especificação do produto **não está neste repositório**. O que
existe são âncoras no código, e este roteiro não vai além delas:

| Fase | Tema                                          | Âncora                                                                                              |
| ---- | --------------------------------------------- | --------------------------------------------------------------------------------------------------- |
| 1    | Base: daemon, IPC, telemetria, supervisão     | `api/types.rs`, `session.ts`, `migrations.rs` — **entregue**                                        |
| 2    | Perfil de hardware                            | `api/types.rs`, `migrations.rs` — **entregue**                                                      |
| 3    | Modelos: catálogo local e download            | `navigation.ts`                                                                                     |
| 4    | Execução de uma tarefa única                  | `navigation.ts`                                                                                     |
| 5    | Agentes: definição e memória                  | `navigation.ts`                                                                                     |
| 6    | Workflows: composição de múltiplos agentes    | `navigation.ts`; `agent-runtime/src/main.ts` cita Agent, Workflow, Tool e Memory chegando na Fase 6 |
| 7    | Calibration Engine                            | `hardware/src/types.rs`, `probes/ram.rs`, `probes/storage.rs`, telas do desktop                     |
| 8    | Ajustes: orçamento de recursos e preferências | `navigation.ts`                                                                                     |
| 9    | —                                             | **sem âncora no repositório**                                                                       |
| 10   | —                                             | **sem âncora no repositório**                                                                       |

Duas peças estão nomeadas no código sem número de fase associado:

- **Resource Broker** — citado em `crates/hardware/src/types.rs` como o
  consumidor previsto de `EffectiveResources`;
- **motor de inferência (llama.cpp)** — citado em `CpuFeatures` como o motivo de
  aquelas extensões, e só aquelas, entrarem no perfil.

O enunciado do trabalho também menciona **escalation para nuvem** como fase
posterior. Nada no código a referencia, então ela não recebe número aqui.

Preencher as duas linhas vazias com conteúdo plausível seria exatamente o erro
que este projeto evita em toda parte: um número inventado que parece um dado.

---

## Entregue

### Fase 1 — Base

**Daemon (`crates/agentd`)**

- HTTP/1.1 em Unix socket `0600` dentro de diretório `0700`, com axum 0.8; nove
  rotas sob `/v1` mais fallbacks de 404 e 405 no mesmo formato de erro.
- Versionamento por header `x-elara-protocol`, anunciado em toda resposta,
  inclusive nas de erro.
- Stream SSE com `id`/`event`/`data`, retomada por `Last-Event-ID`, keep-alive de
  15 s e fechamento ordenado no desligamento.
- `SQLite` com migrações versionadas por `PRAGMA user_version`, tabelas `STRICT`,
  retenção do histórico de eventos por contagem (2 000).
- Registro e heartbeat de runtimes, com expiração por três intervalos perdidos.
- Bootstrap ordenado e desligamento ordenado (ver [architecture.md](architecture.md)).
- Bind que remove socket órfão, recusa arquivo comum sem apagá-lo e nunca
  sequestra o socket de um daemon vivo.

**Telemetria (`crates/telemetry`)**

- `tracing` com console configurável (JSON ou `pretty`) e arquivo sempre JSON,
  com rotação diária em UTC. Sem `tracing-appender`: cada evento é um `write`
  direto, de modo que um `SIGKILL` não perde as últimas linhas.
- Event bus `broadcast` com sequência monotônica e buffer de recentes.
- `ResourceSampler`: contadores brutos → percentuais, com tratamento explícito de
  Δ zero (`0.0`, não `NaN`), contadores que retrocedem e relógio para trás.

**Supervisão (`crates/process-manager`)**

- Um handle por processo: `argv` sem shell, backoff exponencial com teto,
  desligamento SIGTERM → prazo → SIGKILL, `stdout`/`stderr` reemitidos como
  eventos do `tracing`.

**TypeScript**

- `@elara/schemas` — 46 tipos gerados + 46 schemas zod com asserção de
  equivalência em tempo de compilação.
- `@elara/shared` — logger estruturado, caminhos XDG, taxonomia de erros.
- `@elara/sdk` — `AgentdClient` sobre Unix socket, SSE com reconexão e
  classificação de continuidade, backoff.
- `@elara/agent-runtime` — registra-se, mantém heartbeat a 50 % do intervalo
  anunciado, encerra limpo em SIGTERM/SIGINT.

**Desktop**

- Ponte Rust (hyper 1 sobre `UnixStream`) como proxy fino, com códigos de erro
  estáveis e retransmissão do SSE para o webview.
- React 19 + Tailwind 4; telas Painel e Hardware; navegação que mostra as telas
  de fases futuras desabilitadas, com a fase em que chegam.

### Fase 2 — Hardware

- Sondas de host, CPU, RAM, GPU, armazenamento e topologia, todas com raiz
  injetável.
- GPU em três fontes correlacionadas por PCI (NVML → `nvidia-smi` → sysfs), com
  backends exigindo evidência.
- cgroup v1 e v2, com herança do limite mais restritivo até a raiz.
- Fingerprint estável de 128 bits sobre atributos invariantes.
- `EffectiveResources` com `rationale` explicando cada número.

Detalhes em [hardware-profiler.md](hardware-profiler.md).

### Verificação, medida neste repositório

```
cargo test --workspace   396 testes  (agentd 123 · hardware 191+25+8 · telemetry 29
                                      · process-manager 13+1 · desktop 5 · doc 1)
pnpm -r test             114 testes  (schemas 26 · shared 26 · sdk 38
                                      · desktop 13 · agent-runtime 11)
```

Nenhum deles exige GPU, root, rede ou uma máquina específica.

---

## O que ficou explicitamente de fora

Não há stub, esqueleto, enum reservado, tabela vazia nem rota placeholder para
nada disto:

| Ausente                                      | Por quê                                                                   |
| -------------------------------------------- | ------------------------------------------------------------------------- |
| llama.cpp e qualquer inferência              | Fase seguinte. `runtime/` está vazio e nada o preenche.                   |
| Catálogo e download de modelos               | Fase 3. `models/` está vazio.                                             |
| Agent, Workflow, Tool, Memory                | Fases 5–6. O `agent-runtime` diz isso no próprio módulo.                  |
| Calibration Engine                           | Fase 7. Por isso `measuredBandwidthGbps` e `measuredReadMbps` são `null`. |
| Resource Broker                              | Sem fase atestada. `EffectiveResources` já existe esperando por ele.      |
| Escalation para nuvem                        | Sem fase atestada.                                                        |
| Variantes de `Event` para modelo/task/agente | Um enum cheio de variantes mortas esconde o que já funciona.              |
| Códigos de erro não emitidos                 | `ApiErrorKind` só tem as sete classes que algum handler produz.           |
| Tabelas `models`, `tasks`, `workflows`       | Entram junto com o código que as escreve.                                 |

A regra é a mesma em todos os casos: **um tipo que ninguém usa é documentação
falsa disfarçada de preparação**. Ele passa a impressão de que o subsistema
existe, aparece na busca de quem procura o comportamento, e envelhece antes de
alguém precisar dele.

Três desvios do enunciado original que foram deliberados e continuam valendo:

1. **Não existe `runtime.unknown_session` (404).** Uma sessão desconhecida
   responde `200` com `acknowledged: false`, porque um 4xx aqui é fatal para o
   `agent-runtime` (`rethrowIfFatal` em `session.ts`) e transformaria "o daemon
   reiniciou" em "o runtime morreu".
2. **Não há 409 em lugar nenhum.** Nenhuma rota das Fases 1–2 tem conflito de
   estado a reportar.
3. **O limite de corpo é `DefaultBodyLimit`, não `RequestBodyLimitLayer`.** O
   layer do `tower-http` troca o tipo do corpo e exigiria a feature
   `map-request-body`, não habilitada no workspace. O efeito é idêntico: todo
   corpo que este daemon lê passa por um extrator `Json`.

---

## Riscos técnicos reais

Encontrados durante a implementação. Nenhum é hipotético.

### Contrato

- **`u64` vira `bigint` em todo o TypeScript gerado.** `uptimeSeconds`,
  `ramTotalMb`, `heartbeatIntervalMs`, `sampleIntervalMs` e outros. Funciona hoje
  porque os schemas zod aceitam `number | bigint | string`, mas vaza para o
  consumidor e **impede que qualquer corpo de requisição futuro tenha `u64`**
  (`JSON.stringify` lança em `bigint`). A correção seria `#[ts(type = "number")]`
  nos campos `u64`; não foi aplicada porque afeta os três arquivos de contrato de
  uma vez e é decisão do dono do contrato TypeScript.
- **`Event::DaemonStarted` emite `socket_path` em snake_case**, enquanto as
  outras variantes usam camelCase — falta o `rename_all` na variante. Os schemas
  zod espelham o que o Rust de fato emite; corrigir o Rust quebrará o typecheck,
  que é o comportamento desejado.
- **`PROTOCOL_VERSION` vive em três arquivos** (Rust do daemon, `api.ts`, ponte
  do desktop) e nada os gera a partir de um só. O que existe é o teste de
  integração que verifica o header em toda resposta.
- **`pnpm bindings` apaga o diretório de saída antes de regenerar.** Rodá-lo com
  um crate que não compila remove os tipos dele e quebra o build de TypeScript
  com um erro que não se parece com a causa. O procedimento em
  [contracts.md](contracts.md) começa por `cargo check --workspace`.

### Hardware

- **O fingerprint muda se o driver NVIDIA sumir.** Sem NVML o modelo cai para
  `NVIDIA 10de:2d04` e a VRAM vira desconhecida. É honesto, mas invalida
  benchmarks por um motivo irrelevante — uma atualização de kernel pendente de
  reboot, por exemplo. O Calibration Engine precisa tratar isso.
- **Montagens de `/dev/mapper/*` (LVM/LUKS) não são associadas ao disco físico.**
  Exigiria caminhar os `holders` do sysfs. Hoje geram aviso `info` e ficam fora.
- **O cache é dividido pelo número de sockets**, assumindo sockets simétricos.
  Verdadeiro em qualquer máquina de mesa, falso em NUMA heterogêneo.
- **CPUs híbridas dependem de `topology/core_type` ou `cpu_capacity`.** Kernels
  que só expõem `/sys/devices/system/cpu/types/*` reportam `hybrid: null`.
- **As fixtures não cobrem o caminho NVML** (ele é desligado fora da raiz real).
  Quem o exercita são os testes de máquina real, que afirmam só invariantes.
- **Regenerar as fixtures é manual.** O script que as gerou não ficou no
  repositório; editar um arquivo de fixture à mão é o fluxo esperado.

### Daemon

- **`Database::open` e as migrações rodam de forma bloqueante na subida**, sem
  timeout. Não há tráfego ainda, mas um `SQLite` em NFS travado atrasaria a
  subida indefinidamente.
- **Sessões de runtime expiram por _lazy pruning_**, na leitura ou escrita, não
  por uma task de varredura. Não afeta resposta nenhuma, mas uma sessão morta
  ocupa memória em um daemon totalmente ocioso.
- **`register` desaloja a sessão anterior do mesmo `pid`.** Dois runtimes
  distintos com o mesmo pid — impossível na mesma máquina — se atropelariam.
- **A permissão do diretório do socket só é aplicada quando o daemon o cria.**
  Com `ELARA_SOCKET` apontando para um diretório preexistente (`/tmp`, por
  exemplo), o daemon emite um `WARN` e não mexe na permissão — mexer deixaria
  `/tmp` inacessível para o resto do sistema.
- **O desligamento corta conexões presas depois de 3 s.** Streams SSE não
  dependem disso; o prazo cobre o caso patológico de um cliente parado no meio de
  uma resposta.
- **Eventos publicados antes de o gravador subir não seriam persistidos.** Hoje
  o gravador é a primeira coisa depois do barramento e nada se perde, mas quem
  inserir um `publish` mais cedo no `bootstrap` precisa saber: o `broadcast` só
  entrega a quem já assinava.
- **O amostrador chama `provider.detect()` inline** uma vez, no início da task,
  só para obter os nomes das GPUs. São dezenas de milissegundos síncronos dentro
  de uma task assíncrona.

### Supervisão de processos

- **`restarts` não zera com o tempo.** O teto de reinícios vale para a vida
  inteira do handle, não para uma janela deslizante.
- **Saída com código 0 é terminal.** Só código diferente de zero ou sinal
  reinicia. Se algum componente futuro precisar de "restart always", isso vira um
  campo em `RestartPolicy`.
- **Descartar um `SupervisorHandle` sem `shutdown` encerra o filho com prazo fixo
  de 5 s**, e o `Command` usa `kill_on_drop(true)`: órfão é pior que morte
  abrupta.
- **`split_command` não é um shell.** Sem `$VAR`, `~`, glob ou pipe.
  `ELARA_RUNTIME_CMD='pnpm --silent --filter @elara/agent-runtime start'`
  funciona; um comando que dependa de expansão, não. É deliberado: interpretar
  essa string seria executar código arbitrário vindo de variável de ambiente.

### Desktop

- **A janela nunca foi exercitada contra um daemon real de ponta a ponta.** Os
  caminhos que a ponte usa foram conferidos contra `api/routes.rs` e conferem,
  incluindo o `?limit=` do histórico; o que não foi feito é abrir a janela com o
  `agentd` no ar e clicar.
- **`tauri build` não foi executado.** Só `cargo check`, `clippy` e `cargo test`.
  O empacotamento com webview não está verificado.
- **Só existe o tema escuro.** Um `[data-theme="light"]` redeclarando o bloco
  `:root` já basta depois, mas ele não existe.
- **O marcador de orçamento de VRAM só é desenhado por dispositivo quando há
  exatamente uma GPU.** `effective.vramMb` é uma soma, e projetá-la sobre a barra
  de uma GPU entre várias seria mentira. Com duas ou mais, o total aparece só no
  cabeçalho do painel.
- **`daemon.timeout` (10 s) é tratado como "daemon offline" pela UI.** É uma
  escolha: um daemon travado e um ausente pedem a mesma ação do usuário. Se
  alguma rota futura puder demorar mais de 10 s legitimamente, o
  `REQUEST_TIMEOUT` da ponte precisa subir.
- **As fontes são pilhas do sistema.** A CSP do `tauri.conf.json` bloqueia origem
  remota e as dependências estão travadas, então não há arquivo de fonte
  embarcado.
- **Os rótulos de fase das telas futuras não são deriváveis de contrato** — só o
  da Fase 7. Ficaram numa constante única, em `lib/navigation.ts`.

### Portabilidade

- **O projeto é Unix-only, na prática Linux-only.** `/proc`, `/sys`, `statvfs`,
  Unix sockets, bits de permissão, SIGTERM.
- **Há exatamente três `unsafe` em código de produção**, cada um com
  `#[allow(unsafe_code, reason = …)]` e comentário `SAFETY`:
  `libc::statvfs` (espaço livre real), `libc::kill` com SIGTERM (a `std` e o
  `tokio` só expõem SIGKILL) e `libc::getuid` (o fallback `/run/user/{uid}`).
- **A aritmética do `statvfs` assume Linux 64 bits.**
- **O teste de SIGTERM ignorado depende de `SIG_IGN` ser herdado através de
  `exec`** — vale em Linux/glibc. Por isso mora em um binário de teste separado,
  com um único teste.

### Ferramental

- **O TypeScript está preso na 6.0.3, não na 7.x.** O `typescript-eslint@8.65`
  declara `peerDependencies.typescript: ">=4.8.4 <6.1.0"`, e não há release
  (nem `canary`) que aceite a 7. Subir o TypeScript hoje significaria abrir mão
  do lint com type-checking — que é justamente o que sustenta as regras
  `no-unsafe-*` na fronteira IPC. Reavaliar quando o `typescript-eslint`
  publicar suporte à 7.x.
- **O Node 22 é obrigatório e não é o padrão de muitas máquinas.** O pnpm 11
  exige `>= 22.13`. O `.nvmrc`, o campo `engines` e o `check-env.mjs` existem
  para transformar isso num erro legível em vez de um stack trace do pnpm.
- **Compilar os crates em `CARGO_TARGET_DIR` separados custa caro em disco.**
  Cada target dir independente recompila as dependências: a árvore do Tauri
  sozinha passa de 2 GiB. Em máquina com pouco espaço livre, prefira o target
  dir compartilhado do workspace e aceite a serialização pelo lock do cargo.

### Dívida menor

- `tower` e `tracing-subscriber` continuam declarados no `Cargo.toml` do `agentd`
  sem uso direto (chegam via `tower-http` e `elara-telemetry`).
- `serde` está derivado em `ProcessSpec` e `RestartPolicy`, mas ninguém serializa
  esses tipos ainda.
- `daemon_health` é exposto pela ponte do desktop e nenhuma tela o consome: o
  `daemon_status` responde a mesma pergunta com mais dados.
- Não existe rota de desregistro de runtime. No desligamento, o `agent-runtime`
  manda um heartbeat com `healthy: false` e `detail: "runtime encerrando"` — é o
  mais próximo disponível.
- Os testes de processo usam tempo real (backoffs de 5 a 120 ms) em vez de
  `tokio::time::pause()`: o auto-advance do tempo pausado corre contra a I/O real
  de `Child::wait` e tornaria o resultado não determinístico.
- Na suíte do `agentd` o amostrador roda a 25 ms para que `/v1/resources` produza
  um snapshot rápido; isso torna os testes de SSE ruidosos por construção, e eles
  filtram pelo nome do evento.
