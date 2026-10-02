import * as React from "react"
import { cn } from "cn"
import { Badge } from "@/components/ui/badge"
import {
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card"

/** Page title with an optional description and actions. */
export function PageHeader({
  title,
  description,
  children,
}: {
  title: React.ReactNode
  description?: React.ReactNode
  children?: React.ReactNode
}) {
  return (
    <div className="flex flex-wrap items-end justify-between gap-4">
      <div className="min-w-0">
        <h1 className="font-heading text-lg font-semibold">{title}</h1>
        {description && (
          <p className="mt-1 text-xs text-muted-foreground">{description}</p>
        )}
      </div>
      {children}
    </div>
  )
}

export function Section({
  title,
  description,
  action,
  children,
  className,
  flush = false,
}: {
  title: React.ReactNode
  description?: React.ReactNode
  action?: React.ReactNode
  children: React.ReactNode
  className?: string
  /** No horizontal padding, for tables that run edge to edge. */
  flush?: boolean
}) {
  return (
    <Card className={className}>
      <CardHeader>
        <CardTitle>{title}</CardTitle>
        {description && <CardDescription>{description}</CardDescription>}
        {action && <CardAction>{action}</CardAction>}
      </CardHeader>
      <CardContent className={cn(flush && "px-0")}>{children}</CardContent>
    </Card>
  )
}

/** Label/value pairs in a responsive grid. */
export function Fields({
  children,
  className,
}: {
  children: React.ReactNode
  className?: string
}) {
  return (
    <dl
      className={cn(
        "grid grid-cols-[repeat(auto-fill,minmax(11rem,1fr))] gap-x-6 gap-y-4",
        className
      )}
    >
      {children}
    </dl>
  )
}

export function Field({
  label,
  children,
  hint,
}: {
  label: React.ReactNode
  children: React.ReactNode
  hint?: React.ReactNode
}) {
  return (
    <div className="min-w-0">
      <dt className="text-[0.625rem] tracking-wider text-muted-foreground uppercase">
        {label}
      </dt>
      <dd className="mt-1 truncate text-sm">{children}</dd>
      {hint && <dd className="text-xs text-muted-foreground">{hint}</dd>}
    </div>
  )
}

/** A value that is absent in the configuration. */
export function None({ children = "—" }: { children?: React.ReactNode }) {
  return <span className="text-muted-foreground">{children}</span>
}

export function Flag({
  on,
  yes = "yes",
  no = "no",
}: {
  on: boolean
  yes?: string
  no?: string
}) {
  return on ? (
    <Badge variant="secondary">{yes}</Badge>
  ) : (
    <Badge variant="outline" className="text-muted-foreground">
      {no}
    </Badge>
  )
}

export function Stat({
  label,
  value,
  detail,
}: {
  label: React.ReactNode
  value: React.ReactNode
  detail?: React.ReactNode
}) {
  return (
    <Card size="sm">
      <CardHeader>
        <CardDescription className="text-[0.625rem] tracking-wider uppercase">
          {label}
        </CardDescription>
        <CardTitle className="text-2xl font-semibold tabular-nums">
          {value}
        </CardTitle>
        {detail && (
          <div className="text-xs text-muted-foreground">{detail}</div>
        )}
      </CardHeader>
    </Card>
  )
}

export function EmptyState({ children }: { children: React.ReactNode }) {
  return (
    <div className="px-4 py-8 text-center text-xs text-muted-foreground">
      {children}
    </div>
  )
}
