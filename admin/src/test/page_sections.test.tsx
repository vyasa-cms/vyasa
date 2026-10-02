import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import * as themesApi from "@/api/themes";
import type { Section, ThemeTokens, Vocabulary } from "@/api/themes";

vi.mock("@/api/themes", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  vocabulary: vi.fn(),
  listThemes: vi.fn(),
  themeTokens: vi.fn(),
}));

const { composePage, renderPage } = vi.hoisted(() => ({
  composePage: vi.fn(),
  renderPage: vi.fn(),
}));
vi.mock("@/api/client", async (importOriginal) => {
  const mod = await importOriginal<{ api: Record<string, unknown> }>();
  return { ...mod, api: { ...mod.api, composePage, renderPage } };
});

import { PageSections } from "@/components/editor/PageSections";

const vocab: Vocabulary = {
  static_regions: [
    { kind: "content", description: "The words written above" },
    { kind: "header", description: "Site name and tagline" },
  ],
  blocks: [
    {
      kind: "band",
      description: "Full-width band",
      container: true,
      settings_schema: { type: "object", properties: {} },
    },
    {
      kind: "hero",
      description: "Opening statement",
      container: false,
      settings_schema: {
        type: "object",
        properties: { headline: { type: "string" } },
      },
    },
    {
      kind: "cta-band",
      description: "Closing call to action",
      container: false,
      settings_schema: { type: "object", properties: {} },
    },
  ],
  templates: ["index", "single", "archive", "page", "search", "not-found"],
  template_files: [],
  token_schema: {},
};

const tokens: ThemeTokens["tokens"] = {
  version: 1,
  colors: {
    bg: { light: "#ffffff", dark: "#141416" },
    surface: { light: "#fafafa", dark: "#1d1d20" },
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

// `null` means "never saved"; passing `undefined` would fall back to the
// default and quietly test the saved case instead.
function mount(sections: Section[], onChange = vi.fn(), postId: string | null = "12") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <PageSections
        sections={sections}
        onChange={onChange}
        postId={postId ?? undefined}
        document={{ schema_version: 1, blocks: [] }}
      />
    </QueryClientProvider>,
  );
  return onChange;
}

beforeEach(() => {
  vi.clearAllMocks();
  vi.mocked(themesApi.vocabulary).mockResolvedValue(vocab);
  renderPage.mockResolvedValue("<!doctype html><title>p</title><h1>Hi</h1>");
  vi.mocked(themesApi.listThemes).mockResolvedValue([
    { id: "1", name: "blog", version: 2, is_active: true },
  ]);
  vi.mocked(themesApi.themeTokens).mockResolvedValue({
    id: "1",
    name: "blog",
    version: 2,
    is_active: true,
    tokens,
    layout: {
      index: [],
      single: [],
      archive: [],
      page: [],
      search: [],
      "not-found": [],
    },
  });
});

describe("page sections", () => {
  it("says an empty page still renders, rather than looking broken", async () => {
    mount([]);
    expect(
      await screen.findByText(/renders through the theme's page template/i),
    ).toBeInTheDocument();
  });

  it("tells the author how to place the writing they already did", async () => {
    // Without this the prose silently vanishes from a composed page, which
    // is the one failure an author cannot debug from the editor.
    mount([]);
    const hint = await screen.findByText(/Add a/i);
    expect(hint.textContent).toContain("content");
    expect(hint.textContent).toMatch(/header and footer still come/i);
  });

  it("adds a section with an id that is free anywhere in the tree", async () => {
    const user = userEvent.setup();
    const onChange = mount([
      { id: "hero", kind: "hero" },
      { id: "closing", kind: "band", children: [{ id: "hero-2", kind: "cta-band" }] },
    ]);
    await user.click(await screen.findByRole("button", { name: /add section/i }));
    await user.click(screen.getByRole("button", { name: /^hero Opening statement$/ }));

    await waitFor(() => expect(onChange).toHaveBeenCalledTimes(1));
    const next = onChange.mock.calls[0]?.[0] as Section[];
    // `hero` and `hero-2` are both taken, and the nested one counts.
    expect(next.map((s) => s.id)).toContain("hero-3");
  });

  it("edits a section's settings from its schema", async () => {
    const user = userEvent.setup();
    const onChange = mount([{ id: "lead", kind: "hero" }]);
    // The row's own toggle, not one of its move/remove buttons.
    await user.click(
      await screen.findByRole("button", { expanded: false, name: /lead/ }),
    );
    const field = await screen.findByLabelText(/headline/i);
    await user.type(field, "Hi");

    const last = onChange.mock.calls.at(-1)?.[0] as Section[];
    expect(last[0]?.settings).toMatchObject({ headline: expect.any(String) });
  });

  it("does not offer chrome the renderer would skip", async () => {
    const user = userEvent.setup();
    mount([]);
    await user.click(await screen.findByRole("button", { name: /add section/i }));
    // A `header` here would be dropped so the page does not end up with two;
    // offering it would let an author add a section that does nothing.
    expect(screen.queryByRole("button", { name: /^header/ })).toBeNull();
    expect(screen.getByRole("button", { name: /^content/ })).toBeInTheDocument();
  });

  it("will not compose without a theme to compose against", async () => {
    vi.mocked(themesApi.listThemes).mockResolvedValue([]);
    mount([]);
    expect(await screen.findByText(/Activate a theme/i)).toBeInTheDocument();
  });

  it("asks the designer for a page and drops the answer into the tree", async () => {
    const user = userEvent.setup();
    composePage.mockResolvedValue({
      reply: "A hero, your writing, then a dark call to action.",
      sections: [
        { id: "lead", kind: "hero" },
        { id: "words", kind: "content" },
      ],
      warnings: [],
    });
    const onChange = mount([]);

    await user.type(
      await screen.findByLabelText(/what should this page do/i),
      "landing page for launch week",
    );
    await user.click(screen.getByRole("button", { name: "Compose" }));

    await waitFor(() => expect(composePage).toHaveBeenCalledTimes(1));
    const [id, body] = composePage.mock.calls[0] ?? [];
    expect(id).toBe("12");
    expect(body).toMatchObject({ message: "landing page for launch week" });
    // The unsaved document goes with it, so the designer reads what the
    // author sees rather than the last save.
    expect(body).toHaveProperty("content");

    await waitFor(() => expect(onChange).toHaveBeenCalled());
    expect(onChange.mock.calls.at(-1)?.[0]).toEqual([
      { id: "lead", kind: "hero" },
      { id: "words", kind: "content" },
    ]);
    expect(
      await screen.findByText(/A hero, your writing, then a dark call to action./),
    ).toBeInTheDocument();
  });

  it("can undo a proposal, so it never eats hand-built work", async () => {
    const user = userEvent.setup();
    composePage.mockResolvedValue({
      reply: "Done.",
      sections: [{ id: "lead", kind: "hero" }],
      warnings: [],
    });
    const mine: Section[] = [{ id: "mine", kind: "cta-band" }];
    const onChange = mount(mine);

    await user.type(await screen.findByLabelText(/what should this page do/i), "redo it");
    await user.click(screen.getByRole("button", { name: "Compose" }));
    await waitFor(() => expect(onChange).toHaveBeenCalled());

    await user.click(await screen.findByRole("button", { name: /undo/i }));
    expect(onChange.mock.calls.at(-1)?.[0]).toEqual(mine);
  });

  it("shows what is valid but still worth knowing", async () => {
    const user = userEvent.setup();
    composePage.mockResolvedValue({
      reply: "Composed.",
      sections: [{ id: "cta", kind: "cta-band" }],
      warnings: [
        {
          level: "warning",
          path: "sections[0].scope.text-muted",
          message: "text-muted on bg is 2.1:1 in light — below the 4.5:1 needed for body text",
        },
      ],
    });
    mount([]);
    await user.type(await screen.findByLabelText(/what should this page do/i), "dark band");
    await user.click(screen.getByRole("button", { name: "Compose" }));
    // A tree can be perfectly valid and still unreadable; the author sees
    // that here rather than on the published page.
    expect(await screen.findByText(/below the 4.5:1/)).toBeInTheDocument();
  });

  it("renders the unsaved page beside the tree", async () => {
    // Composing without this is blind: the tree says "hero", and whether it
    // is the right hero is a question only the rendered page answers.
    mount([{ id: "lead", kind: "hero" }]);
    expect(await screen.findByTestId("section-preview")).toBeInTheDocument();
    await waitFor(() => expect(renderPage).toHaveBeenCalled());
    const [id, body] = renderPage.mock.calls[0] ?? [];
    expect(id).toBe("12");
    expect(body).toMatchObject({ sections: [{ id: "lead", kind: "hero" }] });
    // The editor's unsaved document goes with it.
    expect(body).toHaveProperty("content");
  });

  it("offers the theme's own breakpoints, not fixed widths", async () => {
    // A theme whose phone breakpoint is 900px would otherwise show tablet
    // rules under a "Phone" label.
    mount([{ id: "lead", kind: "hero" }]);
    expect(await screen.findByLabelText("Desktop")).toBeInTheDocument();
    // sm 640 → phone 390; midpoint of 640 and 900 → tablet 770.
    expect(screen.getByLabelText("Phone — 390px")).toBeInTheDocument();
    expect(screen.getByLabelText("Tablet — 770px")).toBeInTheDocument();
  });

  it("says why when the page cannot be rendered", async () => {
    renderPage.mockRejectedValue(new Error("no theme activated"));
    mount([{ id: "lead", kind: "hero" }]);
    expect(await screen.findByText(/no theme activated/)).toBeInTheDocument();
  });

  it("says what to do first when the page has never been saved", async () => {
    mount([], vi.fn(), null);
    expect(await screen.findByText(/Save the page once/i)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Compose" })).toBeNull();
    // Nothing to render against either.
    expect(screen.queryByTestId("section-preview")).toBeNull();
  });

  it("surfaces a refusal rather than silently doing nothing", async () => {
    const user = userEvent.setup();
    composePage.mockRejectedValue(new Error("activate a theme before composing a page"));
    mount([]);
    await user.type(await screen.findByLabelText(/what should this page do/i), "go");
    await user.click(screen.getByRole("button", { name: "Compose" }));
    expect(await screen.findByText(/activate a theme/i)).toBeInTheDocument();
  });
});
