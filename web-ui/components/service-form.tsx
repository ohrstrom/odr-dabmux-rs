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
  FieldDescription,
  FieldGroup,
  FieldLegend,
  FieldSet,
} from "@/components/ui/field"
import { SelectField, TextField, parseHex } from "@/components/form-fields"
import { Skeleton } from "@/components/ui/skeleton"
import {
  LANGUAGES,
  PTY_TABLE_1,
  SUBCHANNEL_TYPES,
  USER_APPLICATIONS as USER_APPLICATION_NAMES,
  hex,
  protectionLabel,
  sid,
  type ResolvedConfig,
  type Subchannel,
  type SubchannelType,
} from "@/lib/config"
import { useDraft } from "@/hooks/use-draft"
import type { ComponentConfig, ServiceConfig } from "@/lib/ui-api"

type InputSpec = NonNullable<ComponentConfig["input"]>

/** A component that defines its own subchannel, as edited in the form. */
type ComponentForm = {
  /** Set for a component on a shared subchannel, which is not edited here. */
  shared: string | null
  type: SubchannelType
  bitrate: string
  /** `eep_a:3`, or empty for the configured default. */
  protection: string
  /** Empty lets the mux allocate the SubChId. */
  subchannel_id: string
  user_application: string
  /** False when the form cannot represent the user applications, e.g. SPI. */
  user_application_editable: boolean
  /** The configured user applications, for display. */
  user_applications_label: string
  protocol: InputSpec["protocol"]
  /** Input URI, or the path of a packet file. */
  location: string
}

type Form = {
  id: string
  label: string
  short_label: string
  ecc: string
  pty: string
  language: string
  components: ComponentForm[]
}

const NEW_COMPONENT: ComponentForm = {
  shared: null,
  type: "dab_plus",
  bitrate: "72",
  protection: "",
  subchannel_id: "",
  user_application: "slideshow",
  user_application_editable: true,
  user_applications_label: "",
  protocol: "edi",
  location: "",
}

const EMPTY: Form = {
  id: "",
  label: "",
  short_label: "",
  ecc: "",
  pty: "0",
  language: "0",
  components: [NEW_COMPONENT],
}

/** SubChIds are 6 bits. */
const MAX_SUBCHANNEL_ID = 63

const PROTECTIONS = [
  { value: "", label: "Default" },
  ...(["a", "b"] as const).flatMap((profile) =>
    [1, 2, 3, 4].map((level) => ({
      value: `eep_${profile}:${level}`,
      label: `EEP ${level}-${profile.toUpperCase()}`,
    }))
  ),
]

/** Audio types are interchangeable; packet mode needs settings the form lacks. */
const AUDIO_TYPES = (["dab_plus", "mpeg_audio"] as const).map((value) => ({
  value,
  label: SUBCHANNEL_TYPES[value],
}))

const USER_APPLICATIONS = [
  { value: "", label: "None" },
  { value: "slideshow", label: "Slideshow" },
]

const INPUT_LABELS: Record<InputSpec["protocol"], string> = {
  edi: "EDI input",
  sti: "STI input",
  file: "Packet file",
}

const LANGUAGE_ITEMS = Object.entries(LANGUAGES)
  .map(([code, name]) => ({ value: code, label: `${hex(Number(code))} · ${name}` }))
  .sort((a, b) => Number(a.value) - Number(b.value))

function ptyItems(internationalTable: number) {
  return Array.from({ length: 32 }, (_, pty) => ({
    value: String(pty),
    label:
      internationalTable === 1 && PTY_TABLE_1[pty]
        ? `${pty} · ${PTY_TABLE_1[pty]}`
        : String(pty),
  }))
}

function toComponentForm(component: ComponentConfig): ComponentForm {
  const apps = component.user_applications ?? []
  return {
    shared: component.subchannel ?? null,
    type: component.type ?? "dab_plus",
    bitrate: component.bitrate !== undefined ? String(component.bitrate) : "",
    protection: component.protection
      ? `${component.protection.profile}:${component.protection.level}`
      : "",
    subchannel_id:
      component.subchannel_id !== undefined ? String(component.subchannel_id) : "",
    user_application: apps[0] === "slideshow" ? "slideshow" : "",
    user_application_editable:
      apps.length === 0 || (apps.length === 1 && apps[0] === "slideshow"),
    user_applications_label: apps.map((app) => USER_APPLICATION_NAMES[app]).join(", "),
    protocol: component.input?.protocol ?? "edi",
    location: component.input ? inputLocation(component.input) : "",
  }
}

function toForm(service: ServiceConfig): Form {
  return {
    id: sid(service.id),
    label: service.label,
    short_label: service.short_label ?? "",
    ecc: service.ecc !== undefined ? hex(service.ecc) : "",
    pty: String(service.pty ?? 0),
    language: String(service.language ?? 0),
    components: service.components.map(toComponentForm),
  }
}

function inputLocation(input: InputSpec): string {
  return input.protocol === "file" ? input.path : input.uri
}

function withLocation(input: InputSpec, location: string): InputSpec {
  return input.protocol === "file"
    ? { ...input, path: location }
    : { ...input, uri: location }
}

function parseSubchannelId(value: string): number | undefined {
  return value.trim() === "" ? undefined : Number(value)
}

type Errors = Partial<Record<string, string>>

/** Client-side checks; the mux validates everything again. */
function check(form: Form): Errors {
  const errors: Errors = {}
  const id = parseHex(form.id)
  if (id === undefined || id > 0xffffffff) errors.id = "Hexadecimal, such as 4DA4"
  if (!form.label.trim()) errors.label = "Required"
  else if (form.label.length > 16) errors.label = "At most 16 characters"
  if (form.short_label.length > 8) errors.short_label = "At most 8 characters"
  if (form.ecc && (parseHex(form.ecc) ?? 256) > 0xff) errors.ecc = "One byte, such as E1"
  form.components.forEach((c, i) => {
    if (c.shared) return
    const bitrate = Number(c.bitrate)
    if (!Number.isInteger(bitrate) || bitrate <= 0) errors[`${i}.bitrate`] = "kbit/s"
    const subchannelId = parseSubchannelId(c.subchannel_id)
    if (
      subchannelId !== undefined &&
      (!Number.isInteger(subchannelId) || subchannelId < 0 || subchannelId > MAX_SUBCHANNEL_ID)
    ) {
      errors[`${i}.subchannel_id`] = `0 to ${MAX_SUBCHANNEL_ID}, or empty for automatic`
    }
    if (!c.location.trim()) errors[`${i}.location`] = "Required"
  })
  return errors
}

/** Service-level fields from the form, as the configuration stores them. */
function serviceFields(form: Form) {
  return {
    id: parseHex(form.id)!,
    label: form.label,
    short_label: form.short_label || undefined,
    ecc: form.ecc ? parseHex(form.ecc) : undefined,
    pty: Number(form.pty),
    language: Number(form.language),
  }
}

/**
 * The component as configured: `base` with the form's settings, so that
 * settings the form does not show, such as the packet address or input
 * buffering, are kept.
 */
function toComponent(form: ComponentForm, base: ComponentConfig = {}): ComponentConfig {
  if (form.shared) return base
  const [profile, level] = form.protection.split(":")
  const location = form.location.trim()
  return {
    ...base,
    subchannel_id: parseSubchannelId(form.subchannel_id),
    type: form.type,
    bitrate: Number(form.bitrate),
    protection: profile
      ? { profile: profile as "eep_a" | "eep_b", level: Number(level) }
      : undefined,
    input: base.input
      ? withLocation(base.input, location)
      : { protocol: "edi", uri: location },
    user_applications: form.user_application_editable
      ? form.user_application
        ? ["slideshow"]
        : undefined
      : base.user_applications,
  }
}

function newService(form: Form): ServiceConfig {
  return {
    ...serviceFields(form),
    components: form.components.map((c) => toComponent(c)),
  }
}

/** `loaded` with the form's settings. */
function edited(form: Form, loaded: ServiceConfig): ServiceConfig {
  return {
    ...loaded,
    ...serviceFields(form),
    components: loaded.components.map((component, i) =>
      toComponent(form.components[i]!, component)
    ),
  }
}

/** The running subchannel of component `index` of service `id`. */
function resolvedSubchannel(config: ResolvedConfig, id: number, index: number) {
  const name = config.services.find((s) => s.id === id)?.components[index]?.subchannel
  return config.subchannels.find((s) => s.name === name)
}

/** Create a service (`sid` unset) or edit one. */
export function ServiceFormDialog({
  config,
  sid: editing,
  open,
  onOpenChange,
  onSaved,
}: {
  config: ResolvedConfig
  sid?: string
  open: boolean
  onOpenChange: (open: boolean) => void
  onSaved: (service: ServiceConfig) => void
}) {
  const creating = editing === undefined
  const draft = useDraft()
  const [form, setForm] = React.useState<Form>(EMPTY)
  const [loaded, setLoaded] = React.useState<ServiceConfig | null>(null)
  const [error, setError] = React.useState<string | null>(null)
  const [submitted, setSubmitted] = React.useState(false)

  // Fill the form from the draft when the dialog opens.
  React.useEffect(() => {
    if (!open) return
    setError(null)
    setSubmitted(false)
    if (creating) {
      setLoaded(null)
      setForm(EMPTY)
      return
    }
    const service = draft.current?.services.find((s) => sid(s.id) === editing)
    setLoaded(service ?? null)
    if (service) setForm(toForm(service))
    else setError(`No service ${editing}`)
    // Only on opening: later draft changes must not reset the form.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open, creating, editing])

  const set = <K extends keyof Form>(key: K, value: Form[K]) =>
    setForm((f) => ({ ...f, [key]: value }))
  const setComponent = (index: number, change: Partial<ComponentForm>) =>
    setForm((f) => ({
      ...f,
      components: f.components.map((c, i) => (i === index ? { ...c, ...change } : c)),
    }))

  const errors = submitted ? check(form) : {}
  const data = (parseHex(form.id) ?? 0) > 0xffff

  function submit(event: React.FormEvent) {
    event.preventDefault()
    setSubmitted(true)
    const found = check(form)
    const id = parseHex(form.id)
    const taken = draft.current?.services.some(
      (s) => s.id === id && (creating || s.id !== loaded?.id)
    )
    if (taken) {
      setError(`Service ${form.id} exists already`)
      return
    }
    if (Object.keys(found).length) return
    const service = creating ? newService(form) : edited(form, loaded!)
    draft.update((config) => {
      if (creating) {
        config.services.push(service)
      } else {
        const index = config.services.findIndex((s) => s.id === loaded!.id)
        config.services[index] = service
      }
    })
    onSaved(service)
    onOpenChange(false)
  }

  const ready = creating || loaded !== null

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-h-[calc(100svh-2rem)] overflow-y-auto sm:max-w-lg">
        <form onSubmit={submit} className="grid gap-4" noValidate>
          <DialogHeader>
            <DialogTitle>
              {creating ? "Add service" : `Edit ${loaded?.label ?? "service"}`}
            </DialogTitle>
            <DialogDescription>
              {creating
                ? "The service is appended, so existing SubChIds stay as they are. It goes on air when the changes are applied."
                : "Changes collect until they are applied. A new bitrate or protection moves the subchannels after it in the MSC."}
            </DialogDescription>
          </DialogHeader>

          {error && (
            <Alert variant="destructive">
              <WarningCircleIcon />
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}

          {!ready ? (
            !error && <Skeleton className="h-64" />
          ) : (
            <FieldGroup>
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
                  label="Service ID"
                  value={form.id}
                  onChange={(v) => set("id", v.toUpperCase())}
                  error={errors.id}
                  placeholder="4DA4"
                  description="Hexadecimal"
                />
                {!data && (
                  <TextField
                    label="ECC"
                    value={form.ecc}
                    onChange={(v) => set("ecc", v.toUpperCase())}
                    error={errors.ecc}
                    placeholder={hex(config.ensemble.ecc)}
                    description="Only if not the ensemble's"
                  />
                )}
                {!data && (
                  <SelectField
                    label="Programme type"
                    value={form.pty}
                    onChange={(v) => set("pty", v)}
                    items={ptyItems(config.ensemble.international_table)}
                  />
                )}
                {!data && (
                  <SelectField
                    label="Language"
                    value={form.language}
                    onChange={(v) => set("language", v)}
                    items={LANGUAGE_ITEMS}
                  />
                )}
              </div>

              {form.components.map((component, i) => {
                const resolved = loaded
                  ? resolvedSubchannel(config, loaded.id, i)
                  : undefined
                return (
                  <ComponentFields
                    key={i}
                    title={
                      creating
                        ? "Audio component"
                        : `Component ${i} · ${resolved?.name ?? component.shared ?? ""}`
                    }
                    value={component}
                    onChange={(change) => setComponent(i, change)}
                    errors={errors}
                    index={i}
                    resolved={resolved}
                  />
                )
              })}
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
            <Button type="submit" disabled={!ready}>
              {creating ? "Add service" : "Done"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}

function ComponentFields({
  title,
  value: c,
  onChange,
  errors,
  index,
  resolved,
}: {
  title: string
  value: ComponentForm
  onChange: (change: Partial<ComponentForm>) => void
  errors: Errors
  index: number
  /** The running subchannel, when editing. */
  resolved?: Subchannel
}) {
  const error = (field: keyof ComponentForm) => errors[`${index}.${field}`]

  if (c.shared) {
    return (
      <FieldSet>
        <FieldLegend variant="label">{title}</FieldLegend>
        <FieldDescription>
          Uses the shared subchannel {c.shared}
          {resolved &&
            ` (SubChId ${resolved.id}, ${SUBCHANNEL_TYPES[resolved.type]}, ${resolved.bitrate} kbit/s, ${protectionLabel(resolved.protection)})`}
          . Its settings are made under subchannels.
        </FieldDescription>
      </FieldSet>
    )
  }

  const audio = c.type === "dab_plus" || c.type === "mpeg_audio"

  return (
    <FieldSet>
      <FieldLegend variant="label">{title}</FieldLegend>
      <div className="grid grid-cols-2 gap-4">
        {audio ? (
          <SelectField
            label="Type"
            value={c.type}
            onChange={(v) => onChange({ type: v as SubchannelType })}
            items={AUDIO_TYPES}
          />
        ) : (
          <TextField
            label="Type"
            value={SUBCHANNEL_TYPES[c.type]}
            onChange={() => {}}
            disabled
            description="Packet mode; not changeable here"
          />
        )}
        <TextField
          label="Bitrate"
          value={c.bitrate}
          onChange={(v) => onChange({ bitrate: v })}
          error={error("bitrate")}
          inputMode="numeric"
          description="kbit/s"
        />
        <SelectField
          label="Protection"
          value={c.protection}
          onChange={(v) => onChange({ protection: v })}
          items={PROTECTIONS}
        />
        {c.user_application_editable ? (
          <SelectField
            label="User application"
            value={c.user_application}
            onChange={(v) => onChange({ user_application: v })}
            items={USER_APPLICATIONS}
          />
        ) : (
          <TextField
            label="User application"
            value={c.user_applications_label}
            onChange={() => {}}
            disabled
            description="Not changeable here"
          />
        )}
        <TextField
          label="SubChId"
          value={c.subchannel_id}
          onChange={(v) => onChange({ subchannel_id: v })}
          error={error("subchannel_id")}
          inputMode="numeric"
          placeholder={resolved ? `automatic, now ${resolved.id}` : "automatic"}
          description={`0 to ${MAX_SUBCHANNEL_ID}, empty for automatic`}
        />
        <div className={c.protocol === "file" ? "col-span-2" : undefined}>
          <TextField
            label={INPUT_LABELS[c.protocol]}
            value={c.location}
            onChange={(v) => onChange({ location: v })}
            error={error("location")}
            placeholder={c.protocol === "file" ? undefined : "tcp://:9004"}
            description={
              c.protocol === "file" ? "Path on the mux host" : "Where the encoder sends to"
            }
          />
        </div>
      </div>
    </FieldSet>
  )
}
