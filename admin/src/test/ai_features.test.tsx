import { TestRouter } from "./TestRouter";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { SettingsPage } from "@/routes/_auth/settings/index";
import { CommentsPage } from "@/routes/_auth/comments/index";

vi.mock("@/components/editor/MonacoPane", () => ({
  MonacoPane: ({ value }: { value: string }) => <textarea readOnly value={value} aria-label="css" />,
}));

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: {
      ...original.api,
      getOptions: vi.fn(),
      putOptions: vi.fn(),
      listCommentsPage: vi.fn(),
      moderateComment: vi.fn(),
    },
  };
});

const mocked = vi.mocked(api);

function mount(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ConfirmProvider><TestRouter>{node}</TestRouter></ConfirmProvider>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
});

describe("AI feature switches", () => {
  it("reads the switches as booleans and a mode, and saves them typed", async () => {
    const user = userEvent.setup();
    mocked.getOptions.mockResolvedValue({
      site_title: "Vyasa",
      ai_alt_text: true,
      ai_images: false,
      ai_comment_screening: "spam",
    });
    mocked.putOptions.mockResolvedValue(undefined);
    mount(<SettingsPage />);

    const alt = (await screen.findByLabelText("Write alt text for uploaded images")) as HTMLInputElement;
    expect(alt.checked).toBe(true);
    const images = screen.getByLabelText("Image generation") as HTMLInputElement;
    expect(images.checked).toBe(false);
    const mode = screen.getByLabelText("Screen new comments") as HTMLSelectElement;
    expect(mode.value).toBe("spam");

    await user.click(images);
    await user.selectOptions(mode, "flag");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() =>
      expect(mocked.putOptions).toHaveBeenCalledWith({
        ai_images: true,
        ai_comment_screening: "flag",
      }),
    );
  });
});

describe("comment screening in the queue", () => {
  it("shows the verdict next to a screened comment and nothing for the rest", async () => {
    mocked.listCommentsPage.mockResolvedValue({ items: [
      {
        id: "1",
        post_id: "9",
        author_user_id: null,
        author_name: "Spammer",
        content: "Buy now",
        parent_id: null,
        status: "pending",
        created_at: "2026-08-29T00:00:00Z",
        moderation: {
          flagged: true,
          top: [{ category: "harassment/threatening", score: 0.91 }],
          model: "omni-moderation-latest",
          action: "none",
          at: "2026-08-29T00:00:00Z",
        },
      },
      {
        id: "2",
        post_id: "9",
        author_user_id: null,
        author_name: "Reader",
        content: "Lovely",
        parent_id: null,
        status: "pending",
        created_at: "2026-08-29T00:00:00Z",
        moderation: null,
      },
      {
        // A verdict without `top` (older build, hand edit) must not take the
        // whole queue down with it.
        id: "3",
        post_id: "9",
        author_user_id: null,
        author_name: "Partial",
        content: "Hmm",
        parent_id: null,
        status: "pending",
        created_at: "2026-08-29T00:00:00Z",
        moderation: { flagged: true },
      },
    ], total: 1 } as never);
    mount(<CommentsPage />);
    expect(await screen.findByText(/Flagged: harassment threatening 91%/)).toBeInTheDocument();
    expect(screen.getByText("Flagged")).toBeInTheDocument();
    const rows = screen.getAllByRole("listitem").length + screen.queryAllByRole("row").length;
    expect(rows).toBeGreaterThan(0);
    expect(within(document.body).getAllByText("—").length).toBeGreaterThanOrEqual(1);
  });
});

describe("site settings", () => {
  it("offers the moderation modes the server actually understands", async () => {
    // The three values this select used to offer (none/hold_new/all) were
    // rejected by the server's own validator, so the setting never took.
    mocked.getOptions.mockResolvedValue({} as never);
    mount(<SettingsPage />);
    const select = await screen.findByLabelText("Moderation");
    const values = Array.from(select.querySelectorAll("option")).map((o) => o.value);
    expect(values).toEqual(["require_first", "auto_approve", "require_all"]);
  });
});
