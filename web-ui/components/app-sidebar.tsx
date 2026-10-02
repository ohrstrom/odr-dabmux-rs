import type * as React from "react"
import {
  BroadcastIcon,
  ExportIcon,
  LinkSimpleIcon,
  RadioIcon,
  SquaresFourIcon,
  StackIcon,
} from "@phosphor-icons/react"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  SidebarRail,
} from "@/components/ui/sidebar"
import { serviceKey, type ResolvedConfig } from "@/lib/config"
import { href } from "@/hooks/use-route"

export type Page =
  | "overview"
  | "services"
  | "subchannels"
  | "service-following"
  | "output"

export const PAGES: Record<
  Page,
  { title: string; icon: React.ComponentType }
> = {
  overview: { title: "Overview", icon: SquaresFourIcon },
  services: { title: "Services", icon: RadioIcon },
  subchannels: { title: "Subchannels", icon: StackIcon },
  "service-following": { title: "Service following", icon: LinkSimpleIcon },
  output: { title: "Output", icon: ExportIcon },
}

export function AppSidebar({
  config,
  route,
  status,
}: {
  config: ResolvedConfig | null
  route: string[]
  status: React.ReactNode
}) {
  const page = (route[0] ?? "overview") as Page
  const counts: Partial<Record<Page, number>> = config
    ? {
        services: config.services.length,
        subchannels: config.subchannels.length,
        output: config.output.destinations.length,
      }
    : {}

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg" render={<a href={href()} />}>
              <div className="flex aspect-square size-8 items-center justify-center bg-sidebar-primary text-sidebar-primary-foreground">
                <BroadcastIcon className="size-4" />
              </div>
              <div className="grid flex-1 text-left leading-tight">
                <span className="truncate font-semibold">ODR-DabMux</span>
                <span className="truncate text-xs text-muted-foreground">
                  {config?.ensemble.label ?? "…"}
                </span>
              </div>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupLabel>Configuration</SidebarGroupLabel>
          <SidebarGroupContent>
            <SidebarMenu>
              {(Object.keys(PAGES) as Page[]).map((key) => {
                const { title, icon: Icon } = PAGES[key]
                const count = counts[key]
                return (
                  <SidebarMenuItem key={key}>
                    <SidebarMenuButton
                      isActive={page === key && route.length <= 1}
                      tooltip={title}
                      render={<a href={key === "overview" ? href() : href(key)} />}
                    >
                      <Icon />
                      <span>{title}</span>
                    </SidebarMenuButton>
                    {count !== undefined && (
                      <SidebarMenuBadge>{count}</SidebarMenuBadge>
                    )}
                    {key === "services" && config && (
                      <SidebarMenuSub>
                        {config.services.map((service) => (
                          <SidebarMenuSubItem key={service.id}>
                            <SidebarMenuSubButton
                              href={href("services", serviceKey(service))}
                              isActive={
                                page === "services" &&
                                route[1] === serviceKey(service)
                              }
                            >
                              <span>{service.label}</span>
                            </SidebarMenuSubButton>
                          </SidebarMenuSubItem>
                        ))}
                      </SidebarMenuSub>
                    )}
                  </SidebarMenuItem>
                )
              })}
            </SidebarMenu>
          </SidebarGroupContent>
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter>{status}</SidebarFooter>
      <SidebarRail />
    </Sidebar>
  )
}
