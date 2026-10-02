import * as React from "react"
import { WarningCircleIcon } from "@phosphor-icons/react"
import { Alert, AlertDescription } from "@/components/ui/alert"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  Field,
  FieldDescription,
  FieldError,
  FieldGroup,
  FieldLabel,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field"
import { Skeleton } from "@/components/ui/skeleton"
import { Textarea } from "@/components/ui/textarea"
import {
  SelectField,
  SwitchField,
  TextField,
  parseHex,
} from "@/components/form-fields"
import { announce } from "@/components/service-form"
import { hex, localTimeOffset, type Ensemble } from "@/lib/config"
import {
  ApiError,
  getEnsemble,
  patchEnsemble,
  type EnsemblePatch,
} from "@/lib/ui-api"

type Form = {
  label: string
  short_label: string
  id: string
  ecc: string
  international_table: string
  mode: string
  /** `auto`, or the offset in half hours. */
  local_time_offset: string
  reconfiguration_counter: string
  tist: boolean
  tist_offset_ms: string
  tist_at_fct0_ms: string
  tai_utc_offset: string
  /** One URL per line. */
  tai_clock_bulletins: string
}

const MODES = [1, 2, 3, 4].map((mode) => ({
  value: String(mode),
  label: `Mode ${["I", "II", "III", "IV"][mode - 1]}`,
}))

// TS 101 756 table 11; 0 is what configs without the setting send.
const INTERNATIONAL_TABLES = [
  { value: "0", label: "0 · not set" },
  { value: "1", label: "1 · all except North America" },
  { value: "2", label: "2 · North America" },
]

const LOCAL_TIME_OFFSETS = [
  { value: "auto", label: "Automatic" },
  ...Array.from({ length: 49 }, (_, i) => {
    const halfHours = i - 24
    return {
      value: String(halfHours),
      label: localTimeOffset({
        local_time_offset_auto: false,
        local_time_offset_half_hours: halfHours,
      } as Ensemble),
    }
  }),
]

function toForm(e: Ensemble): Form {
  return {
    label: e.label,
    short_label: e.short_label ?? "",
    id: hex(e.id, 4),
    ecc: hex(e.ecc),
    international_table: String(e.international_table),
    mode: String(e.mode),
    local_time_offset: e.local_time_offset_auto
      ? "auto"
      : String(e.local_time_offset_half_hours),
    reconfiguration_counter:
      e.reconfiguration_counter !== null ? String(e.reconfiguration_counter) : "",
    tist: e.tist,
    tist_offset_ms: String(e.tist_offset_ms),
    tist_at_fct0_ms: String(e.tist_at_fct0_ms),
    tai_utc_offset: e.tai_utc_offset !== null ? String(e.tai_utc_offset) : "",
    tai_clock_bulletins: e.tai_clock_bulletins.join("\n"),
  }
}

function integer(value: string): number | undefined {
  const n = Number(value.trim())
  return value.trim() !== "" && Number.isInteger(n) ? n : undefined
}

type Errors = Partial<Record<keyof Form, string>>

/** Client-side checks; the mux validates everything again. */
function check(form: Form): Errors {
  const errors: Errors = {}
  if (!form.label.trim()) errors.label = "Required"
  else if (form.label.length > 16) errors.label = "At most 16 characters"
  if (form.short_label.length > 8) errors.short_label = "At most 8 characters"
  if ((parseHex(form.id) ?? 0x10000) > 0xffff) errors.id = "Four hex digits, such as 4FFF"
  if ((parseHex(form.ecc) ?? 0x100) > 0xff) errors.ecc = "One byte, such as E1"
  const counter = integer(form.reconfiguration_counter)
  if (form.reconfiguration_counter && (counter === undefined || counter < 0 || counter > 1023)) {
    errors.reconfiguration_counter = "0 to 1023, or empty"
  }
  const offset = integer(form.tist_offset_ms)
  if (offset === undefined || Math.abs(offset) > 60_000) {
    errors.tist_offset_ms = "Milliseconds, within ±60000"
  }
  const phase = integer(form.tist_at_fct0_ms)
  if (phase === undefined || phase < 0 || phase > 999) errors.tist_at_fct0_ms = "0 to 999 ms"
  const tai = integer(form.tai_utc_offset)
  if (form.tai_utc_offset && (tai === undefined || tai < 32 || tai > 255)) {
    errors.tai_utc_offset = "Seconds, at least 32, or empty"
  }
  return errors
}

function toEnsemble(form: Form): Ensemble {
  const auto = form.local_time_offset === "auto"
  return {
    label: form.label,
    short_label: form.short_label || null,
    id: parseHex(form.id)!,
    ecc: parseHex(form.ecc)!,
    international_table: Number(form.international_table),
    mode: Number(form.mode),
    local_time_offset_auto: auto,
    local_time_offset_half_hours: auto ? 0 : Number(form.local_time_offset),
    reconfiguration_counter: integer(form.reconfiguration_counter) ?? null,
    tist: form.tist,
    tist_offset_ms: integer(form.tist_offset_ms)!,
    tist_at_fct0_ms: integer(form.tist_at_fct0_ms)!,
    tai_utc_offset: integer(form.tai_utc_offset) ?? null,
    tai_clock_bulletins: form.tai_clock_bulletins
      .split("\n")
      .map((line) => line.trim())
      .filter(Boolean),
  }
}

/** Merge patch with the settings that differ from `loaded`. */
function changes(form: Form, loaded: Ensemble): EnsemblePatch {
  const next = toEnsemble(form)
  const patch: Record<string, unknown> = {}
  for (const key of Object.keys(next) as (keyof Ensemble)[]) {
    if (JSON.stringify(next[key]) !== JSON.stringify(loaded[key])) {
      patch[key] = next[key]
    }
  }
  return patch as EnsemblePatch
}

export function EnsembleFormDialog({
  open,
  onOpenChange,
  onSaved,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved: () => void
}) {
  const [loaded, setLoaded] = React.useState<{
    ensemble: Ensemble
    revision: number
  } | null>(null)
  const [form, setForm] = React.useState<Form | null>(null)
  const [error, setError] = React.useState<string | null>(null)
  const [submitted, setSubmitted] = React.useState(false)
  const [saving, setSaving] = React.useState(false)

  /** Load the current settings; `notice` stays shown above the form. */
  const load = React.useCallback(async (notice: string | null = null) => {
    setError(notice)
    setSubmitted(false)
    setLoaded(null)
    try {
      const result = await getEnsemble()
      setLoaded(result)
      setForm(toForm(result.ensemble))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  React.useEffect(() => {
    if (open) load()
  }, [open, load])

  const set = <K extends keyof Form>(key: K, value: Form[K]) =>
    setForm((f) => (f ? { ...f, [key]: value } : f))

  const errors = submitted && form ? check(form) : {}

  async function submit(event: React.FormEvent) {
    event.preventDefault()
    if (!form || !loaded) return
    setSubmitted(true)
    if (Object.keys(check(form)).length) return
    const patch = changes(form, loaded.ensemble)
    if (Object.keys(patch).length === 0) {
      onOpenChange(false)
      return
    }
    setSaving(true)
    setError(null)
    try {
      const result = await patchEnsemble(patch, loaded.revision)
      announce(result, `${result.ensemble.label} saved`)
      onSaved()
      onOpenChange(false)
    } catch (e) {
      if (e instanceof ApiError && e.stale) {
        await load(
          `${e.message}. Someone else changed the configuration; the form now shows the current values, so your changes need to be made again.`
        )
        return
      }
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  const identityChanged =
    form && loaded
      ? parseHex(form.id) !== loaded.ensemble.id ||
        Number(form.mode) !== loaded.ensemble.mode
      : false

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-lg">
        <form onSubmit={submit} className="grid gap-4" noValidate>
          <DialogHeader>
            <DialogTitle>Edit ensemble</DialogTitle>
            <DialogDescription>
              Changes go on air at the next transmission frame.
            </DialogDescription>
          </DialogHeader>

          {error && (
            <Alert variant="destructive">
              <WarningCircleIcon />
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          {!form || !loaded ? (
            !error && <Skeleton className="h-96" />
          ) : (
            <FieldGroup>
              <FieldSet>
                <FieldLegend variant="label">Identity</FieldLegend>
                <div className="grid grid-cols-2 gap-4">
                  <TextField
                    label="Label"
                    value={form.label}
                    onChange={(v) => set("label", v)}
                    error={errors.label}
                    description="Up to 16 characters"
                    autoFocus
                  />
                  <TextField
                    label="Short label"
                    value={form.short_label}
                    onChange={(v) => set("short_label", v)}
                    error={errors.short_label}
                    description="Up to 8, taken from the label"
                  />
                  <TextField
                    label="Ensemble ID"
                    value={form.id}
                    onChange={(v) => set("id", v.toUpperCase())}
                    error={errors.id}
                    description="Hexadecimal"
                  />
                  <TextField
                    label="ECC"
                    value={form.ecc}
                    onChange={(v) => set("ecc", v.toUpperCase())}
                    error={errors.ecc}
                    description="Extended country code"
                  />
                  <SelectField
                    label="Transmission mode"
                    value={form.mode}
                    onChange={(v) => set("mode", v)}
                    items={MODES}
                  />
                  <SelectField
                    label="International table"
                    value={form.international_table}
                    onChange={(v) => set("international_table", v)}
                    items={INTERNATIONAL_TABLES}
                  />
                </div>
                {identityChanged && (
                  <FieldDescription className="text-destructive">
                    A new ensemble ID or mode makes receivers rescan, and the
                    transmitters must follow a mode change.
                  </FieldDescription>
                )}
              </FieldSet>

              <FieldSet>
                <FieldLegend variant="label">Signalling</FieldLegend>
                <div className="grid grid-cols-2 gap-4">
                  <SelectField
                    label="Local time offset"
                    value={form.local_time_offset}
                    onChange={(v) => set("local_time_offset", v)}
                    items={LOCAL_TIME_OFFSETS}
                  />
                  <TextField
                    label="Reconfiguration counter"
                    value={form.reconfiguration_counter}
                    onChange={(v) => set("reconfiguration_counter", v)}
                    error={errors.reconfiguration_counter}
                    inputMode="numeric"
                    placeholder="automatic"
                    description="0 to 1023, empty for automatic"
                  />
                </div>
              </FieldSet>

              <FieldSet>
                <FieldLegend variant="label">Timestamps</FieldLegend>
                <SwitchField
                  label="TIST"
                  checked={form.tist}
                  onChange={(v) => set("tist", v)}
                  description="Timestamps for synchronised transmitters; needs the TAI offset"
                />
                <div className="grid grid-cols-2 gap-4">
                  <TextField
                    label="TIST offset"
                    value={form.tist_offset_ms}
                    onChange={(v) => set("tist_offset_ms", v)}
                    error={errors.tist_offset_ms}
                    inputMode="numeric"
                    description="ms added to the timestamps"
                  />
                  <TextField
                    label="TIST at FCT 0"
                    value={form.tist_at_fct0_ms}
                    onChange={(v) => set("tist_at_fct0_ms", v)}
                    error={errors.tist_at_fct0_ms}
                    inputMode="numeric"
                    description="ms within the second, 0 to 999"
                  />
                  <TextField
                    label="TAI − UTC"
                    value={form.tai_utc_offset}
                    onChange={(v) => set("tai_utc_offset", v)}
                    error={errors.tai_utc_offset}
                    inputMode="numeric"
                    placeholder="from bulletin"
                    description="Seconds; empty to use the bulletins"
                  />
                </div>
                <BulletinsField
                  value={form.tai_clock_bulletins}
                  onChange={(v) => set("tai_clock_bulletins", v)}
                  error={errors.tai_clock_bulletins}
                />
              </FieldSet>
            </FieldGroup>
          )}

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
            >
              Cancel
            </Button>
            <Button type="submit" disabled={!form || !loaded || saving}>
              {saving ? "Saving…" : "Save"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function BulletinsField({
  value,
  onChange,
  error,
}: {
  value: string
  onChange: (value: string) => void
  error?: string
}) {
  const id = React.useId()
  return (
    <Field data-invalid={error ? true : undefined}>
      <FieldLabel htmlFor={id}>Leap second bulletins</FieldLabel>
      <Textarea
        id={id}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        rows={3}
        className="font-mono"
        placeholder="https://hpiers.obspm.fr/iers/bul/bulc/ntp/leap-seconds.list"
      />
      {error ? (
        <FieldError>{error}</FieldError>
      ) : (
        <FieldDescription>HTTPS URLs, one per line, tried in order</FieldDescription>
      )}
    </Field>
  )
}
