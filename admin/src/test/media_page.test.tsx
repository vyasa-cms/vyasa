import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

const item = (id: string, over: Record<string, unknown> = {}) => ({
  id, owner_id: "1", file_name: `${id}.jpg`, mime: "image/jpeg", byte_size: 2048, storage: "local", path: "x",
  width: 1600, height: 900, blurhash: "LEHV6nWB2yk8", alt: "", caption: "", derivatives: { thumb: { width: 320 } },
  created_at: "2026-09-06T00:00:00Z", sha256: "ab", focal_x: null, focal_y: null, ...over,
});

const { listMediaPage, myCaps } = vi.hoisted(() => ({ listMediaPage: vi.fn(), myCaps: vi.fn() }));
vi.mock("@/api/client", async (importOriginal) => {
  const mod = await importOriginal<{ api: Record<string, unknown> }>();
  return {
    ...mod,
    api: {
      ...mod.api,
      listMediaPage,
      myCaps,
      me: vi.fn().mockResolvedValue({ id: "1" }),
      mediaStats: vi.fn().mockResolvedValue({ count: 57, bytes: 5 * 1024 * 1024, missing_alt: 3, cap_bytes: 50 * 1024 * 1024 }),
      mediaUsage: vi.fn().mockResolvedValue({ posts: [{ id: "9", title: "Uses it", status: "published" }], site_logo: false, site_favicon: false }),
      batchDeleteMedia: vi.fn().mockResolvedValue({ deleted: ["a"], failed: [] }),
      updateMedia: vi.fn().mockImplementation((_id: string, body: Record<string, unknown>) => Promise.resolve(item("a", { alt: body.alt ?? "", caption: body.caption ?? "" }))),
      mediaTranscript: vi.fn(),
    },
  };
});

import { ConfirmProvider } from "@/components/ui/dialog";
import { thumbUrl } from "@/routes/_auth/media/index";

function makeApp() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const Page = (Route as { options: { component: React.ComponentType } }).options.component;
  return (
    <QueryClientProvider client={client}>
      <ConfirmProvider>
        <Page />
      </ConfirmProvider>
    </QueryClientProvider>
  );
}
import { Route } from "@/routes/_auth/media/index";
import type * as React from "react";

beforeEach(() => {
  myCaps.mockReset().mockResolvedValue(["upload_media", "edit_others"]);
});

describe("media page", () => {
  it("tiles load the thumbnail, not the original", () => {
    expect(thumbUrl(item("a") as never)).toBe("/api/v1/media/a/raw?variant=thumb");
    expect(thumbUrl(item("b", { derivatives: {} }) as never)).toBe("/api/v1/media/b/raw");
  });

  it("shows library totals and real pagination, and asks before deleting a used file", async () => {
    listMediaPage.mockResolvedValue({ items: [item("a"), item("b", { alt: "described" })], total: 57 });
    const user = userEvent.setup();
    render(makeApp());
    expect((await screen.findAllByText(/57 files/)).length).toBeGreaterThan(0);
    expect(await screen.findByTestId("media-pagination")).toHaveTextContent("Page 1 of 3 · 57 files");
    expect(screen.getByTestId("missing-alt-banner")).toHaveTextContent("3 images have no alt text in the library");

    await user.click(screen.getByRole("button", { name: /a\.jpg/ }));
    expect(await screen.findByTestId("media-usage")).toHaveTextContent("Uses it");
    await user.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByText(/Used in 1 post/)).toBeInTheDocument();
  });

  it("goes quiet after saving details", async () => {
    listMediaPage.mockResolvedValue({ items: [item("a")], total: 1 });
    const user = userEvent.setup();
    render(makeApp());
    await user.click(await screen.findByRole("button", { name: /a\.jpg/ }));
    const save = await screen.findByTestId("save-details");
    expect(save).toBeDisabled();
    await user.type(screen.getByLabelText("Alt text"), "A hill");
    expect(save).toBeEnabled();
    await user.click(save);
    await waitFor(() => expect(screen.getByTestId("save-details")).toBeDisabled());
  });

  it("is read-only for edit_posts without upload_media", async () => {
    myCaps.mockReset().mockResolvedValue(["edit_posts"]);
    listMediaPage.mockResolvedValue({ items: [item("a")], total: 1 });
    const user = userEvent.setup();
    render(makeApp());

    // The page is still visible (GET /media accepts edit_posts) but every
    // write affordance — upload, delete, edit, replace — needs upload_media.
    await screen.findByTestId("media-grid");
    expect(screen.queryByRole("button", { name: "Upload" })).not.toBeInTheDocument();
    expect(screen.queryByTestId("drop-zone")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: /a\.jpg/ }));
    expect(await screen.findByLabelText("Alt text")).toBeDisabled();
    expect(screen.getByLabelText("Caption")).toBeDisabled();
    expect(screen.getByLabelText("File name")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    expect(screen.queryByTestId("save-details")).not.toBeInTheDocument();
    expect(screen.queryByTestId("replace-file")).not.toBeInTheDocument();
    expect(screen.queryByTestId("image-tools")).not.toBeInTheDocument();
  });
});


it("provides keyboard focal positioning and numeric crop bounds", async () => {
  listMediaPage.mockResolvedValue({ items: [item("a")], total: 1 });
  const user = userEvent.setup(); render(makeApp());
  await user.click(await screen.findByRole("button", { name: /a\.jpg/ }));
  const picker = await screen.findByTestId("focal-picker");
  picker.focus(); await user.keyboard("{ArrowRight}{ArrowDown}");
  expect(screen.getByText(/Now at 51%, 51%/)).toBeInTheDocument();
  await user.click(screen.getByTestId("start-crop"));
  const left = screen.getByLabelText("Left");
  await user.clear(left); await user.type(left, "25");
  expect(screen.getByLabelText("Width")).toHaveValue(75);
  expect(screen.getByTestId("apply-crop")).toBeEnabled();
});
