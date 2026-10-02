import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import * as pluginsApi from "@/api/plugins";
import { PluginSurfacePanels, formatInterval } from "@/components/plugins/surface";

vi.mock("@/api/plugins", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  getPluginSurface: vi.fn(),
  getPluginSettings: vi.fn(),
  putPluginSetting: vi.fn(),
}));

const mocked = vi.mocked(pluginsApi);

const surface: pluginsApi.PluginSurface = {
  blocks: [
    {
      kind: "bookshelf/rating",
      title: "Book rating",
      icon: "★",
      pluginId: "1",
      pluginName: "bookshelf",
    },
  ],
  routes: [
    { method: "GET", path: "/api/v1/plugin/bookshelf/stats", pluginId: "1" },
  ],
  forms: [
    {
      pluginId: "1",
      pluginName: "bookshelf",
      title: "Bookshelf",
      description: "How book ratings are drawn.",
      fields: [
        {
          key: "star",
          label: "Star glyph",
          kind: "select",
          help: "Used by the Book rating block.",
          options: ["★", "●"],
          default: "★",
        },
      ],
    },
  ],
  postTypes: [
    {
      slug: "book",
      singular: "Book",
      plural: "Books",
      public: true,
      hasArchive: true,
      pluginId: "1",
    },
  ],
  taxonomies: [
    {
      slug: "shelf",
      singular: "Shelf",
      plural: "Shelves",
      hierarchical: false,
      public: true,
      pluginId: "1",
    },
  ],
  tasks: [
    {
      pluginId: "1",
      name: "tally",
      everySeconds: 300,
      lastRunAt: null,
      lastStatus: "pending",
      lastError: null,
    },
  ],
  filterPoints: ["post-title", "excerpt"],
  eventNames: ["post-saved", "user-created"],
};

function makeApp() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return (
    <QueryClientProvider client={client}>
      <PluginSurfacePanels />
    </QueryClientProvider>
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.getPluginSurface.mockResolvedValue(surface);
  mocked.getPluginSettings.mockResolvedValue({});
  mocked.putPluginSetting.mockResolvedValue(undefined);
});

describe("plugin surface", () => {
  it("shows what plugins contribute", async () => {
    render(makeApp());
    expect(await screen.findByTestId("plugin-surface")).toBeInTheDocument();
    // Chips interleave label and identifier, so match on the identifiers
    // and on the panel's own text rather than on a single node.
    expect(screen.getByText("bookshelf/rating")).toBeInTheDocument();
    expect(screen.getByText("/book")).toBeInTheDocument();
    const panel = screen.getByTestId("plugin-surface");
    expect(panel.textContent).toContain("Book rating");
    expect(panel.textContent).toContain("Books");
    expect(panel.textContent).toContain("Shelves");
    expect(panel.textContent).toContain("GET /api/v1/plugin/bookshelf/stats");
    expect(screen.getByText(/every 5 minutes/)).toBeInTheDocument();
    expect(screen.getByText(/not run yet/)).toBeInTheDocument();
  });

  it("renders a declared settings form and saves what was chosen", async () => {
    const user = userEvent.setup();
    render(makeApp());
    const select = await screen.findByLabelText("Star glyph");
    // Saving is only offered once something changed.
    expect(screen.getByTestId("plugin-form-save-bookshelf")).toBeDisabled();

    await user.selectOptions(select, "●");
    await user.click(screen.getByTestId("plugin-form-save-bookshelf"));
    await waitFor(() =>
      expect(mocked.putPluginSetting).toHaveBeenCalledWith("1", "star", "●"),
    );
  });

  it("hides itself entirely when no plugin contributes anything", async () => {
    mocked.getPluginSurface.mockResolvedValue({
      blocks: [],
      routes: [],
      forms: [],
      postTypes: [],
      taxonomies: [],
      tasks: [],
      filterPoints: [],
      eventNames: [],
    });
    render(makeApp());
    await waitFor(() => expect(mocked.getPluginSurface).toHaveBeenCalled());
    expect(screen.queryByTestId("plugin-surface")).not.toBeInTheDocument();
  });

  it("describes intervals the way a person would say them", () => {
    expect(formatInterval(60)).toBe("minute");
    expect(formatInterval(300)).toBe("5 minutes");
    expect(formatInterval(3600)).toBe("hour");
    expect(formatInterval(86_400)).toBe("24 hours");
    expect(formatInterval(90)).toBe("90 seconds");
  });
});
