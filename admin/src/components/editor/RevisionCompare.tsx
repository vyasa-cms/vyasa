import * as React from "react";
import { diffWords } from "diff";
import { Button } from "@/components/ui/button";
import { Modal } from "@/components/ui/dialog";
import { formatRelative } from "@/routes/_auth/index";
import { normalizeBlocks, plainTextOf, type Block } from "./blocks";

export interface RevisionSummary {
  id: string;
  title: string;
  content: unknown;
  is_autosave: boolean;
  created_at: string;
  /** Field values recorded with it; null for a revision older than fields. */
  fields?: Record<string, unknown> | null;
}

/**
 * What changed between a revision and what is in the editor now, as a
 * word-level diff of the text. Layout and media are not compared — this
 * answers "what did I write since then", which is the question to answer
 * before loading an old version back into the editor.
 */
export function RevisionCompare({
  revision,
  currentTitle,
  currentBlocks,
  restoring,
  onRestore,
  onClose,
}: {
  revision: RevisionSummary;
  currentTitle: string;
  currentBlocks: Block[];
  restoring: boolean;
  onRestore: () => void;
  onClose: () => void;
}) {
  const before = React.useMemo(
    () => plainTextOf(normalizeBlocks((revision.content as { blocks?: unknown } | null)?.blocks)),
    [revision],
  );
  const after = React.useMemo(() => plainTextOf(currentBlocks), [currentBlocks]);
  const parts = React.useMemo(() => diffWords(before, after), [before, after]);
  const bodyChanged = parts.some((p) => p.added === true || p.removed === true);
  const titleChanged = revision.title !== currentTitle;

  return (
    <Modal
      open
      onClose={onClose}
      size="lg"
      testId="revision-compare"
      title="Compare with this revision"
      description={`${revision.is_autosave ? "Autosave" : "Saved"} ${formatRelative(revision.created_at)}. Highlights show what is different in the editor now.`}
      footer={
        <>
          <Button variant="outline" onClick={onClose}>
            Close
          </Button>
          <Button onClick={onRestore} disabled={restoring}>
            {restoring ? "Loading…" : "Load into editor"}
          </Button>
        </>
      }
    >
      <div className="space-y-3 text-sm">
        <p className="flex flex-wrap gap-3 text-xs text-muted-foreground">
          <span>
            <span className="rounded bg-destructive/20 px-1 text-destructive">removed</span> was in the
            revision
          </span>
          <span>
            <span className="rounded bg-emerald-500/20 px-1">added</span> is only in the editor now
          </span>
        </p>

        {titleChanged ? (
          <p>
            <span className="font-medium">Title: </span>
            <del className="rounded bg-destructive/20 px-0.5 text-destructive">{revision.title || "Untitled"}</del>{" "}
            <ins className="rounded bg-emerald-500/20 px-0.5 no-underline">{currentTitle || "Untitled"}</ins>
          </p>
        ) : null}

        {!bodyChanged && !titleChanged ? (
          <p className="text-muted-foreground">The text is identical to what is in the editor.</p>
        ) : (
          <pre className="max-h-[50vh] overflow-y-auto whitespace-pre-wrap rounded-lg border bg-muted/30 p-3 font-sans leading-relaxed">
            {parts.map((part, i) =>
              part.added === true ? (
                <ins key={i} className="rounded bg-emerald-500/20 no-underline">
                  {part.value}
                </ins>
              ) : part.removed === true ? (
                <del key={i} className="rounded bg-destructive/20 text-destructive">
                  {part.value}
                </del>
              ) : (
                <span key={i}>{part.value}</span>
              ),
            )}
          </pre>
        )}
      </div>
    </Modal>
  );
}
