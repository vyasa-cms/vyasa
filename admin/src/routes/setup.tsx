import * as React from "react";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Field } from "@/components/ui/primitives";
import { Mark } from "@/components/ui/logo";
import { notify } from "@/components/ui/toast";
import { cn } from "@/lib/utils";
import { MailRelayPanel } from "@/components/MailRelayPanel";

export const Route = createFileRoute("/setup")({
  validateSearch: (s: Record<string, unknown>): { again?: string; step?: string } => ({
    ...(typeof s["again"] === "string" ? { again: s["again"] } : {}),
    ...(typeof s["step"] === "string" ? { step: s["step"] } : {}),
  }),
  component: SetupPage,
});

const STEPS = ["welcome", "account", "site", "content", "delivery", "mail", "assistants", "updates", "done"] as const;
type Step = (typeof STEPS)[number];
const LABELS: Record<Step, string> = {
  welcome: "Welcome", account: "Account", site: "Site", content: "Content", delivery: "Delivery",
  mail: "Mail", assistants: "Assistants", updates: "Updates", done: "Done",
};

/**
 * The first-run wizard. Before an administrator exists the setup token
 * is the credential; the account step signs you in and the rest run as
 * you. Each step saves as it goes, so a closed tab resumes where it was.
 */
function SetupPage() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { again, step: wanted } = Route.useSearch();
  const status = useQuery({ queryKey: ["setup-status"], queryFn: api.setupStatus, retry: false });
  const [step, setStep] = React.useState<Step>("welcome");
  const [claimed, setClaimed] = React.useState(false);
  const [skipped, setSkipped] = React.useState<Step[]>([]);
  const [answers, setAnswers] = React.useState<Record<string, unknown>>({});

  // Resume where the server says, once we know.
  React.useEffect(() => {
    const s = status.data;
    if (!s) return;
    if (!s.needs_admin && s.step && s.step !== "done" && (STEPS as readonly string[]).includes(s.step)) {
      setStep(s.step as Step);
      setClaimed(true);
    }
    if (!s.needs_admin && again) {
      setClaimed(true);
      // Settings links straight to one step: "rerun Assistants".
      if (wanted && wanted !== "account" && (STEPS as readonly string[]).includes(wanted)) setStep(wanted as Step);
    }
  }, [status.data, again, wanted]);

  if (status.isPending) return <Shell><p className="text-sm text-muted-foreground">Checking the server…</p></Shell>;
  if (status.data && !status.data.needs_setup && !again) {
    return (
      <Shell>
        <h1 className="font-serif text-2xl font-semibold">Setup is complete</h1>
        <p className="mt-2 text-sm text-muted-foreground">This site already has an administrator and its setup is finished. Every step lives on under Settings, and "Run setup again" there reopens one.</p>
        <div className="mt-4 flex gap-2"><Button onClick={() => void navigate({ to: "/login" })}>Sign in</Button></div>
      </Shell>
    );
  }

  const index = STEPS.indexOf(step);
  const go = (next: Step) => setStep(next);
  const next = () => go(STEPS[Math.min(index + 1, STEPS.length - 1)] as Step);
  const back = () => go(STEPS[Math.max(index - 1, 0)] as Step);
  const skip = () => {
    setSkipped((s) => [...s, step]);
    next();
  };
  const remember = (patch: Record<string, unknown>) => setAnswers((a) => ({ ...a, ...patch }));

  const rail = (
    <ol className="flex flex-wrap gap-1 text-xs xl:sticky xl:top-6 xl:flex-col xl:gap-0.5 xl:self-start xl:text-sm" aria-label="Steps">
      {STEPS.map((s, i) => (
        <li key={s} className={cn("rounded-full px-2 py-0.5 xl:rounded-md xl:px-3 xl:py-1.5", i === index ? "bg-primary text-primary-foreground" : i < index ? "text-foreground" : "text-muted-foreground")} aria-current={i === index ? "step" : undefined}>
          <span className="tabular-nums xl:mr-2 xl:inline-block xl:w-4">{i}</span>
          <span className="xl:hidden"> · </span>
          {LABELS[s]}
        </li>
      ))}
    </ol>
  );
  return (
    <Shell rail={rail}>
      {step === "welcome" ? <Welcome needsAdmin={status.data?.needs_admin ?? true} claimed={claimed} again={Boolean(again)} onClaimed={() => { setClaimed(true); }} onNext={() => go(status.data?.needs_admin ? "account" : "site")} /> : null}
      {step === "account" ? <Account onDone={(email) => { remember({ email }); void queryClient.invalidateQueries({ queryKey: ["me"] }); go("site"); }} onBack={back} /> : null}
      {step === "site" ? <Site siteUrl={window.location.origin} onDone={(url) => { remember({ site_url: url }); next(); }} onBack={back} /> : null}
      {step === "content" ? <Content onDone={next} onBack={back} /> : null}
      {step === "delivery" ? <Delivery onDone={next} onSkip={skip} onBack={back} /> : null}
      {step === "mail" ? <Mail email={String(answers["email"] ?? "")} onDone={next} onSkip={skip} onBack={back} /> : null}
      {step === "assistants" ? <Assistants onDone={next} onSkip={skip} onBack={back} /> : null}
      {step === "updates" ? <Updates onDone={next} onSkip={skip} onBack={back} /> : null}
      {step === "done" ? <Done skipped={skipped} siteUrl={String(answers["site_url"] ?? window.location.origin)} /> : null}
    </Shell>
  );
}

/**
 * Fills the screen it is given: one column on a phone, a step rail beside
 * the form from 1280px, and the form itself growing into three columns
 * of fields on an ultrawide monitor rather than floating in the middle.
 */
function Shell({ children, rail }: { children: React.ReactNode; rail?: React.ReactNode }) {
  return (
    <div className="min-h-dvh bg-background px-4 py-6 text-foreground sm:px-8 xl:px-12 2xl:px-16" data-testid="setup-page">
      <div className="mb-6 flex items-center gap-2 font-serif text-lg font-semibold"><Mark className="text-primary" />Vyasa</div>
      <div className={cn("grid gap-6", rail ? "xl:grid-cols-[16rem_minmax(0,1fr)]" : "")}>
        {rail}
        <div className="min-w-0 rounded-lg border bg-card p-5 sm:p-8 2xl:p-10">{children}</div>
      </div>
    </div>
  );
}

function StepHeader({ title, why }: { title: string; why: string }) {
  return (
    <>
      <h1 className="font-serif text-2xl font-semibold">{title}</h1>
      <p className="mb-5 mt-1 max-w-[70ch] text-sm text-muted-foreground">{why}</p>
    </>
  );
}

function Actions({ onBack, onSkip, primary, pending, onPrimary, form }: { onBack?: () => void; onSkip?: () => void; primary: string; pending?: boolean; onPrimary?: () => void; form?: string }) {
  return (
    <div className="mt-6 flex items-center gap-2 border-t pt-4">
      {onBack ? <Button variant="ghost" type="button" onClick={onBack}>Back</Button> : null}
      {onSkip ? <Button variant="ghost" type="button" onClick={onSkip}>Skip</Button> : null}
      <span className="flex-1" />
      <Button type={form ? "submit" : "button"} form={form} disabled={pending} onClick={onPrimary} data-testid="setup-primary">{pending ? "Working…" : primary}</Button>
    </div>
  );
}

function Welcome({ needsAdmin, claimed, again, onClaimed, onNext }: { needsAdmin: boolean; claimed: boolean; again: boolean; onClaimed: () => void; onNext: () => void }) {
  const [token, setToken] = React.useState("");
  const claim = useMutation({
    mutationFn: () => api.setupClaim(token.trim()),
    onSuccess: onClaimed,
    onError: (e) => notify.error("That token was not accepted", e),
  });
  const checks = useQuery({ queryKey: ["setup-checks"], queryFn: api.setupChecks, enabled: claimed || !needsAdmin, retry: false });
  const ready = claimed || !needsAdmin;
  return (
    <div>
      <StepHeader title="Welcome to Vyasa" why={needsAdmin ? "This site has no users yet. Paste the setup token from the server log to prove you are its operator." : again ? "Run any step again. Each one saves on its own; Skip leaves a step as it is." : "Setup was started before; continue from where it stopped."} />
      {needsAdmin && !claimed ? (
        <form onSubmit={(e) => { e.preventDefault(); claim.mutate(); }} className="space-y-3">
          <Field label="Setup token" htmlFor="setup-token" hint="Printed by `vyasa serve` at boot and written to .run/setup-token on the server.">
            <Input id="setup-token" value={token} onChange={(e) => setToken(e.target.value)} className="font-mono text-sm" placeholder="stp_…" autoFocus />
          </Field>
          <Actions primary="Continue" pending={claim.isPending} onPrimary={() => claim.mutate()} />
        </form>
      ) : null}
      {ready ? (
        <div className="grid gap-2 lg:grid-cols-2 2xl:grid-cols-3" data-testid="setup-checks">
          {(checks.data ?? []).map((c) => (
            <div key={c.name} className="flex items-baseline gap-2 rounded-md border px-3 py-2 text-sm">
              <span aria-hidden="true" className={cn("mt-1 h-2 w-2 shrink-0 rounded-full", c.status === "ok" ? "bg-success" : c.status === "warn" ? "bg-warning" : "bg-destructive")} />
              <span className="font-mono text-xs text-muted-foreground">{c.name}</span>
              <span className="min-w-0 flex-1">{c.detail}</span>
            </div>
          ))}
          {checks.isPending ? <p className="text-sm text-muted-foreground">Testing the environment…</p> : null}
          <div className="lg:col-span-2 2xl:col-span-3"><Actions primary="Continue" onPrimary={onNext} /></div>
        </div>
      ) : null}
    </div>
  );
}

function Account({ onDone, onBack }: { onDone: (email: string) => void; onBack: () => void }) {
  const [v, setV] = React.useState({ email: "", username: "", display_name: "", password: "", timezone: Intl.DateTimeFormat().resolvedOptions().timeZone });
  const set = (k: keyof typeof v) => (e: React.ChangeEvent<HTMLInputElement>) => setV({ ...v, [k]: e.target.value });
  const create = useMutation({
    mutationFn: () => api.setupStep("account", v),
    onSuccess: () => onDone(v.email),
    onError: (e) => notify.error("Couldn't create the account", e),
  });
  const strength = v.password.length >= 16 ? "Strong" : v.password.length >= 12 ? "Good" : "Twelve characters or more";
  return (
    <form id="acct" onSubmit={(e) => { e.preventDefault(); create.mutate(); }}>
      <StepHeader title="Your account" why="The first administrator. You can invite others under Users afterwards." />
      <div className="grid gap-3 sm:grid-cols-2 2xl:grid-cols-3">
        <Field label="Email" htmlFor="s-email"><Input id="s-email" type="email" required value={v.email} onChange={set("email")} /></Field>
        <Field label="Username" htmlFor="s-user" hint="Shown as the author unless a display name is set."><Input id="s-user" value={v.username} onChange={set("username")} className="font-mono text-sm" /></Field>
        <Field label="Display name" htmlFor="s-name"><Input id="s-name" value={v.display_name} onChange={set("display_name")} /></Field>
        <Field label="Timezone" htmlFor="s-tz" hint="From your browser; scheduling uses it."><Input id="s-tz" value={v.timezone} onChange={set("timezone")} /></Field>
        <div className="sm:col-span-2">
          <Field label="Password" htmlFor="s-pw" hint={strength}><Input id="s-pw" type="password" required minLength={12} value={v.password} onChange={set("password")} /></Field>
        </div>
      </div>
      <Actions onBack={onBack} primary="Create account and continue" pending={create.isPending} form="acct" />
    </form>
  );
}

function Site({ siteUrl, onDone, onBack }: { siteUrl: string; onDone: (url: string) => void; onBack: () => void }) {
  const [v, setV] = React.useState({ site_title: "", site_tagline: "", site_url: siteUrl, site_language: "en", date_format: "" });
  const set = (k: keyof typeof v) => (e: React.ChangeEvent<HTMLInputElement>) => setV({ ...v, [k]: e.target.value });
  const [verified, setVerified] = React.useState<boolean | null>(null);
  const save = useMutation({
    mutationFn: () => api.setupStep<{ site_url_verified: boolean }>("site", v),
    onSuccess: (r) => {
      setVerified(r.site_url_verified);
      if (r.site_url_verified) onDone(v.site_url);
      else notify.error("Saved, but that address does not reach this server", "Check it, or continue if the DNS is still settling.");
    },
    onError: (e) => notify.error("Couldn't save the site details", e),
  });
  return (
    <form id="site" onSubmit={(e) => { e.preventDefault(); save.mutate(); }}>
      <StepHeader title="Your site" why="What readers, search engines and feeds call it." />
      <div className="grid gap-3 sm:grid-cols-2 2xl:grid-cols-3">
        <Field label="Site title" htmlFor="s-title"><Input id="s-title" required value={v.site_title} onChange={set("site_title")} /></Field>
        <Field label="Tagline" htmlFor="s-tag" hint="Optional; the header hides it when empty."><Input id="s-tag" value={v.site_tagline} onChange={set("site_tagline")} /></Field>
        <div className="sm:col-span-2">
          <Field label="Site address" htmlFor="s-url" hint={verified === false ? "This address did not reach this server." : "Canonical links, feeds and email links use it."}>
            <Input id="s-url" required value={v.site_url} onChange={set("site_url")} className="font-mono text-sm" />
          </Field>
        </div>
        <Field label="Language" htmlFor="s-lang" hint="A BCP 47 tag, e.g. en or pt-BR."><Input id="s-lang" value={v.site_language} onChange={set("site_language")} /></Field>
        <Field label="Date format" htmlFor="s-date" hint="Leave empty for the default."><Input id="s-date" value={v.date_format} onChange={set("date_format")} placeholder="%-d %B %Y" /></Field>
      </div>
      <p className="mt-3 text-xs text-muted-foreground">Logo and favicon: the Vyasa mark until you upload yours under Settings; nothing to do now.</p>
      <div className="mt-6 flex items-center gap-2 border-t pt-4">
        <Button variant="ghost" type="button" onClick={onBack}>Back</Button>
        <span className="flex-1" />
        {verified === false ? <Button variant="outline" type="button" onClick={() => onDone(v.site_url)}>Continue anyway</Button> : null}
        <Button type="submit" disabled={save.isPending} data-testid="setup-primary">{save.isPending ? "Working…" : "Save and continue"}</Button>
      </div>
    </form>
  );
}

const STARTERS = [
  { name: "blog", label: "Blog", note: "Serif, single column", swatch: ["#fbfaf8", "#8a3324"] },
  { name: "docs", label: "Docs", note: "Sidebar navigation, table of contents", swatch: ["#ffffff", "#1f4e79"] },
  { name: "portfolio", label: "Portfolio", note: "Dark, image-forward", swatch: ["#0d0d0f", "#c9a227"] },
  { name: "storefront-lite", label: "Storefront", note: "Product listing", swatch: ["#fdfbf7", "#1f5c3d"] },
];

function Content({ onDone, onBack }: { onDone: () => void; onBack: () => void }) {
  const [theme, setTheme] = React.useState("blog");
  const [sample, setSample] = React.useState(true);
  const [perPage, setPerPage] = React.useState("10");
  const save = useMutation({
    mutationFn: () => api.setupStep("content", { theme, sample_content: sample, posts_per_page: Number(perPage) || 10 }),
    onSuccess: onDone,
    onError: (e) => notify.error("Couldn't save the content choices", e),
  });
  return (
    <div>
      <StepHeader title="Content" why="A look to start from, and something to look at." />
      <div className="grid grid-cols-2 gap-2 lg:grid-cols-4" role="radiogroup" aria-label="Starter theme">
        {STARTERS.map((s) => (
          <button key={s.name} type="button" role="radio" aria-checked={theme === s.name} onClick={() => setTheme(s.name)} className={cn("rounded-md border p-3 text-left", theme === s.name ? "border-primary ring-1 ring-primary" : "hover:border-input")}>
            <span className="mb-2 grid h-10 grid-cols-[1fr_3fr] gap-1 rounded" style={{ background: s.swatch[0] }}><span className="rounded-sm" style={{ background: s.swatch[1] }} /><span className="rounded-sm opacity-40" style={{ background: s.swatch[1] }} /></span>
            <span className="block font-serif font-semibold">{s.label}</span>
            <span className="block text-xs text-muted-foreground">{s.note}</span>
          </button>
        ))}
      </div>
      <label className="mt-4 flex items-start gap-2 text-sm">
        <input type="checkbox" checked={sample} onChange={(e) => setSample(e.target.checked)} className="mt-1 accent-primary" />
        <span><b>Add sample content</b><br /><span className="text-muted-foreground">A welcome post and an About page, linked from the main menu. Delete them any time.</span></span>
      </label>
      <div className="mt-3 max-w-xs">
        <Field label="Posts per page" htmlFor="s-pp"><Input id="s-pp" value={perPage} onChange={(e) => setPerPage(e.target.value)} /></Field>
      </div>
      <Actions onBack={onBack} primary="Save and continue" pending={save.isPending} onPrimary={() => save.mutate()} />
    </div>
  );
}

function Delivery({ onDone, onSkip, onBack }: { onDone: () => void; onSkip: () => void; onBack: () => void }) {
  const [cache, setCache] = React.useState("0");
  const [cap, setCap] = React.useState("");
  const save = useMutation({
    mutationFn: () => api.setupStep("delivery", { edge_cache_seconds: Number(cache) || 0, ...(cap.trim() !== "" ? { media_storage_cap_mb: Number(cap) } : {}) }),
    onSuccess: onDone,
    onError: (e) => notify.error("Couldn't save delivery settings", e),
  });
  return (
    <div>
      <StepHeader title="Delivery" why="Storage and CDN come from the environment; the Welcome checks say what is set. These two are yours to choose." />
      <div className="grid gap-3 sm:grid-cols-2 2xl:grid-cols-3">
        <Field label="Edge cache, seconds" htmlFor="s-cache" hint="0 without a CDN. With one, 60 is a good start."><Input id="s-cache" value={cache} onChange={(e) => setCache(e.target.value)} /></Field>
        <Field label="Storage cap, MB" htmlFor="s-cap" hint="Uploads stop at the cap; Site health warns near it. Empty for none."><Input id="s-cap" value={cap} onChange={(e) => setCap(e.target.value)} /></Field>
      </div>
      <pre className="mt-3 overflow-x-auto rounded-md border bg-muted/40 p-3 text-xs">{`# For an S3-compatible bucket (AWS, R2, B2, MinIO), add and restart:
VYASA_STORAGE__PROVIDER=s3
VYASA_STORAGE__BUCKET=my-bucket
VYASA_STORAGE__ENDPOINT=https://…
VYASA_STORAGE__ACCESS_KEY_ID=…
VYASA_STORAGE__SECRET_ACCESS_KEY=…`}</pre>
      <Actions onBack={onBack} onSkip={onSkip} primary="Save and continue" pending={save.isPending} onPrimary={() => save.mutate()} />
    </div>
  );
}

function Mail({ email, onDone, onSkip, onBack }: { email: string; onDone: () => void; onSkip: () => void; onBack: () => void }) {
  const [moderation, setModeration] = React.useState("require_first");
  const [newsletter, setNewsletter] = React.useState(false);
  const [relay, setRelay] = React.useState<{ source: string } | null>(null);
  const save = useMutation({
    mutationFn: () => api.setupStep("mail", { comment_moderation: moderation, newsletter_enabled: newsletter }),
    onSuccess: onDone,
    onError: (e) => notify.error("Couldn't save mail settings", e),
  });
  return (
    <div>
      <StepHeader title="Mail & comments" why="Password resets, comment notifications and the newsletter all need a way out. Add a relay here, or leave it for later under Settings." />
      <MailRelayPanel testTo={email} onSaved={(s) => setRelay(s)} />
      <div className="mt-6 grid gap-3 border-t pt-4 lg:grid-cols-2">
        <Field label="Comments" htmlFor="s-mod">
          <select id="s-mod" value={moderation} onChange={(e) => setModeration(e.target.value)} className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm">
            <option value="auto_approve">Publish at once</option>
            <option value="require_first">Hold until the author has one approved comment</option>
            <option value="require_all">Hold every comment for review</option>
          </select>
        </Field>
        <label className="flex items-start gap-2 text-sm">
          <input type="checkbox" checked={newsletter} onChange={(e) => setNewsletter(e.target.checked)} className="mt-1 accent-primary" />
          <span><b>Newsletter</b><br /><span className="text-muted-foreground">Send subscribers each published post. Stays off without a relay{relay?.source === "options" ? "" : "; save one above first"}. It never emails anyone by surprise.</span></span>
        </label>
      </div>
      <Actions onBack={onBack} onSkip={onSkip} primary="Save and continue" pending={save.isPending} onPrimary={() => save.mutate()} />
    </div>
  );
}

function Assistants({ onDone, onSkip, onBack }: { onDone: () => void; onSkip: () => void; onBack: () => void }) {
  const [v, setV] = React.useState({ provider: "anthropic", api_key: "", monthly_cap_usd: "20", audience: "" });
  const [alt, setAlt] = React.useState(true);
  const [screen, setScreen] = React.useState(false);
  const [related, setRelated] = React.useState(false);
  const save = useMutation({
    mutationFn: () => api.setupStep("assistants", { ...(v.api_key.trim() !== "" ? { provider: v.provider, api_key: v.api_key.trim() } : {}), monthly_cap_usd: Number(v.monthly_cap_usd) || 0, alt_text: alt, comment_screening: screen, related_posts: related, audience: v.audience }),
    onSuccess: onDone,
    onError: (e) => notify.error("Couldn't save the assistant settings", e),
  });
  return (
    <div>
      <StepHeader title="Assistants" why="Optional. A text model powers writing help, alt text needs vision, pictures need an image model. Keys are encrypted with the secret key and never shown again." />
      <div className="grid gap-3 sm:grid-cols-2 2xl:grid-cols-3">
        <Field label="Provider" htmlFor="s-prov">
          <select id="s-prov" value={v.provider} onChange={(e) => setV({ ...v, provider: e.target.value })} className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm">
            <option value="anthropic">Anthropic</option><option value="openai">OpenAI</option><option value="openrouter">OpenRouter</option><option value="gemini">Google Gemini</option><option value="mistral">Mistral</option>
          </select>
        </Field>
        <Field label="API key" htmlFor="s-key" hint="Leave empty to set up assistants later."><Input id="s-key" type="password" value={v.api_key} onChange={(e) => setV({ ...v, api_key: e.target.value })} className="font-mono text-sm" /></Field>
        <Field label="Monthly cap, USD" htmlFor="s-capusd" hint="Every assistant refuses past it."><Input id="s-capusd" value={v.monthly_cap_usd} onChange={(e) => setV({ ...v, monthly_cap_usd: e.target.value })} /></Field>
        <Field label="Who is this site for?" htmlFor="s-aud" hint="One sentence the assistants keep in mind."><Input id="s-aud" value={v.audience} onChange={(e) => setV({ ...v, audience: e.target.value })} /></Field>
      </div>
      <div className="mt-3 grid gap-2 text-sm">
        <label className="flex items-center gap-2"><input type="checkbox" checked={alt} onChange={(e) => setAlt(e.target.checked)} className="accent-primary" />Write alt text for uploads that have none</label>
        <label className="flex items-center gap-2"><input type="checkbox" checked={screen} onChange={(e) => setScreen(e.target.checked)} className="accent-primary" />Screen comments before they publish</label>
        <label className="flex items-center gap-2"><input type="checkbox" checked={related} onChange={(e) => setRelated(e.target.checked)} className="accent-primary" />Related posts and semantic search (needs an embedding model)</label>
      </div>
      <Actions onBack={onBack} onSkip={onSkip} primary="Save and continue" pending={save.isPending} onPrimary={() => save.mutate()} />
    </div>
  );
}

function Updates({ onDone, onSkip, onBack }: { onDone: () => void; onSkip: () => void; onBack: () => void }) {
  const [v, setV] = React.useState({ update_channel_url: "", registry_url: `${window.location.origin}/registry/index.json`, trusted_keys: "" });
  const [pair, setPair] = React.useState<{ public_key: string; private_key: string } | null>(null);
  const keygen = useMutation({ mutationFn: () => api.setupStep<{ public_key: string; private_key: string }>("keypair", {}), onSuccess: setPair, onError: (e) => notify.error("Couldn't generate a key", e) });
  const save = useMutation({
    mutationFn: () => api.setupStep("updates", { ...v, trusted_keys: v.trusted_keys.split(/[\s,]+/).filter(Boolean) }),
    onSuccess: onDone,
    onError: (e) => notify.error("Couldn't save update settings", e),
  });
  return (
    <div>
      <StepHeader title="Updates & marketplace" why="Where new releases and packages come from, and whose signatures to trust." />
      <div className="grid gap-3 lg:grid-cols-2 2xl:grid-cols-3">
        <Field label="Update channel" htmlFor="s-uc" hint="Leave empty to keep the official channel."><Input id="s-uc" value={v.update_channel_url} onChange={(e) => setV({ ...v, update_channel_url: e.target.value })} className="font-mono text-sm" /></Field>
        <Field label="Marketplace" htmlFor="s-reg" hint="Themes and plugins offered under Appearance and Plugins."><Input id="s-reg" value={v.registry_url} onChange={(e) => setV({ ...v, registry_url: e.target.value })} className="font-mono text-sm" /></Field>
        <Field label="Trusted keys" htmlFor="s-keys" hint="Hex ed25519 public keys, one per line. Packages signed by anyone else are refused."><Input id="s-keys" value={v.trusted_keys} onChange={(e) => setV({ ...v, trusted_keys: e.target.value })} className="font-mono text-sm" /></Field>
      </div>
      <div className="mt-3 space-y-2">
        <Button type="button" variant="outline" disabled={keygen.isPending} onClick={() => keygen.mutate()}>Generate a plugin signing key for this site</Button>
        {pair ? (
          <div className="rounded-md border bg-muted/40 p-3 text-xs" data-testid="setup-keypair">
            <p className="font-medium">Shown once. Keep the private half with your deploy secrets.</p>
            <p className="mt-1 break-all"><span className="text-muted-foreground">public</span> <code>{pair.public_key}</code></p>
            <p className="break-all"><span className="text-muted-foreground">private</span> <code>{pair.private_key}</code></p>
          </div>
        ) : null}
      </div>
      <Actions onBack={onBack} onSkip={onSkip} primary="Save and finish" pending={save.isPending} onPrimary={() => save.mutate()} />
    </div>
  );
}

function Done({ skipped, siteUrl }: { skipped: Step[]; siteUrl: string }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const finish = useQuery({ queryKey: ["setup-finish"], queryFn: () => api.setupStep<{ indexed: number }>("finish", {}), retry: false });
  const goto = (to: string) => {
    void queryClient.invalidateQueries({ queryKey: ["me"] });
    window.location.assign(to);
  };
  return (
    <div>
      <StepHeader title="Ready" why={`Your site is live at ${siteUrl}.`} />
      <div className="space-y-2 text-sm">
        {finish.isPending ? <p className="text-muted-foreground">Building the search index…</p> : finish.data ? <p>Search index built: {finish.data.indexed} {finish.data.indexed === 1 ? "entry" : "entries"}.</p> : <p className="text-destructive">The final step did not complete; open Settings and run it again.</p>}
        {skipped.length > 0 ? <p className="text-muted-foreground">Skipped: {skipped.map((s) => LABELS[s]).join(", ")}. Settings → Run setup again reopens any one of them.</p> : null}
      </div>
      <div className="mt-6 flex flex-wrap gap-2">
        <Button onClick={() => goto("/admin/posts/new")}>Write your first post</Button>
        <Button variant="outline" onClick={() => goto("/admin/users")}>Invite someone</Button>
        <Button variant="outline" onClick={() => { void navigate({ to: "/" }); }}>Open the dashboard</Button>
        <a href={siteUrl} target="_blank" rel="noreferrer" className="inline-flex h-9 items-center rounded-md border px-3 text-sm">See the site</a>
      </div>
    </div>
  );
}
