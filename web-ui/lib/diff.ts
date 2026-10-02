// Differences between two operator configurations, grouped for display.

import { hex, sid } from "@/lib/config"
import type { OperatorConfig, ServiceConfig } from "@/lib/ui-api"

export type Change = {
  /** Field path within the group, e.g. `components[0].bitrate`. */
  path: string
  before?: unknown
  after?: unknown
}

export type ChangeGroup = {
  key: string
  title: string
  kind: "added" | "removed" | "changed"
  changes: Change[]
}

const SECTION_TITLES: Record<string, string> = {
  ensemble: "Ensemble",
  defaults: "Defaults",
  subchannels: "Shared subchannels",
  other_services: "Other services",
  frequencies: "Frequency information",
  service_changes: "Service changes",
  output: "Output",
}

function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value)
}

/** Leaf differences between `a` and `b`; arrays of another length change whole. */
export function diffValues(a: unknown, b: unknown, path = ""): Change[] {
  if (isObject(a) && isObject(b)) {
    const keys = [...new Set([...Object.keys(a), ...Object.keys(b)])]
    return keys.flatMap((key) =>
      diffValues(a[key], b[key], path ? `${path}.${key}` : key)
    )
  }
  if (Array.isArray(a) && Array.isArray(b) && a.length === b.length) {
    return a.flatMap((item, i) => diffValues(item, b[i], `${path}[${i}]`))
  }
  if (JSON.stringify(a) === JSON.stringify(b)) return []
  return [{ path, before: a, after: b }]
}

function serviceTitle(service: ServiceConfig) {
  return `Service ${sid(service.id)} · ${service.label}`
}

export function diffConfigs(base: OperatorConfig, draft: OperatorConfig): ChangeGroup[] {
  const groups: ChangeGroup[] = []
  const section = (key: string) => {
    const changes = diffValues(
      (base as Record<string, unknown>)[key],
      (draft as Record<string, unknown>)[key]
    )
    if (changes.length) {
      groups.push({ key, title: SECTION_TITLES[key] ?? key, kind: "changed", changes })
    }
  }

  section("ensemble")

  const before = new Map(base.services.map((s) => [s.id, s]))
  const after = new Map(draft.services.map((s) => [s.id, s]))
  for (const service of draft.services) {
    const old = before.get(service.id)
    if (!old) {
      groups.push({
        key: `service-${service.id}`,
        title: serviceTitle(service),
        kind: "added",
        changes: [{ path: "", after: service }],
      })
      continue
    }
    const changes = diffValues(old, service)
    if (changes.length) {
      groups.push({
        key: `service-${service.id}`,
        title: serviceTitle(service),
        kind: "changed",
        changes,
      })
    }
  }
  for (const service of base.services) {
    if (!after.has(service.id)) {
      groups.push({
        key: `service-${service.id}`,
        title: serviceTitle(service),
        kind: "removed",
        changes: [{ path: "", before: service }],
      })
    }
  }
  // Order matters: it sets the CU layout and the allocated SubChIds.
  const kept = (services: ServiceConfig[], other: Map<number, ServiceConfig>) =>
    services.filter((s) => other.has(s.id)).map((s) => sid(s.id))
  const orderBefore = kept(base.services, after)
  const orderAfter = kept(draft.services, before)
  if (orderBefore.join() !== orderAfter.join()) {
    groups.push({
      key: "service-order",
      title: "Service order",
      kind: "changed",
      changes: [{ path: "services", before: orderBefore, after: orderAfter }],
    })
  }

  for (const key of Object.keys(SECTION_TITLES)) {
    if (key !== "ensemble") section(key)
  }
  return groups
}

export function countChanges(groups: ChangeGroup[]): number {
  return groups.reduce((sum, group) => sum + group.changes.length, 0)
}

const HEX_FIELDS = /(^|\.)(id|ecc|eid|pi|transfer_sid|transfer_eid)$/

/** A changed value for display; identifiers in hex, as elsewhere in the UI. */
export function formatValue(path: string, value: unknown): string {
  if (value === undefined || value === null) return "—"
  if (typeof value === "number" && HEX_FIELDS.test(path)) {
    return path.endsWith("ecc") ? hex(value) : sid(value)
  }
  if (typeof value === "string") return value
  return JSON.stringify(value)
}
