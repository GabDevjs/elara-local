/**
 * Painel ao vivo.
 *
 * A tela responde a uma pergunta: o que esta máquina tem para dar ao Elara
 * agora? Por isso cada capacidade aparece duas vezes — o total físico, que é o
 * que o sistema operacional reporta, e o orçamento efetivo, que é o que o
 * Resource Broker pode gastar. Mostrar só o primeiro é o erro que faz um
 * modelo ser carregado e morrer por OOM.
 */
import type { GpuUsage, HardwareResponse, ProbeWarning } from '@elara/schemas';
import { useCallback, type ReactNode } from 'react';

import { CapacityBar, type CapacitySegment } from '../components/CapacityBar.js';
import { ComponentCard } from '../components/ComponentState.js';
import { CoreBars } from '../components/CoreBars.js';
import { Missing, reasonFor } from '../components/Missing.js';
import { Numeric } from '../components/Numeric.js';
import { Panel } from '../components/Panel.js';
import { Sparkline } from '../components/Sparkline.js';
import { EmptyState, ErrorState, LoadingState, OfflineState } from '../components/States.js';
import { fetchHardware } from '../lib/bridge.js';
import { useDaemon } from '../lib/daemon-session.js';
import {
  DASH,
  formatCelsius,
  formatDecimal,
  formatMib,
  formatPercent,
  formatWatts,
  ratioPercent,
  toNumber,
} from '../lib/format.js';
import { useAsync, useReloadOnEpoch } from '../lib/use-async.js';

/** Acima disto o número aparece em âmbar: a máquina está no limite. */
const PRESSURE_PERCENT = 85;

/** Cor do valor conforme a pressão sobre o recurso. */
function pressureTone(percent: number | null): 'live' | 'warn' {
  return percent !== null && percent >= PRESSURE_PERCENT ? 'warn' : 'live';
}

/** Renderiza o painel ao vivo. */
export function Dashboard(): ReactNode {
  const { connection, status, socket, snapshot, cpuHistory, hardwareEpoch, contractIssue, reload } =
    useDaemon();

  const loadHardware = useCallback(() => fetchHardware(false), []);
  const hardware = useAsync<HardwareResponse>(loadHardware);
  useReloadOnEpoch(hardwareEpoch, hardware.reload);

  if (connection === 'offline') {
    return (
      <OfflineState
        socket={socket}
        failure={status.state === 'error' ? status.failure : null}
        onRetry={reload}
      />
    );
  }

  if (status.state === 'error') {
    return <ErrorState failure={status.failure} onRetry={reload} />;
  }

  if (status.state === 'loading') {
    return <LoadingState what="consultando o estado do daemon e a última amostra de recursos" />;
  }

  if (snapshot === null) {
    return (
      <EmptyState
        title="sem amostra ainda"
        detail="O daemon precisa de duas leituras consecutivas para calcular percentuais. A primeira amostra aparece assim que o intervalo de amostragem completar."
      />
    );
  }

  const profile = hardware.value.state === 'ready' ? hardware.value.data.profile : null;
  const warnings: readonly ProbeWarning[] = profile?.warnings ?? [];

  const memory = snapshot.memory;
  const memoryTotal = toNumber(memory.totalMb);
  const memoryUsed = toNumber(memory.usedMb);
  const memoryCached = toNumber(memory.cachedMb);
  const memoryFree = Math.max(memoryTotal - memoryUsed - memoryCached, 0);
  const memoryBudget = profile === null ? null : toNumber(profile.effective.ramMb);

  const memorySegments: readonly CapacitySegment[] = [
    {
      id: 'used',
      label: 'usado',
      value: memoryUsed,
      display: displayMib(memoryUsed),
      color: 'var(--elara-live)',
      opacity: 0.85,
    },
    {
      id: 'cached',
      label: 'cache',
      value: memoryCached,
      display: displayMib(memoryCached),
      color: 'var(--elara-live)',
      opacity: 0.3,
    },
    {
      id: 'free',
      label: 'livre',
      value: memoryFree,
      display: displayMib(memoryFree),
      color: 'var(--elara-ink-faint)',
      opacity: 0.25,
    },
  ];

  const cpuPercent = snapshot.cpu.totalPercent;
  const cpuMeasurement = formatPercent(cpuPercent);
  const memoryMeasurement = formatPercent(memory.usedPercent);

  const vramBudget = profile === null ? null : toNumber(profile.effective.vramMb);
  const singleGpu = snapshot.gpus.length === 1;

  return (
    <div className="flex min-h-0 flex-1 flex-col overflow-y-auto">
      {contractIssue !== null ? (
        <div className="border-b border-warn/40 bg-warn/10 px-4 py-2">
          <p className="label text-warn">contrato divergente</p>
          <p className="font-sans text-[12px] leading-snug text-ink-dim">{contractIssue.message}</p>
        </div>
      ) : null}

      <div className="grid grid-cols-12 gap-3 p-3">
        <Panel
          title="componentes"
          meta={`${status.data.components.length} supervisionados`}
          className="col-span-12"
        >
          {status.data.components.length === 0 ? (
            <p className="font-sans text-[12px] text-ink-dim">
              O daemon está no ar e ainda não registrou nenhum componente.
            </p>
          ) : (
            <ul className="grid grid-cols-1 gap-2 sm:grid-cols-2 xl:grid-cols-4">
              {status.data.components.map((component) => (
                <ComponentCard key={component.name} status={component} />
              ))}
            </ul>
          )}
        </Panel>

        <Panel
          title="cpu"
          meta={profile === null ? undefined : profile.cpu.model}
          className="col-span-12 xl:col-span-7"
        >
          <div className="flex flex-col gap-4">
            <div className="flex items-end justify-between gap-4">
              <div className="flex flex-col gap-1">
                <span className="label">uso agregado</span>
                <Numeric
                  value={cpuMeasurement.value}
                  unit={cpuMeasurement.unit}
                  size="xl"
                  tone={pressureTone(cpuPercent)}
                  live
                />
              </div>
              <dl className="flex gap-5">
                {(['1 min', '5 min', '15 min'] as const).map((label, index) => (
                  <div key={label} className="flex flex-col gap-1">
                    <dt className="label">carga {label}</dt>
                    <dd>
                      <Numeric
                        value={formatDecimal(snapshot.cpu.loadAverage[index] ?? null)}
                        size="sm"
                        tone="dim"
                        live
                      />
                    </dd>
                  </div>
                ))}
              </dl>
            </div>

            {cpuHistory.length > 1 ? (
              <Sparkline
                values={cpuHistory}
                max={100}
                tone="var(--elara-live)"
                label="Histórico de uso agregado de CPU, escala de 0 a 100 por cento"
              />
            ) : (
              <p className="font-sans text-[11px] text-ink-faint">
                o histórico começa a partir da segunda amostra recebida
              </p>
            )}

            <div className="flex flex-col gap-2 border-t border-line pt-3">
              <span className="label">
                por núcleo lógico · {snapshot.cpu.perCorePercent.length} threads
              </span>
              <CoreBars perCorePercent={snapshot.cpu.perCorePercent} />
            </div>
          </div>
        </Panel>

        <Panel
          title="memória"
          meta={memoryBudget === null ? 'orçamento indisponível' : undefined}
          className="col-span-12 xl:col-span-5"
        >
          <div className="flex flex-col gap-4">
            <div className="flex items-end justify-between gap-4">
              <div className="flex flex-col gap-1">
                <span className="label">comprometido</span>
                <Numeric
                  value={memoryMeasurement.value}
                  unit={memoryMeasurement.unit}
                  size="xl"
                  tone={pressureTone(memory.usedPercent)}
                  live
                />
              </div>
              <div className="flex flex-col items-end gap-1">
                <span className="label">disponível</span>
                <Numeric
                  value={formatMib(memory.availableMb).value}
                  unit={formatMib(memory.availableMb).unit}
                  size="md"
                  tone="dim"
                  live
                />
              </div>
            </div>

            <CapacityBar
              total={memoryTotal}
              segments={memorySegments}
              budget={
                memoryBudget === null
                  ? null
                  : {
                      value: memoryBudget,
                      label: 'orçamento Elara',
                      display: displayMib(memoryBudget),
                    }
              }
              label="Memória do sistema dividida entre usado, cache e livre, com o orçamento do Elara marcado"
            />

            <dl className="grid grid-cols-2 gap-x-6 gap-y-2 border-t border-line pt-3">
              <MetricRow label="total físico" measurement={formatMib(memory.totalMb)} />
              <MetricRow
                label="orçamento Elara"
                measurement={memoryBudget === null ? null : formatMib(memoryBudget)}
                tone="compute"
                missingReason={
                  hardware.value.state === 'error'
                    ? hardware.value.failure.message
                    : 'o perfil de hardware ainda não foi carregado'
                }
              />
              <MetricRow label="swap usado" measurement={formatMib(memory.swapUsedMb)} />
              <MetricRow label="swap total" measurement={formatMib(memory.swapTotalMb)} />
            </dl>
          </div>
        </Panel>

        <Panel
          title="gpu"
          meta={
            vramBudget === null || snapshot.gpus.length === 0
              ? undefined
              : `orçamento somado ${displayMib(vramBudget)}`
          }
          className="col-span-12"
        >
          {snapshot.gpus.length === 0 ? (
            <p className="font-sans text-[12px] text-ink-dim">
              Nenhuma GPU foi detectada nesta máquina. A inferência usará a CPU.
            </p>
          ) : (
            <ul className="grid grid-cols-1 gap-3 xl:grid-cols-2">
              {snapshot.gpus.map((gpu) => (
                <GpuCard
                  key={gpu.index}
                  gpu={gpu}
                  warnings={warnings}
                  budgetMb={singleGpu ? vramBudget : null}
                />
              ))}
            </ul>
          )}
        </Panel>
      </div>
    </div>
  );
}

/** Formata MiB no formato compacto usado nas legendas. */
function displayMib(mib: number): string {
  const measurement = formatMib(mib);
  return `${measurement.value} ${measurement.unit}`;
}

/** Linha de métrica dentro de um painel, com tratamento de ausência. */
function MetricRow({
  label,
  measurement,
  tone = 'ink',
  missingReason = null,
}: {
  label: string;
  measurement: { value: string; unit: string } | null;
  tone?: 'ink' | 'dim' | 'compute';
  missingReason?: string | null;
}): ReactNode {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="label">{label}</dt>
      <dd>
        {measurement === null || measurement.value === DASH ? (
          <Missing reason={missingReason} />
        ) : (
          <Numeric value={measurement.value} unit={measurement.unit} size="sm" tone={tone} />
        )}
      </dd>
    </div>
  );
}

/** Cartão de uma GPU no painel ao vivo. */
function GpuCard({
  gpu,
  warnings,
  budgetMb,
}: {
  gpu: GpuUsage;
  warnings: readonly ProbeWarning[];
  budgetMb: number | null;
}): ReactNode {
  const probeReason = reasonFor(warnings, 'gpu');
  const utilization = formatPercent(gpu.utilizationPercent);
  const vramTotal = gpu.vramTotalMb === null ? null : toNumber(gpu.vramTotalMb);
  const vramUsed = gpu.vramUsedMb === null ? null : toNumber(gpu.vramUsedMb);

  const segments: readonly CapacitySegment[] =
    vramTotal === null || vramUsed === null
      ? []
      : [
          {
            id: 'used',
            label: 'em uso',
            value: vramUsed,
            display: displayMib(vramUsed),
            color: 'var(--elara-compute)',
            opacity: 0.9,
          },
          {
            id: 'free',
            label: 'livre',
            value: Math.max(vramTotal - vramUsed, 0),
            display: displayMib(Math.max(vramTotal - vramUsed, 0)),
            color: 'var(--elara-ink-faint)',
            opacity: 0.25,
          },
        ];

  return (
    <li className="flex min-w-0 flex-col gap-3 border border-line bg-raised p-3 rounded-hair">
      <div className="flex items-baseline justify-between gap-3">
        <span className="flex min-w-0 items-baseline gap-2">
          <span className="num shrink-0 text-[11px] text-ink-faint">
            #{String(gpu.index).padStart(2, '0')}
          </span>
          <span className="truncate text-[12px] text-ink" title={gpu.name}>
            {gpu.name}
          </span>
        </span>
        {gpu.utilizationPercent === null ? (
          <Missing reason={probeReason ?? 'a sonda não expõe utilização para esta GPU'} />
        ) : (
          <Numeric
            value={utilization.value}
            unit={utilization.unit}
            size="lg"
            tone={pressureTone(gpu.utilizationPercent)}
            live
          />
        )}
      </div>

      {segments.length > 0 && vramTotal !== null ? (
        <CapacityBar
          total={vramTotal}
          segments={segments}
          budget={
            budgetMb === null || budgetMb <= 0
              ? null
              : { value: budgetMb, label: 'orçamento Elara', display: displayMib(budgetMb) }
          }
          label={`VRAM de ${gpu.name}, dividida entre em uso e livre`}
        />
      ) : (
        <div className="flex items-baseline gap-2">
          <span className="label">vram</span>
          <Missing
            reason={probeReason ?? 'VRAM não reportada — sem NVML ou ROCm nesta máquina'}
            inline
          />
        </div>
      )}

      <dl className="grid grid-cols-3 gap-x-4 border-t border-line pt-2.5">
        <GpuMetric
          label="vram usada"
          measurement={gpu.vramUsedMb === null ? null : formatMib(gpu.vramUsedMb)}
          reason={probeReason}
        />
        <GpuMetric
          label="temperatura"
          measurement={maybe(gpu.temperatureC, formatCelsius)}
          reason={probeReason}
        />
        <GpuMetric
          label="potência"
          measurement={maybe(gpu.powerWatts, formatWatts)}
          reason={probeReason}
        />
      </dl>

      {vramTotal !== null && vramUsed !== null ? (
        <p className="label">
          {formatPercent(ratioPercent(vramUsed, vramTotal)).value}% da VRAM ocupada
        </p>
      ) : null}
    </li>
  );
}

/** Aplica o formatador só quando o valor existe. */
function maybe(
  value: number | null,
  format: (input: number | null) => { value: string; unit: string },
): { value: string; unit: string } | null {
  return value === null ? null : format(value);
}

/** Métrica compacta do cartão de GPU. */
function GpuMetric({
  label,
  measurement,
  reason,
}: {
  label: string;
  measurement: { value: string; unit: string } | null;
  reason: string | null;
}): ReactNode {
  return (
    <div className="flex flex-col gap-0.5">
      <dt className="label">{label}</dt>
      <dd>
        {measurement === null ? (
          <Missing reason={reason} />
        ) : (
          <Numeric value={measurement.value} unit={measurement.unit} size="sm" tone="dim" live />
        )}
      </dd>
    </div>
  );
}
