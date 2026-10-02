import * as React from "react"
import { PencilSimpleIcon } from "@phosphor-icons/react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { EnsembleFormDialog } from "@/components/ensemble-form"
import {
  Field,
  Fields,
  Flag,
  None,
  PageHeader,
  Section,
  Stat,
} from "@/components/fields"
import { MscMap } from "@/components/msc-map"
import { ServicesTable } from "@/components/services"
import {
  MSC_CAPACITY_CU,
  hex,
  localTimeOffset,
  urlHost,
  type ResolvedConfig,
} from "@/lib/config"

export function Overview({
  config,
  onChanged,
}: {
  config: ResolvedConfig
  /** Called after an edit, to reload the configuration. */
  onChanged: () => void
}) {
  const { ensemble, services, subchannels } = config
  const [editing, setEditing] = React.useState(false)
  const usedCu = subchannels.reduce((sum, s) => sum + s.size_cu, 0)
  const bitrate = subchannels.reduce((sum, s) => sum + s.bitrate, 0)
  const data = services.filter((s) => s.data).length
  const linkageSets = services.reduce((sum, s) => sum + s.linking.length, 0)

  return (
    <div className="space-y-6">
      <PageHeader
        title={ensemble.label}
        description={
          <>
            Ensemble {hex(ensemble.id, 4)} · ECC {hex(ensemble.ecc)} ·
            transmission mode {ensemble.mode}
          </>
        }
      >
        <Button variant="outline" size="sm" onClick={() => setEditing(true)}>
          <PencilSimpleIcon data-icon="inline-start" />
          Edit ensemble
        </Button>
      </PageHeader>
      <EnsembleFormDialog
        open={editing}
        onOpenChange={setEditing}
        onSaved={onChanged}
      />

      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        <Stat
          label="Services"
          value={services.length}
          detail={`${services.length - data} programme · ${data} data`}
        />
        <Stat
          label="Subchannels"
          value={subchannels.length}
          detail={`${bitrate} kbit/s in total`}
        />
        <Stat
          label="MSC capacity"
          value={`${Math.round((usedCu / MSC_CAPACITY_CU) * 100)}%`}
          detail={`${usedCu} of ${MSC_CAPACITY_CU} CU`}
        />
        <Stat
          label="Service following"
          value={linkageSets}
          detail={`linkage sets · ${config.frequencies.length} FI lists`}
        />
      </div>

      <Section title="MSC allocation" description="Subchannels in CU order">
        <MscMap subchannels={subchannels} />
      </Section>

      <div className="grid gap-6 xl:grid-cols-2">
        <Section title="Ensemble">
          <Fields>
            <Field label="Label">{ensemble.label}</Field>
            <Field label="Short label">
              {ensemble.short_label ?? <None />}
            </Field>
            <Field label="Ensemble ID">{hex(ensemble.id, 4)}</Field>
            <Field label="ECC">{hex(ensemble.ecc)}</Field>
            <Field label="Mode">{ensemble.mode}</Field>
            <Field label="International table">
              {ensemble.international_table}
            </Field>
            <Field label="Local time offset">
              {localTimeOffset(ensemble)}
            </Field>
            <Field label="Reconfiguration counter">
              {ensemble.reconfiguration_counter ?? <None>automatic</None>}
            </Field>
          </Fields>
        </Section>

        <Section title="Timing">
          <Fields>
            <Field label="TIST">
              <Flag on={ensemble.tist} yes="enabled" no="disabled" />
            </Field>
            <Field label="TIST offset">{ensemble.tist_offset_ms} ms</Field>
            <Field label="TIST at FCT 0">{ensemble.tist_at_fct0_ms} ms</Field>
            <Field label="TAI − UTC">
              {ensemble.tai_utc_offset !== null ? (
                `${ensemble.tai_utc_offset} s`
              ) : (
                <None>from bulletin</None>
              )}
            </Field>
          </Fields>
          {ensemble.tai_clock_bulletins.length > 0 && (
            <div className="mt-4">
              <div className="text-[0.625rem] tracking-wider text-muted-foreground uppercase">
                Leap second bulletins
              </div>
              <ul className="mt-1 space-y-1">
                {ensemble.tai_clock_bulletins.map((url) => (
                  <li key={url} className="truncate text-xs">
                    <Badge variant="outline" className="mr-2">
                      {urlHost(url)}
                    </Badge>
                    <span className="text-muted-foreground">{url}</span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </Section>
      </div>

      <Section title="Services" flush>
        <ServicesTable config={config} />
      </Section>
    </div>
  )
}
