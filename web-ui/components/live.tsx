import * as React from "react"
import {
  CheckCircleIcon,
  HourglassMediumIcon,
  XCircleIcon,
} from "@phosphor-icons/react"
import { cn } from "cn"
import { Badge } from "@/components/ui/badge"
import {
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@/components/ui/table"
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { EmptyState, None, PageHeader, Section, Stat } from "@/components/fields"
import { SelectField } from "@/components/form-fields"
import { LevelMeter } from "@/components/level-meter"
import { serviceKey, subchannelUsers, type ResolvedConfig } from "@/lib/config"
import { href } from "@/hooks/use-route"
import {
  rate,
  useLiveStats,
  type InputState,
  type LiveStats,
  type LiveSubchannel,
} from "@/hooks/use-live-stats"

const FRAME_MS = 24

const INTERVALS = [
  { value: "24", label: "Every frame (24 ms)" },
  { value: "100", label: "100 ms" },
  { value: "250", label: "250 ms" },
  { value: "1000", label: "1 s" },
]

const STORAGE_KEY = "dabmux-live-interval"

function storedInterval(): string {
  try {
    const value = localStorage.getItem(STORAGE_KEY)
    return INTERVALS.some((i) => i.value === value) ? value! : "250"
  } catch {
    return "250"
  }
}

const number = new Intl.NumberFormat()

function perSecond(value: number | null): string | undefined {
  if (value === null) return undefined
  return value === 0 ? "none in the last interval" : `${value.toFixed(1)} per second`
}

/** The running multiplex, live: counters, inputs and outputs. */
export function LivePage({ config }: { config: ResolvedConfig }) {
  const [interval, setIntervalMs] = React.useState(storedInterval)
  const live = useLiveStats(Number(interval))
  const { sample } = live

  const changeInterval = (value: string) => {
    setIntervalMs(value)
    try {
      localStorage.setItem(STORAGE_KEY, value)
    } catch {
      // only a convenience
    }
  }

  const { fps } = live
  const counters = sample?.counters

  return (
    <div className="space-y-6">
      <PageHeader
        title="Live"
        description={
          sample
            ? `Frame ${number.format(sample.frame)} · ${new Date(sample.unix_ms).toISOString().slice(11, 23)} UTC`
            : "Waiting for the first frame…"
        }
      >
        <div className="flex items-end gap-4">
          <Connection connected={live.connected} error={live.error} />
          <div className="w-48">
            <SelectField
              label="Update"
              value={interval}
              onChange={changeInterval}
              items={INTERVALS}
            />
          </div>
        </div>
      </PageHeader>

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <Stat
          label="Frame rate"
          value={fps !== null ? `${fps.toFixed(1)}/s` : "—"}
          detail={`${(1000 / FRAME_MS).toFixed(1)}/s expected`}
        />
        <Stat
          label="Clock drift"
          value={counters ? `${counters.clock_drift_ms} ms` : "—"}
          detail="Frame clock against system time"
        />
        <Stat
          label="Input underflows"
          value={counters ? number.format(counters.input_underflows) : "—"}
          detail={perSecond(rate(live, (s) => s.counters.input_underflows))}
        />
        <Stat
          label="Dropped input frames"
          value={counters ? number.format(counters.input_drops) : "—"}
          detail={perSecond(rate(live, (s) => s.counters.input_drops))}
        />
      </div>

      <Section
        title="Inputs"
        description="Buffer: current fill, with its range over the update interval"
        flush
      >
        <InputsTable config={config} live={live} />
      </Section>

      <Section
        title="Audio levels"
        description="Peak levels the encoders send with the audio, as on air"
      >
        <AudioLevels config={config} live={live} />
      </Section>

      <div className="grid gap-6 xl:grid-cols-2">
        <Section title="Outputs" flush>
          {!sample ? (
            <EmptyState>Waiting for the first frame…</EmptyState>
          ) : sample.outputs.length === 0 ? (
            <EmptyState>No outputs.</EmptyState>
          ) : (
            <Table>
              <TableHeader>
                <TableRow>
                  <TableHead className="pl-4">Protocol</TableHead>
                  <TableHead>Endpoint</TableHead>
                  <TableHead className="pr-4 text-right">Clients</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {sample.outputs.map((output) => (
                  <TableRow key={`${output.protocol}-${output.endpoint}`}>
                    <TableCell className="pl-4">
                      <Badge variant="outline" className="uppercase">
                        {output.protocol}
                      </Badge>
                    </TableCell>
                    <TableCell className="tabular-nums">
                      {output.protocol === "tcp"
                        ? `port ${output.endpoint}`
                        : output.endpoint}
                    </TableCell>
                    <TableCell className="pr-4 text-right tabular-nums">
                      {output.clients ?? <None />}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          )}
        </Section>

        <Section title="Counters" description="Since the mux started">
          {counters ? (
            <dl className="grid grid-cols-2 gap-x-6 gap-y-2 text-xs">
              {(
                [
                  ["Generated frames", counters.generated_frames],
                  ["Configurations activated", counters.config_activations],
                  ["Catch-up frames", counters.catch_up_frames],
                  ["Skipped frames", counters.missed_ticks],
                  ["Late input frames", counters.late_input_frames],
                  ["Invalid timestamps", counters.invalid_timestamps],
                  ["Wrong input sizes", counters.input_size_mismatches],
                  ["Decode errors", counters.decode_errors],
                  ["Send errors", counters.send_errors],
                  ["Frame errors", counters.frame_errors],
                ] as const
              ).map(([label, value]) => (
                <div key={label} className="flex justify-between gap-4 border-b py-1">
                  <dt className="text-muted-foreground">{label}</dt>
                  <dd className="tabular-nums">{number.format(value)}</dd>
                </div>
              ))}
            </dl>
          ) : (
            <EmptyState>Waiting for the first frame…</EmptyState>
          )}
        </Section>
      </div>
    </div>
  )
}

function Connection({ connected, error }: { connected: boolean; error: string | null }) {
  return (
    <span
      className="mb-2 inline-flex items-center gap-1.5 text-xs text-muted-foreground"
      title={error ?? undefined}
    >
      <span
        className={cn(
          "size-2 rounded-full",
          connected ? "bg-status-good" : "bg-status-critical"
        )}
      />
      {connected ? "Streaming" : error ? `Not connected (${error})` : "Reconnecting…"}
    </span>
  )
}

function InputsTable({ config, live }: { config: ResolvedConfig; live: LiveStats }) {
  const { sample } = live
  if (!sample) return <EmptyState>Waiting for the first frame…</EmptyState>
  const users = subchannelUsers(config)

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead className="pl-4">SubChId</TableHead>
          <TableHead>Subchannel</TableHead>
          <TableHead>State</TableHead>
          <TableHead className="w-56">Buffer</TableHead>
          <TableHead className="text-right">Underflows</TableHead>
          <TableHead className="pr-4 text-right">Drops</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {sample.subchannels.map((sub, i) => {
          const underflowRate = rate(live, (s) => s.subchannels[i]?.underflows ?? 0)
          return (
            <TableRow key={sub.name}>
              <TableCell className="pl-4 tabular-nums">
                <a href={href("subchannels", String(sub.id))}>{sub.id}</a>
              </TableCell>
              <TableCell>
                <span className="font-medium">{sub.name}</span>
                {(users.get(sub.name) ?? []).map((service) => (
                  <a
                    key={service.id}
                    href={href("services", serviceKey(service))}
                    className="ml-2 text-muted-foreground underline-offset-4 hover:underline"
                  >
                    {service.label}
                  </a>
                ))}
              </TableCell>
              <TableCell>
                <StateLabel state={sub.state} />
              </TableCell>
              <TableCell>
                <BufferMeter sub={sub} intervalMs={sample.interval_ms} />
              </TableCell>
              <TableCell className="text-right tabular-nums">
                {number.format(sub.underflows)}
                {underflowRate ? (
                  <span className="ml-1 text-muted-foreground">
                    +{underflowRate.toFixed(0)}/s
                  </span>
                ) : null}
              </TableCell>
              <TableCell className="pr-4 text-right tabular-nums">
                {number.format(sub.drops)}
              </TableCell>
            </TableRow>
          )
        })}
      </TableBody>
    </Table>
  )
}

const SILENCE_DB = -90

function AudioLevels({ config, live }: { config: ResolvedConfig; live: LiveStats }) {
  const { sample } = live
  if (!sample) return <EmptyState>Waiting for the first frame…</EmptyState>
  const users = subchannelUsers(config)
  const audio = sample.subchannels.filter((sub) => {
    const type = config.subchannels.find((s) => s.name === sub.name)?.type
    return type === "dab_plus" || type === "mpeg_audio"
  })
  if (audio.length === 0) return <EmptyState>No audio subchannels.</EmptyState>

  return (
    <div className="grid gap-x-8 gap-y-5 md:grid-cols-2 xl:grid-cols-4">
      {audio.map((sub) => {
        const services = users.get(sub.name) ?? []
        return (
          <div key={sub.name} className="min-w-0 space-y-1.5">
            <div className="flex items-baseline justify-between gap-3 text-xs">
              <span className="min-w-0 truncate">
                <span className="font-medium">
                  {services.map((s) => s.label).join(", ") || sub.name}
                </span>
                <span className="ml-2 text-muted-foreground">
                  SubChId {sub.id}
                </span>
              </span>
              {!sub.audio && (
                <span className="shrink-0 text-muted-foreground">
                  {sub.state === "receiving"
                    ? "no levels from the encoder"
                    : STATES[sub.state].label.toLowerCase()}
                </span>
              )}
            </div>
            <LevelMeter
              levels={
                sub.audio
                  ? [sub.audio.left_db, sub.audio.right_db]
                  : [SILENCE_DB, SILENCE_DB]
              }
            />
          </div>
        )
      })}
    </div>
  )
}

const STATES: Record<
  InputState,
  { label: string; icon: React.ComponentType<{ className?: string }>; className: string }
> = {
  receiving: { label: "Receiving", icon: CheckCircleIcon, className: "text-status-good" },
  prebuffering: {
    label: "Prebuffering",
    icon: HourglassMediumIcon,
    className: "text-status-warning",
  },
  underflow: { label: "Underflow", icon: XCircleIcon, className: "text-status-critical" },
}

/** Status colour always with its icon and label. */
function StateLabel({ state }: { state: InputState }) {
  const { label, icon: Icon, className } = STATES[state]
  return (
    <span className="inline-flex items-center gap-1.5">
      <Icon className={cn("size-4", className)} />
      {label}
    </span>
  )
}

function BufferMeter({ sub, intervalMs }: { sub: LiveSubchannel; intervalMs: number }) {
  if (sub.capacity === 0) return <None>packet file</None>
  const pct = (frames: number) => `${Math.min(100, (frames / sub.capacity) * 100)}%`
  return (
    <Tooltip>
      <TooltipTrigger
        render={
          <div className="flex items-center gap-2">
            <div
              className="relative h-2 w-32 shrink-0 overflow-hidden rounded-full bg-msc-dab-plus/15"
              role="meter"
              aria-valuemin={0}
              aria-valuemax={sub.capacity}
              aria-valuenow={sub.buffered}
              aria-label={`Buffer of ${sub.name}`}
            >
              {/* Range over the interval, then the current fill. */}
              <div
                className="absolute inset-y-0 bg-msc-dab-plus/35"
                style={{
                  left: pct(sub.buffered_min),
                  width: `calc(${pct(sub.buffered_max)} - ${pct(sub.buffered_min)})`,
                }}
              />
              <div
                className="absolute inset-y-0 left-0 rounded-full bg-msc-dab-plus"
                style={{ width: pct(sub.buffered) }}
              />
            </div>
            <span className="text-muted-foreground tabular-nums">
              {sub.buffered}/{sub.capacity}
            </span>
          </div>
        }
      />
      <TooltipContent className="flex-col items-start gap-0.5">
        <span>
          {sub.buffered} of {sub.capacity} frames buffered ({sub.buffered * FRAME_MS} ms)
        </span>
        <span>
          {sub.buffered_min}–{sub.buffered_max} over the last {intervalMs} ms
        </span>
      </TooltipContent>
    </Tooltip>
  )
}
