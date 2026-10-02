import { TestRouter } from "./TestRouter";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import * as themesApi from "@/api/themes";
import type { Draft, Vocabulary } from "@/api/themes";

const tokens: Draft["tokens"] = {
  version: 1,
  colors: {
    bg: { light: "#fdfcfb", dark: "#141416" },
    surface: { light: "#ffffff", dark: "#1d1d20" },
    text: { light: "#232323", dark: "#e8e8e6" },
    text_muted: { light: "#5f5f66", dark: "#a3a3ab" },
    border: { light: "#e7e4df", dark: "#2c2c31" },
    primary: { light: "#b3541e", dark: "#f0955c" },
    on_primary: { light: "#ffffff", dark: "#241009" },
  },
  typography: {
    heading: "serif",
    body: "system_ui",
    base_size_px: 17,
    scale_ratio: "perfect_fourth",
    font_faces: [],
  },
  spacing: { unit_px: 4, section_scale: [1, 2, 4, 8] },
  radius_px: 6,
  shadow: "small",
  layout: {
    content_width_px: 720,
    sidebar_width_px: 280,
    density: "comfortable",
    breakpoint_sm_px: 640,
    breakpoint_md_px: 900,
  },
  direction: "ltr",
};

const draft: Draft = {
  id: "77",
  name: "blog (draft)",
  base_theme_id: "1",
  status: "ready",
  status_note: null,
  tokens,
  layout: {
    index: [
      { id: "header", kind: "header" },
      { id: "posts", kind: "latest-posts", settings: { count: 10 } },
      { id: "footer", kind: "footer" },
    ],
    single: [{ id: "body", kind: "post-content" }],
    archive: [{ id: "listing", kind: "latest-posts" }],
    page: [{ id: "body", kind: "post-content" }],
    search: [{ id: "results", kind: "latest-posts" }],
    "not-found": [{ id: "message", kind: "content" }],
  },
  templates: {},
  revision: 1,
  created_at: "2026-08-29T00:00:00Z",
  updated_at: "2026-08-29T00:00:00Z",
  assets: null,
  warnings: [],
};

const vocab: Vocabulary = {
  static_regions: [
    { kind: "header", description: "Site name and tagline" },
    { kind: "content", description: "Main content column" },
    { kind: "post-content", description: "Body of the current post" },
    { kind: "footer", description: "Site footer" },
  ],
  blocks: [
    {
      kind: "band",
      description: "Full-width band",
      container: true,
      settings_schema: { type: "object", properties: {} },
    },
    {
      kind: "cta-band",
      description: "Closing call to action",
      container: false,
      settings_schema: {
        type: "object",
        properties: { headline: { type: "string" } },
      },
    },
    {
      kind: "feature-grid",
      description: "Titled cards",
      container: false,
      settings_schema: {
        type: "object",
        properties: {
          items: {
            type: "array",
            maxItems: 12,
            required: ["title"],
            properties: {},
            items: {
              type: "object",
              required: ["title"],
              properties: { title: { type: "string" }, body: { type: "string" } },
            },
          },
        },
      },
    },
    {
      kind: "latest-posts",
      description: "Most recent published posts",
      settings_schema: {
        type: "object",
        properties: { count: { type: "integer", minimum: 1, maximum: 20 } },
      },
    },
    {
      kind: "search-box",
      description: "Site search form",
      settings_schema: {
        type: "object",
        properties: { placeholder: { type: "string", maxLength: 100 } },
      },
    },
    {
      kind: "collection",
      description: "Entries from any content source",
      category: "content",
      settings_schema: {
        type: "object",
        properties: {
          bind: {
            type: "object",
            properties: {},
          },
          heading: { type: "string" },
          columns: { type: "integer", minimum: 1, maximum: 4 },
        },
      },
    },
  ],
  templates: ["index", "single", "archive", "page", "search", "not-found"],
  template_files: [
    "base.html",
    "index.html",
    "single.html",
    "archive.html",
    "page.html",
    "search.html",
    "not-found.html",
  ].map((name) => ({ name, builtin_source: `<!-- builtin ${name} -->` })),
  token_schema: {},
  sources: [
    {
      slug: "post",
      singular: "Post",
      plural: "Posts",
      public: true,
      has_archive: true,
      plugin: false,
      count: 12,
    },
    {
      slug: "book",
      singular: "Book",
      plural: "Books",
      public: true,
      has_archive: true,
      plugin: true,
      count: 3,
    },
  ],
};

vi.mock("@/api/themes", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  getDraft: vi.fn(),
  vocabulary: vi.fn(),
  applyOps: vi.fn(),
  previewCandidate: vi.fn().mockResolvedValue("<!doctype html><p>preview</p>"),
  listRevisions: vi.fn().mockResolvedValue([
    { seq: 1, note: "Started the draft", source: "start", created_at: "2026-08-29T00:00:00Z" },
  ]),
  listThemes: vi.fn().mockResolvedValue([
    { id: "1", name: "blog", version: 2, is_active: true },
  ]),
  renameDraft: vi.fn(),
  revertDraft: vi.fn(),
  publishDraft: vi.fn().mockResolvedValue({ id: "5", name: "blog", version: 3, is_active: true }),
  chat: vi.fn().mockResolvedValue({ message_id: "m1", status: "generating" }),
  listMessages: vi.fn().mockResolvedValue([]),
  acceptProposal: vi.fn(),
}));

vi.mock("@/components/editor/MonacoPane", () => ({
  MonacoPane: ({
    value,
    onChange,
    ariaLabel,
  }: {
    value: string;
    onChange: (v: string) => void;
    ariaLabel?: string;
  }) => (
    <textarea aria-label={ariaLabel} value={value} onChange={(e) => onChange(e.target.value)} />
  ),
}));

const { aiModels } = vi.hoisted(() => ({ aiModels: vi.fn() }));

vi.mock("@/api/client", async (importOriginal) => {
  const mod = await importOriginal<{ api: Record<string, unknown> }>();
  return {
    ...mod,
    api: {
      ...mod.api,
      listPosts: vi.fn().mockResolvedValue({ items: [], total: 0 }),
      aiModels,
    },
  };
});

import { StudioPage } from "@/components/studio/StudioPage";
import { undoTarget } from "@/components/studio/StudioPage";
import { diffOps } from "@/components/studio/useDraft";
import { uniqueId } from "@/components/studio/sectionTree";
import { slugify } from "@/components/studio/dialogs";

/**
 * jsdom's stand-in always reports "does not match". Studio layout keys off
 * a width query, so a test that wants the three-pane shell has to say so.
 */
function atWidth(px: number) {
  Object.defineProperty(window, "matchMedia", {
    writable: true,
    value: (query: string) => {
      const min = /min-width:\s*(\d+)px/.exec(query);
      const max = /max-width:\s*(\d+)px/.exec(query);
      const matches =
        min !== null ? px >= Number(min[1]) : max !== null ? px <= Number(max[1]) : false;
      return {
        matches,
        media: query,
        onchange: null,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
        addListener: () => undefined,
        removeListener: () => undefined,
        dispatchEvent: () => false,
      };
    },
  });
}

function mount(onExit = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <TestRouter><StudioPage draftId="77" onExit={onExit} /></TestRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  // Back to the default stand-in: every query reports "does not match",
  // which is the narrow layout. A test that wants otherwise calls atWidth.
  atWidth(0);
  localStorage.clear();
  aiModels.mockResolvedValue({
    encrypting_keys: false,
    providers: [],
    kinds: [],
    models: [{ kind: "text", is_default: true, enabled: true }],
  });
  vi.mocked(themesApi.getDraft).mockResolvedValue(structuredClone(draft));
  vi.mocked(themesApi.vocabulary).mockResolvedValue(vocab);
  vi.mocked(themesApi.applyOps).mockImplementation(async () => ({
    ...structuredClone(draft),
    revision: 2,
  }));
});

describe("theme studio", () => {
  it("saves a colour change as one set_tokens revision, and renders the preview from it", async () => {
    const user = userEvent.setup();
    mount();
    const hex = await screen.findByLabelText("Primary light");
    await user.clear(hex);
    await user.type(hex, "#123456");
    await waitFor(
      () => expect(vi.mocked(themesApi.applyOps)).toHaveBeenCalledTimes(1),
      { timeout: 3000 },
    );
    const [id, ops] = vi.mocked(themesApi.applyOps).mock.calls[0] ?? [];
    expect(id).toBe("77");
    expect(ops).toHaveLength(1);
    const op = ops?.[0];
    expect(op?.op).toBe("set_tokens");
    if (op?.op === "set_tokens") {
      expect(op.tokens.colors.primary.light).toBe("#123456");
      expect(op.tokens.colors.primary.dark).toBe("#f0955c");
    }
    await waitFor(() => expect(screen.getByText("Revision 2")).toBeInTheDocument());
    // The preview was asked to render the unsaved candidate.
    const previews = vi.mocked(themesApi.previewCandidate).mock.calls;
    const last = previews[previews.length - 1]?.[0];
    expect(last?.tokens.colors.primary.light).toBe("#123456");
    expect(last?.path).toBe("/");
    expect(last?.base_theme_id).toBe("1");
  });

  it("does not send a half-typed hex value", async () => {
    const user = userEvent.setup();
    mount();
    const hex = await screen.findByLabelText("Primary light");
    await user.clear(hex);
    await user.type(hex, "#12");
    await new Promise((r) => setTimeout(r, 900));
    expect(vi.mocked(themesApi.applyOps)).not.toHaveBeenCalled();
  });

  it("reorders layout blocks with a set_layout op for that template only", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Layers" }));
    await user.click(screen.getByRole("button", { name: "Move posts down" }));
    await waitFor(
      () => expect(vi.mocked(themesApi.applyOps)).toHaveBeenCalledTimes(1),
      { timeout: 3000 },
    );
    const ops = vi.mocked(themesApi.applyOps).mock.calls[0]?.[1] ?? [];
    expect(ops).toEqual([
      {
        op: "set_layout",
        template: "index",
        blocks: [
          { id: "header", kind: "header" },
          { id: "footer", kind: "footer" },
          { id: "posts", kind: "latest-posts", settings: { count: 10 } },
        ],
      },
    ]);
  });

  it("adds a section with a unique id and a settings form from the schema", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Insert" }));
    await user.click(screen.getByRole("button", { name: "search-box" }));

    await user.click(screen.getByRole("tab", { name: "Layers" }));
    const list = screen.getByLabelText("Home sections");
    // The row shows the kind and, beneath it, the derived id — which for a
    // first section of this kind is the same string.
    expect(within(list).getAllByText("search-box")).toHaveLength(2);
    // The rail carries structure and the inspector carries properties, so
    // the schema-driven field for what was just added opens beside the
    // tree rather than unfolding inside the row and pushing it off screen.
    const inspector = screen.getByTestId("inspector");
    expect(within(inspector).getByText("placeholder")).toBeInTheDocument();
    expect(within(list).queryByText("placeholder")).toBeNull();
  });

  it("nests a section into the container above it", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Insert" }));
    await user.click(screen.getByRole("button", { name: "band" }));
    await user.click(screen.getByRole("button", { name: "cta-band" }));

    // The call to action sits just after the band, so it can nest into it.
    await user.click(screen.getByRole("tab", { name: "Layers" }));
    await user.click(
      screen.getByRole("button", { name: "Nest cta-band inside the section above" }),
    );

    await waitFor(() => {
      const ops = vi.mocked(themesApi.applyOps).mock.calls.at(-1)?.[1] ?? [];
      const op = ops[0] as { op: string; blocks: unknown[] } | undefined;
      expect(op?.op).toBe("set_layout");
      const band = (op?.blocks as { kind: string; children?: unknown[] }[]).find(
        (b) => b.kind === "band",
      );
      expect(band?.children).toHaveLength(1);
    });
  });

  it("applies a dark-band scope as role references, not literals", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Insert" }));
    await user.click(screen.getByRole("button", { name: "band" }));

    await user.click(screen.getByRole("button", { name: "Dark band" }));

    await waitFor(() => {
      const ops = vi.mocked(themesApi.applyOps).mock.calls.at(-1)?.[1] ?? [];
      const op = ops[0] as { blocks: { kind: string; scope?: Record<string, unknown> }[] };
      const band = op.blocks.find((b) => b.kind === "band");
      // References rather than hex, so the band tracks the palette.
      expect(band?.scope).toMatchObject({ bg: "$text", text: "$bg" });
    });
  });

  it("edits the repeating items of a marketing section", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Insert" }));
    await user.click(screen.getByRole("button", { name: "feature-grid" }));

    // The array editor is what makes these sections usable by a person and
    // not only by the assistant.
    const items = await screen.findByTestId("items-editor");
    await user.click(within(items).getByRole("button", { name: "Add" }));
    await user.type(await screen.findByLabelText("Item 1 title"), "Fast");

    await waitFor(() => {
      const ops = vi.mocked(themesApi.applyOps).mock.calls.at(-1)?.[1] ?? [];
      const op = ops[0] as { blocks: { kind: string; settings?: Record<string, unknown> }[] };
      const grid = op.blocks.find((b) => b.kind === "feature-grid");
      expect(grid?.settings?.["items"]).toEqual([{ title: "Fast" }]);
    });
  });

  it("applies a template override explicitly, not on every keystroke", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Templates" }));
    const editor = await screen.findByLabelText("single.html source");
    await user.clear(editor);
    await user.type(editor, "<p>mine</p>");
    await new Promise((r) => setTimeout(r, 900));
    expect(vi.mocked(themesApi.applyOps)).not.toHaveBeenCalled();
    await user.click(screen.getByTestId("apply-template"));
    await waitFor(
      () => expect(vi.mocked(themesApi.applyOps)).toHaveBeenCalledTimes(1),
      { timeout: 3000 },
    );
    const ops = vi.mocked(themesApi.applyOps).mock.calls[0]?.[1] ?? [];
    expect(ops).toEqual([{ op: "set_template", name: "single.html", source: "<p>mine</p>" }]);
  });

  it("keeps the edit and shows why when the server rejects it", async () => {
    vi.mocked(themesApi.applyOps).mockRejectedValue(
      new Error("error: colors.primary.light: not a hex colour"),
    );
    const user = userEvent.setup();
    mount();
    const hex = await screen.findByLabelText("Primary light");
    await user.clear(hex);
    await user.type(hex, "#abcdef");
    const alert = await screen.findByRole("alert", {}, { timeout: 3000 });
    expect(alert).toHaveTextContent("colors.primary.light");
    expect(hex).toHaveValue("#abcdef");
  });

  it("publishes as the next version of a slug, optionally going live", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByTestId("publish-button"));
    await screen.findByTestId("publish-dialog");
    const name = screen.getByLabelText("Theme name");
    expect(name).toHaveValue("blog");
    expect(screen.getByText(/Becomes blog v3/)).toBeInTheDocument();
    await user.click(screen.getByLabelText(/Make it live now/));
    await user.click(screen.getByTestId("publish-confirm"));
    await waitFor(() =>
      expect(vi.mocked(themesApi.publishDraft)).toHaveBeenCalledWith("77", {
        name: "blog",
        activate: true,
      }),
    );
  });
});

describe("studio undo", () => {
  it("saves pending edits before undoing, and undoes the newest revision", async () => {
    const order: string[] = [];
    vi.mocked(themesApi.listRevisions).mockResolvedValue([
      { seq: 2, note: "Changed tokens", source: "edit", created_at: "2026-08-29T00:01:00Z" },
    ] as never);
    vi.mocked(themesApi.applyOps).mockImplementation(async () => {
      order.push("save");
      vi.mocked(themesApi.listRevisions).mockResolvedValue([
        { seq: 3, note: "Changed tokens", source: "edit", created_at: "2026-08-29T00:02:00Z" },
      ] as never);
      return { ...structuredClone(draft), revision: 3 };
    });
    vi.mocked(themesApi.revertDraft).mockImplementation(async (_id, seq) => {
      order.push(`revert:${seq}`);
      return { ...structuredClone(draft), revision: 4 };
    });
    const user = userEvent.setup();
    mount();
    const hex = await screen.findByLabelText("Primary light");
    const undo = screen.getByRole("button", { name: /undo/i });
    await waitFor(() => expect(undo).toBeEnabled());
    await user.clear(hex);
    await user.type(hex, "#222222");
    // Straight away, before the autosave delay.
    await user.click(undo);
    await waitFor(() => expect(order).toContain("revert:2"));
    expect(order[0]).toBe("save");
  });
});

describe("studio bindings", () => {
  it("binds a collection to a live source through the picker", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Insert" }));
    await user.click(screen.getByRole("button", { name: "collection" }));

    // The inspector opens on the new section with a real source picker —
    // fed by the vocabulary's live list, plugin types included.
    const source = await screen.findByLabelText("Binding source");
    await user.selectOptions(source, "book");

    await waitFor(() => {
      const ops = vi.mocked(themesApi.applyOps).mock.calls.at(-1)?.[1] ?? [];
      const op = ops[0] as {
        op: string;
        blocks: { kind: string; settings?: { bind?: { source?: string } } }[];
      };
      expect(op.op).toBe("set_layout");
      const grid = op.blocks.find((b) => b.kind === "collection");
      expect(grid?.settings?.bind?.source).toBe("book");
    });
  });

  it("lists every source in the Data panel, plugins marked", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Data" }));
    const panel = screen.getByTestId("data-panel");
    expect(within(panel).getByText("Posts")).toBeInTheDocument();
    expect(within(panel).getByText("Books")).toBeInTheDocument();
    expect(within(panel).getByText("Plugin")).toBeInTheDocument();
  });
});

describe("studio assistant", () => {
  it("sends a request and shows the run in progress", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Assistant" }));
    const input = await screen.findByTestId("assistant-input");
    await user.type(input, "Warmer palette please");
    await user.click(screen.getByTestId("assistant-send"));
    await waitFor(() =>
      expect(vi.mocked(themesApi.chat)).toHaveBeenCalledWith("77", "Warmer palette please", []),
    );
    // Nothing goes through the manual save path.
    expect(vi.mocked(themesApi.applyOps)).not.toHaveBeenCalled();
  });

  it("explains itself when no text model is registered", async () => {
    aiModels.mockResolvedValue({ encrypting_keys: false, providers: [], kinds: [], models: [] });
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Assistant" }));
    expect(await screen.findByTestId("assistant-off")).toHaveTextContent("needs a text model");
    expect(screen.queryByTestId("assistant-input")).toBeNull();
  });
});

describe("studio helpers", () => {
  it("diffOps sends only the sections that changed", () => {
    const noAssets = { css: "", js: "" };
    const synced = {
      tokens,
      layout: draft.layout,
      templates: { "single.html": "a" },
      assets: noAssets,
    };
    const working = {
      tokens: { ...tokens, radius_px: 12 },
      layout: { ...draft.layout, page: [{ id: "x", kind: "content" }] },
      templates: { "index.html": "b" },
      assets: { css: ".vy-card{}", js: "" },
    };
    const ops = diffOps(synced, working);
    expect(ops.map((o) => o.op)).toEqual([
      "set_tokens",
      "set_layout",
      "remove_template",
      "set_template",
      "set_assets",
    ]);
    expect(diffOps(synced, synced)).toEqual([]);
    // Assets on their own are one op, not a whole-document replacement.
    expect(
      diffOps(synced, { ...synced, assets: { css: "a{}", js: "" } }),
    ).toEqual([{ op: "set_assets", css: "a{}", js: "" }]);
  });

  it("undo walks back through reverts instead of bouncing", () => {
    expect(undoTarget(undefined)).toBeNull();
    expect(undoTarget({ seq: 1, source: "start", note: "" })).toBeNull();
    expect(undoTarget({ seq: 5, source: "you", note: "Changed x" })).toBe(4);
    expect(undoTarget({ seq: 6, source: "revert", note: "Went back to revision 4" })).toBe(3);
    expect(undoTarget({ seq: 7, source: "revert", note: "Went back to revision 1" })).toBeNull();
  });

  it("offers what the assistant would change rather than having done it", async () => {
    // It used to commit as it answered: a change nobody wanted still landed
    // on the draft and had to be walked back.
    const user = userEvent.setup();
    vi.mocked(themesApi.listMessages).mockResolvedValue([
      {
        id: "m1",
        role: "you",
        text: "warmer palette",
        revision: null,
        proposal: null,
        created_at: "2026-08-29T00:00:00Z",
      },
      {
        id: "m2",
        role: "assistant",
        text: "Warmed the accent.",
        revision: null,
        proposal: { changes: ["tokens: primary"], base: 1 },
        created_at: "2026-08-29T00:00:01Z",
      },
    ]);
    vi.mocked(themesApi.acceptProposal).mockResolvedValue({
      ...structuredClone(draft),
      revision: 2,
    });
    atWidth(1600);
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Assistant" }));

    const proposal = await screen.findByTestId("proposal");
    expect(within(proposal).getByText("tokens: primary")).toBeInTheDocument();
    // Nothing has been written yet.
    expect(vi.mocked(themesApi.acceptProposal)).not.toHaveBeenCalled();

    await user.click(within(proposal).getByRole("button", { name: /apply/i }));
    await waitFor(() =>
      expect(vi.mocked(themesApi.acceptProposal)).toHaveBeenCalledWith("77", "m2"),
    );
  });

  it("will not apply a proposal the draft has moved past", async () => {
    // Accepting it would discard whatever was done in between.
    vi.mocked(themesApi.listMessages).mockResolvedValue([
      {
        id: "m2",
        role: "assistant",
        text: "Warmed the accent.",
        revision: null,
        proposal: { changes: ["tokens: primary"], base: 0 },
        created_at: "2026-08-29T00:00:01Z",
      },
    ]);
    atWidth(1600);
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    await user.click(screen.getByRole("tab", { name: "Assistant" }));

    const proposal = await screen.findByTestId("proposal");
    expect(within(proposal).queryByRole("button", { name: /apply/i })).toBeNull();
    expect(within(proposal).getByText(/draft has changed since/i)).toBeInTheDocument();
  });

  it("is the same studio at every width", async () => {
    // There is no second layout. Gating the designer on window width meant
    // shipping it where most screens never saw it, and left "I still see
    // the old one" with no answer.
    for (const width of [1600, 1280, 1000, 700]) {
      atWidth(width);
      const view = mount();
      expect(await screen.findByTestId("pane-rail")).toBeInTheDocument();
      const inspector = screen.getByTestId("inspector");
      // With nothing selected the inspector holds the theme's own
      // properties — the tokens — with no tabs left to hide them behind.
      expect(within(inspector).queryAllByRole("tab")).toHaveLength(0);
      expect(within(inspector).getByLabelText("Primary light")).toBeInTheDocument();
      // Every tool hangs on the rail, one icon each.
      expect(
        within(screen.getByTestId("studio-rail"))
          .getAllByRole("tab")
          .map((t) => t.getAttribute("aria-label")),
      ).toEqual(["Insert", "Layers", "Assistant", "Data", "Templates", "Assets"]);
      expect(screen.getByTestId("preview-frame")).toBeInTheDocument();
      view.unmount();
    }
  });

  it("switches between usable mobile panes while retaining the wide layout", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByTestId("pane-rail");
    expect(screen.getByTestId("pane-rail")).toHaveClass("hidden");
    await user.click(screen.getByRole("tab", { name: "tools" }));
    expect(screen.getByTestId("pane-rail")).not.toHaveClass("hidden");
    await user.click(screen.getByRole("tab", { name: "properties" }));
    expect(screen.getByTestId("inspector")).not.toHaveClass("hidden");
    expect(screen.getByTestId("pane-rail")).toHaveClass("hidden");
    expect(screen.getByTestId("studio-panes").className).toContain("xl:grid-cols-");
  });

  it("the canvas and the layout tree share one selection", async () => {
    // The canvas is the other way into the same state: clicking a section
    // on the page opens it in the tree, and opening it in the tree outlines
    // it on the page. Testing it from the tree side proves the wiring
    // without needing an iframe jsdom will not lay out.
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");

    expect(screen.getByText(/click a section to edit it/i)).toBeInTheDocument();

    await user.click(screen.getByRole("tab", { name: "Layers" }));
    const list = screen.getByLabelText("Home sections");
    await user.click(within(list).getByRole("button", { name: /latest-posts/ }));

    expect(screen.getByTestId("canvas-selection")).toHaveTextContent("posts");

    await user.click(screen.getByRole("button", { name: "Clear selection" }));
    expect(screen.queryByTestId("canvas-selection")).toBeNull();
  });

  it("the inspector follows the selection, and lets go of it", async () => {
    // The point of the rearrangement: with nothing selected the inspector
    // is the theme's own properties, and with a section selected it is that
    // section's. Two panels showing settings for different things at once
    // was the thing that made the old studio feel like a form, not a tool.
    const user = userEvent.setup();
    mount();
    await screen.findByLabelText("Primary light");
    const inspector = screen.getByTestId("inspector");
    expect(within(inspector).getByLabelText("Primary light")).toBeInTheDocument();
    expect(within(inspector).queryByTestId("section-properties")).toBeNull();

    await user.click(screen.getByRole("tab", { name: "Layers" }));
    const list = screen.getByLabelText("Home sections");
    await user.click(within(list).getByRole("button", { name: /latest-posts/ }));

    expect(within(inspector).getByTestId("section-properties")).toBeInTheDocument();
    // The tokens step aside while a section owns the panel.
    expect(within(inspector).queryByLabelText("Primary light")).toBeNull();
    // And the properties are not also duplicated in the rail.
    expect(within(list).queryByTestId("section-properties")).toBeNull();

    await user.click(screen.getByRole("button", { name: "Close properties" }));
    expect(within(inspector).getByLabelText("Primary light")).toBeInTheDocument();
  });

  it("ids and names are derived safely", () => {
    expect(uniqueId("latest-posts", [])).toBe("latest-posts");
    expect(uniqueId("latest-posts", ["latest-posts", "latest-posts-2"])).toBe("latest-posts-3");
    expect(slugify("Blog (draft)")).toBe("blog-draft");
    expect(slugify("  My Café!! ")).toBe("my-caf");
  });
});


describe("studio persistence", () => {
  it("flushes an edit when exiting before the autosave debounce", async () => {
    const user = userEvent.setup();
    vi.mocked(themesApi.getDraft).mockResolvedValue({ ...structuredClone(draft), assets: { css: "/* legacy CSS */" } as Draft["assets"] });
    const exit = vi.fn(); mount(exit);
    await user.click(await screen.findByRole("tab", { name: "Assets" }));
    const css = screen.getByRole("textbox", { name: "Stylesheet" });
    await user.click(css); await user.paste("body { color: red; }");
    await user.click(screen.getByRole("button", { name: "Appearance" }));
    await waitFor(() => expect(exit).toHaveBeenCalledOnce());
    expect(themesApi.applyOps).toHaveBeenCalledWith("77", expect.arrayContaining([expect.objectContaining({ op: "set_assets", css: expect.stringContaining("color: red"), js: "" })]));
  });
});
