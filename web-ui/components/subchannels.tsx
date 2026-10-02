import type * as React from "react"
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
  EmptyState,
  Field,
  Fields,
  Flag,
  None,
  PageHeader,
  Section,
} from "@/components/fields"
import { MscMap } from "@/components/msc-map"
import {
  SUBCHANNEL_TYPES,
  inputLabel,
  protectionLabel,
  serviceKey,
  subchannelUsers,
  type Input,
  type ResolvedConfig,
  type Subchannel,
} from "@/lib/config"
import { href } from "@/hooks/use-route"

export function SubchannelsPage({
  config,
  selected,
}: {
  config: ResolvedConfig
  /** SubChId from the route. */
  selected?: number
}) {
  const users = subchannelUsers(config)
  const current = config.subchannels.find((s) => s.id === selected)

  return (
    <div className="space-y-6">
      <PageHeader
        title="Subchannels"
        description={`${config.subchannels.length} subchannels in the MSC`}
      />

      <Section title="MSC allocation">
        <MscMap subchannels={config.subchannels} selected={current?.id} />
      </Section>

      {current && <SubchannelDetail config={config} subchannel={current} />}

      <Section title="All subchannels" flush>
        {config.subchannels.length === 0 ? (
          <EmptyState>No subchannels.</EmptyState>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="pl-4">SubChId</TableHead>
                <TableHead>Name</TableHead>
                <TableHead>Type</TableHead>
                <TableHead className="text-right">Bitrate</TableHead>
                <TableHead>Protection</TableHead>
                <TableHead className="text-right">Start CU</TableHead>
                <TableHead className="text-right">Size CU</TableHead>
                <TableHead>Input</TableHead>
                <TableHead className="pr-4">Services</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {config.subchannels.map((s) => (
                <TableRow
                  key={s.name}
                  data-state={s.id === selected ? "selected" : undefined}
                  className="cursor-pointer"
                  onClick={() => {
                    window.location.hash = href("subchannels", String(s.id))
                  }}
                >
                  <TableCell className="pl-4 tabular-nums">
                    <a href={href("subchannels", String(s.id))}>{s.id}</a>
                    {s.allocated && (
                      <Badge variant="outline" className="ml-2">
                        auto
                      </Badge>
                    )}
                  </TableCell>
                  <TableCell className="font-medium">{s.name}</TableCell>
                  <TableCell>{SUBCHANNEL_TYPES[s.type]}</TableCell>
                  <TableCell className="text-right tabular-nums">
                    {s.bitrate} kbit/s
                  </TableCell>
                  <TableCell>{protectionLabel(s.protection)}</TableCell>
                  <TableCell className="text-right tabular-nums">
                    {s.start_address_cu}
                  </TableCell>
                  <TableCell className="text-right tabular-nums">
                    {s.size_cu}
                  </TableCell>
                  <TableCell className="max-w-64">
                    <InputSummary input={s.input} />
                  </TableCell>
                  <TableCell className="pr-4">
                    <ServiceList services={users.get(s.name) ?? []} />
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Section>
    </div>
  )
}

function InputSummary({ input }: { input: Input }) {
  return (
    <span className="flex min-w-0 items-center gap-2">
      <Badge variant="outline" className="uppercase">
        {input.protocol}
      </Badge>
      <span className="truncate text-xs text-muted-foreground">
        {inputLabel(input)}
      </span>
    </span>
  )
}

function ServiceList({
  services,
}: {
  services: ResolvedConfig["services"]
}) {
  if (services.length === 0) return <None>unused</None>
  return (
    <span className="flex flex-wrap gap-1">
      {services.map((service) => (
        <Badge
          key={service.id}
          variant="secondary"
          render={<a href={href("services", serviceKey(service))} />}
          onClick={(e: React.MouseEvent) => e.stopPropagation()}
        >
          {service.label}
        </Badge>
      ))}
    </span>
  )
}

function SubchannelDetail({
  config,
  subchannel: s,
}: {
  config: ResolvedConfig
  subchannel: Subchannel
}) {
  const users = subchannelUsers(config).get(s.name) ?? []
  const input = s.input

  return (
    <Section
      title={
        <span className="flex items-center gap-2">
          SubChId {s.id} · {s.name}
        </span>
      }
      description={SUBCHANNEL_TYPES[s.type]}
    >
      <div className="space-y-6">
        <Fields>
          <Field
            label="SubChId"
            hint={s.allocated ? "allocated by the mux" : "configured"}
          >
            {s.id}
          </Field>
          <Field label="Bitrate">{s.bitrate} kbit/s</Field>
          <Field label="Protection">{protectionLabel(s.protection)}</Field>
          <Field label="CU range">
            {s.start_address_cu}–{s.start_address_cu + s.size_cu - 1}
          </Field>
          <Field label="Size">{s.size_cu} CU</Field>
          <Field label="Services">
            <ServiceList services={users} />
          </Field>
        </Fields>

        <div>
          <div className="mb-3 text-[0.625rem] tracking-wider text-muted-foreground uppercase">
            Input
          </div>
          <Fields className={cn("rounded-none border p-4")}>
            <Field label="Protocol">
              <span className="uppercase">{input.protocol}</span>
            </Field>
            {input.protocol === "file" ? (
              <Field label="Path">
                <span title={input.path}>{input.path}</span>
              </Field>
            ) : (
              <Field label="URI">{input.uri}</Field>
            )}
            {input.protocol === "edi" && (
              <>
                <Field label="Stream index">{input.stream_index}</Field>
                <Field label="Timing">{input.timing}</Field>
                <Field label="Buffer">{input.buffer_frames} frames</Field>
                <Field label="Prebuffer">{input.prebuffer_frames} frames</Field>
                <Field label="Backpressure">
                  {input.backpressure === null ? (
                    <None>transport default</None>
                  ) : (
                    <Flag on={input.backpressure} />
                  )}
                </Field>
              </>
            )}
          </Fields>
        </div>
      </div>
    </Section>
  )
}
