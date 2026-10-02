import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ImagePlus, Send, Sparkles, X } from "lucide-react";

import { api } from "@/api/client";
import {
  acceptProposal,
  chat,
  listMessages,
  type Draft,
  type StudioMessage,
} from "@/api/themes";
import { MediaPicker } from "@/components/editor/MediaPicker";
import { Button } from "@/components/ui/button";
import { Modal } from "@/components/ui/dialog";
import { notify } from "@/components/ui/toast";
import { formatRelative } from "@/routes/_auth/index";
import { cn } from "@/lib/utils";
import { errorSummary } from "@/lib/error-text";

const SUGGESTIONS = [
  "Warmer palette with one strong accent",
  "More contrast in dark mode",
  "Add a search box to the home page",
  "Wider reading column, larger base size",
  "Show related posts under each article",
];

/** Which kinds of model the assistant can use right now. */
export function useAssistantModels(): { text: boolean; vision: boolean; loaded: boolean } {
  const registry = useQuery({
    queryKey: ["ai-registry"],
    queryFn: () => api.aiModels(),
    staleTime: 60_000,
    retry: false,
  });
  const has = (kind: string) =>
    (registry.data?.models ?? []).some((m) => m.kind === kind && m.is_default && m.enabled);
  return { text: has("text"), vision: has("vision"), loaded: !registry.isPending };
}

/**
 * The conversation with the assistant. Sending queues a run; the draft
 * hook polls while the run is in flight and adopts the revision it
 * writes, so this panel only has to show the messages.
 */
export function AssistantPanel({
  draft,
  models,
  onApplied,
}: {
  draft: Draft;
  models: { text: boolean; vision: boolean };
  /** Called with the new draft once a proposal is accepted. */
  onApplied?: (draft: Draft) => void;
}) {
  const queryClient = useQueryClient();
  const [text, setText] = React.useState("");
  const [images, setImages] = React.useState<{ id: string; name: string }[]>([]);
  const [pickerOpen, setPickerOpen] = React.useState(false);
  const generating = draft.status === "generating";
  const endRef = React.useRef<HTMLDivElement>(null);

  const messages = useQuery({
    queryKey: ["theme-draft-messages", draft.id],
    queryFn: () => listMessages(draft.id),
    refetchInterval: generating ? 2000 : false,
  });
  React.useEffect(() => {
    // Guarded: not every environment implements it (jsdom does not), and
    // failing to scroll must never take the panel down.
    endRef.current?.scrollIntoView?.({ block: "end" });
  }, [messages.data?.length, generating]);

  const send = useMutation({
    mutationFn: (message: string) => chat(draft.id, message, images.map((i) => i.id)),
    onSuccess: () => {
      setText("");
      setImages([]);
      void queryClient.invalidateQueries({ queryKey: ["theme-draft-messages", draft.id] });
      void queryClient.invalidateQueries({ queryKey: ["theme-draft", draft.id] });
    },
    onError: (e) => notify.error("Couldn't ask the assistant", e),
  });

  const accept = useMutation({
    mutationFn: (messageId: string) => acceptProposal(draft.id, messageId),
    onSuccess: (next) => {
      onApplied?.(next);
      void queryClient.invalidateQueries({ queryKey: ["theme-draft-messages", draft.id] });
      // A proposal may have staged menus; the pickers must see them land.
      void queryClient.invalidateQueries({ queryKey: ["menus"] });
      notify.success("Applied", `Revision ${next.revision}. Undo is in the toolbar.`);
    },
    onError: (e) => notify.error("Couldn't apply the change", e),
  });

  const submit = () => {
    const message = text.trim();
    if (message.length < 3 || generating) return;
    send.mutate(message);
  };

  if (!models.text) {
    return (
      <div className="space-y-2 rounded-md border border-dashed p-4 text-sm" data-testid="assistant-off">
        <p className="font-medium">The assistant needs a text model.</p>
        <p className="text-muted-foreground">
          Register one on the{" "}
          <a href="/admin/models" className="text-primary underline">
            AI models
          </a>{" "}
          page and make it the default for text. Everything else in the studio works without it.
        </p>
      </div>
    );
  }

  return (
    <div className="flex h-full min-h-[24rem] flex-col gap-2" data-testid="assistant-panel">
      <div className="min-h-0 flex-1 space-y-2 overflow-y-auto pr-1">
        {(messages.data ?? []).length === 0 ? (
          <p className="rounded-md bg-muted/50 px-3 py-2 text-xs text-muted-foreground">
            Describe what you want — a mood, a colour, a layout change. The
            assistant shows you what it would change; nothing is written
            until you apply it.
          </p>
        ) : null}
        {(messages.data ?? []).map((m: StudioMessage) => (
          <div
            key={m.id}
            className={cn(
              "rounded-lg px-3 py-2 text-sm",
              m.role === "you" ? "ml-6 bg-primary text-primary-foreground" : "border bg-muted/50",
            )}
          >
            <p className="mb-0.5 text-[9px] font-semibold uppercase tracking-[0.1em] opacity-70">
              {m.role === "you" ? "You" : "Assistant"} · {formatRelative(m.created_at)}
            </p>
            <p className="whitespace-pre-wrap">{m.text}</p>
            {m.proposal !== null ? (
              <Proposal
                changes={m.proposal.changes ?? []}
                steps={m.proposal.steps ?? []}
                stale={m.proposal.base !== draft.revision}
                pending={accept.isPending}
                onAccept={() => accept.mutate(m.id)}
              />
            ) : null}
            {m.revision !== null ? (
              <span className="mt-1 block font-mono text-[10px] opacity-80">revision {m.revision}</span>
            ) : null}
          </div>
        ))}
        {generating ? (
          <div
            className="flex items-center gap-2 rounded-lg border bg-muted/50 px-3 py-2 text-sm text-muted-foreground"
            role="status"
          >
            <span className="h-3 w-3 animate-spin rounded-full border-2 border-muted border-t-primary" />
            Working on it…
          </div>
        ) : null}
        {draft.status === "failed" && draft.status_note !== null && !generating ? (
          <p className="text-[11px] text-destructive" title={draft.status_note}>
            Last run failed: {errorSummary(draft.status_note).summary}
          </p>
        ) : null}
        <div ref={endRef} />
      </div>

      <div className="space-y-2 border-t pt-2">
        {images.length > 0 ? (
          <ul className="flex flex-wrap gap-1">
            {images.map((img) => (
              <li
                key={img.id}
                className="inline-flex items-center gap-1 rounded-full border bg-muted/50 px-2 py-0.5 text-[11px]"
              >
                {img.name}
                <button
                  type="button"
                  aria-label={`Remove ${img.name}`}
                  onClick={() => setImages((list) => list.filter((i) => i.id !== img.id))}
                  className="text-muted-foreground hover:text-foreground"
                >
                  <X className="h-3 w-3" aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        ) : null}
        <label className="block">
          <span className="sr-only">Ask the assistant</span>
          <textarea
            rows={3}
            value={text}
            disabled={generating}
            onChange={(e) => setText(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                submit();
              }
            }}
            placeholder={generating ? "Waiting for the assistant…" : "What should change?"}
            className="w-full rounded-md border border-input bg-background p-2 text-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            data-testid="assistant-input"
          />
        </label>
        <div className="flex flex-wrap gap-1">
          {SUGGESTIONS.map((s) => (
            <button
              key={s}
              type="button"
              disabled={generating}
              onClick={() => setText(s)}
              className="rounded-full border bg-muted/50 px-2 py-0.5 text-[11px] text-muted-foreground hover:bg-accent hover:text-foreground disabled:opacity-50"
            >
              {s}
            </button>
          ))}
        </div>
        <div className="flex items-center gap-2">
          {models.vision ? (
            <Button
              size="sm"
              variant="outline"
              disabled={generating || images.length >= 4}
              onClick={() => setPickerOpen(true)}
              title="Show the assistant an image: a moodboard, a site you like"
            >
              <ImagePlus className="h-3.5 w-3.5" aria-hidden="true" />
              Image
            </Button>
          ) : null}
          <Button
            size="sm"
            className="ml-auto"
            disabled={text.trim().length < 3 || generating || send.isPending}
            onClick={submit}
            data-testid="assistant-send"
          >
            {generating ? (
              <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
            ) : (
              <Send className="h-3.5 w-3.5" aria-hidden="true" />
            )}
            Send
          </Button>
        </div>
      </div>

      <Modal
        open={pickerOpen}
        onClose={() => setPickerOpen(false)}
        title="Show the assistant an image"
        description="A moodboard, a screenshot of a site you like, a logo — it will take the palette and feel from it."
        size="lg"
      >
        <MediaPicker
          accept="image"
          onPick={(item) => {
            setImages((list) =>
              list.some((i) => i.id === item.id)
                ? list
                : [...list, { id: item.id, name: item.file_name }],
            );
            setPickerOpen(false);
          }}
        />
      </Modal>
    </div>
  );
}

/**
 * What a reply would change, and the button that writes it.
 *
 * The assistant used to commit as it answered, so a change nobody wanted
 * still landed and had to be undone. Declining is now simply never
 * pressing this.
 */
function Proposal({
  changes,
  stale,
  pending,
  onAccept,
  steps = [],
}: {
  changes: string[];
  stale: boolean;
  pending: boolean;
  onAccept: () => void;
  steps?: { thought: string; tool: string; observation: string }[];
}) {
  return (
    <div className="mt-2 rounded-md border bg-background/60 p-2" data-testid="proposal">
      {steps.length > 0 ? (
        <details className="mb-1.5" data-testid="agent-steps">
          <summary className="cursor-pointer text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground">
            Worked in {steps.length} step{steps.length === 1 ? "" : "s"}
          </summary>
          <ol className="mt-1 space-y-1 border-l pl-2 text-[10px] text-muted-foreground">
            {steps.map((s, i) => (
              <li key={`${i}-${s.tool}`}>
                <span className="font-mono text-primary">{s.tool}</span>
                {s.thought !== "" ? <> — {s.thought}</> : null}
              </li>
            ))}
          </ol>
        </details>
      ) : null}
      <p className="mb-1 text-[10px] font-semibold uppercase tracking-[0.1em] text-muted-foreground">
        Would change
      </p>
      <ul className="mb-2 space-y-0.5 text-[11px] text-muted-foreground">
        {changes.length === 0 ? (
          <li>the draft</li>
        ) : (
          changes.map((c) => <li key={c}>{c}</li>)
        )}
      </ul>
      {stale ? (
        <p className="text-[11px] text-amber-700 dark:text-amber-500">
          The draft has changed since this was proposed. Ask again so the
          assistant works from what is there now.
        </p>
      ) : (
        <Button size="sm" className="h-6 text-[11px]" disabled={pending} onClick={onAccept}>
          <Check className="h-3 w-3" aria-hidden="true" />
          {pending ? "Applying…" : "Apply"}
        </Button>
      )}
    </div>
  );
}
