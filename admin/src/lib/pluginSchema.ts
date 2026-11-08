/**
 * Turns the `attrs-schema` a plugin declares for a block into form fields,
 * and keeps a container block's children in step with its attributes.
 *
 * The schema is a deliberately small subset of JSON Schema: objects with
 * `properties`, and per property `type` (string, number, integer, boolean,
 * array, object), `title`, `description`, `enum`, `format`
 * (`"code"` for monospace text, `"inline-html"` for rich-text strings) and
 * `items`. Anything else is shown as a JSON field, so a plugin that asks
 * for more than the editor understands still gets an editable block.
 */

export type JsonValue = string | number | boolean | null | JsonValue[] | { [key: string]: JsonValue };
export type Attrs = { [key: string]: JsonValue };

export type FieldKind = "text" | "multiline" | "code" | "number" | "boolean" | "choice" | "list" | "rows" | "json";

export interface SchemaField {
  key: string;
  label: string;
  help?: string;
  kind: FieldKind;
  /** For `choice`. */
  options?: string[];
  /** For `rows`: the columns of each row. */
  columns?: SchemaField[];
}

/** How a plugin container block holds its children. */
export interface ContainerSpec {
  /** Kind of each child block (`group`). */
  child: string;
  /** The attribute holding one label per child, when the block has labels (tabs). */
  labels?: string;
}

interface RawSchema {
  type?: string;
  title?: string;
  description?: string;
  enum?: unknown[];
  format?: string;
  properties?: Record<string, RawSchema>;
  items?: RawSchema;
}

function humanise(key: string): string {
  const spaced = key.replace(/[_-]+/g, " ").trim();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

function fieldOf(key: string, raw: RawSchema): SchemaField {
  const base = { key, label: raw.title ?? humanise(key), ...(raw.description ? { help: raw.description } : {}) };
  if (Array.isArray(raw.enum) && raw.enum.every((o) => typeof o === "string")) {
    return { ...base, kind: "choice", options: raw.enum as string[] };
  }
  switch (raw.type) {
    case "string":
      if (raw.format === "code") return { ...base, kind: "code" };
      if (raw.format === "inline-html" || raw.format === "multiline") return { ...base, kind: "multiline" };
      return { ...base, kind: "text" };
    case "number":
    case "integer":
      return { ...base, kind: "number" };
    case "boolean":
      return { ...base, kind: "boolean" };
    case "array": {
      const item = raw.items;
      if (item?.type === "string") return { ...base, kind: "list" };
      if (item?.type === "object" && item.properties) {
        return { ...base, kind: "rows", columns: Object.entries(item.properties).map(([k, v]) => fieldOf(k, v)) };
      }
      return { ...base, kind: "json" };
    }
    default:
      return { ...base, kind: "json" };
  }
}

/** The form fields for a block's attributes; empty when the plugin declared no schema. */
export function fieldsFromSchema(schema: unknown): SchemaField[] {
  const raw = schema as RawSchema | null | undefined;
  if (!raw || typeof raw !== "object" || raw.type !== "object" || !raw.properties) return [];
  return Object.entries(raw.properties).map(([key, value]) => fieldOf(key, value));
}

/** A copy of `attrs` with `key` set; an empty string, list or `null` removes it. */
export function withAttr(attrs: Attrs, key: string, value: JsonValue): Attrs {
  const next: Attrs = { ...attrs };
  const empty = value === "" || value === null || (Array.isArray(value) && value.length === 0);
  if (empty) delete next[key];
  else next[key] = value;
  return next;
}

/** A blank row for a `rows` field: every column empty (booleans false). */
export function blankRow(columns: SchemaField[]): Attrs {
  const row: Attrs = {};
  for (const c of columns) row[c.key] = c.kind === "boolean" ? false : c.kind === "number" ? 0 : "";
  return row;
}

/**
 * The children a container should have: the existing ones, cut or
 * extended with `make()` to exactly `wanted`. Existing children are never
 * reordered or rewritten, so no content is lost by growing; shrinking
 * drops only trailing, empty-looking children unless `force` is set.
 */
export function syncChildren<T extends { children?: unknown[] }>(children: T[], wanted: number, make: () => T, force = false): T[] {
  if (children.length === wanted) return children;
  if (children.length < wanted) return [...children, ...Array.from({ length: wanted - children.length }, make)];
  const dropped = children.slice(wanted);
  const hasContent = dropped.some((c) => (c.children?.length ?? 0) > 0);
  return hasContent && !force ? children : children.slice(0, wanted);
}

/** Reads a plugin's container declaration; `undefined` when the block is not a container. */
export function containerOf(descriptor: { container?: unknown } | undefined): ContainerSpec | undefined {
  const c = descriptor?.container;
  if (!c || typeof c !== "object") return undefined;
  const spec = c as { child?: unknown; labels?: unknown };
  if (typeof spec.child !== "string") return undefined;
  return { child: spec.child, ...(typeof spec.labels === "string" ? { labels: spec.labels } : {}) };
}
