import * as React from "react"

// Live statistics over server-sent events (dabmux/src/api/live.rs). The mux
// follows every 24 ms frame and sends one sample per interval, with buffer
// minimum and maximum over that interval.

export type Counters = {
  generated_frames: number
  config_activations: number
  input_underflows: number
  input_drops: number
  input_size_mismatches: number
  decode_errors: number
  send_errors: number
  missed_ticks: number
  catch_up_frames: number
  buffered_input_frames: number
  late_input_frames: number
  invalid_timestamps: number
  frame_errors: number
  clock_drift_ms: number
}

export type InputState = "receiving" | "prebuffering" | "underflow"

export type LiveSubchannel = {
  name: string
  id: number
  state: InputState
  buffered: number
  capacity: number
  buffered_min: number
  buffered_max: number
  underflows: number
  drops: number
}

export type LiveOutput = {
  protocol: "tcp" | "udp"
  endpoint: string
  clients: number | null
}

export type Sample = {
  frame: number
  unix_ms: number
  /** Frames since the previous sample. */
  frames: number
  /** Time since the previous sample. */
  elapsed_ms: number
  interval_ms: number
  counters: Counters
  subchannels: LiveSubchannel[]
  outputs: LiveOutput[]
  /** When the browser received it. */
  received_ms: number
}

export type LiveStats = {
  sample: Sample | null
  previous: Sample | null
  connected: boolean
  /** Why the stream is not connected, when the mux said so. */
  error: string | null
  /** Frames per second over the last two seconds or so. */
  fps: number | null
}

/** Long enough that 10 or 11 frames per 250 ms average out. */
const RATE_WINDOW_MS = 2000

export function useLiveStats(intervalMs: number): LiveStats {
  const [state, setState] = React.useState<LiveStats>({
    sample: null,
    previous: null,
    connected: false,
    error: null,
    fps: null,
  })
  const history = React.useRef<{ frames: number; elapsed_ms: number }[]>([])

  React.useEffect(() => {
    // EventSource reconnects by itself after errors.
    const url = `/api/ui/stats/stream?interval_ms=${intervalMs}`
    const source = new EventSource(url)
    history.current = []
    source.onopen = () => setState((s) => ({ ...s, connected: true, error: null }))
    source.onerror = () => {
      setState((s) => ({ ...s, connected: false }))
      probe(url).then((error) => setState((s) => ({ ...s, error })))
    }
    source.addEventListener("stats", (event) => {
      const sample = {
        ...(JSON.parse((event as MessageEvent).data) as Omit<Sample, "received_ms">),
        received_ms: Date.now(),
      }
      const recent = history.current
      recent.push({ frames: sample.frames, elapsed_ms: sample.elapsed_ms })
      while (
        recent.length > 1 &&
        recent.slice(1).reduce((sum, r) => sum + r.elapsed_ms, 0) >= RATE_WINDOW_MS
      ) {
        recent.shift()
      }
      const frames = recent.reduce((sum, r) => sum + r.frames, 0)
      const elapsed = recent.reduce((sum, r) => sum + r.elapsed_ms, 0)
      const fps = elapsed > 0 ? (frames * 1000) / elapsed : null
      setState((s) => ({ sample, previous: s.sample, connected: true, error: null, fps }))
    })
    return () => source.close()
  }, [intervalMs])

  return state
}

/**
 * EventSource does not say why it failed; request the stream once more to
 * read the mux's error, and stop as soon as it answers with a stream.
 */
async function probe(url: string): Promise<string | null> {
  const abort = new AbortController()
  try {
    const response = await fetch(url, { signal: abort.signal })
    if (response.ok) return null
    const body = await response.json().catch(() => null)
    return `HTTP ${response.status}${body?.message ? `: ${body.message}` : body?.error ? `: ${body.error}` : ""}`
  } catch {
    return "the mux is not reachable"
  } finally {
    abort.abort()
  }
}

/** Increase of a counter per second between two samples. */
export function rate(
  live: LiveStats,
  value: (sample: Sample) => number
): number | null {
  const { sample, previous } = live
  if (!sample || !previous) return null
  const seconds = (sample.unix_ms - previous.unix_ms) / 1000
  if (seconds <= 0) return null
  return Math.max(0, value(sample) - value(previous)) / seconds
}
