import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Upload } from "lucide-react";
import { api, type MediaResponse } from "@/api/client";
import { notify } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import { isImageFile, prepareImage } from "./prepareImage";

/**
 * Pick from the media library or upload into it, in one small panel. Used
 * by the image block, the gallery editor and the classic editor's media
 * fields, so every place an author needs a file behaves the same way.
 */
export function MediaPicker({
  accept = "image",
  onPick,
  className,
}: {
  accept?: "image" | "any";
  onPick: (media: MediaResponse) => void;
  className?: string;
}) {
  const client = useQueryClient();
  const fileRef = React.useRef<HTMLInputElement>(null);

  const media = useQuery({
    queryKey: ["media", "picker"],
    queryFn: () => api.listMedia(24, 0),
    staleTime: 60_000,
  });

  const upload = useMutation({
    mutationFn: async (files: File[]) => {
      const done: MediaResponse[] = [];
      for (const file of files) {
        // A phone photo is redrawn to fit before it goes up; anything
        // else is sent as it is.
        done.push(await api.uploadMedia(isImageFile(file) ? await prepareImage(file) : file));
      }
      return done;
    },
    onSuccess: (done) => {
      void client.invalidateQueries({ queryKey: ["media"] });
      for (const m of done) onPick(m);
    },
    onError: (e) => notify.error("Couldn't upload", e),
  });
  const busy = upload.isPending;

  const items = (media.data ?? []).filter(
    (m) => accept === "any" || m.mime.startsWith("image/"),
  );

  return (
    <div className={cn("space-y-2", className)} data-testid="media-picker">
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => fileRef.current?.click()}
          disabled={busy}
          className="inline-flex h-9 items-center gap-1.5 rounded-md border bg-background px-3 text-xs font-medium hover:bg-accent disabled:opacity-60"
        >
          <Upload className="h-3.5 w-3.5" aria-hidden="true" />
          {busy ? "Uploading…" : "Upload"}
        </button>
        <input
          ref={fileRef}
          type="file"
          hidden
          multiple
          accept={accept === "image" ? "image/*,.heic,.heif" : undefined}
          onChange={(e) => {
            const files = Array.from(e.target.files ?? []);
            e.target.value = "";
            if (files.length > 0) upload.mutate(files);
          }}
        />
        <span className="text-xs text-muted-foreground">
          {items.length === 0
            ? media.isLoading
              ? "Loading your library…"
              : "Nothing in the library yet"
            : "or pick from the library"}
        </span>
      </div>
      {items.length > 0 ? (
        <div className="flex gap-1.5 overflow-x-auto pb-1">
          {items.map((m) => (
            <button
              key={m.id}
              type="button"
              onClick={() => onPick(m)}
              title={m.file_name}
              aria-label={`Use ${m.file_name}`}
              className="h-16 w-16 shrink-0 overflow-hidden rounded border hover:ring-2 hover:ring-primary focus-visible:outline-hidden focus-visible:ring-2 focus-visible:ring-ring"
            >
              {m.mime.startsWith("image/") ? (
                <img
                  src={`/api/v1/media/${m.id}/raw`}
                  alt=""
                  loading="lazy"
                  className="h-full w-full object-cover"
                />
              ) : (
                <span className="block break-all p-1 text-[9px] leading-tight text-muted-foreground">
                  {m.file_name.slice(0, 18)}
                </span>
              )}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
