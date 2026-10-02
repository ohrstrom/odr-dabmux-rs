import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Button } from "@/components/ui/button"
import { useDraft } from "@/hooks/use-draft"
import type { Service } from "@/lib/config"
import type { OperatorConfig } from "@/lib/ui-api"

/** Remove service `id`, and the shared subchannels only it used. */
function removeService(config: OperatorConfig, id: number) {
  const index = config.services.findIndex((s) => s.id === id)
  if (index < 0) return
  const [removed] = config.services.splice(index, 1)
  const used = new Set(
    config.services.flatMap((s) => s.components.map((c) => c.subchannel))
  )
  for (const component of removed!.components) {
    if (component.subchannel && !used.has(component.subchannel)) {
      delete config.subchannels?.[component.subchannel]
    }
  }
}

export function ServiceDeleteDialog({
  service,
  open,
  onOpenChange,
  onDeleted,
}: {
  service: Service
  open: boolean
  onOpenChange: (open: boolean) => void
  onDeleted: () => void
}) {
  const draft = useDraft()

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete {service.label}?</AlertDialogTitle>
          <AlertDialogDescription>
            When the changes are applied, the service goes off air with its
            components and their subchannels, and subchannels allocated after
            it may get new SubChIds.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <Button
            variant="destructive"
            onClick={() => {
              draft.update((config) => removeService(config, service.id))
              onOpenChange(false)
              onDeleted()
            }}
          >
            Delete
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
