import { cn } from "@/lib/utils";

/**
 * The Vyasa mark: a solid half-disc resolving into lines of text.
 *
 * Geometry mirrors `assets/mark.svg` at the repository root, which is the
 * source of truth for the brand. Kept inline rather than loaded as an image
 * so it inherits `currentColor` and needs no extra request.
 */
export function Mark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 64 64"
      fill="currentColor"
      aria-hidden="true"
      focusable="false"
      className={cn("h-6 w-6", className)}
    >
      <path d="M31 5 A27 27 0 0 0 31 59 Z" />
      <rect x="36" y="14" width="25" height="5.5" rx="2.75" />
      <rect x="36" y="24.5" width="25" height="5.5" rx="2.75" />
      <rect x="36" y="35" width="21" height="5.5" rx="2.75" />
      <rect x="36" y="45.5" width="13" height="5.5" rx="2.75" />
    </svg>
  );
}

/**
 * Mark plus wordmark, matching the lockup used on the repository banner.
 *
 * The wordmark is a serif to echo the Palatino of the banner; the system
 * serif stack is close enough in spirit and costs no webfont.
 */
export function Logo({
  className,
  markClassName,
}: {
  className?: string;
  markClassName?: string;
}) {
  return (
    <span className={cn("flex items-center gap-2", className)}>
      <Mark className={cn("text-primary", markClassName)} />
      <span className="font-serif text-[15px] font-semibold tracking-tight">
        Vyasa
      </span>
    </span>
  );
}
