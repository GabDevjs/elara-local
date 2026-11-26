# Protocolo IPC

HTTP/1.1 sobre um Unix socket. Nove rotas sob `/v1`, uma delas em SSE. Nenhuma
porta TCP é aberta em momento algum.

O contrato de tipos está em `crates/agentd/src/api/types.rs` e as rotas em
`crates/agentd/src/api/routes.rs`. Todos os exemplos deste documento foram
capturados de um `agentd` 0.1.0 real, não escritos à mão.

## O socket

| Origem                       | Caminho                                                |
| ---------------------------- | ------------------------------------------------------ |
| `ELARA_SOCKET`               | exatamente o valor (é o **arquivo**, não um diretório) |
| `XDG_RUNTIME_DIR` (absoluto) | `$XDG_RUNTIME_DIR/elara/agentd.sock`                   |
| nenhum dos dois              | `/run/user/<uid>/elara/agentd.sock`                    |

Um `XDG_RUNTIME_DIR` relativo é ignorado, como manda a especificação XDG: um
`XDG_RUNTIME_DIR=runtime` herdado de um script faria daemon e cliente resolverem
caminhos diferentes conforme o diretório de trabalho de cada processo. Variável
vazia vale como ausente.

A mesma regra está implementada em três lugares independentes, cada um com testes
que fixam os mesmos bytes: `crates/agentd/src/config.rs`,
`packages/shared/src/paths.ts` e `apps/desktop/src-tauri/src/daemon.rs`.

### Permissões

```
srw------- 1 biel biel 0 agentd.sock      # 0600
drwx------ 2 biel biel   elara/           # 0700, quando fomos nós que o criamos
```

O controle de acesso é do sistema de arquivos: só o dono do processo consegue
conectar. Não há token, autenticação nem TLS — e é justamente por não haver porta
TCP que não precisa haver.

O `chmod 0600` vem logo depois do `bind`. A janela entre as duas operações não é
explorável porque o diretório é `0700` — **mas só quando o daemon criou o
diretório**. Com um `ELARA_SOCKET=/tmp/x.sock`, o daemon não mexe na permissão de
`/tmp` (isso deixaria `/tmp` inacessível para o resto do sistema) e apenas emite
um `WARN`.

### Socket órfão

Na subida, se o caminho já existe:

| Situação                 | O que acontece                                 |
| ------------------------ | ---------------------------------------------- |
| existe e alguém atende   | erro `AlreadyServing`; o daemon **não** sobe   |
| existe e ninguém atende  | é removido (com `WARN`) e o bind prossegue     |
| existe e não é um socket | erro `NotASocket`; o arquivo **não** é apagado |

A única forma honesta de distinguir órfão de vivo é tentar conectar. No
desligamento ordenado o socket é removido; um socket que sobrevive ao processo
faz a próxima subida parecer "já tem um daemon rodando" até alguém tentar
conectar.

## Versionamento

Cliente e servidor anunciam a versão do contrato no header `x-elara-protocol`.
Hoje: `1`.

- **Toda** resposta carrega o header, inclusive as de erro — é assim que um
  cliente incompatível descobre com quem está falando.
- Requisição **sem** o header é aceita. Um `curl` de diagnóstico, um cliente em
  desenvolvimento ou um proxy que remove headers desconhecidos não merecem um
  400, e a validação de schema pega qualquer divergência real logo depois.
- Requisição com versão **diferente ou ilegível** é recusada com
  `400 protocol.version_mismatch` — antes de o corpo chegar embaralhado ao zod do
  outro lado.

O corpo de `POST /v1/runtime/register` carrega o mesmo número em
`protocolVersion`, e ele é verificado separadamente: o runtime tem de anunciar a
versão certa mesmo que o header tenha sido perdido no caminho.

## Limites

| Limite                               | Valor       | Onde                             |
| ------------------------------------ | ----------- | -------------------------------- |
| Corpo de requisição                  | 64 KiB      | `DefaultBodyLimit`               |
| `?limit=` do histórico               | 1 … 200     | acima do teto, atende com o teto |
| Keep-alive do SSE                    | 15 s        | comentário `:` no stream         |
| Buffer de replay do SSE              | 256 eventos | `EVENT_BUS_CAPACITY`             |
| Drenagem de conexões no desligamento | 3 s         | `DRAIN_TIMEOUT`                  |

O maior corpo legítimo do contrato é um `RuntimeRegisterRequest` com uma lista de
capacidades — centenas de bytes. 64 KiB dá três ordens de grandeza de folga e
ainda impede que um cliente com bug consuma a memória do daemon.

## Rotas

Nos exemplos, `$S` é o caminho do socket.

### `GET /v1/health`

Barata o bastante para polling agressivo: não consulta banco nem hardware. A
pergunta que responde é "há um daemon atendendo neste socket?".

```sh
curl -s -i --unix-socket $S -H 'x-elara-protocol: 1' http://elara.localhost/v1/health
```

```http
HTTP/1.1 200 OK
content-type: application/json
x-elara-protocol: 1

{"ok":true,"version":"0.1.0","protocolVersion":1,"uptimeSeconds":7}
```

`ok` vira `false` durante o desligamento.

### `GET /v1/status`

Visão completa: versão, pid, instante da subida, caminhos e o estado de cada
componente supervisionado, em ordem alfabética.

```json
{
  "version": "0.1.0",
  "protocolVersion": 1,
  "pid": 2499238,
  "startedAt": "2026-07-27T10:52:58.954385494Z",
  "uptimeSeconds": 7,
  "socketPath": "/run/user/1000/elara-docs/agentd.sock",
  "dataDir": "/home/alguem/.local/share/elara",
  "components": [
    {
      "name": "agent-runtime",
      "state": "unavailable",
      "detail": "ELARA_RUNTIME_CMD não está definida; nenhum agent-runtime será iniciado por este daemon",
      "since": "2026-07-27T10:52:58.954383055Z",
      "restarts": 0
    },
    { "name": "database", "state": "ready", "detail": null, "since": "…", "restarts": 0 },
    { "name": "hardware", "state": "ready", "detail": null, "since": "…", "restarts": 0 },
    {
      "name": "telemetry",
      "state": "ready",
      "detail": "amostrando recursos a cada 1000 ms",
      "since": "…",
      "restarts": 0
    }
  ]
}
```

Estados possíveis: `stopped`, `starting`, `ready`, `degraded`, `failed`,
`unavailable`. `detail` é obrigatório sempre que o estado não é `ready` — um
componente degradado sem explicação é um bug de diagnóstico.

Esta é a rota a consultar quando outra falha: `hardware` em `failed` diz por que
`GET /v1/hardware` está devolvendo 503.

### `GET /v1/hardware`

Perfil corrente, servido do cache em memória. **Não redetecta**: a sonda custa
dezenas de milissegundos e o perfil só muda quando a máquina muda.

```json
{
  "profile": {
    "schemaVersion": 1,
    "fingerprint": "a2278b5f8590940d3b0cd045f2dfa711",
    "collectedAt": "2026-07-27T10:52:58.95320904Z",
    "probeDurationMs": 35,
    "host": {
      "hostname": "biel-pc",
      "os": "linux",
      "distribution": "Pop!_OS 24.04 LTS",
      "kernelVersion": "7.0.11-76070011-generic",
      "architecture": "x86_64",
      "virtualization": null
    },
    "cpu": {
      "model": "Intel(R) Core(TM) i5-6400 CPU @ 2.70GHz",
      "vendor": "intel",
      "physicalCores": 4,
      "logicalThreads": 4,
      "onlineThreads": 4,
      "sockets": 1,
      "baseFrequencyMhz": 2700,
      "maxFrequencyMhz": 3300,
      "cache": { "l1dKb": 128, "l1iKb": 128, "l2Kb": 1024, "l3Kb": 6144 },
      "features": { "avx": true, "avx2": true, "fma": true, "avx512f": false, "…": "…" },
      "hybrid": null
    },
    "ram": {
      "totalMb": 32046,
      "availableMb": 17714,
      "measuredBandwidthGbps": null,
      "bandwidthSource": "not-measured"
    },
    "gpus": [
      {
        "index": 0,
        "vendor": "nvidia",
        "model": "NVIDIA GeForce RTX 5060 Ti",
        "pciAddress": "0000:01:00.0",
        "vramTotalMb": 8151,
        "vramAvailableMb": 7331,
        "driverVersion": "580.173.02",
        "runtimeVersion": "13.0",
        "computeCapability": "12.0",
        "backends": ["cuda", "vulkan"],
        "isIntegrated": false,
        "powerLimitWatts": 180.0,
        "source": "nvml"
      }
    ],
    "storage": [
      {
        "device": "sda",
        "model": "KINGSTON SA400S3",
        "kind": "ssd",
        "sizeMb": 228936,
        "rotational": false,
        "mounts": [
          {
            "path": "/",
            "filesystem": "ext4",
            "totalMb": 215185,
            "availableMb": 2983,
            "readOnly": false
          }
        ],
        "measuredReadMbps": null
      }
    ],
    "topology": {
      "numaNodes": [{ "id": 0, "cpus": [0, 1, 2, 3], "memoryMb": 32046 }],
      "simultaneousMultithreading": false,
      "cgroup": {
        "version": "v2",
        "cpuQuotaCores": null,
        "memoryLimitMb": null,
        "memoryUsageMb": 7470
      }
    },
    "effective": {
      "cpuThreads": 3,
      "ramMb": 15057,
      "vramMb": 6597,
      "vramHeadroomRatio": 0.1,
      "rationale": ["CPU: 4 threads online, menos 1 reservada ao sistema → 3", "…"]
    },
    "warnings": []
  },
  "cached": true,
  "firstSeenAt": "2026-07-27T10:52:58.953305046Z"
}
```

O corpo acima está recortado nos campos repetitivos; a forma completa é a de
`HardwareProfile` em `crates/hardware/src/types.rs`. O significado de cada campo,
o fingerprint e a fórmula de `effective` estão em
[hardware-profiler.md](hardware-profiler.md).

**503 `hardware.unavailable`** quando a detecção falhou e não há cache. O daemon
continua de pé — é o único jeito de a UI mostrar _por que_ falhou.

### `POST /v1/hardware/refresh`

Mesmo corpo, com `cached: false` e uma detecção nova. `firstSeenAt` **não** muda
quando o fingerprint é o mesmo: ele registra quando aquele hardware foi visto pela
primeira vez, não quando foi lido pela última.

Sem corpo de requisição.

### `GET /v1/hardware/history?limit=N`

Perfis já vistos nesta máquina, do mais recente para o mais antigo. `limit`
ausente vale 20; a faixa aceita é 1 a 200; acima do teto o pedido é atendido com
o teto, e não recusado — quem pediu "tudo" recebe o máximo que existe.

```json
[
  {
    "fingerprint": "a2278b5f8590940d3b0cd045f2dfa711",
    "firstSeenAt": "2026-07-27T10:52:58.953305046Z",
    "lastSeenAt": "2026-07-27T10:52:58.953305046Z",
    "cpuModel": "Intel(R) Core(TM) i5-6400 CPU @ 2.70GHz",
    "ramTotalMb": 32046,
    "gpuSummary": "NVIDIA GeForce RTX 5060 Ti (8151 MiB)"
  }
]
```

Um fingerprint novo significa que o hardware mudou e que benchmarks anteriores
não valem mais. Uma GPU sem VRAM detectável aparece no resumo como "VRAM
desconhecida", nunca como "0 MiB".

### `GET /v1/resources`

Última amostra já calculada.

```json
{
  "snapshot": {
    "takenAt": "2026-07-27T10:53:06.963034435Z",
    "intervalMs": 1000,
    "cpu": {
      "totalPercent": 15.721649,
      "perCorePercent": [18.181818, 19.387754, 17.34694, 10.3092785],
      "loadAverage": [3.79, 2.82, 2.75]
    },
    "memory": {
      "totalMb": 32046,
      "usedMb": 14290,
      "availableMb": 17756,
      "cachedMb": 13465,
      "usedPercent": 44.592148,
      "swapUsedMb": 2271,
      "swapTotalMb": 20479
    },
    "gpus": [
      {
        "index": 0,
        "name": "NVIDIA GeForce RTX 5060 Ti",
        "utilizationPercent": 0.0,
        "vramUsedMb": 372,
        "vramTotalMb": 8151,
        "vramUsedPercent": 4.563857,
        "temperatureC": 38.0,
        "powerWatts": 7.541
      }
    ]
  },
  "sampleIntervalMs": 1000
}
```

`snapshot` é `null` na janela entre a subida e a segunda amostra. Não é um erro:
percentual de CPU não existe em um instante — `/proc/stat` só entrega tempo
acumulado, e o número nasce da diferença entre duas leituras.

`usedMb` é `total − available`, o que está de fato comprometido. Não é
`total − free`: a diferença é o cache, que o kernel devolve sob pressão.

### `GET /v1/events`

Stream SSE do barramento interno. Formato do fio:

```
id: 5
event: daemon.started
data: {"sequence":5,"emittedAt":"2026-07-27T10:52:58.955496791Z","payload":{"type":"daemon.started","version":"0.1.0","pid":2499238,"socket_path":"/run/user/1000/elara/agentd.sock"}}

id: 6
event: resources.sampled
data: {"sequence":6,"emittedAt":"…","payload":{"type":"resources.sampled","snapshot":{…}}}

```

- `id` é o `sequence` do envelope — é o que volta em `Last-Event-ID`.
- `event` é o nome estável do evento, útil para filtrar sem desserializar.
- `data` é o `EventEnvelope` inteiro em JSON, incluindo o `sequence` de novo.
- Comentários `:` de keep-alive a cada 15 s. Sem eles, um proxy ou um `undici`
  com timeout fecharia a conexão de um daemon saudável que apenas não teve nada a
  dizer.

O catálogo de eventos é o de `crates/telemetry/src/types.rs`, e contém apenas as
variantes que algum código realmente emite hoje:

| `event`                   | Payload                                          | Persistido |
| ------------------------- | ------------------------------------------------ | ---------- |
| `daemon.started`          | `version`, `pid`, `socket_path`                  | sim        |
| `daemon.stopping`         | `reason`                                         | sim        |
| `hardware.detected`       | `fingerprint`, `probeDurationMs`, `warningCount` | sim        |
| `hardware.probe-failed`   | `error`                                          | sim        |
| `component.state-changed` | `component`, `from`, `to`, `detail`              | sim        |
| `resources.sampled`       | `snapshot`                                       | **não**    |

`resources.sampled` é de alta frequência (um por segundo) e vai só para os
assinantes ao vivo: gravá-lo custaria 86 400 linhas por dia descrevendo um
passado que ninguém consulta.

> **Inconsistência conhecida:** `daemon.started` serializa `socket_path` em
> snake_case, enquanto todas as outras variantes usam camelCase — falta o
> `rename_all` nessa variante do enum. O schema zod espelha o que o Rust de fato
> emite; corrigir o Rust quebrará o typecheck do TypeScript, que é exatamente o
> objetivo do `assertSchemaMatches` (ver [contracts.md](contracts.md)).

#### Retomada

Mandando `Last-Event-ID: <n>`, o daemon reenvia do buffer todos os eventos com
`sequence > n`, em ordem cronológica, e só então emenda no fluxo ao vivo — sem
duplicar nenhum. O buffer guarda 256 eventos.

```sh
curl -s -N --unix-socket $S -H 'Last-Event-ID: 3' http://elara.localhost/v1/events
```

Um `Last-Event-ID` ilegível é ignorado e o cliente recebe só o fluxo ao vivo:
perder o histórico é muito menos grave do que recusar a conexão de uma UI que
reconectou com um id corrompido.

A assinatura acontece **antes** da leitura do histórico. Invertida, a ordem
deixaria uma janela em que um evento publicado entre as duas operações não
estaria nem no histórico nem no fluxo ao vivo — a lacuna mais difícil de
diagnosticar que existe, porque some quando a máquina está ociosa.

#### Lacunas e desligamento

Um assinante lento não derruba o stream: o `broadcast` descarta os eventos mais
antigos, o daemon registra um `WARN`, e o cliente percebe a lacuna pelo salto no
`sequence` (`1 → 4`). Cabe a ele decidir se recarrega o estado.

No desligamento o stream **se fecha sozinho**, de forma ordenada, em vez de a
conexão ser cortada no meio de um frame.

### `POST /v1/runtime/register`

```sh
curl -s --unix-socket $S -X POST \
  -H 'content-type: application/json' -H 'x-elara-protocol: 1' \
  -d '{"name":"agent-runtime","version":"0.1.0","pid":4242,"protocolVersion":1,"capabilities":[]}' \
  http://elara.localhost/v1/runtime/register
```

```json
{
  "sessionId": "abed3526-7132-4e2b-ac02-5ae7e68331f1",
  "heartbeatIntervalMs": 5000,
  "daemonVersion": "0.1.0"
}
```

`sessionId` é um UUID v4 opaco. `heartbeatIntervalMs` é o intervalo **máximo**
tolerado sem notícias; o `agent-runtime` bate a 50 % disso (2,5 s), porque bater
exatamente no limite transformaria qualquer atraso de agendamento em um falso
óbito. Depois de três intervalos sem heartbeat a sessão expira.

`capabilities` é uma lista vazia hoje — anunciar capacidade que não existe faria
o daemon rotear trabalho para um runtime que não sabe executá-lo.

Registrar de novo com o mesmo `pid` desaloja a sessão anterior.

Recusas: `400 request.invalid` (nome vazio, corpo fora do contrato),
`400 protocol.version_mismatch` (`protocolVersion` divergente),
`503 daemon.shutting_down`.

### `POST /v1/runtime/heartbeat`

```json
{ "sessionId": "abed3526-…", "healthy": true, "detail": null }
```

```json
{ "acknowledged": true, "shutdownRequested": false }
```

Duas coisas importantes:

- **Sessão desconhecida devolve `200` com `acknowledged: false`**, não 404. É
  assim que o contrato manda o runtime se registrar de novo. Um 4xx aqui seria
  fatal para ele (ver `rethrowIfFatal` em
  `apps/agent-runtime/src/core/session.ts`) e transformaria "o daemon reiniciou"
  em "o runtime morreu". Como consequência, não existe o código
  `runtime.unknown_session` — criar uma variante de erro que ninguém emite seria
  documentação falsa.
- **`healthy: false` exige `detail`.** Sem ele, `400 request.invalid`: um
  componente degradado no `/v1/status` sem dizer por quê não ajuda ninguém.

`shutdownRequested: true` sinaliza que o daemon está encerrando e o runtime deve
sair.

## Erros

Todo erro — inclusive os que o `axum` geraria sozinho — sai no mesmo corpo:

```json
{ "code": "…", "message": "…", "details": "… ou null" }
```

| `code`                      | HTTP | Quando                                                |
| --------------------------- | ---- | ----------------------------------------------------- |
| `hardware.unavailable`      | 503  | detecção falhou e não há perfil em cache              |
| `protocol.version_mismatch` | 400  | header ou `protocolVersion` divergente/ilegível       |
| `request.invalid`           | 400  | corpo ou query fora do contrato                       |
| `daemon.shutting_down`      | 503  | registro durante o desligamento                       |
| `route.not_found`           | 404  | nenhuma rota casa com o caminho                       |
| `route.method_not_allowed`  | 405  | a rota existe, o método não                           |
| `internal`                  | 500  | falha do daemon (banco, task); nunca culpa do cliente |

Exemplos reais:

```http
HTTP/1.1 404 Not Found
x-elara-protocol: 1

{"code":"route.not_found","message":"esta rota não existe neste daemon","details":"caminho: /v1/nao-existe"}
```

```http
HTTP/1.1 405 Method Not Allowed
allow: GET,HEAD
x-elara-protocol: 1

{"code":"route.method_not_allowed","message":"método não permitido nesta rota","details":"caminho: /v1/health"}
```

```http
HTTP/1.1 400 Bad Request
x-elara-protocol: 1

{"code":"protocol.version_mismatch","message":"este daemon fala a versão 1 do protocolo; o cliente anunciou a versão 2","details":"atualize o cliente ou o daemon para que as versões coincidam"}
```

```http
HTTP/1.1 400 Bad Request
x-elara-protocol: 1

{"code":"request.invalid","message":"limit precisa ser maior que zero","details":"faixa aceita: 1 a 200"}
```

```http
HTTP/1.1 400 Bad Request
x-elara-protocol: 1

{"code":"request.invalid","message":"o corpo da requisição não casa com o contrato","details":"Failed to deserialize the JSON body into the target type: missing field `version` at line 1 column 12"}
```

O `code` é **contrato**: o SDK e a ponte do desktop decidem o que mostrar a
partir dele, então mudar um desses textos é mudar a API. A `message` é escrita
para humanos e pode mudar.

`details` também é escrito para o cliente. A causa técnica — SQL, caminho de
arquivo, erro de I/O — vai para o log e **nunca** para a resposta: o corpo
atravessa a ponte do Tauri e acaba na tela do usuário, e um fragmento de SQL ali
não ajuda ninguém e revela a estrutura interna do daemon. Há teste de regressão
para isso.

Note que **um 405 traz `allow`** e que a resposta de erro **também** anuncia
`x-elara-protocol`.

## Como o `@elara/sdk` valida

`packages/sdk` é o cliente de referência. Quem o usa não precisa saber que existe
um Unix socket nem escrever validação.

```ts
import { AgentdClient } from '@elara/sdk';

const client = new AgentdClient(); // resolve o socket pela mesma regra XDG
const { profile } = await client.hardware(); // HardwareResponse tipado
```

O caminho de uma resposta, da rede ao chamador:

```
undici.Pool(socketPath)
  → status 2xx?           não → ApiErrorSchema no corpo → DaemonResponseError(code do daemon)
  → x-elara-protocol      diverge → ProtocolMismatchError
  → JSON.parse
  → parseOrThrow(Schema, corpo, 'GET /v1/hardware')
  → tipo gerado do Rust
```

`parseOrThrow` lança `SchemaMismatchError` com o caminho do campo em notação
familiar (`gpus[0].vramTotalMb`), a mensagem do zod e o valor recebido. Sem isso,
um daemon de outra versão produziria `undefined` num campo qualquer e o erro
apareceria três camadas acima, sem pista da origem.

Os quatro erros do SDK são quatro decisões diferentes para quem chama:

| Classe                   | `code`               | Significa                             |
| ------------------------ | -------------------- | ------------------------------------- |
| `DaemonUnavailableError` | `daemon.unavailable` | o socket não existe ou recusou        |
| `DaemonTimeoutError`     | `daemon.timeout`     | estourou o orçamento (5 s por padrão) |
| `DaemonResponseError`    | o `code` do daemon   | o daemon respondeu e recusou          |
| `ProtocolMismatchError`  | `protocol.mismatch`  | versões incompatíveis                 |

Duas escolhas do cliente que valem registro:

- **A ausência do header de protocolo não é erro**, só a divergência. Um proxy
  pode remover o header, e a validação de schema pega a divergência real logo
  em seguida.
- **`close()` usa `pool.destroy()`**, não `close()`: um stream SSE aberto
  seguraria um `close()` educado para sempre.

### O stream, pelo SDK

```ts
for await (const item of client.streamEvents({ signal })) {
  if (item.continuity.kind === 'gap') {
    // perdemos item.continuity.missed eventos — recarregar o estado
  }
  console.log(item.envelope.payload.type);
}
```

O SDK reconecta com backoff, retoma pelo último `sequence` visto e classifica a
continuidade em `first`, `continuous`, `gap` (com quantos faltaram) ou `reset`
(a sequência recuou — o daemon reiniciou e recomeçou a contar). Isso transforma
"o número pulou" em uma decisão explícita do consumidor.

### `u64` vira `bigint`

O `ts-rs` mapeia `u64` do Rust para `bigint` no TypeScript, e isso vaza para o
consumidor: `uptimeSeconds`, `ramTotalMb`, `heartbeatIntervalMs` e
`sampleIntervalMs` chegam como `bigint`.

`JSON.parse` produz `number`, então o schema `u64` aceita `number`, `bigint` ou
texto decimal e converte. Valores acima de `Number.MAX_SAFE_INTEGER` são
**rejeitados**, não arredondados: depois do `JSON.parse` o dígito já se perdeu, e
converter inventaria um valor plausível porém errado.

Consequência prática: **corpos de requisição não podem conter `u64`**, porque
`JSON.stringify` lança em `bigint`. Hoje `RuntimeRegisterRequest` e
`RuntimeHeartbeatRequest` só têm `u32`, e há um teste de regressão para isso.

## Diagnóstico manual

```sh
S="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/elara/agentd.sock"

curl -s --unix-socket "$S" http://elara.localhost/v1/health | jq
curl -s --unix-socket "$S" http://elara.localhost/v1/status | jq '.components'
curl -s -N --unix-socket "$S" http://elara.localhost/v1/events   # Ctrl-C encerra
```

O `Host` (`elara.localhost`) é exigido pelo HTTP/1.1 e ignorado pelo daemon.
Nenhuma das rotas exige o header de protocolo.
