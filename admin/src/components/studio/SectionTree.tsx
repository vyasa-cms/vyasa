import * as React from "react";
import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";

import { api } from "@/api/client";
import {
  ArrowDown,
  ArrowLeft,
  ArrowRight,
  ArrowUp,
  ChevronRight,
  Plus,
  Trash2,
} from "lucide-react";

import {
  type ContentSource,
  type Screen,
  type Section,
  type SettingsSchema,
  type TokenSet,
  type Vocabulary,
} from "@/api/themes";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { BindingEditor, type BindValue } from "./BindingEditor";
import { ItemsEditor } from "./ItemsEditor";
import { ScopeEditor } from "./ScopeEditor";
import {
  allIds,
  canIndent,
  canOutdent,
  flatten,
  indentSection,
  insertSection,
  isContainer,
  moveSection,
  outdentSection,
  removeSection,
  uniqueId,
  updateSection,
} from "./sectionTree";

const inputClass =
  "h-7 w-full rounded-md border border-input bg-background px-2 text-xs focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring";

/** Groups for the insert menu, so 25 kinds are not one flat list. */
const GROUPS: { label: string; kinds: string[] }[] = [
  { label: "Layout", kinds: ["band", "columns", "grid", "group"] },
  {
    label: "Marketing",
    kinds: ["hero", "feature-grid", "stats-band", "cta-band", "faq", "logo-wall", "code-tabs", "execution-flow", "status-cards"],
  },
  {
    label: "Content",
    kinds: [
      "content",
      "post-content",
      "latest-posts",
      "related-posts",
      "comments",
      "pagination",
      "toc",
      "read-aloud",
    ],
  },
  {
    label: "Navigation",
    kinds: ["menu", "breadcrumbs", "docs-nav", "search-box", "categories-list", "tag-cloud", "archives"],
  },
];

/** Inputs for one section's settings, driven by its JSON Schema. */
/**
 * The menu picker: the menus that actually exist, by name, with a road to
 * making one. A bare slug field asked authors to already know the answer.
 */
function MenuSelect({
  value,
  onChange,
}: {
  value: string;
  onChange: (slug: string) => void;
}) {
  const menus = useQuery({ queryKey: ["menus"], queryFn: () => api.listMenus() });
  const list = menus.data ?? [];
  return (
    <span className="grid gap-1">
      <select
        aria-label="Menu"
        value={value}
        onChange={(e) => onChange(e.target.value)}
        className={inputClass}
      >
        <option value="">main (default)</option>
        {list.map((m) => (
          <option key={m.id} value={m.slug}>
            {m.name} ({m.slug})
          </option>
        ))}
      </select>
      {list.length === 0 && menus.isSuccess ? (
        <span className="text-[10px] text-muted-foreground">
          No menus yet —{" "}
          <Link to="/menus" className="underline">
            create one
          </Link>{" "}
          and it appears here and on the page.
        </span>
      ) : null}
    </span>
  );
}

function SettingsForm({
  schema,
  settings,
  onChange,
  idPrefix,
  sources = [],
  kind,
}: {
  schema: SettingsSchema;
  settings: Record<string, unknown>;
  onChange: (next: Record<string, unknown>) => void;
  idPrefix: string;
  sources?: ContentSource[];
  /** The section kind, for per-kind field upgrades (menu picker). */
  kind?: string;
}) {
  const props = Object.entries(schema.properties ?? {});
  if (props.length === 0) return null;
  const set = (key: string, value: unknown) => {
    const next = { ...settings };
    if (value === "" || value === undefined || value === null) delete next[key];
    else next[key] = value;
    onChange(next);
  };

  return (
    <div className="grid gap-1.5">
      {props.map(([key, spec]) => {
        const id = `${idPrefix}-${key}`;
        const current = settings[key];
        const label = key.replace(/_/g, " ");

        // A menu is chosen from the menus that exist, not typed from
        // memory.
        if (kind === "menu" && key === "slug") {
          return (
            <label key={key} className="grid grid-cols-[6rem_1fr] items-start gap-2 text-xs">
              <span className="pt-1.5 text-muted-foreground">{label}</span>
              <MenuSelect
                value={typeof current === "string" ? current : ""}
                onChange={(slug) => set(key, slug === "" ? undefined : slug)}
              />
            </label>
          );
        }
        // The data binding gets its own editor: a source picker fed by
        // the live type list, not a JSON field.
        if (key === "bind" && spec.type === "object") {
          const bind = (
            typeof current === "object" && current !== null ? current : {}
          ) as BindValue;
          return (
            <BindingEditor
              key={key}
              value={bind}
              sources={sources}
              onChange={(next) => set(key, next)}
            />
          );
        }
        // Any other object-valued setting has no generic editor yet; a
        // text input would silently corrupt it, so it renders nothing.
        if (spec.type === "object") return null;
        // Arrays of records — feature cards, stats, questions.
        if (spec.type === "array") {
          return (
            <ItemsEditor
              key={key}
              label={label}
              schema={spec as SettingsSchema}
              items={Array.isArray(current) ? (current as Record<string, unknown>[]) : []}
              onChange={(next) => set(key, next.length === 0 ? undefined : next)}
            />
          );
        }
        if (spec.type === "boolean") {
          return (
            <label key={key} className="flex items-center gap-2 text-xs">
              <input
                id={id}
                type="checkbox"
                checked={current === true}
                onChange={(e) => set(key, e.target.checked ? true : undefined)}
              />
              <span>{label}</span>
            </label>
          );
        }
        if (spec.enum !== undefined) {
          return (
            <label key={key} className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
              <span className="text-muted-foreground">{label}</span>
              <select
                id={id}
                value={typeof current === "string" ? current : ""}
                onChange={(e) => set(key, e.target.value)}
                className={inputClass}
              >
                <option value="">Default</option>
                {spec.enum.map((v) => (
                  <option key={v} value={v}>
                    {v}
                  </option>
                ))}
              </select>
            </label>
          );
        }
        if (spec.type === "integer" || spec.type === "number") {
          return (
            <label key={key} className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
              <span className="text-muted-foreground">{label}</span>
              <input
                id={id}
                type="number"
                min={spec.minimum}
                max={spec.maximum}
                placeholder="Default"
                value={typeof current === "number" ? current : ""}
                onChange={(e) =>
                  set(key, e.target.value === "" ? undefined : Number(e.target.value))
                }
                className={inputClass}
              />
            </label>
          );
        }
        return (
          <label key={key} className="grid grid-cols-[6rem_1fr] items-start gap-2 text-xs">
            <span className="pt-1.5 text-muted-foreground">{label}</span>
            <input
              id={id}
              type="text"
              maxLength={spec.maxLength}
              placeholder="Default"
              value={typeof current === "string" ? current : ""}
              onChange={(e) => set(key, e.target.value)}
              className={inputClass}
            />
          </label>
        );
      })}
    </div>
  );
}

/** The insert menu, grouped and filtered. */
function AddSection({
  vocab,
  onAdd,
  exclude,
}: {
  vocab: Vocabulary;
  onAdd: (kind: string) => void;
  exclude: readonly string[];
}) {
  const [open, setOpen] = React.useState(false);
  const known = new Set(
    [
      ...vocab.blocks.map((b) => b.kind),
      ...vocab.static_regions.map((s) => s.kind),
    ].filter((k) => !exclude.includes(k)),
  );
  // Anything the server knows but no group claims still has to be reachable.
  const grouped = new Set(GROUPS.flatMap((g) => g.kinds));
  const groups = [
    ...GROUPS.map((g) => ({ ...g, kinds: g.kinds.filter((k) => known.has(k)) })),
    {
      label: "Other",
      kinds: [...known].filter((k) => !grouped.has(k)).sort(),
    },
  ].filter((g) => g.kinds.length > 0);

  return (
    <div className="relative">
      <Button
        variant="outline"
        size="sm"
        className="h-7 w-full text-xs"
        aria-expanded={open}
        onClick={() => setOpen((v) => !v)}
      >
        <Plus className="h-3.5 w-3.5" aria-hidden="true" />
        Add section
      </Button>
      {open ? (
        <div className="absolute left-0 right-0 top-8 z-20 max-h-72 overflow-y-auto rounded-md border bg-popover p-1.5 shadow-lg">
          {groups.map((group) => (
            <div key={group.label}>
              <p className="px-1.5 pb-0.5 pt-1.5 text-[10px] font-semibold uppercase tracking-wider text-muted-foreground">
                {group.label}
              </p>
              {group.kinds.map((kind) => {
                // Static regions carry descriptions too; looking only at
                // `blocks` left `content` — the one a composed page most
                // needs — as a bare word with nothing explaining it.
                const description =
                  vocab.blocks.find((b) => b.kind === kind)?.description ??
                  vocab.static_regions.find((r) => r.kind === kind)?.description;
                return (
                  <button
                    key={kind}
                    type="button"
                    aria-label={description ? `${kind} ${description}` : kind}
                    onClick={() => {
                      onAdd(kind);
                      setOpen(false);
                    }}
                    className="flex w-full flex-col rounded px-1.5 py-1 text-left hover:bg-accent"
                  >
                    <span className="text-xs">{kind}</span>
                    {description !== undefined && description !== "" ? (
                      <span className="truncate text-[10px] text-muted-foreground">
                        {description}
                      </span>
                    ) : null}
                  </button>
                );
              })}
            </div>
          ))}
        </div>
      ) : null}
    </div>
  );
}

/** What the vocabulary says a kind is, and what settings it takes. */
function describeKind(
  vocab: Vocabulary,
  kind: string,
): { description: string; schema: SettingsSchema | null } {
  const dyn = vocab.blocks.find((b) => b.kind === kind);
  if (dyn !== undefined) return { description: dyn.description, schema: dyn.settings_schema };
  const stat = vocab.static_regions.find((s) => s.kind === kind);
  if (stat !== undefined) return { description: stat.description, schema: null };
  // A namespaced kind the vocabulary does not list is a plugin section
  // whose plugin is off: the layout keeps it, the page shows nothing.
  if (kind.includes("/")) {
    return {
      description:
        "A plugin section that is not currently served — its plugin may be " +
        "disabled. It renders as nothing until the plugin returns.",
      schema: null,
    };
  }
  return { description: "Unknown section", schema: null };
}

/**
 * Everything about one section: what it is, what it is called, what it
 * takes, when it shows, and how it is painted.
 *
 * Split out of the row it used to expand inside so the studio can put the
 * structure in one rail and the properties of the selection in another —
 * the arrangement every design tool converges on, because a settings form
 * unfolding inside a list pushes the rest of the list off the screen.
 */
export function SectionProperties({
  sections,
  section,
  vocab,
  tokens,
  onChange,
  onRenamed,
}: {
  sections: Section[];
  section: Section;
  vocab: Vocabulary;
  tokens: TokenSet;
  onChange: (next: Section[]) => void;
  /** Renaming moves the id a selection is held by, so it has to be told. */
  onRenamed?: (id: string) => void;
}) {
  const { description, schema } = describeKind(vocab, section.kind);
  const edit = (fn: (s: Section) => Section) =>
    onChange(updateSection(sections, section.id, fn));

  return (
    <div className="space-y-2" data-testid="section-properties">
      <p className="text-[11px] text-muted-foreground">{description}</p>

      <label className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
        <span className="text-muted-foreground">Section id</span>
        <input
          aria-label={`${section.kind} id`}
          value={section.id}
          spellCheck={false}
          onChange={(e) => {
            const nextId = e.target.value;
            edit((x) => ({ ...x, id: nextId }));
            onRenamed?.(nextId);
          }}
          className={inputClass}
        />
      </label>

      {schema !== null ? (
        <SettingsForm
          schema={schema}
          settings={section.settings ?? {}}
          idPrefix={section.id}
          sources={vocab.sources ?? []}
          kind={section.kind}
          onChange={(next) =>
            edit((x) => ({
              ...x,
              settings: Object.keys(next).length === 0 ? undefined : next,
            }))
          }
        />
      ) : null}

      <label className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
        <span className="text-muted-foreground">Width</span>
        <select
          aria-label={`${section.id} width`}
          value={section.width ?? ""}
          onChange={(e) => {
            const v = e.target.value;
            edit((x) => ({
              ...x,
              width: v === "" ? undefined : (v as NonNullable<Section["width"]>),
            }));
          }}
          className={inputClass}
        >
          <option value="">Theme width</option>
          <option value="narrow">Narrow — prose column</option>
          <option value="wide">Wide</option>
          <option value="full">Full bleed</option>
        </select>
      </label>

      <label className="grid grid-cols-[6rem_1fr] items-center gap-2 text-xs">
        <span className="text-muted-foreground">Show on</span>
        <select
          aria-label={`${section.id} visibility`}
          value={section.hide_on ?? ""}
          onChange={(e) => {
            const v = e.target.value;
            edit((x) => ({ ...x, hide_on: v === "" ? undefined : (v as Screen) }));
          }}
          className={inputClass}
        >
          <option value="">Every screen</option>
          <option value="desktop">Phones only</option>
          <option value="mobile">Not on phones</option>
        </select>
      </label>

      <div className="border-t pt-2">
        <ScopeEditor
          scope={section.scope}
          tokens={tokens}
          onChange={(next) => edit((x) => ({ ...x, scope: next }))}
        />
      </div>
    </div>
  );
}

/**
 * The section tree editor, without any opinion about what it composes.
 *
 * A theme template and a single page are the same shape — an ordered tree of
 * sections — so they get the same editor. The template picker that used to
 * wrap this lives in LayoutPanel, which is the only thing that differs.
 */
export function SectionTree({
  sections,
  vocab,
  tokens,
  onChange,
  label,
  emptyHint,
  exclude = [],
  selected: controlled,
  onSelectedChange,
  inlineProperties = true,
  showAdd = true,
}: {
  sections: Section[];
  vocab: Vocabulary;
  tokens: TokenSet;
  onChange: (next: Section[]) => void;
  /** Accessible name for the list, e.g. "Home sections". */
  label: string;
  /** What to say when nothing has been added yet. */
  emptyHint: string;
  /** Kinds to leave out of the insert menu. */
  exclude?: readonly string[];
  /**
   * Which section is open, when something outside the tree decides.
   *
   * The studio's canvas is the other way in: clicking a section on the
   * page opens it here. Left undefined the tree keeps its own selection,
   * which is what the page editor wants.
   */
  selected?: string | null;
  onSelectedChange?: (id: string | null) => void;
  /**
   * Whether the tree offers its own "Add section" menu.
   *
   * The page editor has one column, so adding lives here. The studio has
   * an insert library in its rail — a second entry point inside the tree
   * would be two places doing one thing.
   */
  showAdd?: boolean;
  /**
   * Whether an open row unfolds its properties in place.
   *
   * The page editor wants that — it has one column. The studio does not:
   * it shows properties in its inspector, and a second copy inside the
   * rail would be two places to edit one thing.
   */
  inlineProperties?: boolean;
}) {
  const [own, setOwn] = React.useState<string | null>(null);
  const selected = controlled === undefined ? own : controlled;
  const setSelected = (id: string | null) => {
    setOwn(id);
    onSelectedChange?.(id);
  };
  const nodes = flatten(sections);

  // A selection made on the canvas may open a row scrolled out of sight.
  const listRef = React.useRef<HTMLOListElement>(null);
  React.useEffect(() => {
    if (controlled === undefined || controlled === null) return;
    listRef.current
      ?.querySelector(`[data-row-id="${CSS.escape(controlled)}"]`)
      ?.scrollIntoView?.({ block: "nearest" });
  }, [controlled]);

  const apply = onChange;

  const add = (kind: string) => {
    const id = uniqueId(kind, allIds(sections));
    // The vocabulary's sample settings put something real on the page
    // instead of an empty rectangle the author has to guess at.
    const sample = vocab.blocks.find((b) => b.kind === kind)?.sample;
    const section: Section =
      sample !== undefined && Object.keys(sample).length > 0
        ? { id, kind, settings: structuredClone(sample) }
        : { id, kind };
    // Insert after the selection so building a page reads top to bottom,
    // rather than always landing at the end.
    apply(insertSection(sections, section, selected));
    setSelected(id);
  };

  return (
    <div className="space-y-3" data-testid="section-tree">
      {showAdd ? <AddSection vocab={vocab} onAdd={add} exclude={exclude} /> : null}

      <ol ref={listRef} className="space-y-1" aria-label={label}>
        {nodes.map(({ section, depth }) => {
          const open = selected === section.id;
          const holdsChildren = isContainer(section.kind, vocab);

          return (
            <li
              key={section.id}
              data-row-id={section.id}
              style={{ marginLeft: `${depth * 0.85}rem` }}
            >
              <div
                className={cn(
                  "rounded-md border bg-card",
                  open && "border-primary ring-1 ring-primary/30",
                )}
              >
                <div className="flex items-center gap-1 p-1.5">
                  <button
                    type="button"
                    onClick={() => setSelected(open ? null : section.id)}
                    aria-expanded={open}
                    className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
                  >
                    <ChevronRight
                      className={cn(
                        "h-3 w-3 shrink-0 text-muted-foreground transition-transform",
                        open && "rotate-90",
                      )}
                      aria-hidden="true"
                    />
                    <span className="min-w-0">
                      <span className="block truncate text-xs font-medium">
                        {section.kind}
                        {holdsChildren ? (
                          // Parenthesised: a bare digit runs into the kind
                          // name, on screen and in the accessible name, so
                          // a band with one child read as "band1".
                          <span className="ml-1 text-[9px] font-normal text-muted-foreground">
                            ({section.children?.length ?? 0})
                          </span>
                        ) : null}
                      </span>
                      <span className="block truncate font-mono text-[10px] text-muted-foreground">
                        {section.id}
                      </span>
                    </span>
                    {section.scope !== undefined ? (
                      <span
                        title="Has a style scope"
                        aria-label="Has a style scope"
                        className="ml-auto h-2 w-2 shrink-0 rounded-full bg-primary"
                      />
                    ) : null}
                  </button>

                  <div className="flex shrink-0 gap-0.5">
                    <IconBtn
                      label={`Move ${section.id} up`}
                      onClick={() => apply(moveSection(sections, section.id, -1))}
                    >
                      <ArrowUp className="h-3 w-3" aria-hidden="true" />
                    </IconBtn>
                    <IconBtn
                      label={`Move ${section.id} down`}
                      onClick={() => apply(moveSection(sections, section.id, 1))}
                    >
                      <ArrowDown className="h-3 w-3" aria-hidden="true" />
                    </IconBtn>
                    <IconBtn
                      label={`Nest ${section.id} inside the section above`}
                      disabled={!canIndent(sections, section.id, vocab)}
                      onClick={() => apply(indentSection(sections, section.id, vocab))}
                    >
                      <ArrowRight className="h-3 w-3" aria-hidden="true" />
                    </IconBtn>
                    <IconBtn
                      label={`Move ${section.id} out of its container`}
                      disabled={!canOutdent(sections, section.id)}
                      onClick={() => apply(outdentSection(sections, section.id))}
                    >
                      <ArrowLeft className="h-3 w-3" aria-hidden="true" />
                    </IconBtn>
                    <IconBtn
                      label={`Remove ${section.id}`}
                      destructive
                      onClick={() => {
                        apply(removeSection(sections, section.id));
                        if (selected === section.id) setSelected(null);
                      }}
                    >
                      <Trash2 className="h-3 w-3" aria-hidden="true" />
                    </IconBtn>
                  </div>
                </div>

                {open && inlineProperties ? (
                  <div className="border-t p-2">
                    <SectionProperties
                      sections={sections}
                      section={section}
                      vocab={vocab}
                      tokens={tokens}
                      onChange={apply}
                      onRenamed={setSelected}
                    />
                  </div>
                ) : null}
              </div>
            </li>
          );
        })}
      </ol>

      {nodes.length === 0 ? (
        <p className="rounded-md border border-dashed p-4 text-center text-xs text-muted-foreground">
          {emptyHint}
        </p>
      ) : null}
    </div>
  );
}

function IconBtn({
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
      aria-label={label}
      title={label}
      disabled={disabled}
      onClick={onClick}
      className={cn(
        "rounded p-1 text-muted-foreground disabled:opacity-25",
        destructive === true
          ? "hover:bg-destructive/10 hover:text-destructive"
          : "hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}
