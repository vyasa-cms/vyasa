import { useMe } from "@/components/auth";
import { useCapabilities } from "@/lib/capabilities";
import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  Copy,
  ExternalLink,
  FileText,
  Image as ImageIcon,
  Music,
  Replace,
  Search,
  Trash2,
  Upload,
  Video,
} from "lucide-react";
import { api, type MediaResponse } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, ErrorNote, Field, PageHeader, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { Input } from "@/components/ui/input";
import { formatRelative } from "@/routes/_auth/index";
import { cn } from "@/lib/utils";
import { isImageFile, prepareImage } from "@/components/editor/prepareImage";

export const Route = createFileRoute("/_auth/media/")({
  component: MediaPage,
});

const PER_PAGE = 24;

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function isImage(m: MediaResponse): boolean {
  return m.mime.startsWith("image/");
}

function needsAlt(m: MediaResponse): boolean {
  return isImage(m) && (m.alt ?? "") === "";
}

/** The picture a tile shows: the 320px derivative once it exists. */
export function thumbUrl(m: MediaResponse): string {
  const has = m.derivatives !== null && typeof m.derivatives === "object" && "thumb" in (m.derivatives as object);
  return `/api/v1/media/${m.id}/raw${has ? "?variant=thumb" : ""}`;
}

type Kind = "" | "image" | "video" | "audio" | "document" | "other" | "trash";
type Sort = "newest" | "oldest" | "largest" | "smallest" | "name";

function MediaPage() {
  const caps = useCapabilities();
  const me = useMe();
  const owner = caps.can("edit_others") ? undefined : me.data?.id;
  // Reads accept `upload_media` or `edit_posts` (the page is visible either
  // way), but every write route — upload, delete, replace, edit, alt text,
  // restore, transcribe — is `upload_media` only.
  const canWrite = caps.can("upload_media");
  const canEmptyTrash = caps.can("edit_others");
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [page, setPage] = React.useState(1);
  const [search, setSearch] = React.useState("");
  const [query, setQuery] = React.useState("");
  const [kind, setKind] = React.useState<Kind>("");
  const [sort, setSort] = React.useState<Sort>("newest");
  const [selected, setSelected] = React.useState<Set<string>>(new Set());
  const [lastPicked, setLastPicked] = React.useState<string | null>(null);
  const [detail, setDetail] = React.useState<MediaResponse | null>(null);
  const [dragDepth, setDragDepth] = React.useState(0);
  const [onlyMissingAlt, setOnlyMissingAlt] = React.useState(false);
  const [uploading, setUploading] = React.useState<{ name: string; done: number; total: number } | null>(null);
  const inputRef = React.useRef<HTMLInputElement>(null);

  // Typing settles for a moment before it becomes a request.
  React.useEffect(() => {
    const t = setTimeout(() => {
      setQuery(search.trim());
      setPage(1);
    }, 250);
    return () => clearTimeout(t);
  }, [search]);

  const media = useQuery({
    queryKey: ["media", "page", page, query, kind, sort, owner],
    enabled: caps.isSuccess && !!me.data,
    queryFn: () =>
      api.listMediaPage({
        limit: PER_PAGE,
        owner_id: owner,
        offset: (page - 1) * PER_PAGE,
        ...(query !== "" ? { search: query } : {}),
        ...(kind !== "" && kind !== "trash" ? { kind } : {}),
        ...(kind === "trash" ? { trashed: true } : {}),
        ...(sort !== "newest" ? { sort } : {}),
      }),
    placeholderData: (prev) => prev,
  });
  const stats = useQuery({ queryKey: ["media", "stats"], queryFn: api.mediaStats, staleTime: 30_000 });

  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["media"] });

  const upload = useMutation({
    mutationFn: async (files: File[]) => {
      let duplicates = 0;
      let done = 0;
      // Sequential: a phone on a slow connection shouldn't open six
      // parallel uploads and time all of them out.
      for (const file of files) {
        setUploading({ name: file.name, done, total: files.length });
        const ready = isImageFile(file) ? await prepareImage(file) : file;
        const result = await api.uploadMediaDedup(ready);
        if (result.duplicate) duplicates += 1;
        done += 1;
      }
      setUploading(null);
      return { count: files.length, duplicates };
    },
    onSuccess: ({ count, duplicates }) => {
      invalidate();
      const added = count - duplicates;
      notify.success(
        added === 0
          ? "Already in the library"
          : added === 1 && count === 1
            ? "Uploaded 1 file"
            : `Uploaded ${added} file${added === 1 ? "" : "s"}`,
        duplicates > 0 ? `${duplicates} ${duplicates === 1 ? "was" : "were"} already here; the existing copy is used.` : undefined,
      );
    },
    onError: (e) => {
      setUploading(null);
      invalidate();
      notify.error("Upload failed", e);
    },
  });

  const remove = useMutation({
    mutationFn: (ids: string[]) => api.batchDeleteMedia(ids),
    onSuccess: (r) => {
      setSelected(new Set());
      setDetail(null);
      invalidate();
      if (r.failed.length === 0) {
        notify.success(r.deleted.length === 1 ? "File deleted" : `${r.deleted.length} files deleted`);
      } else {
        notify.error(
          `${r.deleted.length} deleted, ${r.failed.length} could not be`,
          r.failed.map((f) => f.message).join("; "),
        );
      }
    },
    onError: (e) => notify.error("Couldn't delete", e),
  });

  const accept = (files: FileList | null) => {
    if (!canWrite) return;
    const list = files === null ? [] : Array.from(files);
    if (list.length > 0) upload.mutate(list);
  };

  React.useEffect(() => {
    if (!canWrite) return undefined;
    const onPaste = (e: ClipboardEvent) => {
      const files = Array.from(e.clipboardData?.files ?? []);
      if (files.length > 0) {
        e.preventDefault();
        upload.mutate(files);
      }
    };
    window.addEventListener("paste", onPaste);
    return () => window.removeEventListener("paste", onPaste);
  }, [upload, canWrite]);

  const restoreMany = useMutation({
    mutationFn: (ids: string[]) => Promise.all(ids.map((id) => api.restoreMedia(id))),
    onSuccess: (rows) => { setSelected(new Set()); invalidate(); notify.success(`${rows.length} ${rows.length === 1 ? "file" : "files"} restored`); },
    onError: (e) => notify.error("Couldn't restore", e),
  });
  const emptyTrash = useMutation({
    mutationFn: () => api.emptyMediaTrash(),
    onSuccess: (r) => { invalidate(); notify.success("Trash emptied", `${r.purged} ${r.purged === 1 ? "file" : "files"} removed for good.`); },
    onError: (e) => notify.error("Couldn't empty the trash", e),
  });
  const askDelete = async (items: MediaResponse[]) => {
    const one = items.length === 1 ? items[0] : undefined;
    // What would break: checked before the question is asked.
    const usages = await Promise.all(items.slice(0, 20).map((m) => api.mediaUsage(m.id).catch(() => null)));
    const inPosts = usages.reduce((n, u) => n + (u?.posts.length ?? 0), 0);
    const identity = usages.some((u) => u?.site_logo || u?.site_favicon);
    const warnings: string[] = [];
    if (inPosts > 0) warnings.push(`Used in ${inPosts} post${inPosts === 1 ? "" : "s"}, which will show a broken image.`);
    if (identity) warnings.push("It is the site logo or favicon.");
    const ok = await confirm({
      title: one !== undefined ? `Delete ${one.file_name}?` : `Delete ${items.length} files?`,
      description:
        warnings.length > 0
          ? `${warnings.join(" ")} This can't be undone.`
          : "Nothing on the site uses it. This can't be undone.",
      confirmLabel: "Delete",
      destructive: true,
    });
    if (ok) remove.mutate(items.map((i) => i.id));
  };

  const all = media.data?.items ?? [];
  const total = media.data?.total ?? 0;
  const pages = Math.max(1, Math.ceil(total / PER_PAGE));
  const items = onlyMissingAlt ? all.filter(needsAlt) : all;
  const byId = new Map(all.map((m) => [m.id, m]));

  const toggle = (id: string, shift: boolean) => {
    const next = new Set(selected);
    if (shift && lastPicked !== null) {
      const ids = items.map((m) => m.id);
      const a = ids.indexOf(lastPicked);
      const b = ids.indexOf(id);
      if (a !== -1 && b !== -1) {
        for (const x of ids.slice(Math.min(a, b), Math.max(a, b) + 1)) next.add(x);
        setSelected(next);
        return;
      }
    }
    if (next.has(id)) next.delete(id);
    else next.add(id);
    setSelected(next);
    setLastPicked(id);
  };
  const allOnPageSelected = items.length > 0 && items.every((m) => selected.has(m.id));

  const capPct =
    stats.data?.cap_bytes && stats.data.cap_bytes > 0 ? Math.round((stats.data.bytes / stats.data.cap_bytes) * 100) : null;

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Media"
        description={
          stats.data
            ? `${stats.data.count} ${stats.data.count === 1 ? "file" : "files"} · ${formatBytes(stats.data.bytes)}${
                capPct !== null ? ` · ${capPct}% of the ${formatBytes(stats.data.cap_bytes ?? 0)} cap` : ""
              }`
            : "Loading…"
        }
        actions={
          canWrite ? (
            <Button size="sm" onClick={() => inputRef.current?.click()}>
              <Upload className="h-4 w-4" aria-hidden="true" />
              Upload
            </Button>
          ) : undefined
        }
      />

      {canWrite ? (
        <>
          <input
            ref={inputRef}
            type="file"
            multiple
            accept="image/*,.heic,.heif,video/*,audio/*,.pdf"
            aria-label="Choose files to upload"
            className="sr-only"
            onChange={(e) => {
              accept(e.target.files);
              e.target.value = "";
            }}
          />

          <div
            onDragEnter={(e) => {
              e.preventDefault();
              setDragDepth((d) => d + 1);
            }}
            onDragOver={(e) => e.preventDefault()}
            onDragLeave={() => setDragDepth((d) => Math.max(0, d - 1))}
            onDrop={(e) => {
              e.preventDefault();
              setDragDepth(0);
              accept(e.dataTransfer.files);
            }}
            className={cn(
              "rounded-lg border-2 border-dashed px-4 py-5 text-center text-sm transition-colors",
              dragDepth > 0 ? "border-primary bg-primary-subtle text-primary" : "border-input bg-muted/40 text-muted-foreground",
            )}
            data-testid="drop-zone"
          >
            {uploading !== null ? (
              <span className="flex items-center justify-center gap-2">
                <span className="h-3.5 w-3.5 animate-spin rounded-full border-2 border-muted border-t-primary" />
                Uploading {uploading.name}
                {uploading.total > 1 ? ` (${uploading.done + 1} of ${uploading.total})` : ""}…
              </span>
            ) : (
              <>
                Drop files here, paste from your clipboard, or{" "}
                <button type="button" onClick={() => inputRef.current?.click()} className="font-medium text-primary underline underline-offset-2">
                  browse
                </button>
                . Phone photos are shrunk to fit and HEIC is converted where the browser can.
              </>
            )}
          </div>
        </>
      ) : null}

      {kind === "trash" ? (
        <p className="flex flex-wrap items-center gap-3 rounded-md border border-warning/40 bg-warning-subtle px-3 py-2 text-sm" data-testid="media-trash-note">
          <span>Files in the trash are hidden from the library and from stats; posts that embed them still show them until they are removed for good.</span>
          {canEmptyTrash ? (
            <Button size="sm" variant="outline" className="h-7" disabled={emptyTrash.isPending} onClick={async () => { if (await confirm({ title: "Empty the trash?", description: "Every file in it is deleted permanently.", confirmLabel: "Empty trash", destructive: true })) emptyTrash.mutate(); }}>Empty trash</Button>
          ) : null}
        </p>
      ) : null}
      <div className="flex flex-wrap items-center gap-2" data-testid="media-filters">
        <div className="relative min-w-[12rem] flex-1">
          <Search className="pointer-events-none absolute left-2 top-1/2 h-4 w-4 -translate-y-1/2 text-muted-foreground" aria-hidden="true" />
          <Input value={search} onChange={(e) => setSearch(e.target.value)} placeholder="Search by name, alt text or caption" aria-label="Search media" className="h-9 pl-8" />
        </div>
        <select
          value={kind}
          onChange={(e) => {
            setKind(e.target.value as Kind);
            setPage(1);
          }}
          aria-label="Kind"
          className="h-9 rounded-md border border-input bg-background px-2 text-sm"
        >
          <option value="">All kinds</option>
          <option value="trash">Trash</option>
          <option value="image">Images</option>
          <option value="video">Video</option>
          <option value="audio">Audio</option>
          <option value="document">Documents</option>
          <option value="other">Other</option>
        </select>
        <select
          value={sort}
          onChange={(e) => {
            setSort(e.target.value as Sort);
            setPage(1);
          }}
          aria-label="Sort"
          className="h-9 rounded-md border border-input bg-background px-2 text-sm"
        >
          <option value="newest">Newest first</option>
          <option value="oldest">Oldest first</option>
          <option value="largest">Largest first</option>
          <option value="smallest">Smallest first</option>
          <option value="name">By name</option>
        </select>
      </div>

      {stats.data && stats.data.missing_alt > 0 ? (
        <button
          type="button"
          onClick={() => setOnlyMissingAlt((v) => !v)}
          className={cn(
            "flex w-full items-center gap-2 rounded-lg border px-3 py-2 text-left text-sm transition-colors",
            onlyMissingAlt ? "border-warning bg-warning-subtle text-warning" : "border-warning/40 bg-warning-subtle/60 text-warning hover:bg-warning-subtle",
          )}
          data-testid="missing-alt-banner"
        >
          <AlertTriangle className="h-4 w-4 shrink-0" aria-hidden="true" />
          <span className="min-w-0 flex-1">
            {stats.data.missing_alt} {stats.data.missing_alt === 1 ? "image has" : "images have"} no alt text in the library.
          </span>
          <span className="shrink-0 text-xs font-medium underline underline-offset-2">{onlyMissingAlt ? "Show all on this page" : "Show these on this page"}</span>
        </button>
      ) : null}

      {selected.size > 0 ? (
        <div className="flex flex-wrap items-center gap-2 rounded-lg border bg-primary-subtle px-3 py-2 text-sm text-primary" data-testid="selection-bar">
          <span className="font-medium">{selected.size} selected</span>
          {kind === "trash" && canWrite ? (
            <Button size="sm" variant="outline" className="h-7" onClick={() => restoreMany.mutate([...selected])}>Restore</Button>
          ) : null}
          {canWrite ? (
            <Button size="sm" variant="outline" className="h-7" onClick={() => void askDelete([...selected].map((id) => byId.get(id)).filter((m): m is MediaResponse => m !== undefined))}>
              <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
              Delete
            </Button>
          ) : null}
          {canWrite ? (
            <BulkAlt ids={[...selected].filter((id) => { const m = byId.get(id); return m !== undefined && needsAlt(m); })} onDone={invalidate} />
          ) : null}
          <Button variant="ghost" size="sm" className="ml-auto h-7 text-primary hover:bg-primary/10" onClick={() => setSelected(new Set())}>
            Clear
          </Button>
        </div>
      ) : null}

      {media.isError ? (
        <ErrorNote title="Couldn't load your media" error={media.error} />
      ) : media.isPending ? (
        <div className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5">
          {Array.from({ length: 10 }, (_, i) => (
            <Skeleton key={i} className="aspect-[4/3] w-full" />
          ))}
        </div>
      ) : items.length === 0 ? (
        <EmptyState
          icon={ImageIcon}
          title={onlyMissingAlt ? "Every image on this page is described" : query !== "" || kind !== "" ? "Nothing matches" : "No files yet"}
          description={onlyMissingAlt ? "Nothing on this page is missing alt text." : query !== "" || kind !== "" ? "Try another search or kind." : "Drop an image above to add your first file."}
        />
      ) : (
        <>
          <label className="flex items-center gap-2 text-xs text-muted-foreground">
            <input type="checkbox" checked={allOnPageSelected} onChange={(e) => setSelected(e.target.checked ? new Set([...selected, ...items.map((m) => m.id)]) : new Set([...selected].filter((id) => !items.some((m) => m.id === id))))} className="h-3.5 w-3.5 accent-primary" aria-label="Select all on this page" />
            Select all on this page · shift-click a tile for a range
          </label>
          <ul className="grid grid-cols-2 gap-3 sm:grid-cols-3 lg:grid-cols-5" data-testid="media-grid">
            {items.map((m) => {
              const isSelected = selected.has(m.id);
              return (
                <li key={m.id} className={cn("group relative overflow-hidden rounded-lg border bg-card transition-colors", isSelected ? "border-primary ring-1 ring-primary" : "hover:border-input")}>
                  <label className="absolute left-2 top-2 z-10 flex h-6 w-6 cursor-pointer items-center justify-center rounded-md bg-background/90 shadow-sm" onClick={(e) => { if (e.shiftKey) { e.preventDefault(); toggle(m.id, true); } }}>
                    <span className="sr-only">Select {m.file_name}</span>
                    <input type="checkbox" checked={isSelected} onChange={() => toggle(m.id, false)} className="h-3.5 w-3.5 accent-primary" />
                  </label>
                  <button type="button" onClick={() => setDetail(m)} className="block w-full text-left">
                    <span className="flex aspect-[4/3] items-center justify-center overflow-hidden bg-muted">
                      {isImage(m) ? (
                        <img src={thumbUrl(m)} alt={m.alt ?? ""} loading="lazy" className="h-full w-full object-cover" style={m.focal_x != null && m.focal_y != null ? { objectPosition: `${Math.round(m.focal_x * 100)}% ${Math.round(m.focal_y * 100)}%` } : undefined} />
                      ) : (
                        <KindIcon m={m} />
                      )}
                    </span>
                    <span className="block p-2">
                      <span className="block truncate font-mono text-[11px]">{m.file_name}</span>
                      <span className="mt-0.5 block text-[10px] text-muted-foreground">
                        {formatBytes(m.byte_size)}
                        {m.width && m.height ? ` · ${m.width}×${m.height}` : ""}
                      </span>
                      {needsAlt(m) ? (
                        <span className="mt-1 inline-flex items-center gap-1 text-[10px] font-medium text-warning">
                          <AlertTriangle className="h-3 w-3" aria-hidden="true" />
                          No alt text
                        </span>
                      ) : null}
                    </span>
                  </button>
                </li>
              );
            })}
          </ul>
        </>
      )}

      {!media.isPending && total > 0 ? (
        <div className="flex items-center justify-between gap-2">
          <span className="text-xs text-muted-foreground" data-testid="media-pagination">
            Page {page} of {pages} · {total} {total === 1 ? "file" : "files"}
            {query !== "" || kind !== "" ? " match" : ""}
          </span>
          <div className="flex gap-2">
            <Button variant="outline" size="sm" disabled={page <= 1} onClick={() => setPage((p) => Math.max(1, p - 1))}>
              Previous
            </Button>
            <Button variant="outline" size="sm" disabled={page >= pages} onClick={() => setPage((p) => p + 1)}>
              Next
            </Button>
          </div>
        </div>
      ) : null}

      <MediaDetail
        item={detail}
        canWrite={canWrite}
        onChange={(m) => {
          setDetail(m);
          invalidate();
        }}
        onClose={() => setDetail(null)}
        onDelete={() => {
          if (detail !== null) void askDelete([detail]);
        }}
      />
    </div>
  );
}

function KindIcon({ m }: { m: MediaResponse }) {
  const Icon = m.mime.startsWith("video/") ? Video : m.mime.startsWith("audio/") ? Music : FileText;
  return (
    <span className="flex flex-col items-center gap-1 text-muted-foreground">
      <Icon className="h-8 w-8" aria-hidden="true" />
      <span className="font-mono text-[10px]">{m.mime.split("/")[1] ?? m.mime}</span>
    </span>
  );
}

/** Writes alt text for every selected image that lacks it, one call each. */
function BulkAlt({ ids, onDone }: { ids: string[]; onDone: () => void }) {
  const run = useMutation({
    mutationFn: async () => {
      let done = 0;
      for (const id of ids) {
        try {
          const r = await api.generateAltText(id);
          if (r.alt) done += 1;
        } catch {
          // One failure does not stop the rest; the count says how many landed.
        }
      }
      return done;
    },
    onSuccess: (done) => {
      onDone();
      notify.success(`Alt text written for ${done} of ${ids.length}`, "Open each to edit it if it isn't quite right.");
    },
  });
  if (ids.length === 0) return null;
  return (
    <Button size="sm" variant="outline" className="h-7" disabled={run.isPending} onClick={() => run.mutate()} data-testid="bulk-alt">
      {run.isPending ? "Writing…" : `Write alt text for ${ids.length}`}
    </Button>
  );
}

function MediaDetail({
  item,
  canWrite,
  onChange,
  onClose,
  onDelete,
}: {
  item: MediaResponse | null;
  /** Whether the signed-in user holds `upload_media`; editing is read-only without it. */
  canWrite: boolean;
  onChange: (m: MediaResponse) => void;
  onClose: () => void;
  onDelete: () => void;
}) {
  const queryClient = useQueryClient();
  const url = item === null ? "" : `/api/v1/media/${item.id}/raw`;
  const [alt, setAlt] = React.useState("");
  const [caption, setCaption] = React.useState("");
  const [fileName, setFileName] = React.useState("");
  const [focal, setFocal] = React.useState<{ x: number; y: number } | null>(null);
  const replaceRef = React.useRef<HTMLInputElement>(null);
  // Image editing: a crop rectangle drawn on the preview (fractions of
  // the picture), plus rotation and flips applied on the server.
  const [cropping, setCropping] = React.useState(false);
  const [version, setVersion] = React.useState(0);
  const [crop, setCrop] = React.useState<{ x: number; y: number; w: number; h: number } | null>(null);
  const dragStart = React.useRef<{ x: number; y: number } | null>(null);
  const edit = useMutation({
    mutationFn: (body: Parameters<typeof api.editMedia>[1]) => api.editMedia(item?.id as string, body),
    onSuccess: (m) => {
      onChange(m);
      setCrop(null);
      setCropping(false);
      setVersion((v) => v + 1);
      void queryClient.invalidateQueries({ queryKey: ["media"] });
      notify.success("Image updated", "Every post that uses it shows the new version.");
    },
    onError: (e) => notify.error("Couldn't edit the image", e),
  });
  const frac = (e: React.MouseEvent<HTMLElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    return { x: Math.min(1, Math.max(0, (e.clientX - r.left) / r.width)), y: Math.min(1, Math.max(0, (e.clientY - r.top) / r.height)) };
  };

  React.useEffect(() => {
    setAlt(item?.alt ?? "");
    setCaption(item?.caption ?? "");
    setFileName(item?.file_name ?? "");
    setFocal(item !== null && item.focal_x != null && item.focal_y != null ? { x: item.focal_x, y: item.focal_y } : null);
  }, [item]);

  const usage = useQuery({
    queryKey: ["media", "usage", item?.id],
    queryFn: () => api.mediaUsage(item?.id as string),
    enabled: item !== null,
  });

  const save = useMutation({
    mutationFn: () =>
      api.updateMedia(item?.id as string, {
        alt,
        caption,
        ...(fileName.trim() !== "" && fileName !== item?.file_name ? { file_name: fileName.trim() } : {}),
        ...(focal !== null ? { focal_x: focal.x, focal_y: focal.y } : {}),
      }),
    onSuccess: (m) => {
      // The panel now holds what the server holds, so Save goes quiet
      // until something changes again.
      onChange(m);
      notify.success("Details saved");
    },
    onError: (e) => notify.error("Couldn't save the details", e),
  });

  const replace = useMutation({
    mutationFn: async (file: File) => api.replaceMedia(item?.id as string, isImageFile(file) ? await prepareImage(file) : file),
    onSuccess: (m) => {
      onChange(m);
      void queryClient.invalidateQueries({ queryKey: ["media"] });
      notify.success("File replaced", "Every post that uses it now shows the new one.");
    },
    onError: (e) => notify.error("Couldn't replace the file", e),
  });

  const dirty =
    item !== null &&
    (alt !== (item.alt ?? "") ||
      caption !== (item.caption ?? "") ||
      (fileName.trim() !== "" && fileName !== item.file_name) ||
      (focal !== null && (focal.x !== item.focal_x || focal.y !== item.focal_y)));

  const generateAlt = useMutation({
    mutationFn: () => api.generateAltText(item?.id as string),
    onSuccess: (r) => {
      if (r.alt) setAlt(r.alt);
      notify.success("Alt text written", "Edit it if it isn't quite right, then save.");
    },
    onError: (e) => notify.error("Couldn't write alt text", e),
  });

  const isAv = item !== null && (item.mime.startsWith("audio/") || item.mime.startsWith("video/"));
  const transcript = useQuery({
    queryKey: ["media", "transcript", item?.id],
    queryFn: () => api.mediaTranscript(item?.id as string),
    enabled: isAv,
    refetchInterval: (q) => (q.state.data?.transcript ? false : 5000),
  });
  const transcribe = useMutation({
    mutationFn: () => api.transcribeMedia(item?.id as string),
    onSuccess: () => notify.success("Transcribing", "The text appears below when it's done."),
    onError: (e) => notify.error("Couldn't start transcription", e),
  });

  const variants = item !== null && item.derivatives && typeof item.derivatives === "object"
    ? Object.entries(item.derivatives as Record<string, { width?: number }>).filter(([k]) => !k.startsWith("webp"))
    : [];

  return (
    <Modal
      open={item !== null}
      onClose={onClose}
      title={item?.file_name ?? ""}
      description={item === null ? undefined : `${item.mime} · ${formatBytes(item.byte_size)}${item.width && item.height ? ` · ${item.width}×${item.height}` : ""} · added ${formatRelative(item.created_at)}`}
      size="lg"
      footer={
        <>
          {canWrite ? (
            <Button variant="outline" onClick={onDelete}>
              <Trash2 className="h-4 w-4" aria-hidden="true" />
              Delete
            </Button>
          ) : null}
          <Button variant="outline" onClick={onClose}>
            Close
          </Button>
          {canWrite ? (
            <Button disabled={!dirty || save.isPending} onClick={() => save.mutate()} data-testid="save-details">
              {save.isPending ? "Saving…" : "Save details"}
            </Button>
          ) : null}
        </>
      }
    >
      {item === null ? null : (
        <div className="space-y-4">
          {isImage(item) ? (
            <div className="space-y-1">
              <div
                role="button"
                tabIndex={0}
                className="relative block w-full cursor-crosshair select-none overflow-hidden rounded-md border"
                style={{ touchAction: cropping ? "none" : "auto" }}
                aria-label={cropping ? "Crop selection; use the crop bounds below" : "Focal point; use arrow keys to adjust"}
                onKeyDown={(e) => {
                  if (cropping || !["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Enter", " "].includes(e.key)) return;
                  e.preventDefault();
                  setFocal(p => ({ x: Math.max(0, Math.min(1, (p?.x ?? 0.5) + (e.key === "ArrowRight" ? 0.01 : e.key === "ArrowLeft" ? -0.01 : 0))), y: Math.max(0, Math.min(1, (p?.y ?? 0.5) + (e.key === "ArrowDown" ? 0.01 : e.key === "ArrowUp" ? -0.01 : 0))) }));
                }}
                title={cropping ? "Drag a rectangle to crop" : "Click the part of the picture that must stay visible when it is cropped"}
                data-testid="focal-picker"
                onPointerDown={(e) => { if (cropping) { e.currentTarget.setPointerCapture(e.pointerId); dragStart.current = frac(e); setCrop(null); } }}
                onPointerMove={(e) => {
                  if (!cropping || !dragStart.current) return;
                  const p = frac(e);
                  const s0 = dragStart.current;
                  setCrop({ x: Math.min(s0.x, p.x), y: Math.min(s0.y, p.y), w: Math.abs(p.x - s0.x), h: Math.abs(p.y - s0.y) });
                }}
                onPointerUp={() => { dragStart.current = null; }}
                onPointerCancel={() => { dragStart.current = null; }}
                onClick={(e) => { if (!cropping) setFocal(frac(e)); }}
              >
                <img src={`${url}?v=${version}`} alt={item.alt ?? ""} className="max-h-72 w-full object-contain" draggable={false} />
                {focal !== null && !cropping ? (
                  <span aria-hidden="true" className="pointer-events-none absolute h-5 w-5 -translate-x-1/2 -translate-y-1/2 rounded-full border-2 border-white bg-primary/80 shadow" style={{ left: `${focal.x * 100}%`, top: `${focal.y * 100}%` }} />
                ) : null}
                {crop && crop.w > 0.01 && crop.h > 0.01 ? (
                  <span aria-hidden="true" className="pointer-events-none absolute border-2 border-white shadow-[0_0_0_9999px_rgba(0,0,0,0.45)]" style={{ left: `${crop.x * 100}%`, top: `${crop.y * 100}%`, width: `${crop.w * 100}%`, height: `${crop.h * 100}%` }} />
                ) : null}
              </div>
              <p className="text-[11px] text-muted-foreground">
                {cropping ? "Drag a rectangle over the part to keep, then press Apply crop." : `Click to set the focal point: cropped cards keep that part in view.${focal !== null ? ` Now at ${Math.round(focal.x * 100)}%, ${Math.round(focal.y * 100)}%.` : ""}`}
              </p>
              {cropping ? <fieldset className="grid grid-cols-2 gap-2 sm:grid-cols-4"><legend className="text-xs">Crop bounds (%)</legend>
                {(["x", "y", "w", "h"] as const).map(key => <label key={key} className="text-xs">{{ x: "Left", y: "Top", w: "Width", h: "Height" }[key]}<Input type="number" min="0" max="100" step="1" value={Math.round((crop ?? { x: 0, y: 0, w: 1, h: 1 })[key] * 100)} onChange={e => {
                  const value = Number(e.target.value); if (!Number.isFinite(value)) return;
                  setCrop(old => { const next = { ...(old ?? { x: 0, y: 0, w: 1, h: 1 }), [key]: Math.max(0, Math.min(100, value)) / 100 }; next.w = Math.min(next.w, 1 - next.x); next.h = Math.min(next.h, 1 - next.y); return next; });
                }} /></label>)}
              </fieldset> : null}
              {canWrite ? (
                <div className="flex flex-wrap items-center gap-1" data-testid="image-tools">
                  <Button variant="outline" size="sm" className="h-7 text-xs" disabled={edit.isPending} onClick={() => edit.mutate({ rotate: 270 })} title="Rotate left">↺ Rotate</Button>
                  <Button variant="outline" size="sm" className="h-7 text-xs" disabled={edit.isPending} onClick={() => edit.mutate({ rotate: 90 })} title="Rotate right">↻ Rotate</Button>
                  <Button variant="outline" size="sm" className="h-7 text-xs" disabled={edit.isPending} onClick={() => edit.mutate({ flip_h: true })}>⇋ Flip</Button>
                  <Button variant="outline" size="sm" className="h-7 text-xs" disabled={edit.isPending} onClick={() => edit.mutate({ flip_v: true })}>⇅ Flip</Button>
                  {cropping ? (
                    <>
                      <Button size="sm" className="h-7 text-xs" disabled={edit.isPending || !crop || crop.w < 0.02 || crop.h < 0.02} onClick={() => edit.mutate({ crop })} data-testid="apply-crop">{edit.isPending ? "Working…" : "Apply crop"}</Button>
                      <Button variant="ghost" size="sm" className="h-7 text-xs" onClick={() => { setCropping(false); setCrop(null); }}>Cancel</Button>
                    </>
                  ) : (
                    <Button variant="outline" size="sm" className="h-7 text-xs" onClick={() => { setCropping(true); setCrop({ x: 0, y: 0, w: 1, h: 1 }); }} data-testid="start-crop">✂ Crop</Button>
                  )}
                </div>
              ) : null}
            </div>
          ) : item.mime.startsWith("video/") ? (
            <video controls preload="metadata" src={url} className="max-h-72 w-full rounded-md border bg-black" />
          ) : item.mime.startsWith("audio/") ? (
            <audio controls preload="metadata" src={url} className="w-full" />
          ) : null}

          {isAv ? (
            <div className="space-y-2 rounded-md border bg-muted/30 p-3" data-testid="media-transcript">
              <div className="flex items-center justify-between gap-2">
                <p className="text-sm font-medium">Transcript</p>
                {canWrite ? (
                  <Button size="sm" variant="outline" disabled={transcribe.isPending} onClick={() => transcribe.mutate()}>
                    {transcript.data?.transcript ? "Redo" : "Transcribe"}
                  </Button>
                ) : null}
              </div>
              {transcript.data?.transcript ? (
                <p className="max-h-40 overflow-y-auto whitespace-pre-wrap text-xs text-muted-foreground">{transcript.data.transcript}</p>
              ) : (
                <p className="text-xs text-muted-foreground">No transcript yet. Uses the transcription model on the AI models page.</p>
              )}
            </div>
          ) : null}

          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-4">
              <Field label="Alt text" htmlFor="media-alt" hint="What a screen reader announces in place of the image. Leave blank only if it is purely decorative.">
                <div className="flex gap-2">
                  <Input id="media-alt" value={alt} onChange={(e) => setAlt(e.target.value)} placeholder="A short description of the image" disabled={!canWrite} />
                  {isImage(item) && canWrite ? (
                    <Button type="button" variant="outline" className="shrink-0" disabled={generateAlt.isPending} onClick={() => generateAlt.mutate()} title="Write alt text with the vision model">
                      {generateAlt.isPending ? "Writing…" : "Generate"}
                    </Button>
                  ) : null}
                </div>
              </Field>
              <Field label="Caption" htmlFor="media-caption" hint="Shown beneath the image where a theme supports it.">
                <Input id="media-caption" value={caption} onChange={(e) => setCaption(e.target.value)} disabled={!canWrite} />
              </Field>
              <Field label="File name" htmlFor="media-name" hint="Renaming keeps the same address, so nothing that uses it breaks.">
                <Input id="media-name" value={fileName} onChange={(e) => setFileName(e.target.value)} className="font-mono text-xs" disabled={!canWrite} />
              </Field>
            </div>

            <div className="space-y-3 text-sm">
              <div>
                <p className="text-xs font-medium text-muted-foreground">Used in</p>
                {usage.data === undefined ? (
                  <p className="text-xs text-muted-foreground">Checking…</p>
                ) : usage.data.posts.length === 0 && !usage.data.site_logo && !usage.data.site_favicon ? (
                  <p className="text-xs text-muted-foreground" data-testid="media-unused">Nothing on the site uses this file.</p>
                ) : (
                  <ul className="space-y-0.5 text-xs" data-testid="media-usage">
                    {usage.data.site_logo ? <li>The site logo</li> : null}
                    {usage.data.site_favicon ? <li>The site favicon</li> : null}
                    {usage.data.posts.map((p) => (
                      <li key={p.id}>
                        <a href={`/admin/posts/${p.id}`} className="underline">{p.title}</a> <span className="text-muted-foreground">({p.status})</span>
                      </li>
                    ))}
                  </ul>
                )}
              </div>
              {variants.length > 0 ? (
                <div>
                  <p className="text-xs font-medium text-muted-foreground">Sizes</p>
                  <ul className="space-y-0.5 text-xs">
                    {variants.map(([name, v]) => (
                      <li key={name} className="flex items-center gap-2">
                        <span className="w-16 font-mono">{name}</span>
                        <span className="text-muted-foreground">{v.width ? `${v.width}px wide` : ""}</span>
                        <button type="button" className="ml-auto text-primary underline" onClick={() => { void navigator.clipboard?.writeText(`${url}?variant=${name}`); notify.success(`Copied the ${name} address`); }}>
                          Copy address
                        </button>
                      </li>
                    ))}
                  </ul>
                </div>
              ) : null}
              {item.blurhash ? (
                <p className="text-[11px] text-muted-foreground">Placeholder hash <span className="font-mono">{item.blurhash}</span></p>
              ) : null}
              {alt.trim() === "" && isImage(item) ? (
                <span className="inline-flex"><Chip tone="warning">No alt text</Chip></span>
              ) : null}
            </div>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <Button variant="outline" size="sm" onClick={() => { void navigator.clipboard?.writeText(url); notify.success("Address copied"); }}>
              <Copy className="h-3.5 w-3.5" aria-hidden="true" />
              Copy address
            </Button>
            <a href={url} target="_blank" rel="noreferrer" className="inline-flex h-8 items-center gap-1.5 rounded-md border px-3 text-sm hover:bg-accent">
              <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
              Open original
            </a>
            {canWrite ? (
              <Button variant="outline" size="sm" disabled={replace.isPending} onClick={() => replaceRef.current?.click()} data-testid="replace-file">
                <Replace className="h-3.5 w-3.5" aria-hidden="true" />
                {replace.isPending ? "Replacing…" : "Replace file"}
              </Button>
            ) : null}
            <input
              ref={replaceRef}
              type="file"
              hidden
              onChange={(e) => {
                const f = e.target.files?.[0];
                e.target.value = "";
                if (f !== undefined) replace.mutate(f);
              }}
            />
          </div>
        </div>
      )}
    </Modal>
  );
}
