import * as React from "react"
import type { ResolvedConfig } from "@/lib/config"

const POLL_MS = 5000

type State = {
  config: ResolvedConfig | null
  error: string | null
  /** Until the first response arrives. */
  loading: boolean
  updatedAt: Date | null
  refresh: () => void
}

/** The running configuration, polled so that hot reloads show up. */
export function useResolvedConfig(): State {
  const [config, setConfig] = React.useState<ResolvedConfig | null>(null)
  const [error, setError] = React.useState<string | null>(null)
  const [loading, setLoading] = React.useState(true)
  const [updatedAt, setUpdatedAt] = React.useState<Date | null>(null)
  const last = React.useRef<string | null>(null)

  const refresh = React.useCallback(async () => {
    try {
      const response = await fetch("/api/config/resolved")
      const body = await response.text()
      if (!response.ok) {
        throw new Error(errorMessage(body) ?? `HTTP ${response.status}`)
      }
      if (body !== last.current) {
        last.current = body
        setConfig(JSON.parse(body) as ResolvedConfig)
      }
      setError(null)
      setUpdatedAt(new Date())
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setLoading(false)
    }
  }, [])

  React.useEffect(() => {
    refresh()
    const timer = setInterval(() => {
      if (document.visibilityState === "visible") refresh()
    }, POLL_MS)
    return () => clearInterval(timer)
  }, [refresh])

  return { config, error, loading, updatedAt, refresh }
}

function errorMessage(body: string): string | null {
  try {
    const parsed = JSON.parse(body)
    return typeof parsed?.error === "string" ? parsed.error : null
  } catch {
    return null
  }
}
