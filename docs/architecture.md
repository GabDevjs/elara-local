# Arquitetura

Este documento descreve o que **existe hoje** (Fases 1 e 2). Onde uma peça é de
fase futura, ela aparece marcada — nunca descrita como se estivesse pronta.

## Os três processos

| Processo            | Onde                 | Linguagem       | Papel                                                          |
| ------------------- | -------------------- | --------------- | -------------------------------------------------------------- |
| **`agentd`**        | `crates/agentd`      | Rust            | dono do estado: hardware, recursos, eventos, banco, supervisão |
| **desktop**         | `apps/desktop`       | Rust + React    | janela Tauri 2; lê o daemon, não decide nada                   |
| **`agent-runtime`** | `apps/agent-runtime` | TypeScript/Node | processo supervisionado; hospedará os agentes (Fase 5–6)       |

O `agentd` é o único processo com estado durável. A janela pode fechar e abrir, o
agent-runtime pode morrer e voltar; o daemon continua com o perfil detectado, o
histórico de fingerprints e o banco.

```
   ┌───────────────────────────────────┐   ┌─────────────────────────────┐
   │ apps/desktop                      │   │ apps/agent-runtime          │
   │                                   │   │                             │
   │  React 19 (webview)               │   │  main.ts → runSession()     │
   │      │ invoke / listen            │   │      │                      │
   │      ▼                            │   │      ▼                      │
   │  src-tauri (ponte Rust)           │   │  @elara/sdk (undici Pool)   │
   └──────────────┬────────────────────┘   └──────────────┬──────────────┘
                  │  HTTP/1.1 + SSE                       │  HTTP/1.1
                  │  hyper 1 sobre UnixStream             │  socketPath
                  └───────────────┬───────────────────────┘
                                  ▼
              /run/user/<uid>/elara/agentd.sock      srw------- (0600)
                                  │                 dentro de um dir 0700
                                  ▼
   ┌──────────────────────────────────────────────────────────────────┐
   │ crates/agentd  —  axum 0.8 sobre tokio::net::UnixListener        │
   │                                                                  │
   │   api/routes.rs   9 rotas /v1 + fallbacks + middleware de versão │
   │   state.rs        perfil em cache, sessões de runtime, componentes│
   │   db/             SQLite (rusqlite bundled), migrações por pragma│
   │   hardware.rs     detecta, persiste, publica                     │
   │   supervisor.rs   opcional: spawna e vigia o agent-runtime  ─────┼──┐
   └───────┬──────────────────────┬───────────────────┬───────────────┘  │
           │                      │                   │                  │
           ▼                      ▼                   ▼                  │
   crates/hardware        crates/telemetry     crates/process-manager ◄──┘
   sondas /proc /sys      logging, bus,        spawn, backoff,
   NVML, statvfs          amostrador           SIGTERM → prazo → SIGKILL
```

O `agent-runtime` tem duas origens possíveis: ou o `agentd` o inicia (quando
`ELARA_RUNTIME_CMD` está definida — é o que o `scripts/dev.mjs` faz), ou ele sobe
por fora e simplesmente se registra. Nos dois casos a relação com o daemon é a
mesma: `POST /v1/runtime/register`, depois heartbeats. Sem
`ELARA_RUNTIME_CMD` o componente `agent-runtime` fica em `unavailable` e o daemon
segue funcionando normalmente.

## Quem fala com quem, e por qual transporte

| De → Para                  | Transporte                                          | O quê                                                         |
| -------------------------- | --------------------------------------------------- | ------------------------------------------------------------- |
| webview → ponte Tauri      | `invoke` (IPC do Tauri)                             | seis comandos, todos devolvendo JSON cru ou `{code, message}` |
| ponte Tauri → `agentd`     | HTTP/1.1 sobre Unix socket (hyper 1 + `TokioIo`)    | uma conexão por requisição, timeout de 10 s                   |
| ponte Tauri → webview      | eventos Tauri `elara://event`, `elara://connection` | cada `EventEnvelope` do SSE, sem alteração                    |
| `agent-runtime` → `agentd` | HTTP/1.1 sobre Unix socket (`undici.Pool`)          | registro + heartbeat a cada 2,5 s                             |
| `agentd` → `agent-runtime` | processo filho: `argv`, `stdout`/`stderr` em pipe   | nenhum protocolo — só ciclo de vida e sinais                  |

Não há nenhuma porta TCP em nenhuma direção, em momento algum.

O SSE tem uma única conexão viva por processo cliente: na janela, a ponte abre o
stream uma vez e o `DaemonProvider` do React o distribui para todos os
componentes. Se cada tela assinasse por conta própria, trocar de tela zeraria o
histórico do gráfico — justamente o que o painel existe para mostrar.

## Camadas

```
┌──────────────────────────────────────────────────────────────── existe ─┐
│ UI          apps/desktop        Painel · Hardware                       │
│ SDK         packages/sdk        AgentdClient · SSE · backoff            │
│ Contrato    packages/schemas    tipos gerados (ts-rs) + zod             │
│ IPC         /v1 sobre Unix socket, versionado por header               │
│ Daemon      crates/agentd       estado, banco, eventos, supervisão      │
│ Telemetria  crates/telemetry    logging · event bus · amostrador        │
│ Processos   crates/process-manager                                      │
│ Hardware    crates/hardware     sondas · fingerprint · EffectiveResources│
└─────────────────────────────────────────────────────────────────────────┘

┌────────────────────────────────────────────────────── fases seguintes ──┐
│ Model Manager     catálogo, download e verificação de pesos    Fase 3   │
│ Execução de tarefa                                             Fase 4   │
│ Agent · Memory                                                 Fase 5   │
│ Workflow · Tool                                                Fase 6   │
│ Calibration Engine  mede banda de RAM e leitura de disco       Fase 7   │
│ Resource Broker     consome EffectiveResources e reparte       —        │
│ Motor de inferência llama.cpp                                  —        │
│ Escalation para nuvem                                          —        │
└─────────────────────────────────────────────────────────────────────────┘
```

Os nomes do bloco inferior não são invenção deste documento: `Resource Broker`,
`Calibration Engine` e `llama.cpp` estão citados nos comentários de
`crates/hardware/src/types.rs` como os consumidores previstos dos campos que já
existem. A numeração das fases vem de `apps/desktop/src/lib/navigation.ts` e é
tratada com a ressalva descrita em [roadmap.md](roadmap.md).

Nenhuma dessas peças tem enum, struct, rota ou tabela reservada no código atual.
Isso é deliberado: um `ApiErrorKind::ModelNotFound` que ninguém emite, ou uma
tabela `models` vazia, seria documentação falsa disfarçada de preparação.

## Ciclo de vida do daemon

A ordem de subida (`crates/agentd/src/server.rs`) não é arbitrária — cada passo
depende do anterior:

```
1. configuração   Cli + ambiente XDG          falha ⇒ mensagem legível, exit != 0
2. logging        console + arquivo diário     falha ⇒ exit != 0
3. runtime tokio  multi-thread
4. banco          abre e migra                 falha ⇒ exit != 0
5. gravador       assina o bus                 (antes da detecção, de propósito)
6. hardware       detecta e persiste           falha ⇒ componente Failed, daemon VIVE
7. amostrador     ResourceSampler a cada 1 s
8. supervisor     agent-runtime, se configurado
9. socket         bind + chmod 0600            falha ⇒ exit != 0
```

O passo 6 é o único que falha sem derrubar o processo, e a razão é direta: uma
máquina com `/proc/cpuinfo` ilegível ainda precisa de um daemon de pé para dizer
isso na tela. O `GET /v1/hardware` responde `503 hardware.unavailable` e o
`GET /v1/status` explica o motivo.

O passo 5 vem antes do 6 porque o `EventBus` é um `broadcast`: ele só entrega a
quem já estava assinando. Se o gravador subisse depois, o `hardware.detected` da
inicialização — justamente o evento que um diagnóstico posterior mais precisa —
nunca chegaria ao banco.

O desligamento desfaz na ordem inversa: `SIGTERM`/`SIGINT` → publica
`daemon.stopping` → marca o estado → `axum` para de aceitar conexões (prazo de
3 s para drenar) → amostrador → `agent-runtime` (SIGTERM, 5 s, SIGKILL) →
gravador de eventos (2 s) → remove o socket. Streams SSE não dependem do prazo de
drenagem: eles observam o mesmo canal `watch` e se fecham sozinhos, porque um
stream SSE por construção nunca termina por conta própria e seguraria o
desligamento para sempre.

## Decisões

### Por que um daemon separado, e não tudo dentro do Tauri

1. **O estado sobrevive à janela.** Detectar hardware custa dezenas de
   milissegundos (35 ms nesta máquina); carregar um modelo, nas fases seguintes,
   custará segundos e gigabytes. Amarrar isso ao ciclo de vida de uma janela
   significaria pagar tudo de novo a cada abertura.
2. **Há mais de um cliente.** A janela e o `agent-runtime` já falam com o daemon
   hoje, pelo mesmo contrato. Um CLI futuro entra sem nada novo.
3. **Isolamento de falha.** O processo que supervisiona outros processos não pode
   morrer junto com um webview que travou.
4. **Testabilidade.** Os testes de integração sobem o daemon inteiro — mesma
   ordem de bootstrap, mesmo banco, mesmos eventos — em um socket temporário,
   trocando apenas o `HardwareProvider` por um mock. Isso não seria possível se a
   inicialização estivesse presa a `tauri::Builder`.

### Por que Unix socket, e não TCP em `localhost`

- **O controle de acesso vira um problema de sistema de arquivos.** Socket `0600`
  dentro de um diretório `0700`: só o dono do processo consegue conectar. A
  alternativa em TCP seria inventar um esquema de token — código de segurança
  escrito à mão, que é exatamente o tipo de código que se quer evitar.
- **Nenhuma porta é aberta.** Não colide com outra aplicação, não aparece num
  scan, não é alcançável de outro contêiner na mesma rede, e não existe o modo de
  falha "porta ocupada por um processo alheio".
- **O caminho é auditável.** `ls -l` mostra dono e permissão; `lsof` mostra quem
  está conectado. Uma porta em `127.0.0.1` não distingue clientes.

O custo é que o cliente precisa **saber o caminho**. Ele é resolvido pela mesma
regra XDG em três implementações independentes — `crates/agentd/src/config.rs`,
`packages/shared/src/paths.ts` e `apps/desktop/src-tauri/src/daemon.rs` — e cada
uma tem testes que fixam os mesmos bytes de saída. Uma divergência aqui produz um
`ENOENT` num socket, que é o erro que menos diz sobre a própria causa.

Consequência visível: como o protocolo HTTP/1.1 exige um header `Host` que não
tem significado aqui, o SDK usa `http://elara.invalid` (domínio reservado pela
RFC 6761, que nunca resolve — se alguma configuração errada desviar o cliente
para TCP, ele falha em vez de conversar com um servidor qualquer) e a ponte do
desktop usa `elara.localhost`.

### Por que a ponte do Tauri é um proxy fino

Os comandos em `apps/desktop/src-tauri/src/commands.rs` devolvem
`serde_json::Value` repassado sem tocar. A ponte **não** conhece
`HardwareProfile`, `ResourceSnapshot` nem nenhum tipo de domínio.

O motivo é aritmético: se ela desserializasse em structs, os mesmos tipos
existiriam em **três** lugares — o Rust do daemon, o TypeScript gerado e o Rust da
ponte. Duas cópias já divergem; três divergem mais rápido. Com o proxy fino há um
só produtor (o daemon) e um só validador (o zod no webview), e a validação
acontece contra schemas gerados do próprio daemon.

O que a ponte de fato faz, e que a UI não teria como fazer sozinha:

- resolve o socket a partir do ambiente (`ELARA_SOCKET` ou `XDG_RUNTIME_DIR`) e
  informa **a procedência**, para que a tela de daemon offline diga onde procurar;
- aplica um orçamento de 10 s por requisição unária — o stream SSE
  propositalmente não tem timeout depois dos cabeçalhos;
- traduz falha de transporte em código estável (`daemon.unreachable`,
  `daemon.timeout`, `daemon.socket-unresolved`, `daemon.transport`,
  `daemon.invalid-response`) e preserva o `code` do daemon quando ele respondeu;
- mantém a task que reconecta o SSE com backoff de 500 ms a 5 s e emite
  `{ connected: bool }` só nas transições.

Nenhum outro módulo do webview chama `invoke`: só `apps/desktop/src/lib/bridge.ts`.
É essa restrição que garante que toda resposta passe por `parseOrThrow` antes de
virar estado de React.

### Por que os tipos nascem em Rust

O daemon é quem produz os dados. A struct que ele serializa **é** a definição — o
resto é tradução. Inverter isso (escrever o tipo em TypeScript e implementar em
Rust) criaria uma definição que nada verifica contra o que realmente sai do
socket.

O fluxo completo, incluindo por que existem duas representações em TypeScript
(tipo gerado + schema zod) e o que fazer quando um tipo muda, está em
[contracts.md](contracts.md).

### Por que o event bus é `broadcast`, e não uma fila

Um produtor lógico, vários consumidores independentes: o stream SSE de cada
cliente, o gravador do `SQLite` e o log. Com uma fila, um consumidor lento
aplicaria contrapressão em quem publica — e quem publica é, entre outros, o
supervisor de processos. Perder eventos é aceitável; travar o supervisor não é.

A perda é **detectável em vez de silenciosa**: cada envelope carrega um
`sequence` monotônico, então um cliente que vê 41 depois de 38 sabe exatamente
quantos perdeu e decide sozinho se recarrega o estado. O `EventEnvelope` é
também o que o `Last-Event-ID` do SSE usa para retomar.

Os eventos de alta frequência (`resources.sampled`, um por segundo) são
publicados no bus e entregues aos assinantes ao vivo, mas **nunca** gravados no
banco nem no log — caso contrário o `SQLite` cresceria 86 400 linhas por dia
descrevendo o passado que ninguém vai consultar.
