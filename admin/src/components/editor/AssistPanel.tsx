import { useMutation } from "@tanstack/react-query";
import { useState } from "react";

/**
 * The four assist kinds the server implements. Titles and excerpts return a
 * list of suggestions; SEO returns one meta pair; tags return names split
 * into ones already in the site's vocabulary and ones that would be new.
 */
type Kind = "title" | "excerpt" | "seo" | "tags" | "links";

const KINDS: { value: Kind; label: string }[] = [
  { value: "title", label: "Titles" },
  { value: "excerpt", label: "Excerpt" },
  { value: "seo", label: "SEO" },
  { value: "tags", label: "Tags" },
  { value: "links", label: "Links" },
];

interface SuggestionsResponse {
  suggestions: string[];
}
interface SeoResponse {
  meta_title: string;
  meta_description: string;
}
interface TagsResponse {
  tags: string[];
  new_tags?: string[];
}
export interface LinkSuggestion {
  phrase: string | null;
  post_id: string;
  title: string;
  url: string;
}
interface LinksResponse {
  links: LinkSuggestion[];
}
type AssistResponse = SuggestionsResponse | SeoResponse | TagsResponse | LinksResponse;

function isSeo(r: AssistResponse): r is SeoResponse {
  return "meta_title" in r;
}
function isTags(r: AssistResponse): r is TagsResponse {
  return "tags" in r;
}
function isLinks(r: AssistResponse): r is LinksResponse {
  return "links" in r;
}

/** One assist action: posts the doc to /ai/assist/{kind} and shows the result. */
export function AssistPanel({
  document,
  vocabulary,
  postId,
  onApplyTitle,
  onApplyExcerpt,
  onApplySeo,
  onApplyTags,
  onApplyLink,
  textAvailable,
}: {
  document: unknown;
  vocabulary: string[];
  /** Saved draft id, so link suggestions never point at the draft itself. */
  postId?: string;
  onApplyTitle: (title: string) => void;
  onApplyExcerpt: (excerpt: string) => void;
  /** Fills the search title and description in the sidebar. */
  onApplySeo?: (seo: { meta_title: string; meta_description: string }) => void;
  /** Optional: absent while the editor cannot resolve tag names to terms. */
  onApplyTags?: (names: string[]) => void;
  /** Wraps `phrase` (or appends a related link when phrase is null). */
  onApplyLink?: (s: LinkSuggestion) => void;
  /** False hides the kinds that need a text model; links stay. */
  textAvailable?: boolean;
}) {
  const kinds = textAvailable === false ? KINDS.filter((k) => k.value === "links") : KINDS;
  const [kind, setKind] = useState<Kind>(textAvailable === false ? "links" : "title");
  const [result, setResult] = useState<AssistResponse | null>(null);

  const assist = useMutation({
    mutationFn: async (): Promise<AssistResponse> => {
      // Links come from the site's own index, not a model, so they have
      // their own endpoint and never spend AI budget.
      const [path, body] =
        kind === "links"
          ? [
              "/api/v1/posts/link-suggestions",
              { content: document, exclude_post_id: postId ?? null },
            ]
          : [`/api/v1/ai/assist/${kind}`, { content: document, vocabulary }];
      const response = await fetch(path, {
        method: "POST",
        credentials: "same-origin",
        headers: { "content-type": "application/json" },
        body: JSON.stringify(body),
      });
      if (!response.ok) {
        const parsed = (await response.json().catch(() => null)) as
          | { message?: string }
          | null;
        throw new Error(parsed?.message ?? "assist failed");
      }
      const data: unknown = await response.json();
      if (kind === "links") {
        return { links: (data as LinkSuggestion[]) ?? [] };
      }
      return data as AssistResponse;
    },
    onSuccess: (data) => setResult(data),
  });

  // Switching kind clears the previous answer: showing tag chips under a
  // heading that now says "SEO" is worse than showing nothing.
  const pick = (next: Kind) => {
    setKind(next);
    setResult(null);
  };

  return (
    <div className="rounded-lg border p-3 space-y-2" data-testid="assist-panel">
      <div className="flex flex-wrap gap-2">
        {kinds.map((k) => (
          <button
            key={k.value}
            type="button"
            aria-pressed={kind === k.value}
            className={
              kind === k.value
                ? "rounded border px-2 py-1 text-sm font-medium"
                : "rounded border px-2 py-1 text-sm"
            }
            onClick={() => pick(k.value)}
          >
            {k.label}
          </button>
        ))}
        <button
          type="button"
          data-testid="suggest-button"
          disabled={assist.isPending}
          className="ml-auto rounded bg-primary px-3 py-1 text-sm text-primary-foreground"
          onClick={() => assist.mutate()}
        >
          {assist.isPending ? "Thinking…" : "Suggest"}
        </button>
      </div>

      {assist.isError && (
        <p role="alert" className="text-sm text-destructive">
          {(assist.error as Error).message}
        </p>
      )}

      {result === null ? null : isLinks(result) ? (
        <LinksResult result={result} onApplyLink={onApplyLink} />
      ) : isSeo(result) ? (
        <SeoResult result={result} onApplyExcerpt={onApplyExcerpt} onApplySeo={onApplySeo} />
      ) : isTags(result) ? (
        <TagsResult result={result} onApplyTags={onApplyTags} />
      ) : (
        <ul className="space-y-2">
          {result.suggestions.map((s) => (
            <li key={s}>
              <button
                type="button"
                className="block w-full rounded border p-2 text-left text-sm hover:bg-accent"
                onClick={() =>
                  kind === "title" ? onApplyTitle(s) : onApplyExcerpt(s)
                }
              >
                {s}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function SeoResult({
  result,
  onApplyExcerpt,
  onApplySeo,
}: {
  result: SeoResponse;
  onApplyExcerpt: (excerpt: string) => void;
  onApplySeo?: (seo: { meta_title: string; meta_description: string }) => void;
}) {
  return (
    <div className="space-y-2 text-sm">
      <div>
        <p className="text-xs font-medium text-muted-foreground">
          Meta title ({result.meta_title.length}/60)
        </p>
        <p className="rounded border p-2">{result.meta_title}</p>
      </div>
      <div>
        <p className="text-xs font-medium text-muted-foreground">
          Meta description ({result.meta_description.length}/155)
        </p>
        <p className="rounded border p-2">{result.meta_description}</p>
      </div>
      <div className="flex flex-wrap gap-2">
        {onApplySeo ? (
          <button
            type="button"
            className="rounded border px-2 py-1 text-sm hover:bg-accent"
            onClick={() => onApplySeo(result)}
          >
            Use as search title and description
          </button>
        ) : null}
        <button
          type="button"
          className="rounded border px-2 py-1 text-sm hover:bg-accent"
          onClick={() => onApplyExcerpt(result.meta_description)}
        >
          Use description as the excerpt
        </button>
      </div>
    </div>
  );
}

function TagsResult({
  result,
  onApplyTags,
}: {
  result: TagsResponse;
  onApplyTags?: (names: string[]) => void;
}) {
  const existing = result.tags ?? [];
  const fresh = result.new_tags ?? [];
  const all = [...existing, ...fresh];

  return (
    <div className="space-y-2 text-sm">
      {existing.length > 0 && (
        <div>
          <p className="text-xs font-medium text-muted-foreground">
            Existing tags
          </p>
          <p className="flex flex-wrap gap-1">
            {existing.map((t) => (
              <span key={t} className="rounded-full border px-2 py-0.5 text-xs">
                {t}
              </span>
            ))}
          </p>
        </div>
      )}
      {fresh.length > 0 && (
        <div>
          <p className="text-xs font-medium text-muted-foreground">
            New tags — these would be created
          </p>
          <p className="flex flex-wrap gap-1">
            {fresh.map((t) => (
              <span key={t} className="rounded-full border px-2 py-0.5 text-xs">
                {t}
              </span>
            ))}
          </p>
        </div>
      )}
      {all.length === 0 ? (
        <p className="text-muted-foreground">No tags suggested.</p>
      ) : onApplyTags ? (
        <button
          type="button"
          className="rounded border px-2 py-1 text-sm hover:bg-accent"
          onClick={() => onApplyTags(all)}
        >
          Apply {all.length} tag{all.length === 1 ? "" : "s"}
        </button>
      ) : null}
    </div>
  );
}

function LinksResult({
  result,
  onApplyLink,
}: {
  result: LinksResponse;
  onApplyLink?: (s: LinkSuggestion) => void;
}) {
  if (result.links.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        No internal link opportunities found — the draft mentions no other
        entry by title.
      </p>
    );
  }
  return (
    <ul className="space-y-2 text-sm" data-testid="link-suggestions">
      {result.links.map((s) => (
        <li key={s.url} className="flex items-center gap-2 rounded border p-2">
          <span className="min-w-0 flex-1">
            <span className="block truncate font-medium">{s.title}</span>
            <span className="block truncate text-xs text-muted-foreground">
              {s.phrase === null ? "related reading" : `mentions “${s.phrase}”`} · {s.url}
            </span>
          </span>
          {onApplyLink ? (
            <button
              type="button"
              className="rounded border px-2 py-1 text-xs hover:bg-accent"
              onClick={() => onApplyLink(s)}
            >
              {s.phrase === null ? "Add link" : "Link it"}
            </button>
          ) : null}
        </li>
      ))}
    </ul>
  );
}
