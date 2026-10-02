import { Toaster as SonnerToaster, toast } from "sonner";
import { useTheme } from "@/lib/theme";
import { errorSummary } from "@/lib/error-text";

/**
 * Toast region. Mounted once in the app shell; bottom-centre on phones so it
 * clears the nav, bottom-right from `sm` up.
 */
export function Toaster() {
  const { resolved } = useTheme();
  return (
    <SonnerToaster
      theme={resolved}
      position="bottom-right"
      closeButton
      richColors={false}
      toastOptions={{
        classNames: {
          toast:
            "group rounded-lg border border-border bg-popover text-popover-foreground shadow-lg",
          description: "text-muted-foreground",
          actionButton:
            "rounded-md bg-primary px-2.5 py-1 text-xs font-medium text-primary-foreground",
          cancelButton:
            "rounded-md bg-muted px-2.5 py-1 text-xs font-medium text-muted-foreground",
          error: "border-destructive/40",
          success: "border-success/40",
        },
      }}
    />
  );
}

function message(error: unknown): string {
  // A toast is gone in seconds; anything past a sentence or two of it was
  // never going to be read. The full text stays on whatever panel raised it.
  return errorSummary(error).summary;
}

/**
 * The admin's feedback vocabulary. Every mutation reports through one of
 * these so success and failure look the same everywhere.
 */
export const notify = {
  success: (title: string, description?: string) =>
    toast.success(title, { description }),

  error: (title: string, error?: unknown) =>
    toast.error(title, {
      description: error === undefined ? undefined : message(error),
      duration: 6000,
    }),

  info: (title: string, description?: string) => toast(title, { description }),

  /** Reversible action: report it, and offer the way back. */
  undo: (title: string, onUndo: () => void, description?: string) =>
    toast.success(title, {
      description,
      duration: 8000,
      action: { label: "Undo", onClick: onUndo },
    }),

  /** Wraps a promise so pending / done / failed all surface without ceremony. */
  promise: <T,>(
    promise: Promise<T>,
    messages: { loading: string; success: string; error: string },
  ) =>
    toast.promise(promise, {
      loading: messages.loading,
      success: messages.success,
      error: (e: unknown) => `${messages.error}: ${message(e)}`,
    }),
};
