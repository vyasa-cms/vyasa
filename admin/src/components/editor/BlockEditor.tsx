import * as React from "react";
import { ChevronDown, ChevronUp, Copy, Plus, Trash2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import {
  type Block,
  type BlockKind,
  BLOCK_KINDS,
  CALLOUT_TONES,
  createBlock,
  mediaUrl,
} from "./blocks";
import { MediaPicker } from "./MediaPicker";
import { MonacoPane } from "./MonacoPane";
import { htmlToInlineMarkdown, inlineMarkdownToHtml } from "./inlineMarkdown";
import { fromLocalInput, toLocalInput } from "./PostChromeSidebar";

function uid() {
  return Math.random().toString(36).slice(2, 9);
}

type DraftBlock = Block & { _id: string };

function toDraft(blocks: Block[]): DraftBlock[] {
  // `children` is absent from the wire format when empty.
  return (blocks ?? []).map((b) => ({
    ...b,
    _id: uid(),
    children: toDraft(b.children ?? []),
  }));
}
function fromDraft(drafts: DraftBlock[]): Block[] {
  return (drafts ?? []).map(({ _id: _id2, ...b }) => ({
    ...b,
    children: fromDraft((b.children ?? []) as DraftBlock[]),
  }));
}

/* ---------- per-kind editors ---------- */

function TextArea({
  value,
  onChange,
  placeholder,
  rows = 3,
  mono,
}: {
  value: string;
  onChange: (v: string) => void;
  placeholder?: string;
  rows?: number;
  mono?: boolean;
}) {
  return (
    <textarea
      value={value}
      onChange={(e) => onChange(e.target.value)}
      placeholder={placeholder}
      rows={rows}
      className={`w-full rounded-md border bg-background p-2 text-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring ${mono ? "font-mono text-xs" : ""}`}
    />
  );
}

/**
 * A field that stores inline HTML but reads and writes the classic
 * editor's Markdown for it. The Markdown is kept as typed while the field
 * has focus, so a half-written `**` does not get rewritten under the
 * caret; the stored HTML is what leaves the component.
 */
function RichTextArea({
  value,
  onChange,
  placeholder,
  rows = 3,
  ariaLabel,
}: {
  value: string;
  onChange: (html: string) => void;
  placeholder?: string;
  rows?: number;
  ariaLabel?: string;
}) {
  const [text, setText] = React.useState(() => htmlToInlineMarkdown(value));
  const focused = React.useRef(false);
  React.useEffect(() => {
    if (!focused.current) setText(htmlToInlineMarkdown(value));
  }, [value]);
  return (
    <textarea
      value={text}
      onFocus={() => {
        focused.current = true;
      }}
      onBlur={() => {
        focused.current = false;
        setText(htmlToInlineMarkdown(value));
      }}
      onChange={(e) => {
        setText(e.target.value);
        onChange(inlineMarkdownToHtml(e.target.value));
      }}
      placeholder={placeholder}
      rows={rows}
      aria-label={ariaLabel}
      data-testid="rich-field"
      className="w-full rounded-md border bg-background p-2 text-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
    />
  );
}

const FORMAT_HINT = "**bold**, *italic*, `code`, ~~struck~~, [text](url); Enter for a line break.";

function BlockFields({
  block,
  onChange,
}: {
  block: DraftBlock;
  onChange: (attrs: Record<string, unknown>) => void;
}) {
  const a = block.attrs as Record<string, unknown>;
  const set = (k: string, v: unknown) => onChange({ ...a, [k]: v });
  const str = (k: string) => (typeof a[k] === "string" ? (a[k] as string) : "");

  switch (block.kind) {
    case "paragraph":
      return <RichTextArea value={str("text")} onChange={(v) => set("text", v)} placeholder="Write paragraph…" ariaLabel="Paragraph text" />;
    case "heading": {
      const level = (a.level as number) ?? 2;
      return (
        <div className="space-y-2">
          <div className="flex gap-2">
            {[2, 3, 4, 5, 6].map((l) => (
              <button
                key={l}
                type="button"
                onClick={() => set("level", l)}
                className={`rounded border px-2 py-1 text-xs ${level === l ? "bg-primary text-primary-foreground" : "bg-muted"}`}
              >
                H{l}
              </button>
            ))}
          </div>
          <RichTextArea value={str("text")} onChange={(v) => set("text", v)} placeholder="Heading text…" rows={1} ariaLabel="Heading text" />
          <p className="text-[11px] text-muted-foreground">The post title is the page&rsquo;s H1, so section headings start at H2.</p>
        </div>
      );
    }
    case "list":
      return (
        <div className="space-y-1">
          <div className="flex gap-2 text-xs">
            <button type="button" onClick={() => set("ordered", false)} className={`rounded border px-2 py-1 ${a.ordered !== true ? "bg-primary text-primary-foreground" : "bg-muted"}`}>• Bulleted</button>
            <button type="button" onClick={() => set("ordered", true)} className={`rounded border px-2 py-1 ${a.ordered === true ? "bg-primary text-primary-foreground" : "bg-muted"}`}>1. Numbered</button>
          </div>
          <p className="text-[11px] text-muted-foreground">Each block inside is one item; a list inside nests under the item before it.</p>
        </div>
      );
    case "quote":
      return (
        <div className="space-y-2">
          <RichTextArea value={str("text")} onChange={(v) => set("text", v)} placeholder="Quote…" ariaLabel="Quote text" />
          <Input value={str("citation")} onChange={(e) => set("citation", e.target.value)} placeholder="Attribution (optional)" />
        </div>
      );
    case "code": {
      const lang = str("language");
      return (
        <div className="space-y-2">
          <Input value={lang} onChange={(e) => set("language", e.target.value === "" ? null : e.target.value)} placeholder="language (rust, js, html… optional)" className="h-7 text-xs" />
          <MonacoPane value={str("code")} language={lang === "" ? "plaintext" : lang} onChange={(v) => set("code", v)} height={180} ariaLabel="Code block" />
        </div>
      );
    }
    case "separator":
      return <hr className="my-2" />;
    case "page_break":
      return <div className="rounded border border-dashed p-2 text-center text-xs text-muted-foreground">— Page Break —</div>;
    case "image":
      return (
        <div className="space-y-2">
          <Input value={str("url")} onChange={(e) => set("url", e.target.value)} placeholder="Image URL (https://… or /api/v1/media/{id}/raw)" />
          <MediaPicker accept="image" onPick={(m) => onChange({ ...a, url: mediaUrl(m.id), alt: str("alt") || (m.alt ?? "") })} />
          <Input value={str("alt")} onChange={(e) => set("alt", e.target.value)} placeholder="Alt text" aria-label="Alt text" />
          {a.url && str("alt").trim() === "" ? (
            <p className="text-[11px] text-amber-600 dark:text-amber-400">No alt text yet. Leave it empty only if the image is decorative.</p>
          ) : null}
          <RichTextArea value={str("caption")} onChange={(v) => set("caption", v)} placeholder="Caption" rows={1} ariaLabel="Caption" />
          {a.url ? <img src={String(a.url)} alt={str("alt")} className="max-h-48 rounded border" loading="lazy" /> : null}
        </div>
      );
    case "gallery":
      return <p className="text-xs text-muted-foreground">Each image block inside is one picture in the gallery. Use “Add inside” to add more.</p>;
    case "video":
    case "audio":
    case "file":
      return (
        <div className="space-y-2">
          <Input value={str("url")} onChange={(e) => set("url", e.target.value)} placeholder={block.kind === "video" ? "Video URL" : block.kind === "audio" ? "Audio URL" : "File URL"} />
          <MediaPicker accept="any" onPick={(m) => set("url", mediaUrl(m.id))} />
          <Input value={str("caption")} onChange={(e) => set("caption", e.target.value)} placeholder={block.kind === "file" ? "Link text (falls back to the file name)" : "Caption"} />
        </div>
      );
    case "table": {
      const rows = (Array.isArray(a.rows) ? (a.rows as unknown[][]) : [["", ""]]).map((r) => r.map((c) => (typeof c === "string" ? c : "")));
      const header = Array.isArray(a.header) ? (a.header as unknown[]).map((c) => (typeof c === "string" ? c : "")) : null;
      const width = Math.max(1, header?.length ?? 0, ...rows.map((r) => r.length));
      const write = (next: { header?: string[] | null; rows?: string[][] }) => {
        const attrs = { ...a };
        if (next.rows !== undefined) attrs.rows = next.rows;
        if (next.header !== undefined) {
          if (next.header === null) delete attrs.header;
          else attrs.header = next.header;
        }
        onChange(attrs);
      };
      const setCell = (r: number, c: number, v: string) => {
        const cpy = rows.map((row) => [...row]);
        if (cpy[r]) cpy[r][c] = v;
        write({ rows: cpy });
      };
      const setHead = (c: number, v: string) => {
        if (header === null) return;
        const cpy = [...header];
        cpy[c] = v;
        write({ header: cpy });
      };
      return (
        <div className="space-y-2">
          <div className="overflow-auto rounded border">
            <table className="w-full text-sm">
              {header !== null ? (
                <thead>
                  <tr>
                    {Array.from({ length: width }, (_, c) => (
                      <th key={c} className="border bg-muted/50 p-1"><Input value={header[c] ?? ""} onChange={(e) => setHead(c, e.target.value)} className="h-7 font-medium" aria-label={`Header ${c + 1}`} /></th>
                    ))}
                    <th className="w-8" />
                  </tr>
                </thead>
              ) : null}
              <tbody>
                {rows.map((row, r) => (
                  <tr key={r}>
                    {Array.from({ length: width }, (_, c) => (
                      <td key={c} className="border p-1"><Input value={row[c] ?? ""} onChange={(e) => setCell(r, c, e.target.value)} className="h-7" aria-label={`Row ${r + 1} cell ${c + 1}`} /></td>
                    ))}
                    <td className="w-8 p-1"><Button variant="ghost" size="sm" aria-label={`Remove row ${r + 1}`} onClick={() => write({ rows: rows.filter((_, j) => j !== r) })}>×</Button></td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <div className="flex flex-wrap gap-2">
            <Button variant="outline" size="sm" onClick={() => write({ rows: [...rows, Array<string>(width).fill("")] })}>+ Row</Button>
            <Button variant="outline" size="sm" onClick={() => write({ rows: rows.map((r) => [...r, ""]), header: header === null ? undefined : [...header, ""] })}>+ Column</Button>
            <Button variant="outline" size="sm" disabled={width <= 1} onClick={() => write({ rows: rows.map((r) => r.slice(0, -1)), header: header === null ? undefined : header.slice(0, -1) })}>− Column</Button>
            <Button variant="outline" size="sm" aria-pressed={header !== null} onClick={() => write(header === null ? { header: rows[0] ?? Array<string>(width).fill(""), rows: rows.slice(1) } : { header: null, rows: [header, ...rows] })}>
              {header === null ? "Add header row" : "Remove header row"}
            </Button>
          </div>
          <Input value={str("caption")} onChange={(e) => set("caption", e.target.value)} placeholder="Caption (optional)" />
        </div>
      );
    }
    case "cover":
      return (
        <div className="space-y-2">
          <Input value={str("url")} onChange={(e) => set("url", e.target.value)} placeholder="Cover image URL" />
          <MediaPicker accept="image" onPick={(m) => set("url", mediaUrl(m.id))} />
          <TextArea value={str("text")} onChange={(v) => set("text", v)} placeholder="Overlay text…" rows={2} />
        </div>
      );
    case "media_text":
      return (
        <div className="space-y-2">
          <Input value={str("url")} onChange={(e) => set("url", e.target.value)} placeholder="Media URL" />
          <MediaPicker accept="image" onPick={(m) => onChange({ ...a, url: mediaUrl(m.id), alt: str("alt") || (m.alt ?? "") })} />
          <Input value={str("alt")} onChange={(e) => set("alt", e.target.value)} placeholder="Alt text" />
          <RichTextArea value={str("text")} onChange={(v) => set("text", v)} placeholder="Text…" ariaLabel="Media text" />
        </div>
      );
    case "button":
      return (
        <div className="flex gap-2">
          <Input value={str("label")} onChange={(e) => set("label", e.target.value)} placeholder="Label" />
          <Input value={str("href")} onChange={(e) => set("href", e.target.value)} placeholder="Links to (https://…, /page, #anchor)" />
        </div>
      );
    case "details":
      return (
        <div className="space-y-2">
          <Input value={str("summary")} onChange={(e) => set("summary", e.target.value)} placeholder="Summary" />
        </div>
      );
    case "embed":
      return <Input value={str("url")} onChange={(e) => set("url", e.target.value)} placeholder="Embed URL (YouTube, etc.)" />;
    case "toc":
      return (
        <div className="flex items-center gap-2 text-xs">
          <span className="text-muted-foreground">Headings down to</span>
          {[2, 3, 4].map((d) => (
            <button key={d} type="button" onClick={() => set("depth", d)} className={`rounded border px-2 py-1 ${(a.depth ?? 3) === d ? "bg-primary text-primary-foreground" : "bg-muted"}`}>H{d}</button>
          ))}
          <span className="text-muted-foreground">Built from the post's headings when it renders.</span>
        </div>
      );
    case "callout":
      return (
        <div className="space-y-2">
          <div className="flex gap-2 text-xs">
            {CALLOUT_TONES.map((t) => (
              <button key={t} type="button" onClick={() => set("tone", t)} className={`rounded border px-2 py-1 capitalize ${(a.tone ?? "note") === t ? "bg-primary text-primary-foreground" : "bg-muted"}`}>{t}</button>
            ))}
          </div>
          <RichTextArea value={str("title")} onChange={(v) => set("title", v)} placeholder="Title (optional)" rows={1} ariaLabel="Callout title" />
        </div>
      );
    case "timed":
      return (
        <div className="grid gap-2 sm:grid-cols-2">
          <label className="text-xs"><span className="text-muted-foreground">Show from</span>
            <Input type="datetime-local" value={toLocalInput(str("from"))} onChange={(e) => set("from", fromLocalInput(e.target.value))} className="mt-1 h-8" /></label>
          <label className="text-xs"><span className="text-muted-foreground">Until</span>
            <Input type="datetime-local" value={toLocalInput(str("until"))} onChange={(e) => set("until", fromLocalInput(e.target.value))} className="mt-1 h-8" /></label>
          <p className="text-[11px] text-muted-foreground sm:col-span-2">Blocks inside show only between these moments; leave either empty for no bound.</p>
        </div>
      );
    case "html":
      return <MonacoPane value={str("html")} language="html" onChange={(v) => set("html", v)} height={180} ariaLabel="HTML block" />;
    case "footnotes":
      return <p className="text-xs text-muted-foreground">Each paragraph inside is one footnote.</p>;
    case "buttons":
    case "group":
    case "row":
    case "columns":
    case "grid":
      return <p className="text-xs text-muted-foreground">Container — add blocks inside.</p>;
    default:
      return <TextArea value={JSON.stringify(a, null, 2)} onChange={(v) => { try { onChange(JSON.parse(v)); } catch { /* ignore */ } }} rows={4} mono />;
  }
}

/* ---------- block chrome + slash menu ---------- */

const SLASH_ITEMS: { kind: BlockKind; label: string }[] = BLOCK_KINDS.map((b) => ({ kind: b.kind, label: b.label }));

export function BlockEditor({
  value,
  onChange,
}: {
  value: Block[];
  onChange: (blocks: Block[]) => void;
}) {
  const [drafts, setDrafts] = React.useState<DraftBlock[]>(() => toDraft(value));
  const [slashFor, setSlashFor] = React.useState<string | null>(null);
  const [slashQuery, setSlashQuery] = React.useState("");

  // The array we last handed to `onChange`. The parent stores it and passes it
  // straight back as `value`, so without this guard the effect below would
  // re-key every block on each keystroke — remounting the field being typed
  // into and dropping the caret.
  const lastEmitted = React.useRef<Block[] | null>(null);

  React.useEffect(() => {
    if (lastEmitted.current === value) return; // our own echo; ids stay stable
    setDrafts(toDraft(value));
  }, [value]);

  const commit = (next: DraftBlock[]) => {
    setDrafts(next);
    const emitted = fromDraft(next);
    lastEmitted.current = emitted;
    onChange(emitted);
  };

  const add = (kind: BlockKind, afterId?: string) => {
    const nb = toDraft([createBlock(kind)])[0] as DraftBlock;
    if (!afterId) return commit([...drafts, nb]);
    const idx = drafts.findIndex((d) => d._id === afterId);
    const nxt = [...drafts];
    nxt.splice(idx + 1, 0, nb);
    commit(nxt);
    setSlashFor(null);
  };

  const update = (id: string, attrs: Record<string, unknown>) => {
    commit(drafts.map((d) => (d._id === id ? { ...d, attrs } : d)));
  };
  const remove = (id: string) => commit(drafts.filter((d) => d._id !== id));
  const move = (id: string, dir: -1 | 1) => {
    const i = drafts.findIndex((d) => d._id === id);
    const j = i + dir;
    if (j < 0 || j >= drafts.length) return;
    const c = [...drafts];
    const [m] = c.splice(i, 1);
    if (m) c.splice(j, 0, m);
    commit(c);
  };
  const duplicate = (id: string) => {
    const src = drafts.find((d) => d._id === id);
    if (!src) return;
    const cpy: DraftBlock = { ...src, _id: uid(), children: toDraft(fromDraft(src.children as DraftBlock[])) };
    const idx = drafts.findIndex((d) => d._id === id);
    const nxt = [...drafts];
    nxt.splice(idx + 1, 0, cpy);
    commit(nxt);
  };

  const filtered = SLASH_ITEMS.filter((it) => it.label.toLowerCase().includes(slashQuery.toLowerCase()) || it.kind.includes(slashQuery.toLowerCase())).slice(0, 8);

  if (drafts.length === 0) {
    return (
      <div className="rounded-xl border border-dashed bg-muted/30 px-6 py-10 text-center">
        <p className="text-sm font-medium">Start writing</p>
        <p className="mx-auto mt-1 max-w-sm text-sm text-muted-foreground">
          Add a paragraph, or pick another kind of block to begin.
        </p>
        <div className="mt-4 flex flex-wrap justify-center gap-2">
          {BLOCK_KINDS.slice(0, 6).map((b) => (
            <Button key={b.kind} variant="outline" size="sm" onClick={() => add(b.kind)}>
              <span aria-hidden="true" className="font-mono text-xs">{b.icon}</span>
              {b.label}
            </Button>
          ))}
        </div>
      </div>
    );
  }

  return (
    <div className="space-y-2" data-testid="block-editor">
      <p className="text-[11px] text-muted-foreground">Formatting: {FORMAT_HINT}</p>
      {drafts.map((b, index) => {
        const meta = BLOCK_KINDS.find((k) => k.kind === b.kind);
        const label = meta?.label ?? b.kind;
        return (
          <div
            key={b._id}
            data-vy-kind={b.kind}
            tabIndex={-1}
            className="group relative rounded-lg border border-transparent px-3 py-2.5 transition-colors hover:border-border hover:bg-card focus-within:border-border focus-within:bg-card focus:outline-none"
          >
            {/* Chrome appears on hover/focus so the document reads as content,
                not as a stack of labelled form cards. Kept keyboard-reachable
                rather than hover-only. */}
            <div className="mb-1.5 flex items-center gap-1 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
              <span className="text-[10px] font-medium uppercase tracking-wider text-muted-foreground">
                {label}
              </span>

              <div className="ml-auto flex items-center gap-0.5">
                <IconButton
                  label={`Move ${label} up`}
                  disabled={index === 0}
                  onClick={() => move(b._id, -1)}
                >
                  <ChevronUp className="h-3.5 w-3.5" aria-hidden="true" />
                </IconButton>
                <IconButton
                  label={`Move ${label} down`}
                  disabled={index === drafts.length - 1}
                  onClick={() => move(b._id, 1)}
                >
                  <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
                </IconButton>
                <IconButton label={`Duplicate ${label}`} onClick={() => duplicate(b._id)}>
                  <Copy className="h-3.5 w-3.5" aria-hidden="true" />
                </IconButton>
                <IconButton
                  label={`Delete ${label}`}
                  destructive
                  onClick={() => remove(b._id)}
                >
                  <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                </IconButton>
              </div>
            </div>

            <BlockFields block={b} onChange={(attrs) => update(b._id, attrs)} />

            {b.children.length > 0 ? (
              <div className="mt-3 border-l-2 pl-3">
                <BlockEditor
                  value={b.children as unknown as Block[]}
                  onChange={(children) =>
                    commit(
                      drafts.map((d) =>
                        d._id === b._id
                          ? { ...d, children: children as unknown as DraftBlock[] }
                          : d,
                      ),
                    )
                  }
                />
              </div>
            ) : null}

            <div className="mt-2 flex flex-wrap gap-1.5 opacity-0 transition-opacity group-hover:opacity-100 group-focus-within:opacity-100">
              <div className="relative">
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 text-xs text-muted-foreground"
                  onClick={() => setSlashFor(slashFor === b._id ? null : b._id)}
                  aria-expanded={slashFor === b._id}
                >
                  <Plus className="h-3.5 w-3.5" aria-hidden="true" />
                  Add block
                </Button>
                {slashFor === b._id ? (
                  <div className="absolute left-0 top-8 z-20 w-60 rounded-lg border bg-popover p-2 shadow-lg">
                    <Input
                      autoFocus
                      placeholder="Search blocks"
                      aria-label="Search blocks"
                      value={slashQuery}
                      onChange={(e) => setSlashQuery(e.target.value)}
                      className="mb-1.5 h-8"
                    />
                    <div className="max-h-56 space-y-0.5 overflow-y-auto">
                      {filtered.map((it) => (
                        <button
                          key={it.kind}
                          type="button"
                          onClick={() => {
                            add(it.kind, b._id);
                            setSlashQuery("");
                          }}
                          className="flex w-full items-center gap-2.5 rounded-md px-2 py-1.5 text-left text-sm hover:bg-accent"
                        >
                          <span
                            aria-hidden="true"
                            className="w-4 text-center font-mono text-xs text-muted-foreground"
                          >
                            {BLOCK_KINDS.find((x) => x.kind === it.kind)?.icon}
                          </span>
                          {it.label}
                        </button>
                      ))}
                      {filtered.length === 0 ? (
                        <p className="px-2 py-2 text-xs text-muted-foreground">
                          Nothing matches “{slashQuery}”.
                        </p>
                      ) : null}
                    </div>
                  </div>
                ) : null}
              </div>

              {isContainerChild(b.kind) ? (
                <Button
                  variant="ghost"
                  size="sm"
                  className="h-7 text-xs text-muted-foreground"
                  onClick={() => {
                    const child = toDraft([createBlock(childKindFor(b.kind))])[0] as DraftBlock;
                    commit(
                      drafts.map((d) =>
                        d._id === b._id
                          ? { ...d, children: [...(d.children as DraftBlock[]), child] }
                          : d,
                      ),
                    );
                  }}
                >
                  <Plus className="h-3.5 w-3.5" aria-hidden="true" />
                  Add inside
                </Button>
              ) : null}
            </div>
          </div>
        );
      })}

      <div className="flex flex-wrap items-center gap-1.5 border-t pt-3">
        <span className="text-xs text-muted-foreground">Add</span>
        {BLOCK_KINDS.slice(0, 8).map((b) => (
          <Button
            key={b.kind}
            variant="ghost"
            size="sm"
            className="h-7 text-xs"
            onClick={() => add(b.kind)}
          >
            <span aria-hidden="true" className="font-mono">{b.icon}</span>
            {b.label}
          </Button>
        ))}
      </div>
    </div>
  );
}

/** Square icon control with a real accessible name. */
function IconButton({
  label,
  onClick,
  disabled,
  destructive,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  destructive?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      className={`inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors disabled:opacity-30 ${
        destructive === true
          ? "hover:bg-destructive-subtle hover:text-destructive"
          : "hover:bg-accent hover:text-foreground"
      }`}
    >
      {children}
    </button>
  );
}

function isContainerChild(kind: BlockKind): boolean {
  return (
    kind === "group" ||
    kind === "columns" ||
    kind === "grid" ||
    kind === "row" ||
    kind === "buttons" ||
    kind === "details" ||
    kind === "cover" ||
    kind === "media_text" ||
    kind === "list" ||
    kind === "gallery" ||
    kind === "footnotes" ||
    kind === "callout" ||
    kind === "timed"
  );
}

/** What "Add inside" puts in a container: the child kind it is made of. */
function childKindFor(kind: BlockKind): BlockKind {
  switch (kind) {
    case "gallery":
      return "image";
    case "buttons":
      return "button";
    default:
      return "paragraph";
  }
}
