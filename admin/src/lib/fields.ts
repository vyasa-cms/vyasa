import { ApiError, type FieldKind } from "@/api/client";

/**
 * A content type's slug: what `posts.type` holds and the first segment of
 * its addresses. The server allows at most 32 characters (the column's
 * width) and refuses reserved words besides; this only catches the shape.
 */
export const TYPE_SLUG_RE = /^[a-z][a-z0-9-]{1,31}$/;

/** A field's key, as entries store it and templates read it (`fields.<key>`). */
export const FIELD_KEY_RE = /^[a-z][a-z0-9_]{0,39}$/;

/** Each kind, in the order the editor offers them, with a plain name. */
export const FIELD_KINDS: { kind: FieldKind; label: string; hint: string }[] = [
  { kind: "text", label: "Text", hint: "One line of text." },
  { kind: "textarea", label: "Long text", hint: "Several lines of plain text." },
  { kind: "number", label: "Number", hint: "A number, optionally within limits." },
  { kind: "boolean", label: "Yes / no", hint: "A checkbox." },
  { kind: "date", label: "Date", hint: "A calendar date." },
  { kind: "choice", label: "Choice", hint: "One (or several) of a fixed list." },
  { kind: "url", label: "Link", hint: "An http(s) address, or a path on this site." },
  { kind: "media", label: "Media", hint: "A file from the media library." },
  { kind: "entry", label: "Entry", hint: "Another post, page or entry." },
];

export function kindLabel(kind: string): string {
  return FIELD_KINDS.find((k) => k.kind === kind)?.label ?? kind;
}

/** A key suggested from a label: "Launch date" → `launch_date`. */
export function keyFromLabel(label: string): string {
  const key = label
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .replace(/[^a-z0-9]+/g, "_")
    .replace(/^[^a-z]+/, "")
    .replace(/_+$/, "")
    .slice(0, 40)
    .replace(/_+$/, "");
  return key;
}

/**
 * Per-field messages from a refused save.
 *
 * The server answers a bad value with one 400 whose message is
 * `fields.<key>: <message>` repeated and joined by `"; "`. Anything that is
 * not such a part (another field of the post, a conflict) is left to the
 * usual error toast.
 */
export function fieldErrorsOf(error: unknown): Record<string, string> {
  if (!(error instanceof ApiError) || error.status !== 400) return {};
  const out: Record<string, string> = {};
  for (const part of error.message.split("; ")) {
    if (!part.startsWith("fields.")) continue;
    const at = part.indexOf(": ");
    if (at < 0) continue;
    const key = part.slice("fields.".length, at);
    if (key !== "" && out[key] === undefined) out[key] = part.slice(at + 2);
  }
  return out;
}
