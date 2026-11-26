# Perfil de hardware

`crates/hardware` responde a uma pergunta: **o que esta máquina consegue
realmente executar?** O contrato de dados está em `crates/hardware/src/types.rs`
e a regra que atravessa tudo é uma só:

> Todo campo que pode não ser detectável em alguma máquina é `Option`. Quando uma
> sonda não sabe, o valor fica `None` e um `ProbeWarning` explica o porquê. O
> sistema nunca inventa um número.

A alternativa — preencher com zero, com um valor teórico ou com um "padrão
razoável" — produz um perfil que parece completo e leva o planejador a prometer
capacidade que não existe. Uma VRAM `null` faz a UI dizer "desconhecida"; uma
VRAM `0` faz o sistema decidir que a GPU não serve.

## Como uma detecção acontece

`LinuxHardwareProvider::detect()` roda todas as sondas em sequência e monta o
`HardwareProfile`. Custa dezenas de milissegundos (35 ms medidos nesta máquina),
então o daemon a chama uma vez na subida e depois só em
`POST /v1/hardware/refresh`.

`sample()` é a operação barata irmã: lê três arquivos e o NVML, e roda uma vez
por segundo no amostrador de telemetria. Misturar as duas seria pagar o custo da
primeira na frequência da segunda.

**Toda sonda recebe uma raiz (`&Path`).** Em produção é `/`; nos testes é uma
árvore de fixtures. É isso que permite verificar a detecção de um contêiner
limitado a 2 núcleos, ou de uma máquina ARM sem GPU, em uma máquina de mesa x86 —
e é o motivo de nenhum teste do projeto exigir GPU, root ou rede.

Sob uma raiz que não seja `/`, o NVML e o `nvidia-smi` ficam **desligados**: um
binário do host descreveria o host, e misturar duas máquinas no mesmo perfil é
pior do que não ter o dado.

## As sondas

### `host` — `probes/host.rs`

| Campo            | Fonte                                                                  |
| ---------------- | ---------------------------------------------------------------------- |
| `hostname`       | `/proc/sys/kernel/hostname`, depois `/etc/hostname`                    |
| `distribution`   | `/etc/os-release`, depois `/usr/lib/os-release` (`PRETTY_NAME`/`NAME`) |
| `kernelVersion`  | `/proc/sys/kernel/osrelease`                                           |
| `architecture`   | `/proc/cpuinfo`                                                        |
| `virtualization` | ver abaixo                                                             |

**Quando falha:** nunca aborta. Cada campo ausente vira `None` mais um
`ProbeWarning` de severidade `info` — um host sem `os-release` não impede o Elara
de rodar.

A arquitetura sai de `/proc/cpuinfo` e **não** de `uname(2)` porque uma syscall
ignoraria a raiz injetada: sob uma fixture ARM lida de um host x86, `uname`
mentiria. Só quando a raiz é a real é que o fallback usa a arquitetura do próprio
binário (com aviso).

A detecção de virtualização testa **contêiner antes de hipervisor**: um contêiner
dentro de uma VM é limitado pelo contêiner, e é esse o dado que muda decisões de
recurso.

```
/.dockerenv                         → docker
/run/.containerenv                  → podman
/proc/1/cgroup contém libpod|podman → podman
                docker|containerd   → docker
                /lxc|lxc.payload    → lxc
/proc/version contém microsoft|wsl  → wsl
DMI (sys_vendor, product_name, …)   → kvm | xen | vmware | virtualbox | hyper-v
flag "hypervisor" em /proc/cpuinfo  → unknown   ("virtualizado, não sei qual")
```

### `cpu` — `probes/cpu.rs`

O ponto delicado é a diferença entre **thread lógica** e **núcleo físico**.
Tratar 16 threads como 16 núcleos faz o planejador prometer o dobro da capacidade
real de uma CPU com SMT.

| Campo                    | Fonte                                                                 |
| ------------------------ | --------------------------------------------------------------------- |
| `model`, `vendor`, flags | `/proc/cpuinfo`                                                       |
| `logicalThreads`         | `/sys/devices/system/cpu/present` (lista tipo `0-3,8,10-11`)          |
| `onlineThreads`          | `/sys/devices/system/cpu/online`                                      |
| `physicalCores`          | `core id`/`cpu cores` do cpuinfo → pares `(package, core)` do sysfs   |
| `sockets`                | `physical id` do cpuinfo → `physical_package_id` do sysfs → 1 (aviso) |
| `baseFrequencyMhz`       | `cpu0/cpufreq/base_frequency` → modelo comercial (`… @ 2.70GHz`)      |
| `maxFrequencyMhz`        | maior `cpuinfo_max_freq` entre todas as CPUs                          |
| `cache`                  | `cpuN/cache/indexN/{level,type,size,shared_cpu_list}`                 |
| `hybrid`                 | `topology/core_type` (Intel) → `cpu_capacity` (ARM big.LITTLE)        |

**Quando falha:** esta é a **única sonda que pode derrubar a detecção**. Sem
`/proc/cpuinfo` não há modelo nem contagem de threads, e o contrato não admite
esses campos vazios — vira `HardwareError::Io` ou `::Parse`, e o daemon marca o
componente `hardware` como `Failed` sem inventar perfil.

Falhas parciais são degradações, não erros: sem topologia de núcleos, o número de
threads é usado como número de núcleos **com um aviso explícito** de que isso
superestima a capacidade quando há SMT.

Duas escolhas que não são óbvias:

- **`cpu MHz` do `/proc/cpuinfo` é ignorado.** É a frequência do instante da
  leitura: num sistema ocioso é a mínima, sob carga é a de turbo, nunca a base.
- **Cada instância de cache é contada uma vez.** Um cache aparece no diretório de
  todas as CPUs que o compartilham; a deduplicação usa `shared_cpu_list` como
  identidade da instância. Sem ela, um L3 de 32 MiB compartilhado por 8 CPUs
  viraria 256 MiB. O total é então dividido pelo número de sockets, o que assume
  sockets simétricos.

### `ram` — `probes/ram.rs`

Tudo vem de `/proc/meminfo`. `availableMb` usa `MemAvailable`, que inclui cache
recuperável; em kernels antigos que não o expõem, cai para
`MemFree + Buffers + Cached` com aviso.

`measuredBandwidthGbps` é **sempre `null`** hoje, e `bandwidthSource` é
`not-measured`. Não há aqui nenhum cálculo teórico a partir do tipo de DDR: a
banda real depende de canais populados, XMP e do controlador. O número aparece
quando o Calibration Engine (Fase 7) medir; até lá, o campo diz honestamente que
não sabe.

**Quando falha:** sem `/proc/meminfo`, `HardwareError` — como a CPU, é dado sem o
qual não há perfil.

### `gpu` — `probes/gpu.rs`

Três fontes, correlacionadas pelo **endereço PCI**:

1. **NVML** — a biblioteca da NVIDIA, carregada dinamicamente. Entrega VRAM
   livre, clock, temperatura, potência e compute capability. Não estar instalada
   é o caso normal em uma máquina AMD ou sem driver proprietário.
2. **`nvidia-smi`** — o mesmo dado, um processo de distância. Só é chamado se o
   NVML falhar, e sempre com argumentos em vetor: nada passa por um shell.
3. **sysfs/PCI** — `/sys/bus/pci/devices`, classes `0x0300` (VGA) e `0x0380`
   (3D). Sempre roda. É a única fonte para AMD e Intel, e identifica o
   dispositivo mesmo sem driver nenhum.

A sonda parte do sysfs e substitui as entradas pelas equivalentes ao vivo,
casando pelo PCI. Uma GPU vista pelo NVML e não pelo sysfs (acontece com
`CUDA_VISIBLE_DEVICES` dentro de contêiner) é **acrescentada**, não descartada.
Os endereços são normalizados antes da comparação porque o NVML escreve o domínio
com oito dígitos (`00000000:01:00.0`) e o sysfs com quatro (`0000:01:00.0`).

**Quando falha:** nunca aborta. Sem NVML e sem `nvidia-smi`, a GPU aparece com
`model` no formato `NVIDIA 10de:2d04` (os IDs PCI crus, verificáveis com
`lspci`), `vramTotalMb: null` e um `ProbeWarning`. Não existe tabela de nomes
comerciais no kernel, e inventar um a partir do ID daria "RTX 4070" para uma 4070
Ti.

**Backends exigem evidência.** Um backend só entra na lista se houver prova de
que ele funciona:

| Backend  | Evidência exigida                                                                                               |
| -------- | --------------------------------------------------------------------------------------------------------------- |
| `cuda`   | driver NVIDIA carregado (`/sys/module/nvidia/version`, `/dev/nvidiactl`, ou NVML/`nvidia-smi` tendo respondido) |
| `rocm`   | `/dev/kfd` existe                                                                                               |
| `vulkan` | um ICD do fabricante em `/usr/share/vulkan/icd.d` ou equivalentes                                               |

Uma lista otimista faria o planejador escolher um caminho que falha na primeira
inferência.

`isIntegrated` importa porque uma GPU integrada compartilha a RAM do sistema:
contar a "VRAM" dela seria contar a mesma memória duas vezes. Uma Intel só é
considerada dedicada quando expõe `lmem_total_bytes` (as Arc discretas expõem).

### `storage` — `probes/storage.rs`

Enumera `/sys/block`, descartando prefixos virtuais: `loop` (imagens montadas —
todo Snap cria uma), `ram`, `zram` (RAM comprimida) e `dm-` (mapeador de
LVM/LUKS, que aponta para um disco já listado).

| Campo        | Fonte                                                               |
| ------------ | ------------------------------------------------------------------- |
| `sizeMb`     | `/sys/block/<dev>/size` × 512 bytes                                 |
| `model`      | `/sys/block/<dev>/device/model`                                     |
| `rotational` | `/sys/block/<dev>/queue/rotational`                                 |
| `kind`       | prefixo `nvme` → `nvme`; senão `rotational` → `hdd`/`ssd`/`unknown` |
| `mounts`     | `/proc/mounts`, ligadas ao disco pai (`nvme0n1p2` → `nvme0n1`)      |
| espaço livre | `statvfs(3)`                                                        |

O espaço livre **não** sai do `/proc`. Só o `statvfs` conhece as reservas do
sistema de arquivos (os 5 % do `ext4`, por exemplo), e usa-se `f_bavail` — blocos
livres para usuários comuns — e não `f_bfree`, que inclui a reserva do
superusuário que o Elara não pode gastar.

Este é o **único `unsafe` do crate**, marcado com `#[allow(unsafe_code)]` e
comentários `SAFETY`, porque a libc não expõe alternativa segura.

**Quando falha:** `/proc/mounts` ilegível vira aviso `warning` e a lista de
montagens fica vazia. Uma montagem que não é atribuível a um disco físico (o caso
de `/dev/mapper/*`) gera aviso `info` e fica de fora.

### `topology` — `probes/topology.rs`

| Campo                        | Fonte                                                     |
| ---------------------------- | --------------------------------------------------------- |
| `numaNodes`                  | `/sys/devices/system/node/node*/{cpulist,meminfo}`        |
| `simultaneousMultithreading` | `cpu/smt/active`, ou `cpu0/topology/thread_siblings_list` |
| `cgroup`                     | `/sys/fs/cgroup` — v2 se houver `cgroup.controllers`      |

O cgroup é a parte que não pode faltar. **Dentro de um contêiner, `/proc/meminfo`
continua mostrando a RAM do host**: quem lê só isso planeja usar 32 GiB num
contêiner de 4 GiB e é morto pelo OOM killer no meio do carregamento do modelo.

Na v2, a sonda percorre do cgroup do processo (lido em `/proc/self/cgroup`) até a
raiz e fica com o **menor** limite encontrado — um limite no pai vale para o
filho mesmo que o filho não o declare. Na v1, lê `cpu.cfs_quota_us` /
`cpu.cfs_period_us` e `memory.limit_in_bytes`, tratando as sentinelas de "sem
limite".

**Quando falha:** sem hierarquia montada, `version: none` e todos os limites
`null`, com aviso `info`. Sem `/sys/devices/system/node`, a lista NUMA fica vazia
com aviso.

## Fingerprint

O fingerprint responde a **uma** pergunta: _os benchmarks que gravei ontem ainda
valem?_ Portanto ele só pode mudar quando o hardware muda.

São 32 caracteres hexadecimais — 128 bits de blake3, o suficiente para tornar
colisão acidental inconcebível e ainda caber num log ou numa coluna de tabela.

Entram no hash, e só eles:

```
cpu.model=<modelo com espaços colapsados>
cpu.physical_cores=<n>
cpu.logical_threads=<n>
cpu.sockets=<n>
cpu.features=<lista ordenada dos nomes das extensões ativas>
ram.total_gib=<total arredondado para o GiB mais próximo>
gpu=<modelo>|<vram total em MiB, ou "desconhecida">   (uma linha por GPU)
```

Ficam **de fora**, deliberadamente: memória disponível, VRAM livre, utilização,
temperatura, frequência instantânea, limite de potência, índice de enumeração e
qualquer timestamp. Se um desses entrasse, cada coleta produziria uma "máquina
nova" e a base de calibração seria descartada a cada segundo.

Três detalhes que existem por causa de comportamento real do kernel e do driver:

- **RAM arredondada para GiB.** O kernel reserva quantidades ligeiramente
  diferentes a cada boot (firmware, carve-out de vídeo), então `MemTotal` oscila
  alguns MiB na mesma máquina.
- **GPUs ordenadas por endereço PCI.** Trocar a ordem de enumeração do driver não
  é trocar de máquina.
- **Extensões como lista de nomes, não como bits.** Acrescentar um campo novo a
  `CpuFeatures` no futuro não invalida os fingerprints já gravados das máquinas
  que não têm a extensão.

`fingerprint::canonical_input()` é público de propósito: quando um fingerprint
muda sem explicação, comparar os dois textos mostra em uma linha o que mudou —
coisa que comparar dois hashes jamais mostraria.

**Limitação real:** o fingerprint muda se o driver NVIDIA for removido. Sem NVML,
o modelo cai para `NVIDIA 10de:2d04` e a VRAM vira `desconhecida`. O
comportamento é honesto — a máquina de fato não é mais a mesma do ponto de vista
de execução —, mas invalida benchmarks quando o driver some por um motivo
irrelevante, como uma atualização de kernel pendente de reboot. O Calibration
Engine precisa saber disso.

## `EffectiveResources`

O orçamento que o Resource Broker deve consumir. **Não** é o total físico, e cada
número vem acompanhado de uma linha em `rationale`: quando o usuário perguntar
"por que só 15 GiB se eu tenho 32?", a resposta já está no perfil.

### CPU

```
threads = max(1, min(threads_online, floor(cota_do_cgroup)))
          − 1  se o resultado for maior que 2
```

A cota fracionária é arredondada **para baixo**: meio núcleo de cota não executa
uma thread inteira, e prometer que executa é o que causa throttling constante.

A reserva de uma thread existe porque, sem ela, uma inferência a todo vapor trava
o compositor do desktop e o próprio daemon deixa de responder ao health check.
Ela some abaixo de três threads, onde 1 de 2 seria metade da máquina.

### RAM

```
orçamento = min(MemAvailable, limite_do_cgroup − uso_atual_do_cgroup)
reserva   = clamp(15% do orçamento, mínimo 512 MiB, máximo metade do orçamento)
ram_mb    = orçamento − reserva
```

`MemAvailable` inclui cache que o kernel _acredita_ poder liberar; sob pressão
real nem sempre libera. A folga de 15 % é o que separa "apertado" de "morto pelo
OOM killer".

O piso de 512 MiB existe porque em orçamentos pequenos 15 % é pouco demais: 15 %
de 2 GiB são 300 MiB, menos do que o próprio daemon e a UI consomem. O teto de
metade existe para que o piso não engula um orçamento minúsculo inteiro.

### VRAM

```
vram_mb = Σ (vram_livre × 0,90)   sobre as GPUs que passam nos três testes
```

Os três testes, e o que acontece quando uma GPU falha em algum:

| Condição                   | Resultado                                            |
| -------------------------- | ---------------------------------------------------- |
| `isIntegrated: true`       | não conta — compartilha a RAM do sistema             |
| `backends` vazio           | não conta — não há caminho de inferência utilizável  |
| `vramAvailableMb` é `null` | não conta — não se soma um número que não se conhece |

Em todos os três casos entra uma linha no `rationale` dizendo qual GPU foi
descartada e por quê. Uma GPU descartada nunca vira `0` silencioso.

Os 10 % de folga (`vramHeadroomRatio: 0.1`) são para o compositor e o driver, que
alocam VRAM sem avisar ninguém.

### Exemplo real

Coletado nesta máquina de desenvolvimento (`GET /v1/hardware`):

```json
{
  "cpuThreads": 3,
  "ramMb": 15057,
  "vramMb": 6597,
  "vramHeadroomRatio": 0.1,
  "rationale": [
    "CPU: 4 threads online, menos 1 reservada ao sistema → 3",
    "RAM: 17714 MiB disponíveis segundo o kernel, menos 2657 MiB de folga (15%, piso de 512 MiB, no máximo metade) → 15057 MiB",
    "GPU 0 (NVIDIA GeForce RTX 5060 Ti): 7331 MiB livres, menos 10% de folga → 6597 MiB"
  ]
}
```

Uma máquina com 32 GiB instalados e uma GPU de 8 GiB oferece, neste instante,
15 GiB e 6,6 GiB. A diferença é o que o resto do sistema já está usando — e é
exatamente por isso que o planejador tem de ler `effective`, não `ram.totalMb`.

## `ProbeWarning`

Toda sonda parcial deixa um registro:

```json
{ "source": "gpu.nvml", "severity": "info", "message": "NVML não carregou (…)" }
```

| Severidade | Significado                                                  |
| ---------- | ------------------------------------------------------------ |
| `info`     | informação secundária ausente; nenhuma decisão muda          |
| `warning`  | dado relevante ausente; as decisões ficam mais conservadoras |
| `error`    | sonda essencial falhou; o perfil não é confiável             |

O daemon usa isso para decidir o estado do componente `hardware`: só avisos
`info` mantêm `ready`; qualquer `warning` ou `error` o degrada para `degraded`,
com o motivo visível em `GET /v1/status`. A UI mostra o `message` ao lado do
campo ausente, para que o usuário leia "VRAM desconhecida porque NVML não
carregou" em vez de ver um zero.

## Limitações conhecidas

Encontradas durante a implementação, não hipotéticas:

- **Montagens de `/dev/mapper/*` (LVM/LUKS) não são associadas ao disco
  físico.** Associá-las exigiria caminhar os `holders` do sysfs. Hoje geram um
  aviso `info` e ficam de fora da lista de montagens do disco.
- **Cache por socket divide o total pelo número de sockets.** Assume sockets
  simétricos — verdadeiro em qualquer máquina de mesa, falso em NUMA heterogêneo.
- **CPUs híbridas dependem de `topology/core_type` ou `cpu_capacity`.** Kernels
  que só expõem `/sys/devices/system/cpu/types/*` reportam `hybrid: null`. Sem
  mentir, mas sem detectar.
- **Sob raiz de fixture, o `statvfs` não encontra o caminho** e grava `0/0` com
  um aviso `info`, porque `MountPoint` não tem campos `Option`. Em `/` os números
  são reais.
- **As fixtures não exercitam o caminho NVML** (ele é desligado fora da raiz
  real). Quem cobre o NVML são os testes de máquina real, que afirmam apenas
  invariantes — threads ≥ 1, físicos ≤ lógicos, total ≥ disponível, fingerprint
  com 32 hex e reprodutível — porque qualquer asserção sobre valores concretos
  amarraria a suíte a uma máquina específica.
- **A aritmética do `statvfs` assume Linux 64 bits**, onde todos os campos da
  estrutura são `u64`.

## Testando

```sh
cargo test -p elara-hardware
```

As fixtures ficam em `crates/hardware/tests/fixtures/`: três árvores de `/proc` e
`/sys` completas — um desktop x86 com NVIDIA, um ARM sem GPU e um contêiner com
cgroup v2. O caso do contêiner é o que prova o ponto central do módulo: sob a
mesma leitura, o host mostra 16 threads e 24 576 MiB disponíveis enquanto o
`EffectiveResources` do contêiner devolve `cpuThreads: 2` e `ramMb: 3047`.

As fixtures foram geradas por script, mas o script não ficou no repositório:
editar um arquivo de fixture à mão é o fluxo esperado.
