// Client for /api/ui (dabmux/src/api/ui.rs): the operator configuration and
// edits of the ensemble and of single services. Edits send the revision they
// are based on, so the mux refuses them if the configuration changed meanwhile.

import type {
  Ensemble,
  SubchannelType,
  UserApplication,
} from "@/lib/config"

/** A service as written in the configuration file, defaults unresolved. */
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

/** JSON Merge Patch of a service: `null` removes a field. */
export type ServicePatch = {
  [K in keyof ServiceConfig]?: ServiceConfig[K] | null
}

export type Applied = { changed: boolean; revision: number; warnings: string[] }

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
  if (body !== undefined) {
    headers["content-type"] =
      method === "PATCH" ? "application/merge-patch+json" : "application/json"
  }
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

const path = (sid: string) => `/services/${encodeURIComponent(sid)}`

export function getService(sid: string) {
  return request<{ service: ServiceConfig; revision: number }>("GET", path(sid))
}

export function createService(service: ServiceConfig, revision?: number) {
  return request<Applied & { service: ServiceConfig }>("POST", "/services", {
    body: service,
    revision,
  })
}

export function patchService(sid: string, patch: ServicePatch, revision?: number) {
  return request<Applied & { service: ServiceConfig }>("PATCH", path(sid), {
    body: patch,
    revision,
  })
}

export function deleteService(sid: string, revision?: number) {
  return request<Applied & { removed_subchannels: string[] }>(
    "DELETE",
    path(sid),
    { revision }
  )
}

/** JSON Merge Patch of the ensemble settings. */
export type EnsemblePatch = { [K in keyof Ensemble]?: Ensemble[K] | null }

export function getEnsemble() {
  return request<{ ensemble: Ensemble; revision: number }>("GET", "/ensemble")
}

export function patchEnsemble(patch: EnsemblePatch, revision?: number) {
  return request<Applied & { ensemble: Ensemble }>("PATCH", "/ensemble", {
    body: patch,
    revision,
  })
}
