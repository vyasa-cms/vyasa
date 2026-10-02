import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as themesApi from "@/api/themes";

const { colors } = vi.hoisted(() => ({
  colors: {
    bg: { light: "#ffffff" },
    surface: { light: "#ffffff" },
    text: { light: "#111111" },
    text_muted: { light: "#555555" },
    border: { light: "#dddddd" },
    primary: { light: "#b3541e" },
    on_primary: { light: "#ffffff" },
  },
}));

vi.mock("@/api/themes", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  listThemes: vi.fn().mockResolvedValue([
    { id: "1", name: "blog", version: 2, is_active: true },
    { id: "3", name: "blog", version: 1, is_active: false },
    { id: "2", name: "portfolio", version: 1, is_active: false },
  ]),
  listDrafts: vi.fn().mockResolvedValue([
    {
      id: "77",
      name: "blog (draft)",
      base_theme_id: "1",
      status: "ready",
      status_note: null,
      revision: 3,
      updated_at: "2026-08-29T00:00:00Z",
      colors,
    },
  ]),
  themeTokens: vi.fn().mockResolvedValue({ tokens: { colors } }),
  createDraft: vi.fn().mockResolvedValue({ id: "99", name: "blog (draft)" }),
  activate: vi.fn().mockResolvedValue(undefined),
  deleteDraft: vi.fn().mockResolvedValue(undefined),
  deleteTheme: vi.fn().mockResolvedValue(undefined),
  rollbackTheme: vi.fn().mockResolvedValue({ id: "3", name: "blog", version: 1, is_active: true }),
  installTheme: vi.fn(),
  listThemeFiles: vi.fn().mockResolvedValue([
    { path: "images/hero.svg", content_type: "image/svg+xml", sha256: "ab", size: 1280,
      url: "/theme-assets/images/hero.svg" },
  ]),
  putThemeFile: vi.fn().mockResolvedValue({ path: "images/new.png", content_type: "image/png",
    sha256: "cd", size: 10, url: "/theme-assets/images/new.png" }),
  deleteThemeFile: vi.fn().mockResolvedValue(undefined),
  chat: vi.fn().mockResolvedValue({ message_id: "m1", status: "generating" }),
}));

const { aiModels } = vi.hoisted(() => ({ aiModels: vi.fn() }));
vi.mock("@/api/client", async (importOriginal) => {
  const mod = await importOriginal<{ api: Record<string, unknown> }>();
  return { ...mod, api: { ...mod.api, aiModels } };
});

import { ConfirmProvider } from "@/components/ui/dialog";
import { AppearancePage } from "@/routes/_auth/appearance";

function makeApp(openStudio = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <ConfirmProvider>
        <AppearancePage openStudio={openStudio} />
      </ConfirmProvider>
    </QueryClientProvider>
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  aiModels.mockResolvedValue({ encrypting_keys: false, providers: [], kinds: [], models: [] });
});

describe("appearance", () => {
  it("opens a draft in the studio", async () => {
    const openStudio = vi.fn();
    const user = userEvent.setup();
    render(makeApp(openStudio));
    await user.click(await screen.findByTestId("open-77"));
    expect(openStudio).toHaveBeenCalledWith("77");
  });

  it("starts a draft from the live theme by default", async () => {
    const openStudio = vi.fn();
    const user = userEvent.setup();
    render(makeApp(openStudio));
    await screen.findByTestId("active-blog");
    await user.click(screen.getByTestId("new-draft"));
    await screen.findByTestId("new-draft-dialog");
    await user.click(screen.getByTestId("create-draft"));
    await waitFor(() =>
      expect(vi.mocked(themesApi.createDraft)).toHaveBeenCalledWith({ base_theme_id: "1" }),
    );
    expect(openStudio).toHaveBeenCalledWith("99");
  });

  it("edits an installed theme by starting a draft from that version", async () => {
    const user = userEvent.setup();
    render(makeApp());
    await user.click(
      await screen.findByRole("button", { name: "Edit portfolio v1 in the studio" }),
    );
    await waitFor(() =>
      expect(vi.mocked(themesApi.createDraft)).toHaveBeenCalledWith({ base_theme_id: "2" }),
    );
  });

  it("offers a roll back only when an earlier version of the live theme exists", async () => {
    const user = userEvent.setup();
    render(makeApp());
    await user.click(await screen.findByRole("button", { name: /Roll back blog/ }));
    await waitFor(() => expect(vi.mocked(themesApi.rollbackTheme)).toHaveBeenCalledWith("blog"));
  });

  // Activating changes what every visitor sees, so it is confirmed rather
  // than firing on the click.
  it("activate asks for confirmation before switching the live theme", async () => {
    const mocked = vi.mocked(themesApi.activate);
    const user = userEvent.setup();
    render(makeApp());
    const buttons = await screen.findAllByRole("button", { name: "Activate" });
    if (buttons[0] === undefined) throw new Error("no activate buttons");
    await user.click(buttons[0]);
    expect(mocked).not.toHaveBeenCalled();
    await screen.findByTestId("confirm-dialog");
    await user.click(screen.getByTestId("confirm-accept"));
    await waitFor(() => expect(mocked).toHaveBeenCalled());
  });

  it("offers to describe a theme only when a text model is registered", async () => {
    render(makeApp());
    await screen.findByTestId("active-blog");
    expect(screen.queryByTestId("theme-builder")).toBeNull();
  });

  it("describing a theme starts a draft, asks the assistant, and opens the studio on it", async () => {
    aiModels.mockResolvedValue({
      encrypting_keys: false,
      providers: [],
      kinds: [],
      models: [{ kind: "text", is_default: true, enabled: true }],
    });
    const openStudio = vi.fn();
    const user = userEvent.setup();
    render(makeApp(openStudio));
    const box = await screen.findByLabelText("site description");
    await user.type(box, "A warm bakery journal");
    await user.click(screen.getByTestId("generate-button"));
    // No second argument: the assistant is a pane of the studio, always on
    // screen, so there is nothing to point it at.
    await waitFor(() => expect(openStudio).toHaveBeenCalledWith("99"));
    expect(vi.mocked(themesApi.createDraft)).toHaveBeenCalledWith({ name: "New design" });
    expect(vi.mocked(themesApi.chat)).toHaveBeenCalledWith("99", "A warm bakery journal");
  });

  it("the live theme cannot be removed from the list", async () => {
    render(makeApp());
    await screen.findByTestId("active-blog");
    expect(screen.queryByRole("button", { name: "Remove blog v2" })).toBeNull();
    expect(screen.getByRole("button", { name: "Remove portfolio v1" })).toBeInTheDocument();
  });
});


describe("bundled files", () => {
  it("lists a version's files and removes one on confirmation", async () => {
    const user = userEvent.setup();
    render(makeApp());
    await user.click(await screen.findByTestId("files-2"));
    const dialog = await screen.findByTestId("theme-files-dialog");
    expect(dialog).toHaveTextContent("Files in portfolio v1");
    expect(await screen.findByText("images/hero.svg")).toBeInTheDocument();
    expect(themesApi.listThemeFiles).toHaveBeenCalledWith("2");

    await user.click(screen.getByRole("button", { name: "Remove images/hero.svg" }));
    await user.click(await screen.findByRole("button", { name: "Remove" }));
    await waitFor(() =>
      expect(themesApi.deleteThemeFile).toHaveBeenCalledWith("2", "images/hero.svg"),
    );
  });

  it("derives a safe bundled path from a chosen file's name", () => {
    expect(themesApi.bundledPathFor("images", "Hero Shot (final).PNG")).toBe(
      "images/hero-shot-final-.png",
    );
    expect(themesApi.bundledPathFor("fonts", "..Fraunces.woff2")).toBe("fonts/fraunces.woff2");
  });
});
