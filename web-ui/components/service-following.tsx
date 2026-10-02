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
  Flag,
  None,
  PageHeader,
  Section,
} from "@/components/fields"
import {
  CHANGE_KINDS,
  LINK_KINDS,
  hex,
  serviceKey,
  sid,
  type FrequencyInformation,
  type LinkageSet,
  type ResolvedConfig,
  type Service,
} from "@/lib/config"
import { href } from "@/hooks/use-route"

function linkId(link: LinkageSet["links"][number]): string {
  switch (link.type) {
    case "dab":
      return sid(link.id)
    case "fm":
      return hex(link.id, 4)
    default:
      return hex(link.id, 6)
  }
}

function ServiceRef({ config, id }: { config: ResolvedConfig; id: number }) {
  const service = config.services.find((s) => s.id === id)
  if (!service) return <span className="tabular-nums">{sid(id)}</span>
  return (
    <a
      className="tabular-nums underline-offset-4 hover:underline"
      href={href("services", serviceKey(service))}
    >
      {sid(id)} <span className="text-muted-foreground">{service.label}</span>
    </a>
  )
}

export function LinkageSetTable({
  rows,
  ensembleEcc,
  showService = true,
}: {
  rows: { service: Service; set: LinkageSet }[]
  ensembleEcc: number
  showService?: boolean
}) {
  if (rows.length === 0) return <EmptyState>No linkage sets.</EmptyState>

  return (
    <Table>
      <TableHeader>
        <TableRow>
          {showService && <TableHead className="pl-4">Key service</TableHead>}
          <TableHead className={showService ? undefined : "pl-4"}>LSN</TableHead>
          <TableHead>Linkage</TableHead>
          <TableHead>Actuator</TableHead>
          <TableHead>International</TableHead>
          <TableHead className="pr-4">Links</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map(({ service, set }) => (
          <TableRow key={`${service.id}-${set.lsn}-${set.hard}`}>
            {showService && (
              <TableCell className="pl-4">
                <a
                  className="underline-offset-4 hover:underline"
                  href={href("services", serviceKey(service))}
                >
                  {sid(service.id)}{" "}
                  <span className="text-muted-foreground">{service.label}</span>
                </a>
              </TableCell>
            )}
            <TableCell className={showService ? "tabular-nums" : "pl-4 tabular-nums"}>
              {hex(set.lsn, 3)}
            </TableCell>
            <TableCell>
              <Badge variant={set.hard ? "secondary" : "outline"}>
                {set.hard ? "hard" : "soft"}
              </Badge>
            </TableCell>
            <TableCell>
              <Flag on={set.active} yes="active" no="inactive" />
            </TableCell>
            <TableCell>
              {set.international === null ? (
                <None>derived</None>
              ) : (
                <Flag on={set.international} />
              )}
            </TableCell>
            <TableCell className="pr-4">
              {set.links.length === 0 ? (
                <None>{set.hard ? "dead link" : "none"}</None>
              ) : (
                <div className="flex flex-col gap-1">
                  {set.links.map((link, i) => (
                    <span key={i} className="flex items-center gap-2 text-xs">
                      <Badge variant="outline" className="w-16">
                        {LINK_KINDS[link.type]}
                      </Badge>
                      <span className="tabular-nums">{linkId(link)}</span>
                      {link.ecc !== null && link.ecc !== ensembleEcc && (
                        <span className="text-muted-foreground">
                          ECC {hex(link.ecc)}
                        </span>
                      )}
                      {link.preference === "low" && (
                        <span className="text-muted-foreground">
                          low preference
                        </span>
                      )}
                    </span>
                  ))}
                </div>
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}

function frequencyRow(fi: FrequencyInformation) {
  switch (fi.type) {
    case "dab":
      return {
        bearer: "DAB",
        id: `EId ${hex(fi.eid, 4)}`,
        otherEnsemble: null,
        frequencies: fi.frequencies.map((f) => (
          <span key={f.mhz} className="flex items-center gap-2">
            <span className="tabular-nums">{f.mhz.toFixed(3)} MHz</span>
            {f.adjacent && <Badge variant="outline">adjacent</Badge>}
            {!f.mode_i && <Badge variant="outline">not mode I</Badge>}
          </span>
        )),
      }
    case "fm":
      return {
        bearer: "FM",
        id: `PI ${hex(fi.pi, 4)}`,
        otherEnsemble: fi.other_ensemble,
        frequencies: fi.frequencies.map((f) => (
          <span key={f} className="tabular-nums">
            {f.toFixed(1)} MHz
          </span>
        )),
      }
    default:
      return {
        bearer: fi.type.toUpperCase(),
        id: `SId ${hex(fi.id, 6)}`,
        otherEnsemble: fi.other_ensemble,
        frequencies: fi.frequencies.map((f) => (
          <span key={f} className="tabular-nums">
            {Math.round(f * 1000)} kHz
          </span>
        )),
      }
  }
}

export function ServiceFollowingPage({ config }: { config: ResolvedConfig }) {
  const linkage = config.services.flatMap((service) =>
    service.linking.map((set) => ({ service, set }))
  )

  return (
    <div className="space-y-6">
      <PageHeader
        title="Service following"
        description="Linkage, frequency information, other ensembles and service changes (TS 103 176)"
      />

      <Section title="Linkage sets" description="FIG 0/6" flush>
        <LinkageSetTable rows={linkage} ensembleEcc={config.ensemble.ecc} />
      </Section>

      <Section title="Frequency information" description="FIG 0/21" flush>
        {config.frequencies.length === 0 ? (
          <EmptyState>No frequency information.</EmptyState>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="pl-4">Bearer</TableHead>
                <TableHead>Identifier</TableHead>
                <TableHead>Continuity</TableHead>
                <TableHead>Other ensemble</TableHead>
                <TableHead className="pr-4">Frequencies</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {config.frequencies.map((fi, i) => {
                const row = frequencyRow(fi)
                return (
                  <TableRow key={i}>
                    <TableCell className="pl-4">
                      <Badge variant="outline">{row.bearer}</Badge>
                    </TableCell>
                    <TableCell className="tabular-nums">{row.id}</TableCell>
                    <TableCell>
                      <Flag on={fi.continuity} />
                    </TableCell>
                    <TableCell>
                      {row.otherEnsemble === null ? (
                        <None>derived</None>
                      ) : (
                        <Flag on={row.otherEnsemble} />
                      )}
                    </TableCell>
                    <TableCell className="pr-4">
                      <div className="flex flex-col gap-1 text-xs">
                        {row.frequencies}
                      </div>
                    </TableCell>
                  </TableRow>
                )
              })}
            </TableBody>
          </Table>
        )}
      </Section>

      <Section
        title="Other ensembles"
        description="FIG 0/24, services carried elsewhere"
        flush
      >
        <OtherEnsembles config={config} />
      </Section>

      <Section title="Service changes" description="FIG 0/20" flush>
        {config.service_changes.length === 0 ? (
          <EmptyState>No announced service changes.</EmptyState>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="pl-4">Service</TableHead>
                <TableHead>SCIdS</TableHead>
                <TableHead>Change</TableHead>
                <TableHead>At</TableHead>
                <TableHead>Transfer to</TableHead>
                <TableHead className="pr-4">Details</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {config.service_changes.map((change, i) => (
                <TableRow key={i}>
                  <TableCell className="pl-4">
                    <ServiceRef config={config} id={change.id} />
                    {change.label && (
                      <span className="ml-2">{change.label}</span>
                    )}
                  </TableCell>
                  <TableCell className="tabular-nums">{change.scids}</TableCell>
                  <TableCell>
                    <Badge variant="secondary">
                      {CHANGE_KINDS[change.change]}
                    </Badge>
                  </TableCell>
                  <TableCell className="tabular-nums">
                    {change.at ? (
                      new Date(change.at).toISOString().replace(".000Z", "Z")
                    ) : (
                      <None>now</None>
                    )}
                  </TableCell>
                  <TableCell className="tabular-nums">
                    {change.transfer_sid !== null ? (
                      <>
                        {sid(change.transfer_sid)}
                        {change.transfer_eid !== null && (
                          <span className="text-muted-foreground">
                            {" "}
                            in {hex(change.transfer_eid, 4)}
                          </span>
                        )}
                      </>
                    ) : (
                      <None />
                    )}
                  </TableCell>
                  <TableCell className="pr-4">
                    <div className="flex flex-wrap gap-1">
                      {change.part_time && (
                        <Badge variant="outline">part time</Badge>
                      )}
                      {change.access_controlled && (
                        <Badge variant="outline">access controlled</Badge>
                      )}
                      {change.ascty !== null && (
                        <Badge variant="outline">ASCTy {change.ascty}</Badge>
                      )}
                      {change.dscty !== null && (
                        <Badge variant="outline">DSCTy {change.dscty}</Badge>
                      )}
                    </div>
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

function OtherEnsembles({ config }: { config: ResolvedConfig }) {
  const rows = [
    ...config.services
      .filter((s) => s.other_ensembles.length > 0)
      .map((s) => ({ id: s.id, ensembles: s.other_ensembles, here: true })),
    ...config.other_services.map((s) => ({ ...s, here: false })),
  ]
  if (rows.length === 0) return <EmptyState>No other ensembles.</EmptyState>

  return (
    <Table>
      <TableHeader>
        <TableRow>
          <TableHead className="pl-4">Service</TableHead>
          <TableHead>Carried here</TableHead>
          <TableHead className="pr-4">Ensembles</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row) => (
          <TableRow key={`${row.here}-${row.id}`}>
            <TableCell className="pl-4">
              <ServiceRef config={config} id={row.id} />
            </TableCell>
            <TableCell>
              <Flag on={row.here} />
            </TableCell>
            <TableCell className="pr-4">
              <div className="flex flex-wrap gap-1">
                {row.ensembles.map((eid) => (
                  <Badge key={eid} variant="outline">
                    {hex(eid, 4)}
                  </Badge>
                ))}
              </div>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  )
}
