import { useMutation, useQuery } from "@tanstack/react-query";
import * as React from "react";
import { LayoutTemplate, Sparkles, Undo2 } from "lucide-react";

import { api } from "@/api/client";
import * as themes from "@/api/themes";
import type { Section } from "@/api/themes";
import { SectionTree } from "@/components/studio/SectionTree";
import {
  moveRelativeTo,
  removeSection,
  updateSection,
} from "@/components/studio/sectionTree";
import { SectionPreview } from "./SectionPreview";
import { Button } from "@/components/ui/button";
import { ErrorNote } from "@/components/ui/primitives";

/**
 * Chrome the theme places itself.
 *
 * The renderer skips these inside a page tree so a composed page cannot end
 * up with two headers — which means offering them here would let an author
 * add a section that does nothing.
 */
const THEME_CHROME = ["header", "nav", "footer", "sidebar-left", "sidebar-right"] as const;

/**
 * The page's own arrangement.
 *
 * A page used to be a title and a block document poured into whatever shape
 * the theme's `page` template had, so every page on a site looked the same.
 * This is where an author says otherwise for one page — a hero, their words,
 * a dark call-to-action band — without touching the theme every other page
 * still uses.
 *
 * Empty means "render through the theme template", which is what nearly
 * every page wants; the panel says so rather than looking broken.
 */
export function PageSections({
  sections,
  onChange,
  postId,
  document,
}: {
  sections: Section[];
  onChange: (next: Section[]) => void;
  /** The saved page. Composing needs somewhere to read the page from. */
  postId?: string;
  /** The block document as the editor holds it, unsaved edits included. */
  document?: unknown;
}) {
  const vocab = useQuery({ queryKey: ["theme-vocabulary"], queryFn: themes.vocabulary });
  // One selection, shared by the tree and the canvas — the same contract
  // the studio keeps, because "click the page, edit the thing" should not
  // depend on which editor you happen to be in.
  const [selected, setSelected] = React.useState<string | null>(null);
  // The scope editor resolves `$role` swatches against the palette the page
  // will actually render in, so it needs the *active* theme's tokens.
  const active = useQuery({
    queryKey: ["active-theme-tokens"],
    queryFn: async () => {
      const list = await themes.listThemes();
      const current = list.find((t) => t.is_active);
      return current === undefined ? null : await themes.themeTokens(current.id);
    },
  });

  if (vocab.isError) {
    return <ErrorNote title="Couldn't load the section library" error={vocab.error} />;
  }
  if (active.isError) {
    return <ErrorNote title="Couldn't load the theme's palette" error={active.error} />;
  }
  if (vocab.data === undefined || active.data === undefined) {
    return <div className="min-h-[50vh] animate-pulse rounded-lg bg-muted/40" />;
  }
  if (active.data === null) {
    return (
      <p className="rounded-lg border border-dashed p-6 text-center text-sm text-muted-foreground">
        Activate a theme before composing a page — sections take their colours
        and spacing from it.
      </p>
    );
  }

  return (
    <div className="space-y-3" data-testid="page-sections">
      <div className="flex items-start gap-2 rounded-lg border bg-muted/30 p-3 text-xs text-muted-foreground">
        <LayoutTemplate className="mt-0.5 h-4 w-4 shrink-0" aria-hidden="true" />
        <p className="min-w-0">
          Sections replace this page's body. Your header and footer still come
          from the theme. Add a <span className="font-medium">content</span>{" "}
          section wherever the writing above should appear.
        </p>
      </div>

      <Compose
        postId={postId}
        document={document}
        sections={sections}
        onChange={onChange}
      />

      {/* Tree and page side by side where there is room; stacked below,
          with the page first — on a phone the thing worth seeing is the
          page, not the outline of it. */}
      <div className="grid gap-3 xl:grid-cols-[22rem_minmax(0,1fr)] 2xl:grid-cols-[26rem_minmax(0,1fr)]">
        {postId === undefined ? null : (
          <div className="xl:order-2">
            <SectionPreview
              postId={postId}
              sections={sections}
              document={document}
              tokens={active.data.tokens}
              selectedId={selected}
              onSelect={setSelected}
              onMove={(id, targetId, place) =>
                onChange(moveRelativeTo(sections, id, targetId, place))
              }
              onInlineEdit={(id, key, value) =>
                onChange(
                  updateSection(sections, id, (s) => ({
                    ...s,
                    settings: { ...s.settings, [key]: value },
                  })),
                )
              }
              onDelete={(id) => {
                onChange(removeSection(sections, id));
                setSelected(null);
              }}
            />
          </div>
        )}
        <div className="min-w-0 xl:order-1">
          <SectionTree
            sections={sections}
            vocab={vocab.data}
            tokens={active.data.tokens}
            onChange={onChange}
            label="Page sections"
            exclude={THEME_CHROME}
            selected={selected}
            onSelectedChange={setSelected}
            emptyHint="No sections — this page renders through the theme's page template. Add one to compose it yourself."
          />
        </div>
      </div>
    </div>
  );
}

/**
 * Ask for a page in a sentence.
 *
 * The proposal lands in the editor rather than in the database: the author
 * sees it in the tree, keeps or undoes it, and saves like any other edit.
 * A suggestion that misses costs a glance, not an undo of real work.
 */
function Compose({
  postId,
  document,
  sections,
  onChange,
}: {
  postId?: string;
  document?: unknown;
  sections: Section[];
  onChange: (next: Section[]) => void;
}) {
  const [message, setMessage] = React.useState("");
  const [reply, setReply] = React.useState<string | null>(null);
  const [notes, setNotes] = React.useState<string[]>([]);
  // What the tree was before, so a proposal can never eat hand-built work.
  const previous = React.useRef<Section[] | null>(null);

  const compose = useMutation({
    mutationFn: () =>
      api.composePage(postId as string, {
        message: message.trim(),
        content: document,
        sections,
      }),
    onSuccess: (result) => {
      previous.current = sections;
      onChange(Array.isArray(result.sections) ? (result.sections as Section[]) : []);
      setReply(result.reply);
      // Contrast notes and the like: the tree is valid, but a band whose
      // secondary text is unreadable only looks wrong once published.
      const stepCount = result.steps?.length ?? 0;
      setNotes([
        ...(stepCount > 1 ? [`worked in ${stepCount} steps — edited, looked, adjusted`] : []),
        ...(result.warnings ?? []).map((w) => w.message),
      ]);
      setMessage("");
    },
  });

  if (postId === undefined) {
    return (
      <p className="rounded-lg border border-dashed p-3 text-xs text-muted-foreground">
        Save the page once and the designer can compose it for you.
      </p>
    );
  }

  return (
    <div className="space-y-2 rounded-lg border bg-card p-3" data-testid="compose">
      <div className="flex items-center gap-1.5">
        <Sparkles className="h-3.5 w-3.5 text-primary" aria-hidden="true" />
        <span className="text-xs font-medium">Compose this page</span>
      </div>

      <textarea
        rows={2}
        value={message}
        aria-label="What should this page do?"
        placeholder="A landing page for launch week: hero, three features, a dark call to action."
        onChange={(e) => setMessage(e.target.value)}
        className="w-full resize-y rounded-md border border-input bg-background p-2 text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      />

      <div className="flex items-center gap-2">
        <Button
          size="sm"
          className="h-7 text-xs"
          disabled={message.trim() === "" || compose.isPending}
          onClick={() => compose.mutate()}
        >
          {compose.isPending ? "Composing…" : "Compose"}
        </Button>
        {previous.current !== null ? (
          <Button
            size="sm"
            variant="ghost"
            className="h-7 text-xs text-muted-foreground"
            onClick={() => {
              const back = previous.current;
              if (back === null) return;
              onChange(back);
              previous.current = null;
              setReply(null);
              setNotes([]);
            }}
          >
            <Undo2 className="h-3 w-3" aria-hidden="true" />
            Undo
          </Button>
        ) : null}
      </div>

      {compose.isError ? (
        <ErrorNote title="Couldn't compose the page" error={compose.error} />
      ) : null}
      {reply !== null ? (
        <p className="text-[11px] leading-snug text-muted-foreground" role="status">
          {reply}
        </p>
      ) : null}
      {notes.length > 0 ? (
        <ul className="space-y-0.5 text-[11px] leading-snug text-amber-700 dark:text-amber-500">
          {notes.map((note) => (
            <li key={note}>{note}</li>
          ))}
        </ul>
      ) : null}
      <p className="text-[10px] text-muted-foreground">
        Nothing is saved until you do.
      </p>
    </div>
  );
}
