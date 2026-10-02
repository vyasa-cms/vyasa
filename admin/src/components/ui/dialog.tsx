import * as React from "react";
import { createPortal } from "react-dom";
import { AlertTriangle, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/**
 * Modal shell: focus trap, Escape to close, scroll lock, click-outside.
 * Rendered into a portal so it escapes any transformed/overflow ancestor.
 */
export function Modal({
  open,
  onClose,
  title,
  description,
  children,
  footer,
  size = "md",
  testId,
}: {
  open: boolean;
  onClose: () => void;
  title: React.ReactNode;
  description?: React.ReactNode;
  children?: React.ReactNode;
  footer?: React.ReactNode;
  size?: "sm" | "md" | "lg";
  testId?: string;
}) {
  const panelRef = React.useRef<HTMLDivElement>(null);
  const titleId = React.useId();
  const descId = React.useId();

  // Read through a ref so a parent passing a fresh `onClose` each render does
  // not re-run the effect below, which would bounce focus out and back in.
  const onCloseRef = React.useRef(onClose);
  onCloseRef.current = onClose;

  React.useEffect(() => {
    if (!open) return;
    const previous = document.activeElement as HTMLElement | null;
    const { overflow } = document.body.style;
    document.body.style.overflow = "hidden";

    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onCloseRef.current();
        return;
      }
      if (e.key !== "Tab" || panelRef.current === null) return;
      const focusables = panelRef.current.querySelectorAll<HTMLElement>(
        'a[href],button:not([disabled]),textarea,input,select,[tabindex]:not([tabindex="-1"])',
      );
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      if (first === undefined || last === undefined) return;
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    };

    document.addEventListener("keydown", onKeyDown, true);
    // Focus the panel so screen readers announce the dialog immediately —
    // unless focus is already inside it (someone started typing in a field).
    const raf = requestAnimationFrame(() => {
      const panel = panelRef.current;
      if (panel !== null && !panel.contains(document.activeElement)) panel.focus();
    });
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      cancelAnimationFrame(raf);
      document.body.style.overflow = overflow;
      previous?.focus?.();
    };
  }, [open]);

  if (!open || typeof document === "undefined") return null;

  const width =
    size === "sm" ? "sm:max-w-sm" : size === "lg" ? "sm:max-w-2xl" : "sm:max-w-md";

  return createPortal(
    <div className="fixed inset-0 z-50 flex items-end justify-center sm:items-center">
      <button
        type="button"
        aria-label="Close dialog"
        onClick={onClose}
        className="absolute inset-0 animate-fade-in bg-black/50 backdrop-blur-[2px]"
      />
      <div
        ref={panelRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={description === undefined ? undefined : descId}
        tabIndex={-1}
        data-testid={testId}
        className={cn(
          "relative z-10 flex max-h-[92dvh] w-full flex-col overflow-hidden rounded-t-xl border bg-popover text-popover-foreground shadow-2xl outline-none",
          "animate-scale-in sm:rounded-xl",
          width,
        )}
      >
        <div className="flex items-start gap-3 border-b px-5 py-4">
          <div className="min-w-0 flex-1">
            <h2 id={titleId} className="text-base font-semibold leading-6">
              {title}
            </h2>
            {description !== undefined ? (
              <p id={descId} className="mt-1 text-sm text-muted-foreground">
                {description}
              </p>
            ) : null}
          </div>
          <button
            type="button"
            onClick={onClose}
            aria-label="Close"
            className="-mr-1 -mt-1 inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-foreground"
          >
            <X className="h-4 w-4" aria-hidden="true" />
          </button>
        </div>

        {children !== undefined ? (
          <div className="min-h-0 flex-1 overflow-y-auto px-5 py-4">{children}</div>
        ) : null}

        {footer !== undefined ? (
          <div className="flex flex-col-reverse gap-2 border-t bg-muted/40 px-5 py-3 sm:flex-row sm:justify-end">
            {footer}
          </div>
        ) : null}
      </div>
    </div>,
    document.body,
  );
}

interface ConfirmOptions {
  title: string;
  /** What will happen, in the user's terms. */
  description?: React.ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Red confirm button + warning icon. */
  destructive?: boolean;
  /** Require typing this exact string first (for genuinely unrecoverable acts). */
  requireTyped?: string;
}

type Resolver = (confirmed: boolean) => void;

const ConfirmContext = React.createContext<
  ((options: ConfirmOptions) => Promise<boolean>) | null
>(null);

/**
 * Provides `useConfirm()`. Mount once near the root; every destructive action
 * in the admin goes through it rather than firing on click.
 */
export function ConfirmProvider({ children }: { children: React.ReactNode }) {
  const [state, setState] = React.useState<
    (ConfirmOptions & { resolve: Resolver }) | null
  >(null);
  const [typed, setTyped] = React.useState("");

  const confirm = React.useCallback(
    (options: ConfirmOptions) =>
      new Promise<boolean>((resolve) => {
        setTyped("");
        setState({ ...options, resolve });
      }),
    [],
  );

  const settle = React.useCallback(
    (result: boolean) => {
      state?.resolve(result);
      setState(null);
    },
    [state],
  );

  const gateOk =
    state?.requireTyped === undefined || typed.trim() === state.requireTyped;

  return (
    <ConfirmContext.Provider value={confirm}>
      {children}
      <Modal
        open={state !== null}
        onClose={() => settle(false)}
        testId="confirm-dialog"
        title={
          <span className="flex items-center gap-2">
            {state?.destructive === true ? (
              <AlertTriangle
                className="h-4 w-4 shrink-0 text-destructive"
                aria-hidden="true"
              />
            ) : null}
            {state?.title ?? ""}
          </span>
        }
        description={state?.description}
        footer={
          <>
            <Button variant="outline" onClick={() => settle(false)}>
              {state?.cancelLabel ?? "Cancel"}
            </Button>
            <Button
              variant={state?.destructive === true ? "destructive" : "default"}
              disabled={!gateOk}
              onClick={() => settle(true)}
              data-testid="confirm-accept"
            >
              {state?.confirmLabel ?? "Confirm"}
            </Button>
          </>
        }
      >
        {state?.requireTyped !== undefined ? (
          <label className="flex flex-col gap-1.5 text-sm">
            <span>
              Type <code className="font-mono font-semibold">{state.requireTyped}</code>{" "}
              to confirm
            </span>
            <input
              autoFocus
              value={typed}
              onChange={(e) => setTyped(e.target.value)}
              className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            />
          </label>
        ) : null}
      </Modal>
    </ConfirmContext.Provider>
  );
}

/**
 * Returns an async `confirm(options)`. Resolves true only when the user
 * accepts, so callers read as `if (await confirm({...})) { ...mutate }`.
 */
export function useConfirm(): (options: ConfirmOptions) => Promise<boolean> {
  const ctx = React.useContext(ConfirmContext);
  // Outside a provider (tests mounting a page in isolation) nothing should
  // silently delete — resolve false rather than throwing.
  return ctx ?? (() => Promise.resolve(false));
}
