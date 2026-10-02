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
  PageHeader,
  Section,
} from "@/components/fields"
import type { ResolvedConfig } from "@/lib/config"

export function OutputPage({ config }: { config: ResolvedConfig }) {
  const { output, ensemble } = config

  return (
    <div className="space-y-6">
      <PageHeader title="Output" description="EDI towards the modulators" />

      <Section title="EDI">
        <Fields>
          <Field label="Destinations">{output.destinations.length}</Field>
          <Field label="Tag packet alignment">
            {output.tagpacket_alignment} bytes
          </Field>
          <Field label="TIST">
            <Flag on={ensemble.tist} yes="enabled" no="disabled" />
          </Field>
          <Field label="TIST offset">{ensemble.tist_offset_ms} ms</Field>
        </Fields>
      </Section>

      <Section title="Destinations" flush>
        {output.destinations.length === 0 ? (
          <EmptyState>No destinations.</EmptyState>
        ) : (
          <Table>
            <TableHeader>
              <TableRow>
                <TableHead className="pl-4">Protocol</TableHead>
                <TableHead>Endpoint</TableHead>
                <TableHead className="text-right">Queue</TableHead>
                <TableHead className="pr-4 text-right">Preroll</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {output.destinations.map((d, i) => (
                <TableRow key={i}>
                  <TableCell className="pl-4">
                    <Badge variant="outline" className="uppercase">
                      {d.protocol}
                    </Badge>
                  </TableCell>
                  {d.protocol === "tcp" ? (
                    <>
                      <TableCell className="tabular-nums">
                        listens on port {d.listen_port}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {d.max_frames_queued} frames
                      </TableCell>
                      <TableCell className="pr-4 text-right tabular-nums">
                        {d.preroll_ms} ms
                      </TableCell>
                    </>
                  ) : (
                    <>
                      <TableCell className="tabular-nums">
                        sends to {d.address}:{d.port}
                      </TableCell>
                      <TableCell className="text-right text-muted-foreground">
                        —
                      </TableCell>
                      <TableCell className="pr-4 text-right text-muted-foreground">
                        —
                      </TableCell>
                    </>
                  )}
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </Section>
    </div>
  )
}
