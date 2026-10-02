// Types for GET /api/config/resolved (dabmux/src/config.rs, ResolvedConfig)
// and helpers to present them.

export type ResolvedConfig = {
  ensemble: Ensemble
  services: Service[]
  subchannels: Subchannel[]
  other_services: OtherService[]
  frequencies: FrequencyInformation[]
  service_changes: ServiceChange[]
  output: Output
}

export type Ensemble = {
  id: number
  ecc: number
  label: string
  short_label: string | null
  local_time_offset_half_hours: number
  local_time_offset_auto: boolean
  international_table: number
  reconfiguration_counter: number | null
  mode: number
  tist: boolean
  tist_offset_ms: number
  tist_at_fct0_ms: number
  tai_utc_offset: number | null
  tai_clock_bulletins: string[]
}

export type UserApplication = "slideshow" | "spi"

export type Service = {
  id: number
  ecc: number | null
  label: string
  short_label: string | null
  pty: number
  language: number
  linking: LinkageSet[]
  other_ensembles: number[]
  data: boolean
  components: Component[]
}

export type Component = {
  scids: number
  scid?: number
  subchannel: string
  user_applications: UserApplication[]
  packet?: { address: number; dscty: number; data_groups: boolean }
}

export type SubchannelType = "dab_plus" | "mpeg_audio" | "enhanced_packet"

export type Input =
  | {
      protocol: "edi"
      uri: string
      stream_index: number
      buffer_frames: number
      prebuffer_frames: number
      timing: string
      backpressure: boolean | null
    }
  | { protocol: "sti"; uri: string }
  | { protocol: "file"; path: string }

export type Subchannel = {
  name: string
  id: number
  allocated: boolean
  type: SubchannelType
  bitrate: number
  protection: { profile: "eep_a" | "eep_b"; level: number }
  start_address_cu: number
  size_cu: number
  input: Input
}

export type LinkKind = "dab" | "fm" | "drm" | "amss"

export type LinkageSet = {
  lsn: number
  hard: boolean
  active: boolean
  international: boolean | null
  links: {
    type: LinkKind
    id: number
    ecc: number | null
    preference: "normal" | "low"
  }[]
}

export type OtherService = { id: number; ensembles: number[] }

export type FrequencyInformation =
  | {
      type: "dab"
      eid: number
      continuity: boolean
      frequencies: { mhz: number; adjacent: boolean; mode_i: boolean }[]
    }
  | {
      type: "fm"
      pi: number
      continuity: boolean
      other_ensemble: boolean | null
      frequencies: number[]
    }
  | {
      type: "drm" | "amss"
      id: number
      continuity: boolean
      other_ensemble: boolean | null
      frequencies: number[]
    }

export type ServiceChange = {
  id: number
  scids: number
  change: "identity" | "addition" | "local_removal" | "global_removal"
  part_time: boolean
  at: string | null
  transfer_sid: number | null
  transfer_eid: number | null
  access_controlled: boolean
  ascty: number | null
  dscty: number | null
  label: string | null
  short_label: string | null
}

export type Destination =
  | { protocol: "udp"; address: string; port: number }
  | {
      protocol: "tcp"
      listen_port: number
      max_frames_queued: number
      preroll_ms: number
    }

export type Output = { destinations: Destination[]; tagpacket_alignment: number }

/** Capacity units of the MSC per common interleaved frame (all modes). */
export const MSC_CAPACITY_CU = 864

/** Upper-case hex, padded to `digits`. */
export function hex(value: number, digits = 2): string {
  return value.toString(16).toUpperCase().padStart(digits, "0")
}

/** Service IDs are 16 bits for programme services, 32 for data services. */
export function sid(value: number): string {
  return hex(value, value > 0xffff ? 8 : 4)
}

export function serviceKey(service: Service): string {
  return sid(service.id)
}

export function protectionLabel(p: Subchannel["protection"]): string {
  return `EEP ${p.level}-${p.profile === "eep_a" ? "A" : "B"}`
}

export const SUBCHANNEL_TYPES: Record<SubchannelType, string> = {
  dab_plus: "DAB+",
  mpeg_audio: "DAB (MP2)",
  enhanced_packet: "Packet (FEC)",
}

export const LINK_KINDS: Record<LinkKind, string> = {
  dab: "DAB",
  fm: "FM (RDS)",
  drm: "DRM",
  amss: "AMSS",
}

export const CHANGE_KINDS: Record<ServiceChange["change"], string> = {
  identity: "Identity change",
  addition: "Addition",
  local_removal: "Local removal",
  global_removal: "Global removal",
}

export const USER_APPLICATIONS: Record<UserApplication, string> = {
  slideshow: "Slideshow",
  spi: "SPI",
}

export function inputLabel(input: Input): string {
  return input.protocol === "file" ? input.path : input.uri
}

export function localTimeOffset(ensemble: Ensemble): string {
  if (ensemble.local_time_offset_auto) return "automatic"
  const minutes = ensemble.local_time_offset_half_hours * 30
  const sign = minutes < 0 ? "−" : "+"
  const abs = Math.abs(minutes)
  return `UTC${sign}${Math.floor(abs / 60)}:${String(abs % 60).padStart(2, "0")}`
}

// TS 101 756 V2.5.1, table 12 (international table 1, all except North America).
export const PTY_TABLE_1 = [
  "None", "News", "Current Affairs", "Information", "Sport", "Education",
  "Drama", "Arts", "Science", "Talk", "Pop Music", "Rock Music",
  "Easy Listening", "Light Classical", "Classical Music", "Other Music",
  "Weather", "Finance", "Children's", "Factual", "Religion", "Phone In",
  "Travel", "Leisure", "Jazz and Blues", "Country Music", "National Music",
  "Oldies Music", "Folk Music", "Documentary",
]

export function ptyLabel(pty: number, internationalTable: number): string {
  const name = internationalTable === 1 ? PTY_TABLE_1[pty] : undefined
  return name ? `${pty} · ${name}` : String(pty)
}

// TS 101 756 V2.5.1, tables 9 and 10.
export const LANGUAGES: Record<number, string> = {
  0x00: "Unknown", 0x01: "Albanian", 0x02: "Breton", 0x03: "Catalan",
  0x04: "Croatian", 0x05: "Welsh", 0x06: "Czech", 0x07: "Danish",
  0x08: "German", 0x09: "English", 0x0a: "Spanish", 0x0b: "Esperanto",
  0x0c: "Estonian", 0x0d: "Basque", 0x0e: "Faroese", 0x0f: "French",
  0x10: "Frisian", 0x11: "Irish", 0x12: "Gaelic", 0x13: "Galician",
  0x14: "Icelandic", 0x15: "Italian", 0x16: "Sami", 0x17: "Latin",
  0x18: "Latvian", 0x19: "Luxembourgian", 0x1a: "Lithuanian",
  0x1b: "Hungarian", 0x1c: "Maltese", 0x1d: "Dutch", 0x1e: "Norwegian",
  0x1f: "Occitan", 0x20: "Polish", 0x21: "Portuguese", 0x22: "Romanian",
  0x23: "Romansh", 0x24: "Serbian", 0x25: "Slovak", 0x26: "Slovene",
  0x27: "Finnish", 0x28: "Swedish", 0x29: "Turkish", 0x2a: "Flemish",
  0x2b: "Walloon", 0x40: "Background sound", 0x45: "Zulu",
  0x46: "Vietnamese", 0x47: "Uzbek", 0x48: "Urdu", 0x49: "Ukrainian",
  0x4a: "Thai", 0x4b: "Telugu", 0x4c: "Tatar", 0x4d: "Tamil",
  0x4e: "Tadzhik", 0x4f: "Swahili", 0x50: "Sranan Tongo", 0x51: "Somali",
  0x52: "Sinhalese", 0x53: "Shona", 0x54: "Serbo-Croat", 0x55: "Rusyn",
  0x56: "Russian", 0x57: "Quechua", 0x58: "Pushtu", 0x59: "Punjabi",
  0x5a: "Persian", 0x5b: "Papiamento", 0x5c: "Oriya", 0x5d: "Nepali",
  0x5e: "Ndebele", 0x5f: "Marathi", 0x60: "Moldavian", 0x61: "Malaysian",
  0x62: "Malagasay", 0x63: "Macedonian", 0x64: "Laotian", 0x65: "Korean",
  0x66: "Khmer", 0x67: "Kazakh", 0x68: "Kannada", 0x69: "Japanese",
  0x6a: "Indonesian", 0x6b: "Hindi", 0x6c: "Hebrew", 0x6d: "Hausa",
  0x6e: "Gurani", 0x6f: "Gujurati", 0x70: "Greek", 0x71: "Georgian",
  0x72: "Fulani", 0x73: "Dari", 0x74: "Chuvash", 0x75: "Chinese",
  0x76: "Burmese", 0x77: "Bulgarian", 0x78: "Bengali", 0x79: "Belorussian",
  0x7a: "Bambora", 0x7b: "Azerbaijani", 0x7c: "Assamese", 0x7d: "Armenian",
  0x7e: "Arabic", 0x7f: "Amharic",
}

export function languageLabel(code: number): string {
  const name = LANGUAGES[code]
  return name ? `${hex(code)} · ${name}` : hex(code)
}

/** Services that use a subchannel, keyed by subchannel name. */
export function subchannelUsers(config: ResolvedConfig): Map<string, Service[]> {
  const users = new Map<string, Service[]>()
  for (const service of config.services) {
    for (const component of service.components) {
      const list = users.get(component.subchannel) ?? []
      if (!list.includes(service)) list.push(service)
      users.set(component.subchannel, list)
    }
  }
  return users
}

export function urlHost(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}
