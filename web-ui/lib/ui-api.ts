// Client for /api/ui (dabmux/src/api/ui.rs). The UI edits a copy of the
// operator configuration in the browser, has the mux validate it, and
// applies it whole, together with the revision it is based on, so the mux
// refuses it if its configuration changed meanwhile. Saving writes the
// running configuration to the configuration file.

import type {
  Ensemble,
  ResolvedConfig,
  SubchannelType,
  UserApplication,
} from "@/lib/config"

/** The configuration as written in the configuration file, defaults unresolved. */
export type OperatorConfig = {
  ensemble: Ensemble
  defaults?: Record<string, unknown>
  subchannels?: Record<string, Record<string, unknown>>
  services: ServiceConfig[]
  other_services?: unknown[]
  frequencies?: unknown[]
  service_changes?: unknown[]
  output: Record<string, unknown>
}

export type ServiceConfig = {
  id: number
  ecc?: number
  label: string
  short_label?: string
  pty?: number
  language?: number
  components: ComponentConfig[]
  linking?: unknown[]
  other_ensembles?: number[]
}

export type ComponentConfig = {
  subchannel?: string
  subchannel_id?: number
  type?: SubchannelType
  bitrate?: number
  protection?: { profile: "eep_a" | "eep_b"; level: number }
  input?:
    | { protocol: "edi"; uri: string; [setting: string]: unknown }
    | { protocol: "sti"; uri: string }
    | { protocol: "file"; path: string }
  user_applications?: UserApplication[]
  packet_address?: number
  dscty?: number
  data_groups?: boolean
}

/** An announced multiplex reconfiguration: the mux switches at `at` (system time). */
export type ScheduledSwitch = { at: string; cif_count: number }

export type Applied = {
  changed: boolean
  revision: number
  warnings: string[]
  scheduled: ScheduledSwitch | null
}

export type Preview = { resolved: ResolvedConfig; warnings: string[] }

export class ApiError extends Error {
  constructor(
    readonly status: number,
    message: string
  ) {
    super(message)
  }

  /** The configuration changed since it was loaded (HTTP 412). */
  get stale() {
    return this.status === 412
  }
}

async function request<T>(
  method: string,
  path: string,
  { body, revision }: { body?: unknown; revision?: number } = {}
): Promise<T> {
  const headers: Record<string, string> = {}
  if (body !== undefined) headers["content-type"] = "application/json"
  if (revision !== undefined) headers["if-match"] = `"${revision}"`
  const response = await fetch(`/api/ui${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  })
  const text = await response.text()
  let parsed: unknown = null
  try {
    parsed = text ? JSON.parse(text) : null
  } catch {
    // not JSON, e.g. a proxy error page
  }
  if (!response.ok) {
    const message =
      (parsed as { error?: string } | null)?.error ??
      `HTTP ${response.status} ${response.statusText}`
    throw new ApiError(response.status, message)
  }
  return parsed as T
}

/** Where the configuration file is, and whether it differs from the running one. */
export type FileStatus = { path: string; unsaved: boolean }

export type Saved = { path: string; backup: string; revision: number }

export function getConfig() {
  return request<{
    config: OperatorConfig
    revision: number
    file: FileStatus | null
    scheduled: ScheduledSwitch | null
  }>("GET", "/config")
}

/** Validate without applying. */
export function previewConfig(config: OperatorConfig) {
  return request<Preview>("POST", "/config/preview", { body: config })
}

/** Apply `config`; without `revision`, regardless of changes made meanwhile. */
export function applyConfig(config: OperatorConfig, revision?: number) {
  return request<Applied>("PUT", "/config", { body: config, revision })
}

/**
 * Write the running configuration to the configuration file. Without
 * `force`, the mux refuses when the file was changed on disk meanwhile.
 */
export function saveConfig(revision: number, force = false) {
  return request<Saved>("POST", `/config/save${force ? "?force=true" : ""}`, {
    revision,
  })
}
