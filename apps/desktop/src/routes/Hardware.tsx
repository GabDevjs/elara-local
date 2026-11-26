/**
 * Perfil de hardware detectado.
 *
 * A tela é longa de propósito: é o laudo da máquina, e um laudo não resume.
 * Onde a sonda não conseguiu ler, aparece um travessão e o motivo — nunca um
 * zero. Onde o número depende de uma fase que ainda não existe, ela diz qual
 * fase, em vez de estimar.
 */
import type {
  CpuFeatures,
  CpuVendor,
  GpuProfile,
  GpuVendor,
  HardwareHistoryEntry,
  HardwareResponse,
  StorageKind,
  StorageProfile,
  Virtualization,
} from '@elara/schemas';
import { useCallback, useState, type ReactNode } from 'react';

import { Button } from '../components/Button.js';
import { DataList, type DataRow } from '../components/DataList.js';
import { Missing, reasonFor } from '../components/Missing.js';
import { Numeric } from '../components/Numeric.js';
import { Panel } from '../components/Panel.js';
import { EmptyState, ErrorState, LoadingState, OfflineState } from '../components/States.js';
import { WarningList } from '../components/Warnings.js';
import {
  asFailure,
  fetchHardware,
  fetchHardwareHistory,
  type BridgeFailure,
} from '../lib/bridge.js';
import { useDaemon } from '../lib/daemon-session.js';
import {
  DASH,
  formatCache,
  formatCount,
  formatDecimal,
  formatFrequency,
  formatInstant,
  formatMib,
  formatMillis,
  formatPercent,
  type Measurement,
} from '../lib/format.js';
import { useAsync, useReloadOnEpoch } from '../lib/use-async.js';

/** Quantos perfis anteriores a tela lista. */
const HISTORY_LIMIT = 20;

/** Nome comercial de cada fabricante de CPU. */
const CPU_VENDORS: Record<CpuVendor, string> = {
  amd: 'AMD',
  intel: 'Intel',
  arm: 'ARM',
  apple: 'Apple',
  other: 'outro',
};

/** Nome comercial de cada fabricante de GPU. */
const GPU_VENDORS: Record<GpuVendor, string> = {
  nvidia: 'NVIDIA',
  amd: 'AMD',
  intel: 'Intel',
  apple: 'Apple',
  unknown: 'desconhecido',
};

/** Classe física do armazenamento. */
const STORAGE_KINDS: Record<StorageKind, string> = {
  nvme: 'NVMe',
  ssd: 'SSD',
  hdd: 'HDD',
  unknown: 'desconhecido',
};

/** Ambiente de virtualização. */
const VIRTUALIZATION: Record<Virtualization, string> = {
  kvm: 'KVM',
  xen: 'Xen',
  'vm-ware': 'VMware',
  'hyper-v': 'Hyper-V',
  'virtual-box': 'VirtualBox',
  wsl: 'WSL',
  docker: 'Docker',
  podman: 'Podman',
  lxc: 'LXC',
  unknown: 'virtualizado, tipo não identificado',
};

/** Rótulo de cada extensão de CPU, na ordem em que importam à inferência. */
const CPU_FEATURE_LABELS: readonly (readonly [keyof CpuFeatures, string])[] = [
  ['avx2', 'AVX2'],
  ['avx', 'AVX'],
  ['fma', 'FMA'],
  ['f16c', 'F16C'],
  ['avx512f', 'AVX-512F'],
  ['avx512Vnni', 'AVX-512 VNNI'],
  ['avx512Bf16', 'AVX-512 BF16'],
  ['amxInt8', 'AMX INT8'],
  ['amxBf16', 'AMX BF16'],
  ['sse42', 'SSE4.2'],
  ['ssse3', 'SSSE3'],
  ['sse3', 'SSE3'],
  ['neon', 'NEON'],
  ['dotprod', 'DotProd'],
  ['i8mm', 'I8MM'],
  ['sve', 'SVE'],
];

/** Renderiza a tela de hardware. */
export function Hardware(): ReactNode {
  const { connection, socket, status, hardwareEpoch, reload } = useDaemon();
  const [redetecting, setRedetecting] = useState(false);
  const [redetectFailure, setRedetectFailure] = useState<BridgeFailure | null>(null);

  const loadProfile = useCallback(() => fetchHardware(false), []);
  const profileResource = useAsync<HardwareResponse>(loadProfile);
  useReloadOnEpoch(hardwareEpoch, profileResource.reload);

  const loadHistory = useCallback(() => fetchHardwareHistory(HISTORY_LIMIT), []);
  const historyResource = useAsync<HardwareHistoryEntry[]>(loadHistory);
  useReloadOnEpoch(hardwareEpoch, historyResource.reload);

  const redetect = useCallback(() => {
    setRedetecting(true);
    setRedetectFailure(null);
    // A falha da re-detecção precisa de tratamento próprio: recarregar o perfil
    // depois de um `POST` que falhou traria o perfil em cache de volta, e a tela
    // ficaria idêntica ao estado anterior como se nada tivesse dado errado.
    void fetchHardware(true).then(
      () => {
        setRedetecting(false);
        profileResource.reload();
        historyResource.reload();
      },
      (error: unknown) => {
        setRedetecting(false);
        setRedetectFailure(asFailure(error));
      },
    );
  }, [historyResource, profileResource]);

  if (connection === 'offline') {
    return (
      <OfflineState
        socket={socket}
        failure={status.state === 'error' ? status.failure : null}
        onRetry={reload}
      />
    );
  }

  if (profileResource.value.state === 'loading') {
    return <LoadingState what="lendo o perfil de hardware detectado pelo daemon" />;
  }

  if (profileResource.value.state === 'error') {
    return <ErrorState failure={profileResource.value.failure} onRetry={profileResource.reload} />;
  }

  const { profile, cached, firstSeenAt } = profileResource.value.data;
  const probeDuration = formatMillis(profile.probeDurationMs);

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      <div className="flex flex-wrap items-center justify-between gap-3 border-b border-line bg-surface px-4 py-3">
        <div className="flex min-w-0 flex-col gap-1">
          <span className="label">impressão digital da máquina</span>
          <span className="num truncate text-[13px] text-live" title={profile.fingerprint}>
            {profile.fingerprint}
          </span>
          <span className="font-sans text-[11px] text-ink-faint">
            visto pela primeira vez em {formatInstant(firstSeenAt)} · esquema v
            {profile.schemaVersion}
          </span>
        </div>

        <div className="flex items-center gap-5">
          <div className="flex flex-col gap-0.5">
            <span className="label">coletado em</span>
            <span className="num text-[12px] text-ink-dim">
              {formatInstant(profile.collectedAt)}
            </span>
          </div>
          <div className="flex flex-col gap-0.5">
            <span className="label">duração da sondagem</span>
            <Numeric value={probeDuration.value} unit={probeDuration.unit} size="sm" tone="dim" />
          </div>
          <div className="flex flex-col gap-0.5">
            <span className="label">origem</span>
            <span className={`text-[12px] ${cached ? 'text-ink-dim' : 'text-live'}`}>
              {cached ? 'cache do daemon' : 'detecção nova'}
            </span>
          </div>
          <Button
            onClick={redetect}
            primary
            disabled={redetecting}
            title={
              redetecting
                ? 'a re-detecção está em andamento'
                : 're-executa todas as sondas; leva alguns segundos'
            }
          >
            {redetecting ? 're-detectando…' : 're-detectar'}
          </Button>
        </div>

        {redetectFailure !== null ? (
          <div className="w-full border border-fail/40 bg-fail/10 px-3 py-2">
            <p className="label text-fail">a re-detecção falhou · {redetectFailure.code}</p>
            <p className="font-sans text-[12px] leading-snug text-ink-dim">
              {redetectFailure.message} O perfil abaixo continua sendo o da última detecção
              bem-sucedida.
            </p>
          </div>
        ) : null}
      </div>

      <div className="grid grid-cols-12 gap-3 p-3">
        <Panel title="host" className="col-span-12 xl:col-span-6">
          <DataList
            rows={[
              { label: 'hostname', value: textOrMissing(profile.host.hostname) },
              { label: 'sistema', value: profile.host.os },
              { label: 'distribuição', value: textOrMissing(profile.host.distribution) },
              { label: 'kernel', value: textOrMissing(profile.host.kernelVersion) },
              { label: 'arquitetura', value: profile.host.architecture },
              {
                label: 'virtualização',
                value:
                  profile.host.virtualization === null ? (
                    <span className="text-ink-dim">nenhuma detectada — máquina física</span>
                  ) : (
                    VIRTUALIZATION[profile.host.virtualization]
                  ),
              },
            ]}
          />
        </Panel>

        <Panel
          title="cpu"
          meta={CPU_VENDORS[profile.cpu.vendor]}
          className="col-span-12 xl:col-span-6"
        >
          <div className="flex flex-col gap-3">
            <DataList
              rows={[
                { label: 'modelo', value: profile.cpu.model },
                { label: 'arquitetura', value: profile.cpu.architecture },
                { label: 'núcleos físicos', value: formatCount(profile.cpu.physicalCores) },
                { label: 'threads lógicas', value: formatCount(profile.cpu.logicalThreads) },
                {
                  label: 'threads online',
                  value: formatCount(profile.cpu.onlineThreads),
                  ...(profile.cpu.onlineThreads < profile.cpu.logicalThreads
                    ? { note: 'há threads offline; o orçamento de CPU usa apenas as online' }
                    : {}),
                },
                { label: 'sockets', value: formatCount(profile.cpu.sockets) },
                {
                  label: 'frequência base',
                  value: measurementOrMissing(
                    formatFrequency(profile.cpu.baseFrequencyMhz),
                    'não exposta por este kernel',
                  ),
                },
                {
                  label: 'frequência máxima',
                  value: measurementOrMissing(
                    formatFrequency(profile.cpu.maxFrequencyMhz),
                    'não exposta por este kernel',
                  ),
                },
                ...(profile.cpu.hybrid === null
                  ? []
                  : [
                      {
                        label: 'núcleos híbridos',
                        value: `${formatCount(profile.cpu.hybrid.performanceCores)} P · ${formatCount(profile.cpu.hybrid.efficiencyCores)} E`,
                      },
                    ]),
              ]}
            />

            <div className="flex flex-col gap-2 border-t border-line pt-3">
              <span className="label">cache por socket</span>
              <DataList
                columns={3}
                rows={[
                  {
                    label: 'L1d',
                    value: measurementOrMissing(formatCache(profile.cpu.cache.l1dKb), null),
                  },
                  {
                    label: 'L1i',
                    value: measurementOrMissing(formatCache(profile.cpu.cache.l1iKb), null),
                  },
                  {
                    label: 'L2',
                    value: measurementOrMissing(formatCache(profile.cpu.cache.l2Kb), null),
                  },
                  {
                    label: 'L3',
                    value: measurementOrMissing(formatCache(profile.cpu.cache.l3Kb), null),
                  },
                ]}
              />
            </div>

            <div className="flex flex-col gap-2 border-t border-line pt-3">
              <span className="label">extensões que o llama.cpp explora</span>
              <ul className="flex flex-wrap gap-1.5">
                {CPU_FEATURE_LABELS.map(([key, label]) => {
                  const present = profile.cpu.features[key];
                  return (
                    <li
                      key={key}
                      className={`border px-1.5 py-0.5 text-[10px] rounded-hair ${
                        present
                          ? 'border-ok/40 bg-ok/10 text-ok'
                          : 'border-line text-ink-faint line-through'
                      }`}
                      title={present ? `${label}: disponível` : `${label}: ausente nesta CPU`}
                    >
                      {label}
                    </li>
                  );
                })}
              </ul>
            </div>
          </div>
        </Panel>

        <Panel title="memória" className="col-span-12 xl:col-span-6">
          <DataList
            rows={[
              { label: 'total', value: measurementOrMissing(formatMib(profile.ram.totalMb), null) },
              {
                label: 'disponível',
                value: measurementOrMissing(formatMib(profile.ram.availableMb), null),
                note: 'MemAvailable do kernel: inclui cache recuperável',
              },
              { label: 'livre', value: measurementOrMissing(formatMib(profile.ram.freeMb), null) },
              {
                label: 'swap total',
                value: measurementOrMissing(formatMib(profile.ram.swapTotalMb), null),
              },
              {
                label: 'swap livre',
                value: measurementOrMissing(formatMib(profile.ram.swapFreeMb), null),
              },
              {
                label: 'largura de banda',
                value:
                  profile.ram.measuredBandwidthGbps === null ? (
                    <Missing
                      reason={
                        profile.ram.bandwidthSource === 'not-measured'
                          ? 'não medido (Fase 7) — o Calibration Engine ainda não existe'
                          : 'a medição não produziu resultado'
                      }
                      inline
                    />
                  ) : (
                    <Numeric
                      value={formatDecimal(profile.ram.measuredBandwidthGbps)}
                      unit="GB/s"
                      size="sm"
                    />
                  ),
                note:
                  profile.ram.bandwidthSource === 'not-measured'
                    ? 'nenhuma estimativa teórica é exibida no lugar'
                    : 'benchmark local do tipo STREAM triad',
              },
            ]}
          />
        </Panel>

        <Panel
          title="recursos efetivos"
          meta="o que o Resource Broker pode gastar"
          className="col-span-12 xl:col-span-6"
        >
          <div className="flex flex-col gap-3">
            <DataList
              columns={3}
              rows={[
                {
                  label: 'threads de CPU',
                  value: (
                    <Numeric
                      value={formatCount(profile.effective.cpuThreads)}
                      size="md"
                      tone="compute"
                    />
                  ),
                },
                {
                  label: 'RAM utilizável',
                  value: (
                    <Numeric
                      value={formatMib(profile.effective.ramMb).value}
                      unit={formatMib(profile.effective.ramMb).unit}
                      size="md"
                      tone="compute"
                    />
                  ),
                },
                {
                  label: 'VRAM utilizável',
                  value: (
                    <Numeric
                      value={formatMib(profile.effective.vramMb).value}
                      unit={formatMib(profile.effective.vramMb).unit}
                      size="md"
                      tone="compute"
                    />
                  ),
                },
              ]}
            />

            <div className="flex items-baseline gap-2 border-t border-line pt-3">
              <span className="label">folga de VRAM reservada ao driver</span>
              <Numeric
                value={formatPercent(profile.effective.vramHeadroomRatio * 100).value}
                unit="%"
                size="sm"
                tone="dim"
              />
            </div>

            <div className="flex flex-col gap-1.5 border-t border-line pt-3">
              <span className="label">como cada número foi obtido</span>
              {profile.effective.rationale.length === 0 ? (
                <p className="font-sans text-[12px] text-warn">
                  O daemon não explicou o cálculo. Um orçamento sem justificativa não deve ser usado
                  para dimensionar carga.
                </p>
              ) : (
                <ol className="flex flex-col gap-1">
                  {profile.effective.rationale.map((line, index) => (
                    <li
                      key={line}
                      className="grid grid-cols-[auto_1fr] items-baseline gap-x-2 font-sans text-[12px] leading-snug text-ink-dim"
                    >
                      <span className="num text-[10px] text-compute">
                        {String(index + 1).padStart(2, '0')}
                      </span>
                      <span>{line}</span>
                    </li>
                  ))}
                </ol>
              )}
            </div>
          </div>
        </Panel>

        <Panel
          title="gpus"
          meta={`${profile.gpus.length} detectada${profile.gpus.length === 1 ? '' : 's'}`}
          className="col-span-12"
        >
          {profile.gpus.length === 0 ? (
            <p className="font-sans text-[12px] text-ink-dim">
              Nenhuma GPU detectada. A inferência ficará a cargo da CPU.
            </p>
          ) : (
            <ul className="grid grid-cols-1 gap-3 xl:grid-cols-2">
              {profile.gpus.map((gpu) => (
                <GpuDetail key={gpu.index} gpu={gpu} reason={reasonFor(profile.warnings, 'gpu')} />
              ))}
            </ul>
          )}
        </Panel>

        <Panel
          title="armazenamento"
          meta={`${profile.storage.length} dispositivo${profile.storage.length === 1 ? '' : 's'}`}
          className="col-span-12"
        >
          {profile.storage.length === 0 ? (
            <p className="font-sans text-[12px] text-ink-dim">
              Nenhum dispositivo de bloco foi enumerado.{' '}
              {reasonFor(profile.warnings, 'storage') ?? ''}
            </p>
          ) : (
            <ul className="flex flex-col gap-3">
              {profile.storage.map((device) => (
                <StorageDetail key={device.device} device={device} />
              ))}
            </ul>
          )}
        </Panel>

        <Panel title="topologia e limites" className="col-span-12 xl:col-span-6">
          <div className="flex flex-col gap-3">
            <DataList
              rows={[
                {
                  label: 'SMT / hyper-threading',
                  value: profile.topology.simultaneousMultithreading ? 'ativo' : 'inativo',
                },
                { label: 'nós NUMA', value: formatCount(profile.topology.numaNodes.length) },
                { label: 'cgroup', value: cgroupVersionLabel(profile.topology.cgroup.version) },
                {
                  label: 'cota de CPU',
                  value:
                    profile.topology.cgroup.cpuQuotaCores === null ? (
                      <Missing reason="sem cota de CPU imposta ao processo" />
                    ) : (
                      <Numeric
                        value={formatDecimal(profile.topology.cgroup.cpuQuotaCores)}
                        unit="núcleos"
                        size="sm"
                      />
                    ),
                },
                {
                  label: 'limite de memória',
                  value:
                    profile.topology.cgroup.memoryLimitMb === null ? (
                      <Missing reason="sem limite de memória imposto ao processo" />
                    ) : (
                      <Numeric
                        value={formatMib(profile.topology.cgroup.memoryLimitMb).value}
                        unit={formatMib(profile.topology.cgroup.memoryLimitMb).unit}
                        size="sm"
                        tone="warn"
                      />
                    ),
                },
                {
                  label: 'memória em uso no cgroup',
                  value:
                    profile.topology.cgroup.memoryUsageMb === null ? (
                      <Missing reason="o cgroup não expõe uso corrente" />
                    ) : (
                      <Numeric
                        value={formatMib(profile.topology.cgroup.memoryUsageMb).value}
                        unit={formatMib(profile.topology.cgroup.memoryUsageMb).unit}
                        size="sm"
                      />
                    ),
                },
              ]}
            />

            {profile.topology.numaNodes.length > 0 ? (
              <ul className="flex flex-col gap-1.5 border-t border-line pt-3">
                {profile.topology.numaNodes.map((node) => (
                  <li key={node.id} className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
                    <span className="label">nó {node.id}</span>
                    <span className="num text-[11px] text-ink-dim">
                      {node.cpus.length} CPUs · {node.cpus.join(', ')}
                    </span>
                    <span className="num text-[11px] text-ink-faint">
                      {node.memoryMb === null
                        ? DASH
                        : `${formatMib(node.memoryMb).value} ${formatMib(node.memoryMb).unit}`}
                    </span>
                  </li>
                ))}
              </ul>
            ) : null}
          </div>
        </Panel>

        <Panel
          title="avisos das sondas"
          meta={`${profile.warnings.length} registrado${profile.warnings.length === 1 ? '' : 's'}`}
          className="col-span-12 xl:col-span-6"
        >
          <WarningList warnings={profile.warnings} />
        </Panel>

        <Panel title="perfis já vistos nesta máquina" className="col-span-12">
          <HistoryTable resource={historyResource.value} onRetry={historyResource.reload} />
        </Panel>
      </div>
    </div>
  );
}

/** Nome legível da versão do cgroup. */
function cgroupVersionLabel(version: 'none' | 'v1' | 'v2'): string {
  if (version === 'none') return 'fora de cgroup';
  return version === 'v1' ? 'v1' : 'v2';
}

/** Texto, ou travessão com motivo quando o campo é nulo. */
function textOrMissing(value: string | null): ReactNode {
  return value ?? <Missing reason="não exposto por este sistema" />;
}

/** Medida, ou travessão com motivo quando ela não existe. */
function measurementOrMissing(measurement: Measurement, reason: string | null): ReactNode {
  if (measurement.value === DASH) return <Missing reason={reason} />;
  return <Numeric value={measurement.value} unit={measurement.unit} size="sm" />;
}

/** Cartão detalhado de uma GPU. */
function GpuDetail({ gpu, reason }: { gpu: GpuProfile; reason: string | null }): ReactNode {
  const rows: DataRow[] = [
    { label: 'fabricante', value: GPU_VENDORS[gpu.vendor] },
    { label: 'endereço PCI', value: textOrMissing(gpu.pciAddress) },
    {
      label: 'VRAM total',
      value:
        gpu.vramTotalMb === null ? (
          <Missing reason={reason} />
        ) : (
          measurementOrMissing(formatMib(gpu.vramTotalMb), reason)
        ),
    },
    {
      label: 'VRAM livre',
      value:
        gpu.vramAvailableMb === null ? (
          <Missing reason={reason ?? 'a sonda não expõe VRAM livre'} />
        ) : (
          measurementOrMissing(formatMib(gpu.vramAvailableMb), reason)
        ),
      note: 'menor que o total por causa do compositor do desktop',
    },
    { label: 'driver', value: textOrMissing(gpu.driverVersion) },
    { label: 'runtime', value: textOrMissing(gpu.runtimeVersion) },
    { label: 'compute capability', value: textOrMissing(gpu.computeCapability) },
    {
      label: 'limite de potência',
      value:
        gpu.powerLimitWatts === null ? (
          <Missing reason={reason} />
        ) : (
          <Numeric value={formatDecimal(gpu.powerLimitWatts, 0)} unit="W" size="sm" />
        ),
    },
    { label: 'memória', value: gpu.isIntegrated ? 'integrada — compartilha a RAM' : 'dedicada' },
    { label: 'sonda', value: probeSourceLabel(gpu.source) },
  ];

  return (
    <li className="flex min-w-0 flex-col gap-3 border border-line bg-raised p-3 rounded-hair">
      <div className="flex items-baseline gap-2">
        <span className="num text-[11px] text-ink-faint">
          #{String(gpu.index).padStart(2, '0')}
        </span>
        <span className="truncate text-[13px] text-ink">{gpu.model}</span>
      </div>

      <div className="flex flex-wrap items-center gap-1.5">
        <span className="label">backends</span>
        {gpu.backends.length === 0 ? (
          <Missing reason="nenhum backend de inferência utilizável foi identificado" inline />
        ) : (
          gpu.backends.map((backend) => (
            <span
              key={backend}
              className="border border-compute/40 bg-compute/10 px-1.5 py-0.5 text-[10px] text-compute rounded-hair"
            >
              {backend}
            </span>
          ))
        )}
      </div>

      <DataList rows={rows} />
    </li>
  );
}

/** Nome legível da sonda que produziu os dados da GPU. */
function probeSourceLabel(source: GpuProfile['source']): string {
  switch (source) {
    case 'nvml':
      return 'NVML — dados completos e ao vivo';
    case 'nvidia-smi':
      return 'nvidia-smi — fallback de linha de comando';
    case 'sysfs':
      return '/sys/class/drm — identidade, sem métricas ao vivo';
  }
}

/** Cartão de um dispositivo de bloco e suas montagens. */
function StorageDetail({ device }: { device: StorageProfile }): ReactNode {
  return (
    <li className="flex min-w-0 flex-col gap-2 border border-line bg-raised p-3 rounded-hair">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <span className="num text-[13px] text-ink">{device.device}</span>
        <span className="text-[12px] text-ink-dim">{device.model ?? 'modelo não exposto'}</span>
        <span className="label ml-auto">{STORAGE_KINDS[device.kind]}</span>
      </div>

      <DataList
        columns={3}
        rows={[
          { label: 'capacidade', value: measurementOrMissing(formatMib(device.sizeMb), null) },
          {
            label: 'rotacional',
            value:
              device.rotational === null ? (
                <Missing reason="o kernel não expôs o atributo rotational" />
              ) : device.rotational ? (
                'sim'
              ) : (
                'não'
              ),
          },
          {
            label: 'leitura sequencial',
            value:
              device.measuredReadMbps === null ? (
                <Missing
                  reason="não medido (Fase 7) — o Calibration Engine ainda não existe"
                  inline
                />
              ) : (
                <Numeric value={formatDecimal(device.measuredReadMbps, 0)} unit="MB/s" size="sm" />
              ),
          },
        ]}
      />

      {device.mounts.length === 0 ? (
        <p className="font-sans text-[11px] text-ink-faint">
          nenhum ponto de montagem para este dispositivo
        </p>
      ) : (
        <div className="overflow-x-auto border-t border-line pt-2">
          <table className="w-full min-w-[520px] text-left">
            <thead>
              <tr>
                <th className="label pb-1 font-normal">montagem</th>
                <th className="label pb-1 font-normal">sistema de arquivos</th>
                <th className="label pb-1 text-right font-normal">total</th>
                <th className="label pb-1 text-right font-normal">disponível</th>
                <th className="label pb-1 text-right font-normal">modo</th>
              </tr>
            </thead>
            <tbody>
              {device.mounts.map((mount) => (
                <tr key={mount.path} className="border-t border-line">
                  <td className="num py-1 text-[11px] text-ink">{mount.path}</td>
                  <td className="num py-1 text-[11px] text-ink-dim">{mount.filesystem}</td>
                  <td className="num py-1 text-right text-[11px] text-ink-dim">
                    {formatMib(mount.totalMb).value} {formatMib(mount.totalMb).unit}
                  </td>
                  <td className="num py-1 text-right text-[11px] text-ink-dim">
                    {formatMib(mount.availableMb).value} {formatMib(mount.availableMb).unit}
                  </td>
                  <td className="py-1 text-right text-[11px] text-ink-faint">
                    {mount.readOnly ? 'somente leitura' : 'leitura e escrita'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </li>
  );
}

/** Tabela dos perfis já registrados para esta máquina. */
function HistoryTable({
  resource,
  onRetry,
}: {
  resource:
    | { state: 'loading' }
    | { state: 'ready'; data: HardwareHistoryEntry[] }
    | { state: 'error'; failure: { code: string; message: string } };
  onRetry: () => void;
}): ReactNode {
  if (resource.state === 'loading') {
    return <p className="font-sans text-[12px] text-ink-faint">carregando o histórico…</p>;
  }

  if (resource.state === 'error') {
    return <ErrorState failure={resource.failure} onRetry={onRetry} />;
  }

  if (resource.data.length === 0) {
    return (
      <EmptyState
        title="nenhum perfil registrado"
        detail="Este é o primeiro perfil desta máquina, ou o banco do daemon foi zerado. Uma impressão digital nova indica que o hardware mudou e que benchmarks anteriores deixaram de valer."
      />
    );
  }

  return (
    <div className="overflow-x-auto">
      <table className="w-full min-w-[720px] text-left">
        <thead>
          <tr>
            <th className="label pb-1 font-normal">impressão digital</th>
            <th className="label pb-1 font-normal">CPU</th>
            <th className="label pb-1 text-right font-normal">RAM</th>
            <th className="label pb-1 font-normal">GPU</th>
            <th className="label pb-1 font-normal">primeiro registro</th>
            <th className="label pb-1 font-normal">último registro</th>
          </tr>
        </thead>
        <tbody>
          {resource.data.map((entry) => (
            <tr key={entry.fingerprint} className="border-t border-line">
              <td className="num py-1 text-[11px] text-live">{entry.fingerprint}</td>
              <td className="py-1 text-[11px] text-ink-dim">{entry.cpuModel}</td>
              <td className="num py-1 text-right text-[11px] text-ink-dim">
                {formatMib(entry.ramTotalMb).value} {formatMib(entry.ramTotalMb).unit}
              </td>
              <td className="py-1 text-[11px] text-ink-dim">{entry.gpuSummary}</td>
              <td className="num py-1 text-[11px] text-ink-faint">
                {formatInstant(entry.firstSeenAt)}
              </td>
              <td className="num py-1 text-[11px] text-ink-faint">
                {formatInstant(entry.lastSeenAt)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
