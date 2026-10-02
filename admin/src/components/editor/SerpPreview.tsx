/**
 * How this entry will look where it is actually found: a search result
 * and a social card, drawn from the same fields the public head emits
 * (title, excerpt-as-description, featured image). Pure presentation —
 * nothing here talks to the server.
 */
export function SerpPreview({
  title,
  description,
  path,
  imageUrl,
  siteName,
}: {
  title: string;
  description: string;
  path: string;
  imageUrl?: string | null;
  siteName?: string;
}) {
  const origin = typeof window === "undefined" ? "" : window.location.origin;
  const host = origin.replace(/^https?:\/\//, "");
  const shownTitle = title.trim() === "" ? "Untitled" : title;
  const shownDesc =
    description.trim() === ""
      ? "No excerpt yet — search engines will pick their own snippet."
      : description;
  return (
    <div className="space-y-3 text-sm" data-testid="serp-preview">
      <div className="rounded border p-3">
        <p className="truncate text-xs text-muted-foreground">
          {host}
          {path}
        </p>
        <p className="truncate text-base text-[#1a0dab] dark:text-[#8ab4f8]">{shownTitle}</p>
        <p className="line-clamp-2 text-xs text-muted-foreground">{shownDesc}</p>
        <Budget label="Title" value={shownTitle.length} max={60} />
        <Budget label="Description" value={description.trim().length} max={160} />
      </div>
      <div className="overflow-hidden rounded border">
        {imageUrl ? (
          <img src={imageUrl} alt="Social preview" className="max-h-40 w-full object-cover" />
        ) : (
          <div className="flex h-24 items-center justify-center bg-muted text-xs text-muted-foreground">
            No featured image — links will unfurl without a picture
          </div>
        )}
        <div className="border-t p-2">
          <p className="truncate text-xs uppercase text-muted-foreground">{host}</p>
          <p className="truncate text-sm font-medium">{shownTitle}</p>
          <p className="truncate text-xs text-muted-foreground">
            {siteName ?? ""}
          </p>
        </div>
      </div>
    </div>
  );
}

/** A character budget with a quiet warning past it. */
function Budget({ label, value, max }: { label: string; value: number; max: number }) {
  const over = value > max;
  return (
    <p className={over ? "mt-1 text-xs text-destructive" : "mt-1 text-xs text-muted-foreground"}>
      {label}: {value}/{max}
      {over ? " — will be cut off in results" : ""}
    </p>
  );
}
