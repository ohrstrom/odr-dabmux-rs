import * as React from "react"
import { createRoot } from "react-dom/client"
import { ArrowClockwiseIcon, WarningCircleIcon } from "@phosphor-icons/react"
import { cn } from "cn"
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert"
import {
  Breadcrumb,
  BreadcrumbItem,
  BreadcrumbLink,
  BreadcrumbList,
  BreadcrumbPage,
  BreadcrumbSeparator,
} from "@/components/ui/breadcrumb"
import { Button } from "@/components/ui/button"
import { Separator } from "@/components/ui/separator"
import {
  SidebarInset,
  SidebarProvider,
  SidebarTrigger,
} from "@/components/ui/sidebar"
import { Skeleton } from "@/components/ui/skeleton"
import { TooltipProvider } from "@/components/ui/tooltip"
import { AppSidebar, PAGES, type Page } from "@/components/app-sidebar"
import { EmptyState } from "@/components/fields"
import { ModeToggle } from "@/components/mode-toggle"
import { OutputPage } from "@/components/output"
import { Overview } from "@/components/overview"
import { ServiceDetail, ServicesPage } from "@/components/services"
import { ServiceFollowingPage } from "@/components/service-following"
import { SubchannelsPage } from "@/components/subchannels"
import { ThemeProvider } from "@/components/theme-provider"
import { serviceKey, type ResolvedConfig } from "@/lib/config"
import { useResolvedConfig } from "@/hooks/use-resolved-config"
import { href, useRoute } from "@/hooks/use-route"

function App() {
  const route = useRoute()
  const { config, error, loading, updatedAt, refresh } = useResolvedConfig()
  const page: Page = route[0] && route[0] in PAGES ? (route[0] as Page) : "overview"

  return (
    <SidebarProvider>
      <AppSidebar
        config={config}
        route={route}
        status={<ConnectionStatus error={error} updatedAt={updatedAt} />}
      />
      <SidebarInset>
        <header className="sticky top-0 z-10 flex h-12 shrink-0 items-center gap-2 border-b bg-background/95 px-4 backdrop-blur">
          <SidebarTrigger className="-ml-1" />
          <Separator orientation="vertical" className="mr-2 h-4 self-center" />
          <Crumbs config={config} page={page} route={route} />
          <div className="ml-auto flex items-center gap-1">
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Reload configuration"
              onClick={refresh}
            >
              <ArrowClockwiseIcon />
            </Button>
            <ModeToggle />
          </div>
        </header>

        <div className="mx-auto w-full max-w-7xl flex-1 space-y-4 p-4 md:p-6">
          {error && (
            <Alert variant="destructive">
              <WarningCircleIcon />
              <AlertTitle>
                {config
                  ? "Lost contact with the mux; showing the last configuration"
                  : "Cannot load the configuration"}
              </AlertTitle>
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          {config ? (
            <Content config={config} page={page} route={route} />
          ) : loading ? (
            <Loading />
          ) : null}
        </div>
      </SidebarInset>
    </SidebarProvider>
  )
}

function Content({
  config,
  page,
  route,
}: {
  config: ResolvedConfig
  page: Page
  route: string[]
}) {
  switch (page) {
    case "services": {
      if (!route[1]) return <ServicesPage config={config} />
      const service = config.services.find((s) => serviceKey(s) === route[1])
      return service ? (
        <ServiceDetail config={config} service={service} />
      ) : (
        <EmptyState>
          No service {route[1]} in the running configuration.{" "}
          <a className="underline" href={href("services")}>
            All services
          </a>
        </EmptyState>
      )
    }
    case "subchannels":
      return (
        <SubchannelsPage
          config={config}
          selected={route[1] !== undefined ? Number(route[1]) : undefined}
        />
      )
    case "service-following":
      return <ServiceFollowingPage config={config} />
    case "output":
      return <OutputPage config={config} />
    default:
      return <Overview config={config} />
  }
}

function Crumbs({
  config,
  page,
  route,
}: {
  config: ResolvedConfig | null
  page: Page
  route: string[]
}) {
  const title = PAGES[page].title
  let detail: string | undefined
  if (page === "services" && route[1]) {
    detail =
      config?.services.find((s) => serviceKey(s) === route[1])?.label ??
      route[1]
  } else if (page === "subchannels" && route[1]) {
    detail = `SubChId ${route[1]}`
  }

  return (
    <Breadcrumb>
      <BreadcrumbList>
        {detail ? (
          <>
            <BreadcrumbItem className="hidden sm:block">
              <BreadcrumbLink href={href(page)}>{title}</BreadcrumbLink>
            </BreadcrumbItem>
            <BreadcrumbSeparator className="hidden sm:block" />
            <BreadcrumbItem>
              <BreadcrumbPage>{detail}</BreadcrumbPage>
            </BreadcrumbItem>
          </>
        ) : (
          <BreadcrumbItem>
            <BreadcrumbPage>{title}</BreadcrumbPage>
          </BreadcrumbItem>
        )}
      </BreadcrumbList>
    </Breadcrumb>
  )
}

function ConnectionStatus({
  error,
  updatedAt,
}: {
  error: string | null
  updatedAt: Date | null
}) {
  const ok = !error && updatedAt !== null
  return (
    <div className="flex items-center gap-2 px-2 py-1 text-xs text-muted-foreground group-data-[collapsible=icon]:justify-center group-data-[collapsible=icon]:px-0">
      <span
        className={cn(
          "size-2 shrink-0 rounded-full",
          ok ? "bg-emerald-500" : error ? "bg-destructive" : "bg-muted-foreground"
        )}
      />
      <span className="truncate group-data-[collapsible=icon]:hidden">
        {ok
          ? `Connected · ${updatedAt.toLocaleTimeString()}`
          : error
            ? "Disconnected"
            : "Connecting…"}
      </span>
    </div>
  )
}

function Loading() {
  return (
    <div className="space-y-6">
      <Skeleton className="h-8 w-64" />
      <div className="grid grid-cols-2 gap-4 lg:grid-cols-4">
        {Array.from({ length: 4 }, (_, i) => (
          <Skeleton key={i} className="h-24" />
        ))}
      </div>
      <Skeleton className="h-28" />
      <Skeleton className="h-64" />
    </div>
  )
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <ThemeProvider>
      <TooltipProvider>
        <App />
      </TooltipProvider>
    </ThemeProvider>
  </React.StrictMode>
)
