import * as React from "react"
import {
  CaretDownIcon,
  CaretUpIcon,
  CheckCircleIcon,
  FloppyDiskIcon,
  SpinnerIcon,
  WarningCircleIcon,
  WarningIcon,
} from "@phosphor-icons/react"
import { cn } from "cn"
import { toast } from "sonner"
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { useDraft } from "@/hooks/use-draft"
import { formatValue, type ChangeGroup } from "@/lib/diff"
import { ApiError, type Applied, type ServiceConfig } from "@/lib/ui-api"

function announceApplied(applied: Applied) {
  if (applied.scheduled) {
    const at = new Date(applied.scheduled.at).toLocaleTimeString()
    toast.success("Reconfiguration announced", {
      description: `Revision ${applied.revision} goes on air at ${at} (CIF ${applied.scheduled.cif_count}). Further changes are refused until then.`,
    })
  } else {
    toast.success("Changes applied", {
      description: applied.changed
        ? `Revision ${applied.revision} is on air.`
        : "The running multiplex did not change.",
    })
  }
  for (const warning of applied.warnings) toast.warning(warning)
}

function fileName(path: string) {
  return path.split("/").pop() || path
}

function message(e: unknown) {
  return e instanceof Error ? e.message : String(e)
}

/**
 * Pending changes at the bottom of the page: their state, an expandable
 * diff, and the actions to apply, discard or save them.
 */
export function ChangesBar({ onApplied }: { onApplied: () => void }) {
  const draft = useDraft()
  const [expanded, setExpanded] = React.useState(false)
  const [confirmDiscard, setConfirmDiscard] = React.useState(false)
  const [busy, setBusy] = React.useState(false)
  const [applyError, setApplyError] = React.useState<string | null>(null)

  React.useEffect(() => {
    if (draft.count === 0) {
      setExpanded(false)
      setApplyError(null)
    }
  }, [draft.count])

  async function save(force = false) {
    setBusy(true)
    try {
      const saved = await draft.save(force)
      toast.success(`Saved to ${fileName(saved.path)}`, {
        description: `The previous file is kept as ${fileName(saved.backup)}.`,
      })
    } catch (e) {
      const conflict = e instanceof ApiError && e.status === 409
      toast.error("Not saved", {
        description: message(e),
        action: conflict ? { label: "Overwrite", onClick: () => save(true) } : undefined,
      })
    } finally {
      setBusy(false)
    }
  }

  async function apply({ force = false, save = false } = {}) {
    setBusy(true)
    setApplyError(null)
    try {
      const { applied, saved, saveError } = await draft.apply({ force, save })
      announceApplied(applied)
      if (saved) toast.success(`Saved to ${fileName(saved.path)}`)
      if (saveError) {
        toast.error("Applied, but not saved", { description: saveError.message })
      }
      onApplied()
    } catch (e) {
      setApplyError(message(e))
      setExpanded(true)
    } finally {
      setBusy(false)
    }
  }

  if (draft.count === 0) {
    if (!draft.file?.unsaved) return null
    return (
      <Bar>
        <div className="flex min-w-0 flex-1 items-center gap-2" title={draft.file.path}>
          <FloppyDiskIcon className="size-4 shrink-0 text-muted-foreground" />
          <span className="truncate">
            The running configuration is not saved to{" "}
            <span className="font-medium">{fileName(draft.file.path)}</span>
          </span>
        </div>
        <Button size="sm" disabled={busy} onClick={() => save()}>
          Save to file
        </Button>
      </Bar>
    )
  }

  const blocked = draft.validating || draft.invalid !== null || busy

  return (
    <>
      <Bar
        detail={
          expanded && (
            <ChangeDetails
              groups={draft.changes}
              invalid={applyError ?? draft.invalid}
              warnings={draft.preview?.warnings ?? []}
            />
          )
        }
      >
        <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-3 gap-y-1">
          <span className="font-medium">
            {draft.count} unapplied change{draft.count === 1 ? "" : "s"}
          </span>
          <Status
            validating={draft.validating}
            invalid={applyError ?? draft.invalid}
            warnings={draft.preview?.warnings.length ?? 0}
          />
          {draft.stale && (
            <span className="text-destructive">
              The mux configuration changed since you started editing.
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            aria-expanded={expanded}
            onClick={() => setExpanded((v) => !v)}
          >
            {expanded ? (
              <CaretDownIcon data-icon="inline-start" />
            ) : (
              <CaretUpIcon data-icon="inline-start" />
            )}
            {expanded ? "Hide changes" : "Show changes"}
          </Button>
          <Button
            variant="outline"
            size="sm"
            disabled={busy}
            onClick={() => setConfirmDiscard(true)}
          >
            Discard
          </Button>
          {draft.stale ? (
            <Button
              variant="destructive"
              size="sm"
              disabled={blocked}
              onClick={() => apply({ force: true })}
            >
              Apply anyway
            </Button>
          ) : (
            <>
              {draft.file && (
                <Button
                  variant="outline"
                  size="sm"
                  disabled={blocked}
                  onClick={() => apply({ save: true })}
                >
                  Apply & save
                </Button>
              )}
              <Button size="sm" disabled={blocked} onClick={() => apply()}>
                Apply changes
              </Button>
            </>
          )}
        </div>
      </Bar>

      <AlertDialog open={confirmDiscard} onOpenChange={setConfirmDiscard}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Discard {draft.count} changes?</AlertDialogTitle>
            <AlertDialogDescription>
              The pages show the running configuration again. This cannot be
              undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Keep editing</AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={() => {
                draft.discard()
                setConfirmDiscard(false)
              }}
            >
              Discard
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

function Bar({
  children,
  detail,
}: {
  children: React.ReactNode
  detail?: React.ReactNode
}) {
  return (
    <div className="sticky bottom-0 z-10 w-full min-w-0 border-t bg-background/95 text-xs backdrop-blur">
      {detail && <div className="max-h-[50vh] overflow-y-auto border-b">{detail}</div>}
      <div className="mx-auto flex w-full max-w-7xl flex-wrap items-center justify-between gap-3 px-4 py-3 md:px-6">
        {children}
      </div>
    </div>
  )
}

function Status({
  validating,
  invalid,
  warnings,
}: {
  validating: boolean
  invalid: string | null
  warnings: number
}) {
  if (validating) {
    return (
      <span className="inline-flex items-center gap-1.5 text-muted-foreground">
        <SpinnerIcon className="size-3.5 animate-spin" /> Validating…
      </span>
    )
  }
  if (invalid) {
    return (
      <span className="inline-flex min-w-0 items-center gap-1.5 text-destructive">
        <WarningCircleIcon className="size-3.5 shrink-0" />
        <span className="truncate">Not valid: {invalid}</span>
      </span>
    )
  }
  return (
    <span className="inline-flex items-center gap-1.5 text-muted-foreground">
      <CheckCircleIcon className="size-3.5 text-emerald-500" /> Valid
      {warnings > 0 && (
        <span className="inline-flex items-center gap-1 text-amber-600 dark:text-amber-400">
          <WarningIcon className="size-3.5" />
          {warnings} warning{warnings === 1 ? "" : "s"}
        </span>
      )}
    </span>
  )
}

const KIND_LABELS: Record<ChangeGroup["kind"], string> = {
  added: "added",
  removed: "removed",
  changed: "changed",
}

function ChangeDetails({
  groups,
  invalid,
  warnings,
}: {
  groups: ChangeGroup[]
  invalid: string | null
  warnings: string[]
}) {
  return (
    <div className="mx-auto w-full max-w-7xl space-y-4 px-4 py-4 md:px-6">
      {invalid && (
        <p className="flex items-start gap-2 text-destructive">
          <WarningCircleIcon className="mt-0.5 size-3.5 shrink-0" />
          {invalid}
        </p>
      )}
      {warnings.map((warning) => (
        <p
          key={warning}
          className="flex items-start gap-2 text-amber-600 dark:text-amber-400"
        >
          <WarningIcon className="mt-0.5 size-3.5 shrink-0" />
          {warning}
        </p>
      ))}
      {groups.map((group) => (
        <section key={group.key}>
          <h3 className="mb-1.5 flex items-center gap-2 font-medium">
            {group.title}
            <Badge
              variant={group.kind === "changed" ? "outline" : "secondary"}
              className={cn(
                group.kind === "added" && "text-emerald-600 dark:text-emerald-400",
                group.kind === "removed" && "text-destructive"
              )}
            >
              {KIND_LABELS[group.kind]}
            </Badge>
          </h3>
          {group.kind === "changed" ? (
            <table className="w-full border-collapse">
              <tbody>
                {group.changes.map((change) => (
                  <tr key={change.path} className="border-t align-top">
                    <td className="w-1/4 py-1 pr-4 text-muted-foreground">
                      {change.path || "—"}
                    </td>
                    <td className="w-[37.5%] py-1 pr-4 break-all text-destructive/80 line-through decoration-destructive/40">
                      {formatValue(change.path, change.before)}
                    </td>
                    <td className="py-1 break-all text-emerald-600 dark:text-emerald-400">
                      {formatValue(change.path, change.after)}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          ) : (
            <ServiceSummary
              service={(group.changes[0]?.after ?? group.changes[0]?.before) as ServiceConfig}
            />
          )}
        </section>
      ))}
    </div>
  )
}

function ServiceSummary({ service }: { service: ServiceConfig }) {
  return (
    <ul className="space-y-0.5 text-muted-foreground">
      {service.components.map((c, i) => (
        <li key={i}>
          component {i}:{" "}
          {c.subchannel
            ? `shared subchannel ${c.subchannel}`
            : [
                c.type,
                c.bitrate !== undefined && `${c.bitrate} kbit/s`,
                c.subchannel_id !== undefined && `SubChId ${c.subchannel_id}`,
                c.input && ("uri" in c.input ? c.input.uri : c.input.path),
              ]
                .filter(Boolean)
                .join(" · ")}
        </li>
      ))}
    </ul>
  )
}
