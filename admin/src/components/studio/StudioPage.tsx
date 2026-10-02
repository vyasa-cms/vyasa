import { useUnsavedChanges } from "@/lib/use-unsaved-changes";
import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  AlertTriangle,
  ArrowLeft,
  Check,
  Database,
  History,
  LayoutTemplate,
  Layers,
  Palette,
  Plus,
  Rocket,
  Sparkles,
  X,
  Undo2,
  FileCode2,
  Code2,
} from "lucide-react";

import {
  draftPreviewUrl,
  listRevisions,
  renameDraft,
  revertDraft,
} from "@/api/themes";
import { Button } from "@/components/ui/button";
import { Chip, ErrorNote, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { cn } from "@/lib/utils";

import type { Section, TemplateType } from "@/api/themes";
import { templateForPath } from "@/lib/preview-template";

import { AssistantPanel, useAssistantModels } from "./AssistantPanel";
import { DataPanel } from "./DataPanel";
import { InsertPanel } from "./InsertPanel";
import { SectionProperties } from "./SectionTree";
import {
  allIds,
  findSection,
  insertRelativeTo,
  insertSection,
  moveRelativeTo,
  removeSection,
  uniqueId,
  updateSection,
} from "./sectionTree";
import { HistoryDialog, PublishDialog, slugify } from "./dialogs";
import { LayoutPanel } from "./LayoutPanel";
import { PreviewPane } from "./PreviewPane";
import { AssetsPanel } from "./AssetsPanel";
import { TemplatesPanel } from "./TemplatesPanel";
import { TokensPanel } from "./TokensPanel";
import { useDraft } from "./useDraft";

/**
 * The tool rail: every way of working on the draft, one icon each.
 *
 * Libraries and structure live on the left; the inspector on the right is
 * contextual — the selected section's properties, or the theme's own
 * (tokens) when nothing is selected, which is the same rule the studio
 * has had since it shipped.
 */
type Rail = "insert" | "layers" | "assistant" | "data" | "templates" | "assets";

const RAIL: { key: Rail; label: string; icon: typeof Palette }[] = [
  { key: "insert", label: "Insert", icon: Plus },
  { key: "layers", label: "Layers", icon: Layers },
  { key: "assistant", label: "Assistant", icon: Sparkles },
  { key: "data", label: "Data", icon: Database },
  { key: "templates", label: "Templates", icon: FileCode2 },
  { key: "assets", label: "Assets", icon: Code2 },
];

/**
 * The revision "undo" should land on. Reverts are themselves revisions,
 * so after going back to #4 (recorded as #6) the next undo continues to
 * #3 rather than bouncing between #5 and #4.
 */
export function undoTarget(
  latest: { seq: number; source: string; note: string } | undefined,
): number | null {
  if (latest === undefined || latest.seq <= 1) return null;
  if (latest.source === "revert") {
    const m = /revision (\d+)/.exec(latest.note);
    const from = m?.[1] === undefined ? null : Number(m[1]);
    if (from !== null && from > 1) return from - 1;
    return null;
  }
  return latest.seq - 1;
}

export function StudioPage({
  draftId,
  onExit,
}: {
  draftId: string;
  onExit: () => void;
}) {
  const queryClient = useQueryClient();
  const d = useDraft(draftId);
  const canLeave = useUnsavedChanges(d.isDirty, d.flush);
  // One confirmation per exit: `leave` stands the route blocker down for
  // the navigation it confirmed.
  const exit = async () => { await canLeave.leave(onExit); };
  const models = useAssistantModels();
  const [mobilePane, setMobilePane] = React.useState<"tools" | "preview" | "properties">("preview");
  const [rail, setRail] = React.useState<Rail>("insert");
  // What is picked on the canvas, and which tree it belongs to. Held here
  // rather than in either pane because both of them drive it.
  const [selected, setSelected] = React.useState<string | null>(null);
  const [template, setTemplate] = React.useState<TemplateType>("index");
  const [historyOpen, setHistoryOpen] = React.useState(false);
  const [publishOpen, setPublishOpen] = React.useState(false);
  const [editingName, setEditingName] = React.useState<string | null>(null);

  const revisions = useQuery({
    queryKey: ["theme-draft-revisions", draftId],
    queryFn: () => listRevisions(draftId),
  });
  // Any saved revision changes the history.
  const revision = d.draft?.revision;
  React.useEffect(() => {
    void queryClient.invalidateQueries({
      queryKey: ["theme-draft-revisions", draftId],
    });
  }, [revision, draftId, queryClient]);

  const rename = useMutation({
    mutationFn: (name: string) => renameDraft(draftId, name),
    onSuccess: (draft) => {
      queryClient.setQueryData(["theme-draft", draftId], draft);
      void queryClient.invalidateQueries({ queryKey: ["theme-drafts"] });
    },
    onError: (e) => notify.error("Couldn't rename the draft", e),
  });

  const undo = useMutation({
    mutationFn: (seq: number) => revertDraft(draftId, seq),
    onSuccess: (draft, seq) => {
      d.reset(draft);
      notify.success(`Back to revision ${seq}`);
    },
    onError: (e) => notify.error("Couldn't undo", e),
  });

  /**
   * Save what's pending first — otherwise the revert lands on the server
   * copy and the last few edits vanish — then undo the newest revision,
   * which after the flush may be the edit just saved.
   */
  const undoLatest = async () => {
    try {
      await d.flush();
    } catch (e) {
      notify.error("Couldn't save the draft", e);
      return;
    }
    const latest = await queryClient.fetchQuery({
      queryKey: ["theme-draft-revisions", draftId],
      queryFn: () => listRevisions(draftId),
      staleTime: 0,
    });
    const to = undoTarget(latest[0]);
    if (to !== null) undo.mutate(to);
  };

  if (d.loadError !== null && d.loadError !== undefined) {
    return (
      <div className="space-y-3">
        <Button variant="ghost" size="sm" onClick={() => void exit()}>
          <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          Appearance
        </Button>
        <ErrorNote title="Couldn't open the draft" error={d.loadError} />
      </div>
    );
  }
  if (
    d.isLoading ||
    d.draft === undefined ||
    d.working === null ||
    d.vocab === undefined
  ) {
    return (
      <div className="space-y-3">
        <Skeleton className="h-8 w-64" />
        <Skeleton className="h-[60vh] w-full rounded-lg" />
      </div>
    );
  }

  const draft = d.draft;
  const target = undoTarget(revisions.data?.[0]);
  const warningCount = draft.warnings.length;

  // The tree the canvas is actually showing. Ids repeat across templates —
  // every template has a `header` — so the previewed path is what says
  // which `header` a click on the page meant.
  const canvasTemplate = templateForPath(d.previewPath);
  const railSections = d.working.layout[template];
  const selectedSection =
    selected === null ? undefined : findSection(railSections, selected);

  const pickOnCanvas = (id: string | null) => {
    setSelected(id);
    if (id === null) return;
    setTemplate(canvasTemplate);
    setRail("layers");
  };

  /** Dropping a dragged section at a spot on the page. */
  const moveOnCanvas = (id: string, targetId: string, place: "before" | "after") => {
    const tree = d.working?.layout[canvasTemplate] ?? [];
    d.setLayout(canvasTemplate, moveRelativeTo(tree, id, targetId, place));
    setTemplate(canvasTemplate);
  };

  /** A pick from the insert library, landing after the selection. */
  const insertKind = (kind: string) => {
    const tree = d.working?.layout[canvasTemplate] ?? [];
    const sample = d.vocab?.blocks.find((b) => b.kind === kind)?.sample;
    const id = uniqueId(kind, allIds(tree));
    const section: Section =
      sample !== undefined && Object.keys(sample).length > 0
        ? { id, kind, settings: structuredClone(sample) }
        : { id, kind };
    d.setLayout(canvasTemplate, insertSection(tree, section, selected));
    setTemplate(canvasTemplate);
    setSelected(id);
  };

  /** A card dropped on the canvas: insert exactly where the line was. */
  const insertKindAt = (
    kind: string,
    targetId: string | null,
    place: "before" | "after",
  ) => {
    const tree = d.working?.layout[canvasTemplate] ?? [];
    const sample = d.vocab?.blocks.find((b) => b.kind === kind)?.sample;
    const id = uniqueId(kind, allIds(tree));
    const section: Section =
      sample !== undefined && Object.keys(sample).length > 0
        ? { id, kind, settings: structuredClone(sample) }
        : { id, kind };
    const next =
      targetId === null
        ? insertSection(tree, section, null)
        : insertRelativeTo(tree, section, targetId, place);
    d.setLayout(canvasTemplate, next);
    setTemplate(canvasTemplate);
    setSelected(id);
  };

  /** Text edited in place on the canvas. Same tree, same sync. */
  const editOnCanvas = (id: string, key: string, value: string) => {
    const tree = d.working?.layout[canvasTemplate] ?? [];
    d.setLayout(
      canvasTemplate,
      updateSection(tree, id, (s) => ({
        ...s,
        settings: { ...s.settings, [key]: value },
      })),
    );
  };

  /** Delete asked for from the canvas keyboard. */
  const deleteOnCanvas = (id: string) => {
    const tree = d.working?.layout[canvasTemplate] ?? [];
    d.setLayout(canvasTemplate, removeSection(tree, id));
    setSelected(null);
  };

  const railPanel =
    rail === "insert" ? (
      <InsertPanel vocab={d.vocab} onInsert={insertKind} />
    ) : rail === "layers" ? (
      <LayoutPanel
        layout={d.working.layout}
        vocab={d.vocab}
        tokens={d.working.tokens}
        onChange={d.setLayout}
        template={template}
        onTemplateChange={setTemplate}
        selected={selected}
        onSelectedChange={setSelected}
        inlineProperties={false}
        showAdd={false}
      />
    ) : rail === "assistant" ? (
      <AssistantPanel draft={draft} models={models} onApplied={d.reset} />
    ) : rail === "data" ? (
      <DataPanel vocab={d.vocab} />
    ) : rail === "templates" ? (
      <TemplatesPanel
        templates={d.working.templates}
        vocab={d.vocab}
        onApply={d.setTemplate}
        onRemove={d.removeTemplate}
      />
    ) : (
      <AssetsPanel assets={d.working.assets} onChange={d.setAssets} />
    );

  return (
    /* `dark` is chosen, not inherited: the studio is a place you go to look
       at a page, and light chrome around a light artboard leaves nothing to
       say where the tool stops and the site begins. */
    <div
      className="dark flex min-h-[calc(100vh-7rem)] flex-col gap-3 rounded-xl bg-background p-3 text-foreground"
      data-testid="studio"
    >
      <div className="flex flex-wrap items-center gap-2">
        <Button variant="ghost" size="sm" onClick={() => void exit()}>
          <ArrowLeft className="h-4 w-4" aria-hidden="true" />
          <span className="hidden sm:inline">Appearance</span>
        </Button>
        {editingName === null ? (
          <button
            type="button"
            onClick={() => setEditingName(draft.name)}
            title="Rename"
            className="min-w-0 truncate rounded px-1 text-base font-semibold hover:bg-accent sm:text-lg"
          >
            {draft.name}
          </button>
        ) : (
          <input
            autoFocus
            aria-label="Draft name"
            value={editingName}
            onChange={(e) => setEditingName(e.target.value)}
            onBlur={() => {
              const name = editingName.trim();
              if (name !== "" && name !== draft.name) rename.mutate(name);
              setEditingName(null);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") setEditingName(null);
            }}
            className="h-8 rounded-md border bg-background px-2 text-base font-semibold"
          />
        )}
        <Chip tone="info">Revision {draft.revision}</Chip>
        <SaveIndicator state={d.saveState} />
        {warningCount > 0 ? (
          <button
            type="button"
            onClick={() => setSelected(null)}
            className="inline-flex"
            title={draft.warnings
              .map((w) => `${w.path}: ${w.message}`)
              .join("\n")}
          >
            <Chip tone="warning">
              <AlertTriangle className="h-3 w-3" aria-hidden="true" />
              {warningCount} {warningCount === 1 ? "warning" : "warnings"}
            </Chip>
          </button>
        ) : null}

        <div className="ml-auto flex items-center gap-1.5">
          <Button
            size="sm"
            variant="ghost"
            disabled={target === null || undo.isPending}
            onClick={() => { if (target !== null) void undoLatest(); }}
            title={
              target === null
                ? "Nothing to undo"
                : `Go back to revision ${target}`
            }
          >
            <Undo2 className="h-4 w-4" aria-hidden="true" />
            <span className="hidden sm:inline">Undo</span>
          </Button>
          <Button
            size="sm"
            variant="ghost"
            onClick={() => setHistoryOpen(true)}
          >
            <History className="h-4 w-4" aria-hidden="true" />
            <span className="hidden sm:inline">History</span>
          </Button>
          <Button
            size="sm"
            onClick={() => { void d.flush().then(() => setPublishOpen(true)).catch(e => notify.error("Couldn't save the draft", e)); }}
            data-testid="publish-button"
          >
            <Rocket className="h-4 w-4" aria-hidden="true" />
            Publish…
          </Button>
        </div>
      </div>

      {d.conflict !== null ? (
        <div
          role="alert"
          data-testid="draft-conflict"
          className="flex flex-wrap items-start gap-3 rounded-md border border-amber-500/40 bg-amber-500/10 px-3 py-2 text-sm"
        >
          <div className="min-w-0 flex-1">
            <p className="font-medium">
              The draft changed elsewhere (revision {d.conflict.revision}).
            </p>
            <p className="text-xs text-muted-foreground">
              Autosave is paused. Keep your edits to apply only the sections
              you changed on top of the newer version, or take the newer
              version and drop your unsaved edits.
            </p>
          </div>
          <Button size="sm" onClick={d.keepMine}>
            Keep my edits
          </Button>
          <Button size="sm" variant="outline" onClick={d.takeTheirs}>
            Take the newer version
          </Button>
        </div>
      ) : null}

      {d.saveError !== null ? (
        <div
          role="alert"
          className="flex flex-wrap items-start gap-3 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-sm"
        >
          <div className="min-w-0 flex-1">
            <p className="font-medium text-destructive">
              This change wasn&rsquo;t saved.
            </p>
            <pre className="whitespace-pre-wrap font-mono text-[11px] text-destructive/90">
              {d.saveError}
            </pre>
          </div>
          <Button size="sm" variant="outline" onClick={d.discard}>
            Discard the change
          </Button>
        </div>
      ) : null}

      <div className="flex gap-2 xl:hidden" role="tablist" aria-label="Studio panes">
        {(["tools", "preview", "properties"] as const).map(pane => <button type="button" role="tab" key={pane} aria-selected={mobilePane === pane} onClick={() => setMobilePane(pane)} className={cn("min-w-0 flex-1 rounded-md border px-2 py-2 text-sm capitalize", mobilePane === pane && "bg-primary text-primary-foreground")}>{pane}</button>)}
      </div>
      <div className="min-h-0 flex-1 overflow-x-auto">
        <div
          className={cn(
            "grid h-full min-h-0 min-w-0 gap-3 xl:min-w-[64rem]",
            rail === "templates" || rail === "assets"
              ? "xl:grid-cols-[2.75rem_28rem_minmax(0,1fr)_16rem] 2xl:grid-cols-[2.75rem_30rem_minmax(0,1fr)_19rem]"
              : "xl:grid-cols-[2.75rem_15rem_minmax(0,1fr)_16rem] 2xl:grid-cols-[2.75rem_17rem_minmax(0,1fr)_19rem]",
          )}
          data-testid="studio-panes"
        >
          {/* The tool rail: one icon per way of working on the draft. */}
          <div
            role="tablist"
            aria-label="Tools"
            aria-orientation="vertical"
            className={cn("max-h-[calc(100vh-11rem)] items-center gap-1 rounded-lg border bg-card py-1.5 xl:flex xl:flex-col", mobilePane === "tools" ? "flex flex-wrap" : "hidden")}
            data-testid="studio-rail"
          >
            {RAIL.map((t) => {
              const Icon = t.icon;
              return (
                <button
                  key={t.key}
                  role="tab"
                  aria-selected={rail === t.key}
                  aria-label={t.label}
                  title={t.label}
                  onClick={() => setRail(t.key)}
                  className={cn(
                    "inline-flex h-8 w-8 items-center justify-center rounded-md",
                    rail === t.key
                      ? "bg-primary/15 text-primary"
                      : "text-muted-foreground hover:bg-accent hover:text-foreground",
                  )}
                >
                  <Icon className="h-4 w-4" aria-hidden="true" />
                </button>
              );
            })}
          </div>

          {/* The rail's panel. Code panels get more room: an editor in a
              sidebar column is a keyhole, not a tool. */}
          <div
            className={cn("max-h-[calc(100vh-11rem)] min-h-0 min-w-0 flex-col rounded-lg border bg-card xl:flex", mobilePane === "tools" ? "flex" : "hidden")}
            data-testid="pane-rail"
          >
            <div className="border-b px-3 py-2 text-xs font-medium">
              {RAIL.find((t) => t.key === rail)?.label}
            </div>
            <div className="min-h-0 flex-1 overflow-y-auto p-3">{railPanel}</div>
          </div>

          <div className={cn("min-w-0 xl:block", mobilePane !== "preview" && "hidden")}><PreviewPane
            html={d.previewHtml}
            error={d.previewError}
            loading={d.previewLoading}
            path={d.previewPath}
            onPathChange={d.setPreviewPath}
            openUrl={draftPreviewUrl(draftId, d.previewPath)}
            title={draft.name}
            tokens={d.working.tokens}
            selectedId={selected}
            onSelect={pickOnCanvas}
            onMove={moveOnCanvas}
            onInlineEdit={editOnCanvas}
            onDelete={deleteOnCanvas}
            onInsertKind={insertKindAt}
          /></div>

          {/* The inspector is contextual: the selected section's
              properties, or the theme's own — its tokens — when nothing
              is selected. */}
          <div
            className={cn("max-h-[calc(100vh-11rem)] min-h-0 min-w-0 flex-col rounded-lg border bg-card xl:flex", mobilePane === "properties" ? "flex" : "hidden")}
            data-testid="inspector"
          >
            {selectedSection === undefined ? (
              <>
                <div className="flex items-center gap-2 border-b px-3 py-2">
                  <Palette
                    className="h-3.5 w-3.5 shrink-0 text-primary"
                    aria-hidden="true"
                  />
                  <span className="text-xs font-medium">Theme</span>
                </div>
                <div className="min-h-0 flex-1 overflow-y-auto p-3">
                  <TokensPanel
                    tokens={d.working.tokens}
                    warnings={draft.warnings}
                    onChange={d.setTokens}
                  />
                </div>
              </>
            ) : (
              <>
                {/* With something selected the inspector is about that, not
                    about the theme. The tokens come back when you let go. */}
                <div className="flex items-center gap-2 border-b px-3 py-2">
                  <LayoutTemplate
                    className="h-3.5 w-3.5 shrink-0 text-primary"
                    aria-hidden="true"
                  />
                  <span className="min-w-0">
                    <span className="block truncate text-xs font-medium">
                      {selectedSection.kind}
                    </span>
                    <span className="block truncate font-mono text-[10px] text-muted-foreground">
                      {selectedSection.id}
                    </span>
                  </span>
                  <button
                    type="button"
                    aria-label="Close properties"
                    onClick={() => setSelected(null)}
                    className="ml-auto rounded p-1 text-muted-foreground hover:bg-accent hover:text-foreground"
                  >
                    <X className="h-3.5 w-3.5" aria-hidden="true" />
                  </button>
                </div>
                <div className="min-h-0 flex-1 overflow-y-auto p-3">
                  <SectionProperties
                    sections={railSections}
                    section={selectedSection}
                    vocab={d.vocab}
                    tokens={d.working.tokens}
                    onChange={(next) => d.setLayout(template, next)}
                    onRenamed={setSelected}
                  />
                </div>
              </>
            )}
          </div>
        </div>
      </div>

      <HistoryDialog
        open={historyOpen}
        onClose={() => setHistoryOpen(false)}
        draftId={draftId}
        currentRevision={draft.revision}
        onReverted={d.reset}
      />
      <PublishDialog
        open={publishOpen}
        onClose={() => setPublishOpen(false)}
        draftId={draftId}
        suggestedName={slugify(draft.name.replace(/\(draft\)/i, "")) || "theme"}
        onPublished={(_theme, activated) => {
          if (activated) onExit();
        }}
      />
    </div>
  );
}

function SaveIndicator({ state }: { state: "saved" | "saving" | "error" }) {
  if (state === "saving") {
    return (
      <span
        className="inline-flex items-center gap-1 text-[11px] text-muted-foreground"
        role="status"
      >
        <span className="h-2.5 w-2.5 animate-spin rounded-full border-2 border-muted border-t-primary" />
        Saving
      </span>
    );
  }
  if (state === "error") {
    return (
      <span
        className="inline-flex items-center gap-1 text-[11px] text-destructive"
        role="status"
      >
        <AlertTriangle className="h-3 w-3" aria-hidden="true" />
        Not saved
      </span>
    );
  }
  return (
    <span
      className="inline-flex items-center gap-1 text-[11px] text-muted-foreground"
      role="status"
    >
      <Check className="h-3 w-3" aria-hidden="true" />
      Saved
    </span>
  );
}

