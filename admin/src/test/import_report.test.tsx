import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";
import { ExportImportPanel } from "@/components/ExportImportPanel";

vi.mock("@/components/ui/toast", () => ({
  notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), undo: vi.fn() },
}));

beforeEach(() => {
  vi.restoreAllMocks();
});

describe("import report", () => {
  it("lists every warning, including field values the import dropped, and the content types it made", async () => {
    const user = userEvent.setup();
    const warnings = [
      "post \"lamp\": fields.photo dropped: media files are not imported, so its media id names nothing here",
      "post \"lamp\": fields.maker dropped: entry 5 was not in the archive",
      "comment 3 skipped",
      "menu \"main\": item 2 skipped",
      "post \"x\": raw meta.fields removed",
    ];
    vi.spyOn(api, "importArchive").mockResolvedValue({
      users: 0, terms: 0, posts: 2, comments: 0, menus: 0, options: 0, skipped: 1, roles: 0,
      content_types: 1, content_fields: 3, warnings,
    } as never);
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><ExportImportPanel /></QueryClientProvider>);
    await user.upload(screen.getByLabelText("archive file"), new File(["{}"], "site.json", { type: "application/json" }));
    await user.click(screen.getByRole("button", { name: "Import" }));

    const report = await screen.findByTestId("import-report");
    expect(report).toHaveTextContent("1 content types");
    expect(report).toHaveTextContent("3 fields");
    const list = within(report).getByRole("list");
    expect(within(list).getAllByRole("listitem")).toHaveLength(5);
    expect(list).toHaveTextContent("fields.photo dropped");
    expect(list).toHaveTextContent("fields.maker dropped");
  });
});
