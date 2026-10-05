import * as React from "react"

// Stereo peak meter. Levels arrive with each live sample; the bars animate
// between them: instant rise, steady fall, and a peak marker that holds.

const SCALE = [-48, -44, -40, -36, -32, -28, -24, -20, -16, -12, -8, -4, 0]
const MIN_DB = -48
const MAX_DB = 0
/** Bar fall, in percent of the scale per second. */
const DECAY_RATE = 25
const PEAK_HOLD_MS = 2000
const PEAK_DECAY_RATE = 10

function dbToPercent(db: number) {
  const clamped = Math.max(MIN_DB, Math.min(MAX_DB, db))
  return ((clamped - MIN_DB) / (MAX_DB - MIN_DB)) * 100
}

function Tick({ dB, i, total }: { dB: number; i: number; total: number }) {
  const pct = total === 1 ? 50 : (i / (total - 1)) * 100
  const isFirst = i === 0
  const isLast = i === total - 1

  return (
    <div
      className="absolute top-0 flex h-full w-7 items-center justify-center"
      style={{ left: `${pct}%`, transform: "translateX(-50%)" }}
    >
      <span
        className={
          isFirst ? "invisible px-0.5" : `px-0.5 ${isLast ? "-ml-0.5" : "-ml-2"}`
        }
      >
        {dB}
      </span>
      {isFirst || isLast ? null : (
        <>
          <div className="absolute -top-[3px] left-1/2 h-1.5 w-px bg-current" />
          <div className="absolute -bottom-[3px] left-1/2 h-1.5 w-px bg-current" />
        </>
      )}
    </div>
  )
}

function Channel({
  label,
  barRef,
  peakRef,
}: {
  label: string
  barRef: React.Ref<HTMLDivElement>
  peakRef: React.Ref<HTMLDivElement>
}) {
  return (
    <>
      <div className="flex h-3 items-center bg-muted px-1.5 font-medium">{label}</div>
      <div className="relative h-3 border-l border-current bg-muted">
        <div ref={barRef} className="h-full bg-status-good" style={{ width: "0%" }} />
        <div className="pointer-events-none absolute inset-0">
          <div
            ref={peakRef}
            className="absolute top-0 h-full w-0.5 -translate-x-1/2 bg-status-warning"
            style={{ left: "0%" }}
          />
        </div>
      </div>
    </>
  )
}

/** Left and right peak level in dBFS. */
export function LevelMeter({ levels }: { levels: [number, number] }) {
  const barL = React.useRef<HTMLDivElement>(null)
  const barR = React.useRef<HTMLDivElement>(null)
  const peakL = React.useRef<HTMLDivElement>(null)
  const peakR = React.useRef<HTMLDivElement>(null)

  // The animation loop reads the latest levels without restarting.
  const levelsRef = React.useRef(levels)
  levelsRef.current = levels

  React.useEffect(() => {
    const bar = { l: 0, r: 0 }
    const peak = { l: 0, r: 0, holdL: 0, holdR: 0 }
    let lastFrame = performance.now()
    let raf = 0

    const update = (now: number) => {
      const delta = (now - lastFrame) / 1000
      lastFrame = now
      const fall = DECAY_RATE * delta
      const peakFall = PEAK_DECAY_RATE * delta

      const [dbL, dbR] = levelsRef.current
      const pctL = dbToPercent(dbL)
      const pctR = dbToPercent(dbR)

      bar.l = pctL > bar.l ? pctL : Math.max(pctL, bar.l - fall)
      bar.r = pctR > bar.r ? pctR : Math.max(pctR, bar.r - fall)

      if (pctL > peak.l) {
        peak.l = pctL
        peak.holdL = now
      }
      if (pctR > peak.r) {
        peak.r = pctR
        peak.holdR = now
      }
      if (now - peak.holdL > PEAK_HOLD_MS) peak.l = Math.max(pctL, peak.l - peakFall)
      if (now - peak.holdR > PEAK_HOLD_MS) peak.r = Math.max(pctR, peak.r - peakFall)
      peak.l = Math.max(peak.l, bar.l)
      peak.r = Math.max(peak.r, bar.r)

      if (barL.current) barL.current.style.width = `${bar.l}%`
      if (barR.current) barR.current.style.width = `${bar.r}%`
      if (peakL.current) peakL.current.style.left = `${peak.l}%`
      if (peakR.current) peakR.current.style.left = `${peak.r}%`

      raf = requestAnimationFrame(update)
    }

    raf = requestAnimationFrame(update)
    return () => cancelAnimationFrame(raf)
  }, [])

  return (
    <div
      className="grid grid-cols-[48px_1fr] font-mono text-[9px] leading-none text-muted-foreground"
      role="meter"
      aria-valuemin={MIN_DB}
      aria-valuemax={MAX_DB}
      aria-valuenow={Math.max(...levels)}
      aria-valuetext={`Left ${levels[0]} dBFS, right ${levels[1]} dBFS`}
    >
      <Channel label="L" barRef={barL} peakRef={peakL} />
      <div className="flex h-4 items-center justify-center bg-muted">dBFS</div>
      <div className="relative h-4 border-l border-current">
        {SCALE.map((dB, i) => (
          <Tick key={dB} dB={dB} i={i} total={SCALE.length} />
        ))}
      </div>
      <Channel label="R" barRef={barR} peakRef={peakR} />
    </div>
  )
}
