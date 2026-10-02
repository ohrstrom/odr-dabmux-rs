import * as React from "react"
import {
  ArrowLeftIcon,
  PencilSimpleIcon,
  PlusIcon,
  TrashIcon,
} from "@phosphor-icons/react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
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
  None,
  PageHeader,
  Section,
} from "@/components/fields"
import { ServiceDeleteDialog } from "@/components/service-delete"
import { ServiceFormDialog } from "@/components/service-form"
import { LinkageSetTable } from "@/components/service-following"
import {
  SUBCHANNEL_TYPES,
  USER_APPLICATIONS,
  hex,
  languageLabel,
  protectionLabel,
  ptyLabel,
  serviceKey,
  sid,
  type ResolvedConfig,
  type Service,
} from "@/lib/config"
import { useDraft } from "@/hooks/use-draft"
import { href } from "@/hooks/use-route"

export function ServiceKind({ service }: { service: Service }) {
  return service.data ? (
    <Badge variant="outline">data</Badge>
  ) : (
    <Badge variant="secondary">programme</Badge>
  )
}

/** Added or edited in the pending changes. */
function PendingBadge({ id }: { id: number }) {
  const { changes } = useDraft()
  const kind = changes.find((group) => group.key === `service-${id}`)?.kind
  if (!kind) return null
  return (
    <Badge variant="outline" className="ml-2 text-amber-600 dark:text-amber-400">
      {kind === "added" ? "new" : "edited"}
    </Badge>
  )
}

export function ServicesTable({ config }: { config: ResolvedConfig }) {
  const { services, ensemble } = config
  if (services.length === 0) return <EmptyState>No services.</EmptyState>

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead className="pl-4">SId</TableHead>
          <TableHead>Label</TableHead>
          <TableHead>Short</TableHead>
          <TableHead>Kind</TableHead>
          <TableHead>PTy</TableHead>
          <TableHead>Components</TableHead>
          <TableHead className="pr-4 text-right">Linkage sets</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {services.map((service) => (
          <TableRow
            key={service.id}
            className="cursor-pointer"
            onClick={() => {
              window.location.hash = href("services", serviceKey(service))
            }}
          >
            <TableCell className="pl-4 tabular-nums">
              <a href={href("services", serviceKey(service))}>
                {sid(service.id)}
              </a>
            </TableCell>
            <TableCell className="font-medium">
              {service.label}
              <PendingBadge id={service.id} />
            </TableCell>
            <TableCell>{service.short_label ?? <None />}</TableCell>
            <TableCell>
              <ServiceKind service={service} />
            </TableCell>
            <TableCell>
              {service.data ? (
                <None />
              ) : (
                ptyLabel(service.pty, ensemble.international_table)
              )}
            </TableCell>
            <TableCell>
              <div className="flex flex-wrap gap-1">
                {service.components.map((c) => (
                  <Badge key={c.subchannel} variant="outline">
                    {c.subchannel}
                  </Badge>
                ))}
                {service.components
                  .flatMap((c) => c.user_applications)
                  .map((app, i) => (
                    <Badge key={`${app}-${i}`} variant="secondary">
                      {USER_APPLICATIONS[app]}
                    </Badge>
                  ))}
              </div>
            </TableCell>
            <TableCell className="pr-4 text-right tabular-nums">
              {service.linking.length || <None />}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}

export function ServicesPage({
  config,
  onChanged,
}: {
  config: ResolvedConfig
  /** Called after an edit, to reload the configuration. */
  onChanged: () => void
}) {
  const [adding, setAdding] = React.useState(false)

  return (
    <div className="space-y-6">
      <PageHeader
        title="Services"
        description={`${config.services.length} services in ${config.ensemble.label}`}
      >
        <Button size="sm" onClick={() => setAdding(true)}>
          <PlusIcon data-icon="inline-start" />
          Add service
        </Button>
      </PageHeader>
      <ServiceFormDialog
        config={config}
        open={adding}
        onOpenChange={setAdding}
        onSaved={(service) => {
          onChanged()
          window.location.hash = href("services", sid(service.id))
        }}
      />
      <Section title="All services" flush>
        <ServicesTable config={config} />
      </Section>
    </div>
  )
}

export function ServiceDetail({
  config,
  service,
  onChanged,
}: {
  config: ResolvedConfig
  service: Service
  onChanged: () => void
}) {
  const { ensemble } = config
  const [editing, setEditing] = React.useState(false)
  const [deleting, setDeleting] = React.useState(false)
  const subchannels = new Map(config.subchannels.map((s) => [s.name, s]))
  const ecc = service.ecc ?? ensemble.ecc

  return (
    <div className="space-y-6">
      <PageHeader
        title={
          <span className="flex items-center gap-3">
            {service.label}
            <ServiceKind service={service} />
          </span>
        }
        description={`SId ${sid(service.id)}`}
      >
        <div className="flex gap-2">
          <Button
            variant="ghost"
            size="sm"
            nativeButton={false}
            render={<a href={href("services")} />}
          >
            <ArrowLeftIcon data-icon="inline-start" />
            All services
          </Button>
          <Button variant="outline" size="sm" onClick={() => setEditing(true)}>
            <PencilSimpleIcon data-icon="inline-start" />
            Edit
          </Button>
          <Button variant="destructive" size="sm" onClick={() => setDeleting(true)}>
            <TrashIcon data-icon="inline-start" />
            Delete
          </Button>
        </div>
      </PageHeader>
      <ServiceFormDialog
        config={config}
        sid={serviceKey(service)}
        open={editing}
        onOpenChange={setEditing}
        onSaved={(saved) => {
          onChanged()
          if (saved.id !== service.id) {
            window.location.hash = href("services", sid(saved.id))
          }
        }}
      />
      <ServiceDeleteDialog
        service={service}
        open={deleting}
        onOpenChange={setDeleting}
        onDeleted={() => {
          onChanged()
          window.location.hash = href("services")
        }}
      />

      <Section title="Service">
        <Fields>
          <Field label="Label">{service.label}</Field>
          <Field label="Short label">{service.short_label ?? <None />}</Field>
          <Field label="Service ID">{sid(service.id)}</Field>
          <Field
            label="ECC"
            hint={service.ecc === null ? "ensemble ECC" : "differs from ensemble"}
          >
            {hex(ecc)}
          </Field>
          {!service.data && (
            <>
              <Field label="Programme type">
                {ptyLabel(service.pty, ensemble.international_table)}
              </Field>
              <Field label="Language">{languageLabel(service.language)}</Field>
            </>
          )}
          <Field label="Other ensembles">
            {service.other_ensembles.length ? (
              <span className="flex flex-wrap gap-1">
                {service.other_ensembles.map((eid) => (
                  <Badge key={eid} variant="outline">
                    {hex(eid, 4)}
                  </Badge>
                ))}
              </span>
            ) : (
              <None />
            )}
          </Field>
        </Fields>
      </Section>

      <Section title="Components" flush>
        <Table>
          <TableHeader>
            <TableRow>
              <TableHead className="pl-4">SCIdS</TableHead>
              <TableHead>Subchannel</TableHead>
              <TableHead>Type</TableHead>
              <TableHead>Bitrate</TableHead>
              <TableHead>Protection</TableHead>
              <TableHead>Packet</TableHead>
              <TableHead className="pr-4">User applications</TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {service.components.map((c) => {
              const sub = subchannels.get(c.subchannel)
              return (
                <TableRow key={c.subchannel}>
                  <TableCell className="pl-4 tabular-nums">{c.scids}</TableCell>
                  <TableCell>
                    {sub ? (
                      <a
                        className="underline-offset-4 hover:underline"
                        href={href("subchannels", String(sub.id))}
                      >
                        {sub.id} · {c.subchannel}
                      </a>
                    ) : (
                      c.subchannel
                    )}
                  </TableCell>
                  <TableCell>{sub ? SUBCHANNEL_TYPES[sub.type] : <None />}</TableCell>
                  <TableCell className="tabular-nums">
                    {sub ? `${sub.bitrate} kbit/s` : <None />}
                  </TableCell>
                  <TableCell>
                    {sub ? protectionLabel(sub.protection) : <None />}
                  </TableCell>
                  <TableCell>
                    {c.packet ? (
                      <span className="text-xs">
                        address {c.packet.address} · DSCTy {c.packet.dscty}
                        {c.packet.data_groups && " · data groups"}
                        {c.scid !== undefined && ` · SCId ${c.scid}`}
                      </span>
                    ) : (
                      <None />
                    )}
                  </TableCell>
                  <TableCell className="pr-4">
                    {c.user_applications.length ? (
                      <span className="flex flex-wrap gap-1">
                        {c.user_applications.map((app) => (
                          <Badge key={app} variant="secondary">
                            {USER_APPLICATIONS[app]}
                          </Badge>
                        ))}
                      </span>
                    ) : (
                      <None />
                    )}
                  </TableCell>
                </TableRow>
              )
            })}
          </TableBody>
        </Table>
      </Section>

      <Section
        title="Linkage sets"
        description="Service following, FIG 0/6"
        flush
      >
        <LinkageSetTable
          rows={service.linking.map((set) => ({ service, set }))}
          ensembleEcc={ensemble.ecc}
          showService={false}
        />
      </Section>
    </div>
  )
}
