import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { History, Rocket } from "lucide-react";

import {
  listRevisions,
  listThemes,
  publishDraft,
  revertDraft,
  type Draft,
  type Revision,
  type ThemeSummary,
} from "@/api/themes";
import { Button } from "@/components/ui/button";
import { Modal } from "@/components/ui/dialog";
import { Chip, Field, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { formatRelative } from "@/routes/_auth/index";

const SOURCE_LABEL: Record<Revision["source"], { label: string; tone: "neutral" | "info" | "success" | "warning" }> = {
  start: { label: "Start", tone: "neutral" },
  you: { label: "You", tone: "info" },
  assistant: { label: "Assistant", tone: "success" },
  revert: { label: "Went back", tone: "warning" },
};

export function HistoryDialog({
  open,
  onClose,
  draftId,
  currentRevision,
  onReverted,
}: {
  open: boolean;
  onClose: () => void;
  draftId: string;
  currentRevision: number;
  onReverted: (draft: Draft) => void;
}) {
  const revisions = useQuery({
    queryKey: ["theme-draft-revisions", draftId],
    queryFn: () => listRevisions(draftId),
    enabled: open,
  });
  const revert = useMutation({
    mutationFn: (seq: number) => revertDraft(draftId, seq),
    onSuccess: (draft, seq) => {
      onReverted(draft);
      notify.success(`Back to revision ${seq}`, "Recorded as a new revision, so you can return.");
      onClose();
    },
    onError: (e) => notify.error("Couldn't go back", e),
  });

  return (
    <Modal open={open} onClose={onClose} title="History" description="Every change, newest first." testId="history-dialog">
      {revisions.isPending ? (
        <div className="space-y-2">
          {Array.from({ length: 4 }, (_, i) => (
            <Skeleton key={i} className="h-10 w-full" />
          ))}
        </div>
      ) : revisions.isError ? (
        <p className="text-sm text-destructive">{revisions.error.message}</p>
      ) : (
        <ol className="max-h-[60vh] divide-y overflow-y-auto">
          {(revisions.data ?? []).map((r) => {
            const meta = SOURCE_LABEL[r.source] ?? SOURCE_LABEL.you;
            return (
              <li key={r.seq} className="flex items-center gap-3 py-2 text-sm">
                <span className="w-8 shrink-0 font-mono text-xs text-muted-foreground">
                  #{r.seq}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="block truncate">{r.note}</span>
                  <span className="block text-[11px] text-muted-foreground">
                    {formatRelative(r.created_at)}
                  </span>
                </span>
                <Chip tone={meta.tone}>{meta.label}</Chip>
                {r.seq === currentRevision ? (
                  <span className="w-20 text-right text-[11px] text-muted-foreground">Current</span>
                ) : (
                  <Button
                    size="sm"
                    variant="outline"
                    className="w-20"
                    disabled={revert.isPending}
                    onClick={() => revert.mutate(r.seq)}
                  >
                    <History className="h-3.5 w-3.5" aria-hidden="true" />
                    Go back
                  </Button>
                )}
              </li>
            );
          })}
        </ol>
      )}
    </Modal>
  );
}

/** A package name from a working title: `Blog (draft)` → `blog-draft`. */
export function slugify(name: string): string {
  return name
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 60);
}

export function PublishDialog({
  open,
  onClose,
  draftId,
  suggestedName,
  onPublished,
}: {
  open: boolean;
  onClose: () => void;
  draftId: string;
  suggestedName: string;
  onPublished: (theme: ThemeSummary, activated: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = React.useState(suggestedName);
  const [activate, setActivate] = React.useState(false);
  React.useEffect(() => {
    if (open) setName(suggestedName);
  }, [open, suggestedName]);
  const themes = useQuery({ queryKey: ["themes"], queryFn: listThemes, enabled: open });
  const valid = /^[a-z0-9-]{1,60}$/.test(name);
  const existing = (themes.data ?? []).filter((t) => t.name === name);
  const nextVersion = existing.reduce((max, t) => Math.max(max, t.version), 0) + 1;
  const live = (themes.data ?? []).find((t) => t.is_active);

  const publish = useMutation({
    mutationFn: () => publishDraft(draftId, { name, activate }),
    onSuccess: (theme) => {
      void queryClient.invalidateQueries({ queryKey: ["themes"] });
      notify.success(
        activate ? `${theme.name} v${theme.version} is live` : `Published ${theme.name} v${theme.version}`,
        activate ? "Every visitor sees it now." : "Installed. Activate it from Appearance when ready.",
      );
      onPublished(theme, activate);
      onClose();
    },
    onError: (e) => notify.error("Couldn't publish", e),
  });

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Publish this draft"
      description="Installs the draft as a theme version. The draft stays, so you can keep working and publish again."
      testId="publish-dialog"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={!valid || publish.isPending}
            onClick={() => publish.mutate()}
            data-testid="publish-confirm"
          >
            <Rocket className="h-4 w-4" aria-hidden="true" />
            {publish.isPending ? "Publishing…" : activate ? "Publish and go live" : "Publish"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field
          label="Theme name"
          htmlFor="publish-name"
          hint={
            valid
              ? existing.length > 0
                ? `Becomes ${name} v${nextVersion} (${existing.length} earlier ${existing.length === 1 ? "version" : "versions"} kept for rollback).`
                : `A new theme, ${name} v1.`
              : undefined
          }
          error={valid ? null : "Lowercase letters, digits and dashes only (1–60)."}
        >
          <input
            id="publish-name"
            value={name}
            spellCheck={false}
            onChange={(e) => setName(e.target.value.toLowerCase())}
            className="h-9 w-full rounded-md border border-input bg-background px-3 font-mono text-sm"
          />
        </Field>
        <label className="flex items-start gap-2 text-sm">
          <input
            type="checkbox"
            checked={activate}
            onChange={(e) => setActivate(e.target.checked)}
            className="mt-0.5"
          />
          <span>
            <span className="block font-medium">Make it live now</span>
            <span className="block text-xs text-muted-foreground">
              {live === undefined
                ? "Every visitor sees this design straight away."
                : `Replaces “${live.name} v${live.version}” for every visitor. You can switch back from Appearance.`}
            </span>
          </span>
        </label>
      </div>
    </Modal>
  );
}
