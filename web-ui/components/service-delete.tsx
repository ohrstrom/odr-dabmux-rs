import * as React from "react"
import { toast } from "sonner"
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
import { announce } from "@/components/service-form"
import { serviceKey, type Service } from "@/lib/config"
import { deleteService } from "@/lib/ui-api"

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
  const [deleting, setDeleting] = React.useState(false)

  async function remove() {
    setDeleting(true)
    try {
      const result = await deleteService(serviceKey(service))
      announce(result, `${service.label} deleted`)
      if (result.removed_subchannels.length) {
        toast.info(
          `Removed the shared subchannels ${result.removed_subchannels.join(", ")}, which no other service used.`
        )
      }
      onOpenChange(false)
      onDeleted()
    } catch (e) {
      toast.error(`Could not delete ${service.label}`, {
        description: e instanceof Error ? e.message : String(e),
      })
    } finally {
      setDeleting(false)
    }
  }

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>Delete {service.label}?</AlertDialogTitle>
          <AlertDialogDescription>
            The service goes off air with its components and their
            subchannels. Subchannels allocated after it may get new SubChIds.
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <Button variant="destructive" disabled={deleting} onClick={remove}>
            {deleting ? "Deleting…" : "Delete"}
          </Button>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
