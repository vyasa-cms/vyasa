import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  Check,
  ExternalLink,
  Images,
  Palette,
  PenLine,
  Plus,
  Sparkles,
  Trash2,
  Undo2,
  Upload,
} from "lucide-react";

import {
  activate,
  bundledPathFor,
  chat,
  createDraft,
  deleteDraft,
  deleteTheme,
  deleteThemeFile,
  installTheme,
  listDrafts,
  listThemeFiles,
  listThemes,
  putThemeFile,
  rollbackTheme,
  slotValue,
  themePreviewUrl,
  type ColorPalette,
  type DraftSummary,
  type ThemeFile,
  type ThemeSummary,
} from "@/api/themes";
import { Button } from "@/components/ui/button";
import { Modal, useConfirm } from "@/components/ui/dialog";
import {
  Chip,
  EmptyState,
  ErrorNote,
  Field,
  PageHeader,
  Panel,
  Skeleton,
} from "@/components/ui/primitives";
import { RegistryBrowser } from "@/components/RegistryBrowser";
import { notify } from "@/components/ui/toast";
import { useAssistantModels } from "@/components/studio/AssistantPanel";
import { formatRelative } from "@/routes/_auth/index";
import { errorSummary } from "@/lib/error-text";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/appearance/")({
  component: AppearanceRoute,
});

function AppearanceRoute() {
  const navigate = useNavigate();
  return (
    <AppearancePage
      openStudio={(draftId) =>
        void navigate({ to: "/appearance/studio/$draftId", params: { draftId } })
      }
    />
  );
}

/** Three swatches summarising a palette, so a list isn't just names. */
function Swatches({ colors }: { colors: ColorPalette | null | undefined }) {
  if (colors === null || colors === undefined) {
    return <Skeleton className="h-6 w-16 rounded" />;
  }
  return (
    <span className="flex shrink-0 gap-1" aria-hidden="true">
      {(["bg", "primary", "text"] as const).map((role) => (
        <span
          key={role}
          className="h-6 w-5 rounded border"
          style={{ background: slotValue(colors[role], false) }}
        />
      ))}
    </span>
  );
}

function ThemeSwatches({ id }: { id: string }) {
  const tokens = useQuery({
    queryKey: ["theme-tokens", id],
    queryFn: () => import("@/api/themes").then((m) => m.themeTokens(id)),
    retry: false,
    staleTime: 300_000,
  });
  return <Swatches colors={tokens.data?.tokens.colors} />;
}

export function AppearancePage({
  openStudio,
}: {
  openStudio: (draftId: string) => void;
}) {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [newOpen, setNewOpen] = React.useState(false);
  const [prompt, setPrompt] = React.useState("");
  const fileRef = React.useRef<HTMLInputElement>(null);
  const models = useAssistantModels();

  const themes = useQuery({ queryKey: ["themes"], queryFn: listThemes });
  const [filesFor, setFilesFor] = React.useState<ThemeSummary | null>(null);
  const drafts = useQuery({ queryKey: ["theme-drafts"], queryFn: listDrafts });
  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["themes"] });
    void queryClient.invalidateQueries({ queryKey: ["theme-drafts"] });
  };

  const start = useMutation({
    mutationFn: (input: { name?: string; base_theme_id?: string }) => createDraft(input),
    onSuccess: (draft) => {
      invalidate();
      openStudio(draft.id);
    },
    onError: (e) => notify.error("Couldn't start a draft", e),
  });

  // "Describe a theme": a draft from the live theme with the description
  // as its first request. The studio opens with the assistant on screen,
  // so there is nothing to point it at.
  const describe = useMutation({
    mutationFn: async (text: string) => {
      const draft = await createDraft({ name: "New design" });
      await chat(draft.id, text);
      return draft;
    },
    onSuccess: (draft) => {
      setPrompt("");
      invalidate();
      openStudio(draft.id);
    },
    onError: (e) => notify.error("Couldn't start the design", e),
  });

  const activateMutation = useMutation({
    mutationFn: (id: string) => activate(id),
    onSuccess: () => {
      invalidate();
      notify.success("Theme activated", "Your site is using it now.");
    },
    onError: (e) => notify.error("Couldn't activate the theme", e),
  });

  const removeTheme = useMutation({
    mutationFn: (id: string) => deleteTheme(id),
    onSuccess: () => {
      invalidate();
      notify.success("Theme removed");
    },
    onError: (e) => notify.error("Couldn't remove the theme", e),
  });

  const rollback = useMutation({
    mutationFn: (name: string) => rollbackTheme(name),
    onSuccess: (theme) => {
      invalidate();
      notify.success(`Rolled back to ${theme.name} v${theme.version}`);
    },
    onError: (e) => notify.error("Couldn't roll back", e),
  });

  const removeDraft = useMutation({
    mutationFn: (id: string) => deleteDraft(id),
    onSuccess: () => {
      invalidate();
      notify.success("Draft discarded");
    },
    onError: (e) => notify.error("Couldn't discard the draft", e),
  });

  const upload = useMutation({
    mutationFn: (file: File) => installTheme(file),
    onSuccess: (theme) => {
      invalidate();
      notify.success(`Installed ${theme.name} v${theme.version}`, "Activate it when you're ready.");
    },
    onError: (e) => notify.error("Couldn't install the package", e),
  });

  const askActivate = async (name: string, id: string) => {
    const live = themes.data?.find((t) => t.is_active);
    const ok = await confirm({
      title: `Make “${name}” your live theme?`,
      description:
        live === undefined
          ? "Every visitor sees this design straight away."
          : `Every visitor sees this design straight away, replacing “${live.name}”. You can switch back at any time.`,
      confirmLabel: "Activate",
    });
    if (ok) activateMutation.mutate(id);
  };

  const askDeleteTheme = async (t: ThemeSummary) => {
    const ok = await confirm({
      title: `Remove ${t.name} v${t.version}?`,
      description: "The installed version is deleted. Drafts started from it are kept.",
      confirmLabel: "Remove",
      destructive: true,
    });
    if (ok) removeTheme.mutate(t.id);
  };

  const askDeleteDraft = async (d: DraftSummary) => {
    const ok = await confirm({
      title: `Discard “${d.name}”?`,
      description: "The draft and its history are deleted. Anything you published from it stays.",
      confirmLabel: "Discard",
      destructive: true,
    });
    if (ok) removeDraft.mutate(d.id);
  };

  const live = themes.data?.find((t) => t.is_active);
  const canRollBack =
    live !== undefined &&
    (themes.data ?? []).some((t) => t.name === live.name && t.version < live.version);

  return (
    <div className="space-y-5 sm:space-y-6">
      <PageHeader
        title="Appearance"
        description="Your installed themes, and the drafts you're working on in the studio."
        actions={
          <>
            <input
              ref={fileRef}
              type="file"
              accept=".vytheme,.zip"
              className="hidden"
              aria-label="Theme package"
              onChange={(e) => {
                const file = e.target.files?.[0];
                if (file !== undefined) upload.mutate(file);
                e.target.value = "";
              }}
            />
            <Button
              variant="outline"
              size="sm"
              disabled={upload.isPending}
              onClick={() => fileRef.current?.click()}
            >
              <Upload className="h-4 w-4" aria-hidden="true" />
              {upload.isPending ? "Installing…" : "Upload package"}
            </Button>
            <Button size="sm" onClick={() => setNewOpen(true)} data-testid="new-draft">
              <Plus className="h-4 w-4" aria-hidden="true" />
              New draft
            </Button>
          </>
        }
      />

      <RegistryBrowser kind="theme" />

      {models.text ? (
        <Panel
          testId="theme-builder"
          title="Describe a theme"
          description="Tell the assistant about your site. It starts a draft from your live theme and shapes it; nothing goes live until you publish."
        >
          <div className="space-y-3">
            <label className="block">
              <span className="sr-only">Site description</span>
              <textarea
                aria-label="site description"
                rows={3}
                value={prompt}
                onChange={(e) => setPrompt(e.target.value)}
                placeholder="A reading-focused journal for a Rust systems blog. Warm paper feel, generous line height, one rust-orange accent."
                className="w-full rounded-md border border-input bg-background p-3 text-sm placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              />
            </label>
            <div className="flex flex-wrap items-center gap-2">
              <Button
                data-testid="generate-button"
                disabled={prompt.trim().length < 5 || describe.isPending}
                onClick={() => describe.mutate(prompt.trim())}
              >
                <Sparkles className="h-4 w-4" aria-hidden="true" />
                {describe.isPending ? "Starting…" : "Design it"}
              </Button>
              <span className="text-xs text-muted-foreground">
                Opens in the studio while the assistant works.
              </span>
            </div>
          </div>
        </Panel>
      ) : null}

      <section className="space-y-3" data-testid="drafts-section">
        <h2 className="text-sm font-semibold">Drafts</h2>
        {drafts.isPending ? (
          <Skeleton className="h-24 w-full rounded-lg" />
        ) : drafts.isError ? (
          <ErrorNote title="Couldn't load drafts" error={drafts.error} />
        ) : (drafts.data ?? []).length === 0 ? (
          <EmptyState
            icon={Sparkles}
            title="No drafts yet"
            description="Start one from your live theme and shape it in the studio. Nothing changes on your site until you publish."
            action={
              <Button size="sm" variant="outline" onClick={() => setNewOpen(true)}>
                Start a draft
              </Button>
            }
          />
        ) : (
          <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3" data-testid="draft-list">
            {(drafts.data ?? []).map((d) => (
              <li key={d.id} className="rounded-lg border bg-card p-3">
                <div className="flex items-start gap-2">
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{d.name}</p>
                    <p className="text-xs text-muted-foreground">
                      Revision {d.revision} · edited {formatRelative(d.updated_at)}
                    </p>
                  </div>
                  <Swatches colors={d.colors} />
                </div>
                {d.status === "generating" ? (
                  <p className="mt-2 text-xs text-muted-foreground">The assistant is working on it…</p>
                ) : d.status === "failed" ? (
                  <p
                    className="mt-2 line-clamp-3 text-xs text-destructive"
                    title={d.status_note ?? undefined}
                  >
                    {errorSummary(d.status_note ?? "The last assistant run failed.").summary}
                  </p>
                ) : null}
                <div className="mt-3 flex flex-wrap items-center gap-2">
                  <Button size="sm" onClick={() => openStudio(d.id)} data-testid={`open-${d.id}`}>
                    <PenLine className="h-3.5 w-3.5" aria-hidden="true" />
                    Open in studio
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => void askDeleteDraft(d)}
                    aria-label={`Discard ${d.name}`}
                  >
                    <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                  </Button>
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="space-y-3" data-testid="themes-section">
        <div className="flex items-center gap-3">
          <h2 className="text-sm font-semibold">Installed themes</h2>
          {canRollBack && live !== undefined ? (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => rollback.mutate(live.name)}
              disabled={rollback.isPending}
            >
              <Undo2 className="h-3.5 w-3.5" aria-hidden="true" />
              Roll back {live.name}
            </Button>
          ) : null}
        </div>
        {themes.isPending ? (
          <Skeleton className="h-24 w-full rounded-lg" />
        ) : themes.isError ? (
          <ErrorNote title="Couldn't load themes" error={themes.error} />
        ) : (themes.data ?? []).length === 0 ? (
          <EmptyState
            icon={Palette}
            title="No themes installed"
            description="Upload a theme package to get started."
          />
        ) : (
          <ul className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3" data-testid="themes-list">
            {(themes.data ?? []).map((t: ThemeSummary) => (
              <li
                key={t.id}
                className={cn(
                  "rounded-lg border bg-card p-3",
                  t.is_active && "border-success ring-1 ring-success/40",
                )}
              >
                <div className="flex items-start gap-2">
                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium">{t.name}</p>
                    <p className="text-xs text-muted-foreground">
                      Version {t.version}
                      {t.package_version !== null && t.package_version !== undefined && t.package_version !== t.version
                        ? ` · package ${t.package_version}`
                        : ""}
                    </p>
                  </div>
                  <ThemeSwatches id={t.id} />
                </div>
                <div className="mt-3 flex flex-wrap items-center gap-2">
                  {t.is_active ? (
                    <span data-testid={`active-${t.name}`}>
                      <Chip tone="success">
                        <Check className="h-3 w-3" aria-hidden="true" />
                        Live
                      </Chip>
                    </span>
                  ) : (
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => void askActivate(t.name, t.id)}
                    >
                      Activate
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => start.mutate({ base_theme_id: t.id })}
                    disabled={start.isPending}
                    aria-label={`Edit ${t.name} v${t.version} in the studio`}
                  >
                    <PenLine className="h-3.5 w-3.5" aria-hidden="true" />
                    Edit
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    onClick={() => setFilesFor(t)}
                    aria-label={`Files bundled with ${t.name} v${t.version}`}
                    data-testid={`files-${t.id}`}
                  >
                    <Images className="h-3.5 w-3.5" aria-hidden="true" />
                    Files
                  </Button>
                  <a
                    href={themePreviewUrl(t.id)}
                    target="_blank"
                    rel="noreferrer"
                    data-testid={`preview-${t.id}`}
                    className="inline-flex h-8 items-center gap-1.5 rounded-md px-2 text-xs text-muted-foreground hover:bg-accent hover:text-foreground"
                  >
                    <ExternalLink className="h-3.5 w-3.5" aria-hidden="true" />
                    Preview
                  </a>
                  {!t.is_active ? (
                    <Button
                      size="sm"
                      variant="ghost"
                      className="ml-auto"
                      onClick={() => void askDeleteTheme(t)}
                      aria-label={`Remove ${t.name} v${t.version}`}
                    >
                      <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                    </Button>
                  ) : null}
                </div>
              </li>
            ))}
          </ul>
        )}
      </section>

      <ThemeFilesDialog theme={filesFor} onClose={() => setFilesFor(null)} />

      <NewDraftDialog
        open={newOpen}
        onClose={() => setNewOpen(false)}
        themes={themes.data ?? []}
        pending={start.isPending}
        onCreate={(input) => start.mutate(input)}
      />
    </div>
  );
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(1)} MB`;
}

/**
 * The pictures and fonts one installed version bundles. Files belong to
 * the version, not to a draft: a change here is live the moment the
 * version is, and a studio publish carries the set forward.
 */
function ThemeFilesDialog({
  theme,
  onClose,
}: {
  theme: ThemeSummary | null;
  onClose: () => void;
}) {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const id = theme?.id ?? "";
  const files = useQuery({
    queryKey: ["theme-files", id],
    queryFn: () => listThemeFiles(id),
    enabled: theme !== null,
  });
  const refresh = () => void queryClient.invalidateQueries({ queryKey: ["theme-files", id] });
  const upload = useMutation({
    mutationFn: ({ dir, file }: { dir: "images" | "fonts"; file: File }) =>
      putThemeFile(id, bundledPathFor(dir, file.name), file),
    onSuccess: (f) => {
      notify.success(`Added ${f.path}`);
      refresh();
    },
    onError: (e) => notify.error("That did not work", e),
  });
  const remove = useMutation({
    mutationFn: (path: string) => deleteThemeFile(id, path),
    onSuccess: refresh,
    onError: (e) => notify.error("That did not work", e),
  });

  const pick = (dir: "images" | "fonts") => {
    const input = document.createElement("input");
    input.type = "file";
    input.accept =
      dir === "images"
        ? ".png,.jpg,.jpeg,.gif,.webp,.avif,.svg"
        : ".woff2,.woff,.ttf,.otf";
    input.hidden = true;
    input.onchange = () => {
      const file = input.files?.[0];
      if (file !== undefined) upload.mutate({ dir, file });
      input.remove();
    };
    // Attached while open: Safari ignores a click on a detached input.
    document.body.appendChild(input);
    input.click();
  };

  return (
    <Modal
      open={theme !== null}
      onClose={onClose}
      title={theme ? `Files in ${theme.name} v${theme.version}` : "Files"}
      description="Pictures and fonts this version ships. Refer to them as /theme-assets/images/… or /theme-assets/fonts/… in the layout, the tokens, or the theme's stylesheet."
      testId="theme-files-dialog"
      footer={
        <>
          <Button variant="outline" onClick={() => pick("images")} disabled={upload.isPending}>
            <Upload className="h-3.5 w-3.5" aria-hidden="true" />
            Add picture
          </Button>
          <Button variant="outline" onClick={() => pick("fonts")} disabled={upload.isPending}>
            <Upload className="h-3.5 w-3.5" aria-hidden="true" />
            Add font
          </Button>
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
        </>
      }
    >
      {files.isPending ? (
        <Skeleton className="h-16" />
      ) : files.isError ? (
        <ErrorNote title="Couldn't load the files" error={files.error} />
      ) : (files.data ?? []).length === 0 ? (
        <p className="text-sm text-muted-foreground" data-testid="theme-files-empty">
          This version bundles no files yet.
        </p>
      ) : (
        <ul className="divide-y rounded-md border text-sm" data-testid="theme-files-list">
          {(files.data ?? []).map((f: ThemeFile) => (
            <li key={f.path} className="flex items-center gap-3 px-3 py-2">
              {f.content_type.startsWith("image/") ? (
                <img
                  src={f.url}
                  alt=""
                  className="h-8 w-8 rounded object-cover bg-muted"
                  loading="lazy"
                />
              ) : (
                <span className="flex h-8 w-8 items-center justify-center rounded bg-muted text-[10px] uppercase text-muted-foreground">
                  {f.path.split(".").pop()}
                </span>
              )}
              <div className="min-w-0 flex-1">
                <p className="truncate font-mono text-xs">{f.path}</p>
                <p className="text-xs text-muted-foreground">
                  {f.content_type} · {formatBytes(f.size)}
                </p>
              </div>
              <Button
                size="sm"
                variant="ghost"
                aria-label={`Remove ${f.path}`}
                disabled={remove.isPending}
                onClick={() =>
                  void confirm({
                    title: `Remove ${f.path}?`,
                    description: "Anything in the theme that refers to it will show a missing file.",
                    confirmLabel: "Remove",
                    destructive: true,
                  }).then((ok) => {
                    if (ok) remove.mutate(f.path);
                  })
                }
              >
                <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
              </Button>
            </li>
          ))}
        </ul>
      )}
    </Modal>
  );
}

function NewDraftDialog({
  open,
  onClose,
  themes,
  pending,
  onCreate,
}: {
  open: boolean;
  onClose: () => void;
  themes: ThemeSummary[];
  pending: boolean;
  onCreate: (input: { name?: string; base_theme_id?: string }) => void;
}) {
  const live = themes.find((t) => t.is_active);
  const [name, setName] = React.useState("");
  const [base, setBase] = React.useState<string>("");
  React.useEffect(() => {
    if (open) {
      setName("");
      setBase(live?.id ?? "");
    }
  }, [open, live?.id]);

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Start a draft"
      description="A working copy you can shape and preview. Publish it when it's ready."
      testId="new-draft-dialog"
      footer={
        <>
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={pending}
            data-testid="create-draft"
            onClick={() =>
              onCreate({
                ...(name.trim() === "" ? {} : { name: name.trim() }),
                ...(base === "" ? {} : { base_theme_id: base }),
              })
            }
          >
            {pending ? "Starting…" : "Open in studio"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Start from" htmlFor="draft-base" hint="Copies the theme's colours, layout and templates.">
          <select
            id="draft-base"
            value={base}
            onChange={(e) => setBase(e.target.value)}
            className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
          >
            {themes.map((t) => (
              <option key={t.id} value={t.id}>
                {t.name} v{t.version}
                {t.is_active ? " (live)" : ""}
              </option>
            ))}
          </select>
        </Field>
        <Field label="Working title" htmlFor="draft-name" hint="Optional. You can rename it in the studio.">
          <input
            id="draft-name"
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder={live === undefined ? "New theme" : `${live.name} (draft)`}
            className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm"
          />
        </Field>
      </div>
    </Modal>
  );
}
