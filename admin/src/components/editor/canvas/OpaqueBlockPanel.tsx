import * as React from "react";
import type { Editor } from "@tiptap/core";
import { Trash2, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { BLOCK_KINDS, isPluginKind, mediaUrl, type Block } from "../blocks";
import { MediaPicker } from "../MediaPicker";
import { api } from "@/api/client";
import { notify } from "@/components/ui/toast";
import { blockToNode } from "./serialize";
import { useQuery } from "@tanstack/react-query";

interface FieldSpec {
  key: string;
  label: string;
  hint?: string;
  kind?: "url" | "text" | "multiline" | "code";
  /** Offer the media library for this field. */
  media?: "image" | "any";
}

/**
 * Which attributes each opaque kind exposes, in the author's language.
 * Names are the server's (`docs/BLOCKS.md`): `label`/`href` on buttons,
 * `caption` as the visible text of a file link, and so on.
 */
const FIELDS: Record<string, FieldSpec[]> = {
  video: [
    { key: "url", label: "Video", kind: "url", media: "any" },
    { key: "caption", label: "Caption", hint: "Shown beneath the player." },
  ],
  audio: [
    { key: "url", label: "Audio", kind: "url", media: "any" },
    { key: "caption", label: "Caption" },
  ],
  file: [
    { key: "url", label: "File", kind: "url", media: "any" },
    { key: "caption", label: "Link text", hint: "What readers click. Falls back to the file name." },
  ],
  embed: [{ key: "url", label: "Address to embed", kind: "url", hint: "YouTube, Vimeo and similar." }],
  cover: [
    { key: "url", label: "Background image", kind: "url", media: "image" },
    { key: "text", label: "Overlay text" },
  ],
  button: [
    { key: "label", label: "Label" },
    { key: "href", label: "Links to", kind: "url", hint: "https://…, /a-page, #anchor or mailto:" },
  ],
  details: [{ key: "summary", label: "Summary", hint: "The line readers click to expand." }],
  media_text: [
    { key: "url", label: "Media", kind: "url", media: "image" },
    { key: "alt", label: "Alt text" },
    { key: "text", label: "Text", kind: "multiline" },
  ],
  html: [
    {
      key: "html",
      label: "Markup",
      kind: "code",
      hint: "Sanitised on publish: scripts, styles and event handlers are removed.",
    },
  ],
  page_break: [],
  gallery: [],
  toc: [{ key: "depth", label: "Headings down to level", hint: "2, 3 or 4. Built from the post's headings when it renders." }],
  form: [{ key: "form_slug", label: "Form", hint: "The form's slug from the Forms page, e.g. contact-us." }],
};

const NOTES: Record<string, string> = {
  page_break: "Splits the post into pages at this point. Nothing to configure.",
};

/**
 * Inline settings for the selected non-text block.
 *
 * The canvas keeps these kinds opaque so nothing is lost on a round trip; this
 * panel is where their details get edited, rather than putting form fields
 * back into the writing column.
 */
/**
 * Settings for a block a plugin contributed.
 *
 * The plugin owns the attribute schema, and the admin has no way to know
 * it, so this edits the attributes as JSON rather than pretending to
 * understand them. Invalid JSON is reported and *not* written, so a
 * half-typed brace cannot destroy the block's stored settings.
 */
function PluginAttrs({
  kind,
  attrs,
  onChange,
}: {
  kind: string;
  attrs: Record<string, unknown>;
  onChange: (next: Record<string, unknown>) => void;
}) {
  const [draft, setDraft] = React.useState(() =>
    JSON.stringify(attrs, null, 2),
  );
  const [error, setError] = React.useState<string | null>(null);

  const commit = (text: string) => {
    setDraft(text);
    try {
      const parsed: unknown = JSON.parse(text === "" ? "{}" : text);
      if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
        setError("Settings must be a JSON object.");
        return;
      }
      setError(null);
      onChange(parsed as Record<string, unknown>);
    } catch {
      setError("That isn't valid JSON yet.");
    }
  };

  return (
    <Field
      label="Settings"
      hint={`Read by the ${kind.split("/")[0] ?? ""} plugin, which renders this block on your site.`}
      htmlFor="plugin-attrs"
    >
      <textarea
        id="plugin-attrs"
        data-testid="plugin-attrs"
        value={draft}
        onChange={(e) => commit(e.target.value)}
        rows={8}
        spellCheck={false}
        className="w-full rounded-md border bg-background p-2 font-mono text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      />
      {error === null ? null : (
        <p className="text-xs text-destructive" role="alert">
          {error}
        </p>
      )}
    </Field>
  );
}

export function OpaqueBlockPanel({
  editor,
  pos,
}: {
  editor: Editor;
  pos: number;
}) {
  const node = editor.state.doc.nodeAt(pos);
  const kind = String(node?.attrs["kind"] ?? "");
  const attrs = (node?.attrs["attrs"] ?? {}) as Record<string, unknown>;
  const children = (Array.isArray(node?.attrs["children"]) ? node?.attrs["children"] : []) as Block[];
  const meta = BLOCK_KINDS.find((k) => k.kind === kind);
  const fields = FIELDS[kind];
  const [mediaFor, setMediaFor] = React.useState<string | null>(null);

  const write = (patch: Record<string, unknown>) => {
    editor
      .chain()
      .command(({ tr }) => {
        const current = tr.doc.nodeAt(pos);
        if (current === null) return false;
        tr.setNodeMarkup(pos, undefined, { ...current.attrs, ...patch });
        return true;
      })
      .run();
  };
  const set = (key: string, value: unknown) =>
    write({ attrs: { ...attrs, [key]: value } });
  const setChildren = (next: Block[]) => write({ children: next });

  const remove = () => {
    const current = editor.state.doc.nodeAt(pos);
    if (current === null) return;
    editor
      .chain()
      .focus()
      .deleteRange({ from: pos, to: pos + current.nodeSize })
      .run();
  };

  return (
    <div className="mt-3 rounded-lg border bg-card" data-testid="opaque-panel">
      <div className="flex items-center gap-2 border-b px-4 py-2.5">
        <span
          aria-hidden="true"
          className="flex h-6 w-6 items-center justify-center rounded border bg-background font-mono text-[10px] text-muted-foreground"
        >
          {meta?.icon ?? "?"}
        </span>
        <h3 className="text-sm font-medium">{meta?.label ?? kind} settings</h3>
        <Button
          variant="ghost"
          size="sm"
          className="ml-auto h-7 text-destructive hover:bg-destructive-subtle"
          onClick={remove}
        >
          <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
          Remove
        </Button>
      </div>

      <div className="space-y-4 p-4">
        {fields === undefined && isPluginKind(kind) ? (
          <PluginAttrs
            kind={kind}
            attrs={attrs}
            onChange={(next) => write({ attrs: next })}
          />
        ) : fields === undefined ? (
          <p className="text-sm text-muted-foreground">
            This block has no editable settings here yet. Switch to the classic
            editor to change its details — your content is preserved either way.
          </p>
        ) : null}

        {NOTES[kind] !== undefined ? (
          <p className="text-sm text-muted-foreground">{NOTES[kind]}</p>
        ) : null}

        {kind === "gallery" ? (
          <GalleryFields images={children} onChange={setChildren} />
        ) : null}

        {kind === "embed" ? (
          <EmbedPreview url={String(attrs["url"] ?? "")} title={String(attrs["title"] ?? "")} onTitle={(t) => set("title", t)} />
        ) : null}

        {(kind === "audio" || kind === "video") && mediaIdOf(attrs["url"]) !== null ? (
          <TranscriptTools
            mediaId={mediaIdOf(attrs["url"]) as string}
            onInsert={(text) => {
              const current = editor.state.doc.nodeAt(pos);
              if (current === null) return;
              const details: Block = {
                kind: "details",
                attrs: { summary: "Transcript" },
                children: text
                  .split(/\n{2,}/)
                  .map((t) => t.trim())
                  .filter((t) => t !== "")
                  .map((t) => ({ kind: "paragraph" as const, attrs: { text: t }, children: [] })),
              };
              editor
                .chain()
                .focus()
                .insertContentAt(pos + current.nodeSize, blockToNode(details) as unknown as Record<string, unknown>)
                .run();
            }}
          />
        ) : null}

        {(fields ?? []).map((f) => (
          <Field key={f.key} label={f.label} hint={f.hint} htmlFor={`op-${f.key}`}>
            {f.kind === "multiline" || f.kind === "code" ? (
              <textarea
                id={`op-${f.key}`}
                value={String(attrs[f.key] ?? "")}
                onChange={(e) => set(f.key, e.target.value)}
                rows={f.kind === "code" ? 8 : 3}
                spellCheck={f.kind !== "code"}
                className={
                  "w-full rounded-md border bg-background p-2 text-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring" +
                  (f.kind === "code" ? " font-mono text-xs" : "")
                }
              />
            ) : (
              <div className="flex gap-2">
                <Input
                  id={`op-${f.key}`}
                  value={String(attrs[f.key] ?? "")}
                  onChange={(e) => set(f.key, e.target.value)}
                  placeholder={f.kind === "url" ? "https://…" : undefined}
                  className={f.kind === "url" ? "font-mono text-xs" : undefined}
                />
                {f.media !== undefined ? (
                  <Button
                    variant="outline"
                    size="sm"
                    className="h-9 shrink-0"
                    aria-pressed={mediaFor === f.key}
                    onClick={() => setMediaFor(mediaFor === f.key ? null : f.key)}
                  >
                    {mediaFor === f.key ? <X className="h-3.5 w-3.5" aria-hidden="true" /> : "Library"}
                  </Button>
                ) : null}
              </div>
            )}
            {f.media !== undefined && mediaFor === f.key ? (
              <MediaPicker
                accept={f.media}
                className="pt-1"
                onPick={(m) => {
                  set(f.key, mediaUrl(m.id));
                  setMediaFor(null);
                }}
              />
            ) : null}
          </Field>
        ))}
      </div>
    </div>
  );
}

/** A gallery is a list of image blocks; edit them as a strip of thumbnails. */
function GalleryFields({
  images,
  onChange,
}: {
  images: Block[];
  onChange: (next: Block[]) => void;
}) {
  const update = (index: number, patch: Record<string, unknown>) =>
    onChange(
      images.map((im, i) => (i === index ? { ...im, attrs: { ...im.attrs, ...patch } } : im)),
    );
  const removeAt = (index: number) => onChange(images.filter((_, i) => i !== index));
  const move = (index: number, dir: -1 | 1) => {
    const j = index + dir;
    if (j < 0 || j >= images.length) return;
    const next = [...images];
    const [item] = next.splice(index, 1);
    if (item !== undefined) next.splice(j, 0, item);
    onChange(next);
  };

  return (
    <div className="space-y-3" data-testid="gallery-fields">
      {images.length === 0 ? (
        <p className="text-sm text-muted-foreground">No images yet — upload or pick some below.</p>
      ) : (
        <ul className="space-y-2">
          {images.map((im, i) => {
            const url = String(im.attrs["url"] ?? "");
            return (
              <li key={`${i}-${url}`} className="flex items-center gap-2">
                <span className="h-12 w-12 shrink-0 overflow-hidden rounded border bg-muted">
                  {url !== "" ? (
                    <img src={url} alt="" loading="lazy" className="h-full w-full object-cover" />
                  ) : null}
                </span>
                <Input
                  value={String(im.attrs["alt"] ?? "")}
                  onChange={(e) => update(i, { alt: e.target.value })}
                  placeholder="Alt text"
                  aria-label={`Alt text for image ${i + 1}`}
                  className="h-8"
                />
                <Button variant="ghost" size="sm" className="h-8 px-2" aria-label="Move earlier" disabled={i === 0} onClick={() => move(i, -1)}>
                  ↑
                </Button>
                <Button variant="ghost" size="sm" className="h-8 px-2" aria-label="Move later" disabled={i === images.length - 1} onClick={() => move(i, 1)}>
                  ↓
                </Button>
                <Button variant="ghost" size="sm" className="h-8 px-2 text-destructive" aria-label={`Remove image ${i + 1}`} onClick={() => removeAt(i)}>
                  <X className="h-3.5 w-3.5" aria-hidden="true" />
                </Button>
              </li>
            );
          })}
        </ul>
      )}
      <MediaPicker
        accept="image"
        onPick={(m) =>
          onChange([
            ...images,
            { kind: "image", attrs: { url: mediaUrl(m.id), alt: m.alt ?? "" }, children: [] },
          ])
        }
      />
    </div>
  );
}

/** What the provider says the embed is: a thumbnail and a title. */
function EmbedPreview({ url, title, onTitle }: { url: string; title: string; onTitle: (t: string) => void }) {
  const preview = useQuery({
    queryKey: ["embed-preview", url],
    queryFn: () => api.embedPreview(url),
    enabled: /^https?:\/\//.test(url),
    retry: false,
    staleTime: 10 * 60 * 1000,
  });
  // The provider's title becomes the iframe's accessible name unless the
  // author already wrote one.
  React.useEffect(() => {
    const t = preview.data?.title;
    if (t && title === "") onTitle(t);
  }, [preview.data?.title]);
  if (!/^https?:\/\//.test(url)) return null;
  if (preview.isError) {
    return <p className="text-xs text-muted-foreground" data-testid="embed-preview-error">{(preview.error as Error).message}</p>;
  }
  const d = preview.data;
  if (d === undefined) return <p className="text-xs text-muted-foreground">Looking up the embed…</p>;
  return (
    <div className="flex items-center gap-3 rounded-md border bg-muted/30 p-2" data-testid="embed-preview">
      {d.thumbnail_url ? <img src={d.thumbnail_url} alt="" className="h-14 w-24 rounded object-cover" loading="lazy" /> : null}
      <span className="min-w-0">
        <span className="block truncate text-sm font-medium">{d.title ?? url}</span>
        <span className="block truncate text-xs text-muted-foreground">{[d.provider, d.author_name].filter(Boolean).join(" · ")}</span>
      </span>
    </div>
  );
}

/** The library id inside `/api/v1/media/{id}/raw`, or null for other URLs. */
function mediaIdOf(url: unknown): string | null {
  if (typeof url !== "string") return null;
  const m = /\/api\/v1\/media\/(\d+)\/raw/.exec(url);
  return m?.[1] ?? null;
}

/** Transcribe a library audio/video file and drop the text in as a block. */
function TranscriptTools({ mediaId, onInsert }: { mediaId: string; onInsert: (text: string) => void }) {
  const [transcript, setTranscript] = React.useState<string | null | undefined>(undefined);
  const [queued, setQueued] = React.useState(false);

  const load = React.useCallback(async () => {
    try {
      const r = await api.mediaTranscript(mediaId);
      setTranscript(r.transcript);
      return r.transcript;
    } catch {
      setTranscript(null);
      return null;
    }
  }, [mediaId]);

  React.useEffect(() => {
    void load();
  }, [load]);

  // While a transcription is queued, look again every few seconds.
  React.useEffect(() => {
    if (!queued) return;
    const t = setInterval(() => {
      void load().then((text) => {
        if (text !== null && text !== undefined) setQueued(false);
      });
    }, 5000);
    return () => clearInterval(t);
  }, [queued, load]);

  const start = async () => {
    try {
      await api.transcribeMedia(mediaId);
      setQueued(true);
      notify.success("Transcribing", "This can take a minute; the text appears here when it's done.");
    } catch (e) {
      notify.error("Couldn't start transcription", e);
    }
  };

  return (
    <div className="space-y-2 rounded-md border bg-muted/30 p-3" data-testid="transcript-tools">
      <p className="text-sm font-medium">Transcript</p>
      {transcript ? (
        <>
          <p className="max-h-32 overflow-y-auto whitespace-pre-wrap text-xs text-muted-foreground">{transcript}</p>
          <div className="flex gap-2">
            <Button size="sm" variant="outline" onClick={() => onInsert(transcript)}>
              Insert as a block
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void start()}>
              Redo
            </Button>
          </div>
        </>
      ) : (
        <div className="flex items-center gap-2">
          <Button size="sm" variant="outline" disabled={queued} onClick={() => void start()}>
            {queued ? "Transcribing…" : "Transcribe"}
          </Button>
          <span className="text-xs text-muted-foreground">Uses the transcription model on the Models page.</span>
        </div>
      )}
    </div>
  );
}
