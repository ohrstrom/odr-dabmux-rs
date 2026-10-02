import { cn } from "cn"
import {
  Tooltip,
  TooltipContent,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import {
  MSC_CAPACITY_CU,
  SUBCHANNEL_TYPES,
  protectionLabel,
  type Subchannel,
  type SubchannelType,
} from "@/lib/config"
import { href } from "@/hooks/use-route"

const TYPE_CLASSES: Record<SubchannelType, string> = {
  dab_plus: "bg-msc-dab-plus",
  mpeg_audio: "bg-msc-mpeg-audio",
  enhanced_packet: "bg-msc-packet",
}

/** The MSC as a bar of 864 capacity units, one segment per subchannel. */
export function MscMap({
  subchannels,
  selected,
}: {
  subchannels: Subchannel[]
  /** SubChId to highlight. */
  selected?: number
}) {
  const used = subchannels.reduce((sum, s) => sum + s.size_cu, 0)
  const types = [...new Set(subchannels.map((s) => s.type))]

  return (
    <div className="space-y-3">
      <div className="relative h-10 w-full overflow-hidden bg-muted bg-[repeating-linear-gradient(135deg,transparent_0_6px,var(--border)_6px_7px)] ring-1 ring-foreground/10">
        {subchannels.map((s) => (
          <Tooltip key={s.name}>
            <TooltipTrigger
              render={
                <a
                  href={href("subchannels", String(s.id))}
                  aria-label={`Subchannel ${s.id}, ${s.name}`}
                  className={cn(
                    "absolute inset-y-0 border-r border-background transition-[filter,opacity] hover:brightness-110",
                    TYPE_CLASSES[s.type],
                    selected !== undefined &&
                      s.id !== selected &&
                      "opacity-40",
                    s.id === selected && "ring-2 ring-ring ring-inset"
                  )}
                  style={{
                    left: `${(s.start_address_cu / MSC_CAPACITY_CU) * 100}%`,
                    width: `${(s.size_cu / MSC_CAPACITY_CU) * 100}%`,
                  }}
                />
              }
            />
            <TooltipContent className="flex-col items-start gap-0.5">
              <span className="font-medium">
                SubChId {s.id} · {s.name}
              </span>
              <span>
                {SUBCHANNEL_TYPES[s.type]} · {s.bitrate} kbit/s ·{" "}
                {protectionLabel(s.protection)}
              </span>
              <span>
                CU {s.start_address_cu}–{s.start_address_cu + s.size_cu - 1} (
                {s.size_cu} CU)
              </span>
            </TooltipContent>
          </Tooltip>
        ))}
      </div>
      <div className="flex flex-wrap items-center justify-between gap-3 text-xs text-muted-foreground">
        <div className="flex flex-wrap gap-4">
          {types.map((type) => (
            <span key={type} className="inline-flex items-center gap-1.5">
              <span className={cn("size-2.5", TYPE_CLASSES[type])} />
              {SUBCHANNEL_TYPES[type]}
            </span>
          ))}
          <span className="inline-flex items-center gap-1.5">
            <span className="size-2.5 bg-muted ring-1 ring-foreground/20" />
            free
          </span>
        </div>
        <span className="tabular-nums">
          {used} / {MSC_CAPACITY_CU} CU used ·{" "}
          {MSC_CAPACITY_CU - used} CU free
        </span>
      </div>
    </div>
  )
}
