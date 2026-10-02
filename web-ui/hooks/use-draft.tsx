import * as React from "react"
import { countChanges, diffConfigs, type ChangeGroup } from "@/lib/diff"
import {
  ApiError,
  applyConfig,
  getConfig,
  previewConfig,
  saveConfig,
  type Applied,
  type FileStatus,
  type OperatorConfig,
  type Preview,
  type Saved,
} from "@/lib/ui-api"

// Edits collect in a draft of the operator configuration, kept in the
// browser until they are applied or discarded. The mux validates the draft
// as it changes, and the pages show the draft as the mux would run it.

const STORAGE_KEY = "dabmux-draft"
const POLL_MS = 5000
const PREVIEW_DELAY_MS = 300

type Base = { config: OperatorConfig; revision: number; file: FileStatus | null }

type DraftContext = {
  /** The running operator configuration, once loaded. */
  base: Base | null
  /** The configuration with the pending changes, or the base without any. */
  current: OperatorConfig | null
  changes: ChangeGroup[]
  count: number
  /** The draft as the mux would run it; null without changes. */
  preview: Preview | null
  /** Why the mux would refuse the draft. */
  invalid: string | null
  validating: boolean
  /** The mux configuration changed since the draft was started. */
  stale: boolean
  /** The configuration file, and whether the running configuration is saved there. */
  file: FileStatus | null
  /** Change a copy of the current configuration. */
  update: (change: (config: OperatorConfig) => void) => void
  discard: () => void
  /**
   * Apply the draft; `force` even when the mux configuration changed
   * meanwhile, `save` and then write it to the configuration file.
   */
  apply: (options?: { force?: boolean; save?: boolean }) => Promise<{
    applied: Applied
    saved?: Saved
    saveError?: Error
  }>
  /**
   * Write the running configuration to the configuration file; `force`
   * overwrites changes made to the file on disk.
   */
  save: (force?: boolean) => Promise<Saved>
}

const Context = React.createContext<DraftContext | null>(null)

type Stored = { base: OperatorConfig; draft: OperatorConfig }

function load(): Stored | null {
  try {
    const raw = localStorage.getItem(STORAGE_KEY)
    return raw ? (JSON.parse(raw) as Stored) : null
  } catch {
    return null
  }
}

function store(value: Stored | null) {
  try {
    if (value) localStorage.setItem(STORAGE_KEY, JSON.stringify(value))
    else localStorage.removeItem(STORAGE_KEY)
  } catch {
    // Storage unavailable: the draft lasts until the page is closed.
  }
}

const same = (a: unknown, b: unknown) => JSON.stringify(a) === JSON.stringify(b)

export function DraftProvider({ children }: { children: React.ReactNode }) {
  const [base, setBase] = React.useState<Base | null>(null)
  /** The configuration the draft started from. */
  const [origin, setOrigin] = React.useState<OperatorConfig | null>(null)
  const [draft, setDraft] = React.useState<OperatorConfig | null>(null)
  const [preview, setPreview] = React.useState<Preview | null>(null)
  const [invalid, setInvalid] = React.useState<string | null>(null)
  const [validating, setValidating] = React.useState(false)
  /** Whether the draft saved in the browser was read, after the first load. */
  const [restored, setRestored] = React.useState(false)
  const restoring = React.useRef(false)

  const fetchBase = React.useCallback(async () => {
    try {
      const next = await getConfig()
      setBase((current) =>
        current &&
        current.revision === next.revision &&
        same(current.config, next.config) &&
        same(current.file, next.file)
          ? current
          : next
      )
      if (!restoring.current) {
        restoring.current = true
        const saved = load()
        if (saved) {
          setOrigin(saved.base)
          setDraft(saved.draft)
        }
        setRestored(true)
      }
    } catch {
      // The connection status shows elsewhere; keep the last base.
    }
  }, [])

  React.useEffect(() => {
    fetchBase()
    const timer = setInterval(() => {
      if (document.visibilityState === "visible") fetchBase()
    }, POLL_MS)
    return () => clearInterval(timer)
  }, [fetchBase])

  const changes = React.useMemo(
    () => (origin && draft ? diffConfigs(origin, draft) : []),
    [origin, draft]
  )
  const count = countChanges(changes)
  const pending = draft !== null && count > 0
  const stale = pending && base !== null && origin !== null && !same(base.config, origin)

  React.useEffect(() => {
    // Before the saved draft is read, writing would delete it.
    if (!restored) return
    store(pending && origin && draft ? { base: origin, draft } : null)
  }, [restored, pending, origin, draft])

  // Validate the draft whenever it changes.
  React.useEffect(() => {
    if (!pending || !draft) {
      setPreview(null)
      setInvalid(null)
      setValidating(false)
      return
    }
    let cancelled = false
    setValidating(true)
    const timer = setTimeout(async () => {
      try {
        const result = await previewConfig(draft)
        if (cancelled) return
        setPreview(result)
        setInvalid(null)
      } catch (e) {
        if (cancelled) return
        setInvalid(e instanceof Error ? e.message : String(e))
      } finally {
        if (!cancelled) setValidating(false)
      }
    }, PREVIEW_DELAY_MS)
    return () => {
      cancelled = true
      clearTimeout(timer)
    }
  }, [pending, draft])

  const current = pending ? draft : (base?.config ?? null)

  const update = React.useCallback(
    (change: (config: OperatorConfig) => void) => {
      if (!base) return
      const startFrom = pending && draft ? draft : base.config
      const next = structuredClone(startFrom)
      change(next)
      if (!pending) setOrigin(base.config)
      setDraft(next)
    },
    [base, draft, pending]
  )

  const discard = React.useCallback(() => {
    setDraft(null)
    setOrigin(null)
  }, [])

  const apply = React.useCallback(
    async ({ force = false, save = false } = {}) => {
      if (!draft || !base) throw new Error("no changes to apply")
      if (stale && !force) {
        throw new ApiError(412, "the mux configuration changed since these edits started")
      }
      try {
        const applied = await applyConfig(draft, force ? undefined : base.revision)
        setDraft(null)
        setOrigin(null)
        // The draft is on air even if saving fails.
        let saved: Saved | undefined
        let saveError: Error | undefined
        if (save) {
          try {
            saved = await saveConfig(applied.revision)
          } catch (e) {
            saveError = e instanceof Error ? e : new Error(String(e))
          }
        }
        return { applied, saved, saveError }
      } finally {
        await fetchBase()
      }
    },
    [draft, base, stale, fetchBase]
  )

  const save = React.useCallback(
    async (force = false) => {
      if (!base) throw new Error("configuration not loaded")
      try {
        return await saveConfig(base.revision, force)
      } finally {
        await fetchBase()
      }
    },
    [base, fetchBase]
  )

  const value: DraftContext = {
    base,
    current,
    changes: pending ? changes : [],
    count: pending ? count : 0,
    preview: pending ? preview : null,
    invalid: pending ? invalid : null,
    validating: pending && validating,
    stale,
    file: base?.file ?? null,
    update,
    discard,
    apply,
    save,
  }

  return <Context.Provider value={value}>{children}</Context.Provider>
}

export function useDraft(): DraftContext {
  const context = React.useContext(Context)
  if (!context) throw new Error("useDraft must be used within a DraftProvider")
  return context
}
