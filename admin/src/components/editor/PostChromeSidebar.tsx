import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Check, ChevronDown, Sparkles } from "lucide-react";
import { notify } from "@/components/ui/toast";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Chip, Field } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";
import { MediaPicker } from "./MediaPicker";
import { mediaUrl } from "./blocks";

export interface ChromeValue {
  slug: string;
  excerpt: string;
  seo_title: string;
  seo_description: string;
  status: string;
  type: string;
  /** Undefined keeps the stored password; an empty string removes it. */
  password: string | undefined;
  /** Pinned to the top of the home listing. */
  sticky: boolean;
  /** BCP 47 tag; empty means the site's language. */
  lang: string;
  /** Another entry this one translates (id as a string), or empty. */
  translation_of: string;
  /** The other members of the translation group, read-only. */
  translations: { id: string; lang: string; title: string; status: string }[];
  scheduled_for: string;
  parent_id: string;
  term_ids: string[];
  /** The post's lead picture: cards, social previews and the theme use it. */
  featured_media_id: string;
  featured_media_url: string;
  featured_blurhash: string;
  /** `"x% y%"` from the library's focal point, for cropped cards. */
  featured_focal: string;
  /** SEO: focus keyphrase, canonical, robots, social, schema. */
  seo_keyphrase: string;
  seo_canonical: string;
  seo_noindex: boolean;
  seo_nofollow: boolean;
  og_title: string;
  og_description: string;
  og_image: string;
  schema_type: string;
  product_price: string;
  product_currency: string;
}

const STATUSES = [
  { value: "draft", label: "Draft", hint: "Only you can see it." },
  { value: "published", label: "Published", hint: "Live on your site." },
  { value: "scheduled", label: "Scheduled", hint: "Goes live at the time you set." },
  { value: "private", label: "Private", hint: "Needs the password to read." },
];

/**
 * The schedule field shows and reads wall-clock time in the configured site timezone;
 * the post stores an instant. Appending "Z" to what the field said treated
 * a local time as UTC, so a post scheduled for 9am went live at 9am UTC.
 */
export function toLocalInput(iso: string, zone?: string): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const parts = new Intl.DateTimeFormat("en-CA", {
    timeZone: zone, year: "numeric", month: "2-digit", day: "2-digit",
    hour: "2-digit", minute: "2-digit", hourCycle: "h23",
  }).formatToParts(d);
  const get = (type: string) => parts.find((p) => p.type === type)?.value ?? "";
  return `${get("year")}-${get("month")}-${get("day")}T${get("hour")}:${get("minute")}`;
}
export function fromLocalInput(local: string, zone?: string): string {
  if (!/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(local)) return "";
  const wall = Date.parse(`${local}Z`);
  if (!Number.isFinite(wall)) return "";
  // Sample both sides of a DST transition. A missing hour has no match;
  // a repeated hour uses its earlier occurrence, consistently.
  const matches = new Set<number>();
  for (const hours of [-36, -12, 0, 12, 36]) {
    const sample = wall + hours * 3600000;
    const offset = Date.parse(`${toLocalInput(new Date(sample).toISOString(), zone)}Z`) - sample;
    const candidate = wall - offset;
    if (toLocalInput(new Date(candidate).toISOString(), zone) === local) matches.add(candidate);
  }
  return matches.size ? new Date(Math.min(...matches)).toISOString() : "";
}

/** Collapsible section so the sidebar stays scannable on a phone. */
function Section({
  title,
  summary,
  defaultOpen = false,
  children,
}: {
  title: string;
  summary?: React.ReactNode;
  defaultOpen?: boolean;
  children: React.ReactNode;
}) {
  const [open, setOpen] = React.useState(defaultOpen);
  return (
    <div className="border-b last:border-b-0">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full items-center gap-2 px-4 py-3 text-left hover:bg-muted/50"
      >
        <span className="text-sm font-medium">{title}</span>
        {!open && summary !== undefined ? (
          <span className="ml-auto min-w-0 truncate text-xs text-muted-foreground">
            {summary}
          </span>
        ) : (
          <span className="ml-auto" />
        )}
        <ChevronDown
          className={cn(
            "h-4 w-4 shrink-0 text-muted-foreground transition-transform",
            open && "rotate-180",
          )}
          aria-hidden="true"
        />
      </button>
      {open ? <div className="space-y-4 px-4 pb-4">{children}</div> : null}
    </div>
  );
}

export function PostChromeSidebar({
  value,
  onChange,
  document,
  postId,
}: {
  value: ChromeValue;
  onChange: (patch: Partial<ChromeValue>) => void;
  /** The block document, for the suggest buttons. */
  document?: unknown;
  /** The saved post's id, for the read-aloud tools. */
  postId?: string;
}) {
  const siteOptions = useQuery({ queryKey: ["options"], queryFn: () => api.getOptions() });
  const timezone = typeof siteOptions.data?.timezone === "string" ? siteOptions.data.timezone : "UTC";
  const suggestExcerpt = useMutation({
    mutationFn: () => api.assist<{ suggestions: string[] }>("excerpt", document),
    onSuccess: (r) => {
      const first = r.suggestions.find((x) => x.trim() !== "");
      if (first) onChange({ excerpt: first.trim() });
      else notify.error("No suggestion came back");
    },
    onError: (e) => notify.error("Couldn't suggest an excerpt", e),
  });
  const suggestSeo = useMutation({
    mutationFn: () => api.assist<{ meta_title: string; meta_description: string }>("seo", document),
    onSuccess: (r) => onChange({ seo_title: r.meta_title.trim(), seo_description: r.meta_description.trim() }),
    onError: (e) => notify.error("Couldn't suggest search text", e),
  });
  const terms = useQuery({ queryKey: ["terms"], queryFn: () => api.listTerms() });
  const allTerms = terms.data ?? [];
  const selected = allTerms.filter((t) => value.term_ids.includes(t.id));
  const queryClient = useQueryClient();
  const [newTag, setNewTag] = React.useState("");
  // A tag typed here is created at once and ticked; an existing one of the
  // same name (any case) is ticked instead of duplicated.
  const addTag = useMutation({
    mutationFn: async (name: string) => {
      const found = allTerms.find(
        (t) => t.taxonomy === "tag" && t.name.toLowerCase() === name.toLowerCase(),
      );
      return found ?? api.createTerm({ taxonomy: "tag", name });
    },
    onSuccess: (t) => {
      setNewTag("");
      onChange({ term_ids: value.term_ids.includes(t.id) ? value.term_ids : [...value.term_ids, t.id] });
      void queryClient.invalidateQueries({ queryKey: ["terms"] });
    },
    onError: (e) => notify.error("Couldn't add the tag", e),
  });

  return (
    <div className="overflow-hidden rounded-lg border bg-card">
      <Section
        title="Status & visibility"
        summary={STATUSES.find((s) => s.value === value.status)?.label ?? value.status}
        defaultOpen
      >
        <fieldset className="space-y-1.5">
          <legend className="sr-only">Status</legend>
          {STATUSES.map((s) => (
            <label
              key={s.value}
              className={cn(
                "flex cursor-pointer items-start gap-2.5 rounded-md border p-2.5 text-sm transition-colors",
                value.status === s.value
                  ? "border-primary bg-primary-subtle"
                  : "hover:bg-muted/50",
              )}
            >
              <input
                type="radio"
                name="post-status"
                value={s.value}
                checked={value.status === s.value}
                onChange={(e) => onChange({ status: e.target.value })}
                className="mt-0.5 accent-primary"
              />
              <span className="min-w-0">
                <span className="block font-medium">{s.label}</span>
                <span className="block text-xs text-muted-foreground">{s.hint}</span>
              </span>
            </label>
          ))}
        </fieldset>

        {value.status === "scheduled" ? (
          <Field
            label="Publish at"
            htmlFor="post-schedule"
            hint={`Site time (${timezone}). A repeated daylight-saving hour uses its first occurrence.`}
          >
            <Input
              id="post-schedule"
              type="datetime-local"
              value={toLocalInput(value.scheduled_for, timezone)}
              disabled={siteOptions.isPending}
              onChange={(e) => {
                const instant = fromLocalInput(e.target.value, timezone);
                if (e.target.value && !instant) { notify.error("That time does not exist in the site timezone", "Choose a time outside the daylight-saving clock change."); return; }
                onChange({ scheduled_for: instant });
              }}
            />
          </Field>
        ) : null}

        <div className="mt-3 grid gap-2 sm:grid-cols-2">
          <Field label="Language" htmlFor="post-lang" hint="Empty: the site's language.">
            <>
              <Input id="post-lang" list="post-lang-options" value={value.lang} onChange={(e) => onChange({ lang: e.target.value.trim() })} placeholder="en" className="font-mono text-sm" />
              <datalist id="post-lang-options">{["en", "hi", "es", "fr", "de", "pt-BR", "ja", "zh", "ar"].map((l) => <option key={l} value={l} />)}</datalist>
            </>
          </Field>
          <Field label="Translation of" htmlFor="post-translation-of" hint="Another entry's id; both get hreflang links.">
            <Input id="post-translation-of" value={value.translation_of} onChange={(e) => onChange({ translation_of: e.target.value.trim() })} placeholder="post id" className="font-mono text-sm" />
          </Field>
        </div>
        {value.translations.length > 0 ? (
          <ul className="mt-1 text-xs text-muted-foreground" data-testid="post-translations">
            {value.translations.map((tr) => <li key={tr.id}>{tr.lang || "site language"}: <a href={`/admin/posts/${tr.id}`} className="underline-offset-2 hover:underline">{tr.title}</a> ({tr.status})</li>)}
          </ul>
        ) : null}

        {value.type !== "page" ? (
          <label className="mt-2 flex items-start gap-2 text-sm">
            <input type="checkbox" className="mt-0.5 accent-primary" checked={value.sticky} onChange={(e) => onChange({ sticky: e.target.checked })} data-testid="post-sticky" />
            <span><b>Pin to the top</b><br /><span className="text-xs text-muted-foreground">Stays first on the home page whatever is published after it.</span></span>
          </label>
        ) : null}

        {value.status === "private" ? (
          <Field
            label="Password"
            htmlFor="post-password"
            hint="Leave unchanged to keep the existing password, or remove it explicitly."
          >
            <Input
              id="post-password"
              type="password"
              value={value.password ?? ""}
              onChange={(e) => onChange({ password: e.target.value })}
            />
            <Button type="button" variant="ghost" onClick={() => onChange({ password: "" })}>
              Remove password
            </Button>
          </Field>
        ) : null}
      </Section>

      <Section
        title="Address"
        summary={value.slug === "" ? "From the title" : `/${value.slug}`}
      >
        <Field
          label="URL slug"
          htmlFor="post-slug"
          hint={
            value.slug === ""
              ? "Generated from the title when you save."
              : `Readers will find this at /${value.slug}`
          }
        >
          <Input
            id="post-slug"
            value={value.slug}
            onChange={(e) => onChange({ slug: e.target.value })}
            placeholder="from the title"
            className="font-mono text-xs"
          />
        </Field>

        <Field
          label="Type"
          htmlFor="post-type"
          hint={
            value.type === "post" || value.type === "page"
              ? "Pages sit outside your posting timeline."
              : "An entry of a content type stays that type."
          }
        >
          <select
            id="post-type"
            value={value.type}
            disabled={value.type !== "post" && value.type !== "page"}
            onChange={(e) => onChange({ type: e.target.value })}
            className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          >
            <option value="post">Post</option>
            <option value="page">Page</option>
            {value.type !== "post" && value.type !== "page" ? <option value={value.type}>{value.type}</option> : null}
          </select>
        </Field>

        {value.type === "page" ? (
          <Field
            label="Parent page"
            htmlFor="post-parent"
            hint="Optional — nests this page under another."
          >
            <Input
              id="post-parent"
              value={value.parent_id}
              onChange={(e) => onChange({ parent_id: e.target.value })}
              placeholder="none"
            />
          </Field>
        ) : null}
      </Section>

      <Section
        title="Featured image"
        summary={value.featured_media_url === "" ? "None" : "Set"}
      >
        {value.featured_media_url !== "" ? (
          <div className="space-y-2" data-testid="featured-image">
            <img
              src={value.featured_media_url}
              alt=""
              className="max-h-40 w-full rounded-md border object-cover"
              loading="lazy"
            />
            <Button
              type="button"
              variant="ghost"
              size="sm"
              onClick={() => onChange({ featured_media_id: "", featured_media_url: "", featured_blurhash: "", featured_focal: "" })}
            >
              Remove
            </Button>
          </div>
        ) : (
          <p className="text-xs text-muted-foreground">
            Shown on cards and in link previews. Without one, the first picture in the post is used.
            Set the picture's focal point on the Media page and cropped cards keep it in view.
          </p>
        )}
        <MediaPicker
          accept="image"
          onPick={(m) =>
            onChange({
              featured_media_id: m.id,
              featured_media_url: `${mediaUrl(m.id)}?variant=medium`,
              featured_blurhash: m.blurhash ?? "",
              featured_focal:
                m.focal_x != null && m.focal_y != null
                  ? `${Math.round(m.focal_x * 100)}% ${Math.round(m.focal_y * 100)}%`
                  : "",
            })
          }
        />
      </Section>

      <Section
        title="Excerpt"
        summary={value.excerpt === "" ? "None" : "Set"}
      >
        <Field
          label="Summary"
          htmlFor="post-excerpt"
          hint="Shown in listings and search results. Left blank, the opening lines are used."
        >
          <textarea
            id="post-excerpt"
            rows={3}
            value={value.excerpt}
            onChange={(e) => onChange({ excerpt: e.target.value })}
            placeholder="A short description…"
            className="w-full rounded-md border border-input bg-background p-2 text-sm placeholder:text-muted-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
        </Field>
        {document !== undefined ? (
          <Button type="button" variant="outline" size="sm" className="mt-2" disabled={suggestExcerpt.isPending} onClick={() => suggestExcerpt.mutate()}>
            <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
            {suggestExcerpt.isPending ? "Thinking…" : "Suggest"}
          </Button>
        ) : null}
      </Section>

      <Section
        title="Search preview"
        summary={value.seo_description === "" ? "Default" : "Set"}
      >
        <Field label="Title" htmlFor="post-seo-title" hint="What search engines show as the heading. Defaults to the post title.">
          <Input id="post-seo-title" value={value.seo_title} onChange={(e) => onChange({ seo_title: e.target.value })} maxLength={70} />
        </Field>
        <Field label="Description" htmlFor="post-seo-description" hint={`${value.seo_description.length}/155 characters.`}>
          <textarea
            id="post-seo-description"
            rows={2}
            value={value.seo_description}
            onChange={(e) => onChange({ seo_description: e.target.value })}
            maxLength={200}
            className="w-full rounded-md border border-input bg-background p-2 text-sm placeholder:text-muted-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          />
        </Field>
        {document !== undefined ? (
          <Button type="button" variant="outline" size="sm" className="mt-2" disabled={suggestSeo.isPending} onClick={() => suggestSeo.mutate()}>
            <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
            {suggestSeo.isPending ? "Thinking…" : "Suggest both"}
          </Button>
        ) : null}
      </Section>

      {postId !== undefined ? <ReadAloudSection postId={postId} /> : null}

      <Section
        title="Categories & tags"
        summary={
          selected.length === 0 ? "None" : `${selected.length} selected`
        }
      >
        {selected.length > 0 ? (
          <div className="flex flex-wrap gap-1">
            {selected.map((t) => (
              <Chip key={t.id} tone="info" dot={false}>
                {t.name}
              </Chip>
            ))}
          </div>
        ) : null}

        <form
          className="flex gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            const name = newTag.trim();
            if (name !== "") addTag.mutate(name);
          }}
        >
          <Input
            value={newTag}
            onChange={(e) => setNewTag(e.target.value)}
            placeholder="New tag"
            aria-label="New tag"
            className="h-8"
          />
          <Button type="submit" variant="outline" size="sm" className="h-8 shrink-0" disabled={addTag.isPending || newTag.trim() === ""}>
            Add
          </Button>
        </form>

        <div className="max-h-56 overflow-y-auto rounded-md border">
          {terms.isPending ? (
            <p className="px-3 py-3 text-xs text-muted-foreground">Loading…</p>
          ) : allTerms.length === 0 ? (
            <p className="px-3 py-3 text-xs text-muted-foreground">
              No categories or tags yet. Create them under Categories.
            </p>
          ) : (
            <ul className="divide-y">
              {allTerms.map((t) => {
                const on = value.term_ids.includes(t.id);
                return (
                  <li key={t.id}>
                    <label className="flex cursor-pointer items-center gap-2 px-3 py-2 text-sm hover:bg-muted/50">
                      <input
                        type="checkbox"
                        checked={on}
                        onChange={(e) =>
                          onChange({
                            term_ids: e.target.checked
                              ? [...value.term_ids, t.id]
                              : value.term_ids.filter((x) => x !== t.id),
                          })
                        }
                        className="h-3.5 w-3.5 accent-primary"
                      />
                      <span className="min-w-0 flex-1 truncate">{t.name}</span>
                      <span className="shrink-0 text-[10px] uppercase tracking-wide text-muted-foreground">
                        {t.taxonomy}
                      </span>
                      {on ? (
                        <Check
                          className="h-3.5 w-3.5 shrink-0 text-primary"
                          aria-hidden="true"
                        />
                      ) : null}
                    </label>
                  </li>
                );
              })}
            </ul>
          )}
        </div>
      </Section>
    </div>
  );
}

export function PreviewButton({ postId }: { postId: string }) {
  const onPreview = async () => {
    const { url } = await api.previewToken(postId);
    window.open(api.previewPageUrl(url), "_blank");
  };
  return (
    <Button variant="outline" size="sm" onClick={onPreview}>
      Preview
    </Button>
  );
}

/** The post's audio version: play it if it exists, or ask for one. */
function ReadAloudSection({ postId }: { postId: string }) {
  const client = useQueryClient();
  const audio = useQuery({
    queryKey: ["post", postId, "audio"],
    queryFn: () => api.postAudio(postId),
  });
  const [queued, setQueued] = React.useState(false);
  React.useEffect(() => {
    if (!queued) return;
    const t = setInterval(() => void client.invalidateQueries({ queryKey: ["post", postId, "audio"] }), 5000);
    return () => clearInterval(t);
  }, [queued, client, postId]);
  React.useEffect(() => {
    if (audio.data?.media_id) setQueued(false);
  }, [audio.data?.media_id]);
  const generate = useMutation({
    mutationFn: () => api.readAloud(postId),
    onSuccess: () => {
      setQueued(true);
      notify.success("Recording", "The audio appears here when it's ready.");
    },
    onError: (e) => notify.error("Couldn't start the recording", e),
  });
  return (
    <Section title="Read aloud" summary={audio.data?.media_id ? "Ready" : "None"}>
      {audio.data?.media_id ? (
        <audio controls preload="none" src={audio.data.url} className="w-full" data-testid="post-audio" />
      ) : (
        <p className="text-xs text-muted-foreground">
          No audio version yet. Uses the speech model on the AI models page.
        </p>
      )}
      <Button type="button" variant="outline" size="sm" className="mt-2" disabled={generate.isPending || queued} onClick={() => generate.mutate()}>
        <Sparkles className="h-3.5 w-3.5" aria-hidden="true" />
        {queued ? "Recording…" : audio.data?.media_id ? "Re-record" : "Generate audio"}
      </Button>
    </Section>
  );
}
