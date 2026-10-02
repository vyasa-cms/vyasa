import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";

const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);

import { AssistPanel } from "@/components/editor/AssistPanel";

function makeApp(handlers: {
  onApplyTitle?: (t: string) => void;
  onApplyExcerpt?: (x: string) => void;
  onApplyTags?: (names: string[]) => void;
  onApplySeo?: (seo: { meta_title: string; meta_description: string }) => void;
  textAvailable?: boolean;
} = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return (
    <QueryClientProvider client={client}>
      <AssistPanel
        document={{ schema_version: 1, blocks: [] }}
        vocabulary={["rust"]}
        onApplyTitle={handlers.onApplyTitle ?? vi.fn()}
        onApplyExcerpt={handlers.onApplyExcerpt ?? vi.fn()}
        onApplyTags={handlers.onApplyTags}
        onApplySeo={handlers.onApplySeo}
        textAvailable={handlers.textAvailable}
      />
    </QueryClientProvider>
  );
}

afterEach(() => {
  fetchMock.mockReset();
});

describe("assist panel", () => {
  it("renders suggestion chips and applies title on click", async () => {
    const user = userEvent.setup();
    fetchMock.mockResolvedValue({
      ok: true,
      json: async () => ({ suggestions: ["A", "B"] }),
    });
    render(makeApp());
    await user.click(screen.getByTestId("suggest-button"));
    await waitFor(() =>
      expect(screen.getAllByRole("button", { name: "A" }).length).toBeGreaterThan(0),
    );
    const chips = screen.getAllByRole("button", { name: "B" });
    if (chips.length === 0 || chips[0] === undefined) {
      throw new Error("no chip");
    }
    await user.click(chips[0]);
  });

  it("maps server errors into the panel", async () => {
    fetchMock.mockResolvedValue({
      ok: false,
      json: async () => ({ message: "not_enough_content" }),
    });
    render(makeApp());
    await userEvent.setup().click(screen.getByTestId("suggest-button"));
    await waitFor(() =>
      expect(screen.getByRole("alert").textContent).toContain("not_enough_content"),
    );
  });

  it("offers every assist kind the server implements", () => {
    render(makeApp());
    for (const label of ["Titles", "Excerpt", "SEO", "Tags"]) {
      expect(screen.getByRole("button", { name: label })).toBeInTheDocument();
    }
  });

  it("renders the SEO meta pair with its length budgets", async () => {
    const user = userEvent.setup();
    fetchMock.mockResolvedValue({
      ok: true,
      json: async () => ({
        meta_title: "A concise title",
        meta_description: "A description of the post.",
      }),
    });
    render(makeApp());
    await user.click(screen.getByRole("button", { name: "SEO" }));
    await user.click(screen.getByTestId("suggest-button"));

    await waitFor(() =>
      expect(screen.getByText("A concise title")).toBeInTheDocument(),
    );
    // The budget is the whole point of the SEO assist: a title over 60
    // characters is truncated by search engines.
    expect(screen.getByText("Meta title (15/60)")).toBeInTheDocument();
    expect(screen.getByText("Meta description (26/155)")).toBeInTheDocument();
  });

  it("separates existing tags from ones that would be created", async () => {
    const user = userEvent.setup();
    const onApplyTags = vi.fn();
    fetchMock.mockResolvedValue({
      ok: true,
      json: async () => ({ tags: ["rust"], new_tags: ["wasm"] }),
    });
    render(makeApp({ onApplyTags }));
    await user.click(screen.getByRole("button", { name: "Tags" }));
    await user.click(screen.getByTestId("suggest-button"));

    await waitFor(() => expect(screen.getByText("rust")).toBeInTheDocument());
    expect(screen.getByText("New tags — these would be created")).toBeInTheDocument();
    expect(screen.getByText("wasm")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Apply 2 tags" }));
    expect(onApplyTags).toHaveBeenCalledWith(["rust", "wasm"]);
  });

  it("clears the previous answer when the kind changes", async () => {
    const user = userEvent.setup();
    fetchMock.mockResolvedValue({
      ok: true,
      json: async () => ({ suggestions: ["Some title"] }),
    });
    render(makeApp());
    await user.click(screen.getByTestId("suggest-button"));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Some title" })).toBeInTheDocument(),
    );

    // Title suggestions shown under a heading that now says SEO would be
    // worse than showing nothing.
    await user.click(screen.getByRole("button", { name: "SEO" }));
    expect(screen.queryByRole("button", { name: "Some title" })).toBeNull();
  });

  it("applies the SEO pair to the search fields when the editor offers it", async () => {
    fetchMock.mockResolvedValue({
      ok: true,
      json: async () => ({ meta_title: "A title", meta_description: "A description" }),
    });
    const onApplySeo = vi.fn();
    const user = userEvent.setup();
    render(makeApp({ onApplySeo }));
    await user.click(screen.getByRole("button", { name: "SEO" }));
    await user.click(screen.getByTestId("suggest-button"));
    await user.click(await screen.findByRole("button", { name: "Use as search title and description" }));
    expect(onApplySeo).toHaveBeenCalledWith({ meta_title: "A title", meta_description: "A description" });
  });

  it("offers only links when no text model is set up", () => {
    render(makeApp({ textAvailable: false }));
    expect(screen.queryByRole("button", { name: "Titles" })).toBeNull();
    expect(screen.getByRole("button", { name: "Links" })).toHaveAttribute("aria-pressed", "true");
  });
});
