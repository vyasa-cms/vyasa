import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { api, ApiError, type ContentField } from "@/api/client";
import { PostEditor } from "@/components/editor/PostEditor";
import { notify } from "@/components/ui/toast";
import { fieldErrorsOf } from "@/lib/fields";
import { TestRouter } from "./TestRouter";

vi.mock("@/components/ui/toast", () => ({
  notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), undo: vi.fn() },
}));

function def(key: string, label: string, kind: ContentField["kind"], position: number, extra: Partial<ContentField> = {}): ContentField {
  return { type_slug: "product", key, label, help: "", kind, required: false, options: {}, position, created_at: "", updated_at: "", ...extra };
}

const DEFS: ContentField[] = [
  def("tagline", "Tagline", "text", 0, { required: true, help: "One line under the name." }),
  def("story", "Story", "textarea", 1),
  def("price", "Price", "number", 2, { options: { min: 0, step: 0.01 } }),
  def("in_stock", "In stock", "boolean", 3),
  def("launch", "Launch date", "date", 4),
  def("colour", "Colour", "choice", 5, { options: { choices: ["red", "blue"] } }),
  def("sizes", "Sizes", "choice", 6, { options: { choices: ["S", "M", "L"], multiple: true } }),
  def("manual", "Manual", "url", 7),
  def("photo", "Photo", "media", 8),
  def("maker", "Maker", "entry", 9, { options: { entry_type: "brand" } }),
];

const post = {
  id: "12",
  title: "Lamp",
  content: { schema_version: 1, blocks: [] },
  excerpt: "",
  slug: "lamp",
  status: "draft",
  type: "product",
  updated_at: "2026-09-01T00:00:00Z",
  created_at: "2026-09-01T00:00:00Z",
  layout: [],
  scheduled_for: null,
  parent_id: null,
  meta: { seo_title: "" },
  author_id: "1",
  fields: { price: 10, colour: "red", photo: "77", maker: "5" },
  fields_missing: [] as string[],
};

function mount(id = "12") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <TestRouter>
        <PostEditor mode={{ kind: "edit", id }} />
      </TestRouter>
    </QueryClientProvider>,
  );
  return client;
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.clearAllMocks();
  localStorage.setItem("vyasa-editor-mode", "classic");
  vi.spyOn(api, "me").mockResolvedValue({ id: "1" } as never);
  vi.spyOn(api, "myCaps").mockResolvedValue(["edit_posts", "edit_others", "publish_posts"] as never);
  vi.spyOn(api, "getPost").mockImplementation(async (id: string) =>
    id === "5"
      ? ({ ...structuredClone(post), id: "5", title: "Acme", type: "brand", fields: {} } as never)
      : (structuredClone(post) as never),
  );
  vi.spyOn(api, "listRevisions").mockResolvedValue([] as never);
  vi.spyOn(api, "postTerms").mockResolvedValue([] as never);
  vi.spyOn(api, "getOptions").mockResolvedValue({} as never);
  vi.spyOn(api, "listTerms").mockResolvedValue([] as never);
  vi.spyOn(api, "listFields").mockResolvedValue(structuredClone(DEFS));
  vi.spyOn(api, "listContentTypes").mockResolvedValue([] as never);
  vi.spyOn(api, "listMedia").mockResolvedValue([] as never);
  vi.spyOn(api, "getMedia").mockResolvedValue({ id: "77", file_name: "lamp.jpg", mime: "image/jpeg" } as never);
  vi.spyOn(api, "listPosts").mockResolvedValue({ items: [], total: 0, page: 1, per_page: 10 } as never);
  vi.spyOn(api, "lockPost").mockResolvedValue({ mine: true, holder_name: null, seen_ago_secs: null });
  vi.spyOn(api, "unlockPost").mockResolvedValue(undefined);
  vi.spyOn(api, "aiAvailable").mockResolvedValue({ text: false, vision: false, image: false, transcription: false, speech: false });
  vi.spyOn(api, "autosave").mockResolvedValue({});
});

afterEach(() => {
  vi.useRealTimers();
  localStorage.clear();
});

async function panel() {
  const p = await screen.findByTestId("fields-panel");
  await waitFor(() => expect(within(p).getByLabelText(/Price/)).toHaveValue(10));
  return p;
}

describe("the editor's fields panel", () => {
  it("splits a server validation message into one message per field", () => {
    expect(
      fieldErrorsOf(new ApiError(400, { code: "validation_failed", message: "fields.price: must be at least 0; fields.tagline: Tagline is required to publish or schedule" })),
    ).toEqual({ price: "must be at least 0", tagline: "Tagline is required to publish or schedule" });
    expect(fieldErrorsOf(new ApiError(400, { code: "validation_failed", message: "slug is taken" }))).toEqual({});
    expect(fieldErrorsOf(new ApiError(409, { code: "conflict", message: "fields.price: x" }))).toEqual({});
    expect(fieldErrorsOf(new Error("fields.price: x"))).toEqual({});
  });

  it("renders every kind with the stored values, help and a required marker", async () => {
    mount();
    const p = await panel();
    expect(within(p).getByLabelText(/Tagline/)).toHaveAttribute("type", "text");
    expect(within(p).getByText("One line under the name.")).toBeInTheDocument();
    expect(within(p).getByLabelText(/Tagline/).closest("[data-field]")).toHaveTextContent("required");
    expect(within(p).getByLabelText(/Story/).tagName).toBe("TEXTAREA");
    expect(within(p).getByLabelText(/Price/)).toHaveAttribute("type", "number");
    expect(within(p).getByLabelText(/Price/)).toHaveAttribute("step", "0.01");
    expect(within(p).getByRole("checkbox", { name: /In stock/ })).not.toBeChecked();
    expect(within(p).getByLabelText(/Launch date/)).toHaveAttribute("type", "date");
    expect(within(p).getByLabelText(/Colour/)).toHaveValue("red");
    expect(within(p).getByRole("checkbox", { name: "M" })).not.toBeChecked();
    expect(within(p).getByLabelText(/Manual/)).toHaveAttribute("inputmode", "url");
    expect(await within(p).findByText("lamp.jpg")).toBeInTheDocument();
    expect(await within(p).findByText("Acme")).toBeInTheDocument();
  });

  it("sends typed values in `fields`, never in meta, and does not block a draft save on a missing required field", async () => {
    const user = userEvent.setup();
    const update = vi.spyOn(api, "updatePost").mockImplementation(async () => structuredClone(post) as never);
    mount();
    const p = await panel();
    await user.clear(within(p).getByLabelText(/Price/));
    await user.type(within(p).getByLabelText(/Price/), "12.5");
    await user.click(within(p).getByRole("checkbox", { name: /In stock/ }));
    await user.type(within(p).getByLabelText(/Launch date/), "2026-10-01");
    await user.selectOptions(within(p).getByLabelText(/Colour/), "blue");
    await user.click(within(p).getByRole("checkbox", { name: "M" }));
    await user.click(within(p).getByRole("checkbox", { name: "S" }));
    await user.type(within(p).getByLabelText(/Manual/), "/manuals/lamp");
    await user.click(within(p).getByRole("button", { name: "Remove Photo" }));

    await user.click(screen.getAllByRole("button", { name: "Save draft" })[0]!);
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    const body = update.mock.calls[0]![1];
    expect(body.fields).toEqual({
      price: 12.5,
      in_stock: true,
      launch: "2026-10-01",
      colour: "blue",
      sizes: ["S", "M"],
      manual: "/manuals/lamp",
      maker: "5",
    });
    // Tagline is required and empty: the draft saved anyway.
    expect(body.meta).not.toHaveProperty("fields");
  });

  it("leaves `fields` out of a save that did not touch them, so values a deleted field left are not wiped", async () => {
    const user = userEvent.setup();
    const update = vi.spyOn(api, "updatePost").mockImplementation(async () => structuredClone(post) as never);
    mount();
    await panel();
    await user.type(screen.getByPlaceholderText("Post title"), "!");
    await user.click(screen.getAllByRole("button", { name: "Save draft" })[0]!);
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update.mock.calls[0]![1]).not.toHaveProperty("fields");
  });

  it("puts each server message next to its field, and clears it once the field is edited", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "updatePost").mockRejectedValue(
      new ApiError(400, { code: "validation_failed", message: "fields.price: must be at least 0; fields.tagline: Tagline is required to publish or schedule" }),
    );
    mount();
    const p = await panel();
    await user.clear(within(p).getByLabelText(/Price/));
    await user.type(within(p).getByLabelText(/Price/), "-1");
    await user.click(screen.getAllByRole("button", { name: "Save draft" })[0]!);

    const priceBox = (await within(p).findByText("must be at least 0")).closest("[data-field]") as HTMLElement;
    expect(priceBox).toHaveAttribute("data-field", "price");
    expect(within(p).getByText("Tagline is required to publish or schedule").closest("[data-field]")).toHaveAttribute("data-field", "tagline");
    expect(within(p).getByLabelText(/Price/)).toHaveAttribute("aria-invalid", "true");

    await user.type(within(p).getByLabelText(/Price/), "0");
    expect(within(p).queryByText("must be at least 0")).not.toBeInTheDocument();
    expect(within(p).getByText("Tagline is required to publish or schedule")).toBeInTheDocument();
  });

  it("marks references whose media item or entry is gone", async () => {
    vi.mocked(api.getPost).mockImplementation(async () => ({ ...structuredClone(post), fields_missing: ["photo"] }) as never);
    mount();
    const p = await panel();
    const photo = within(p).getByText(/no longer exists/i).closest("[data-field]");
    expect(photo).toHaveAttribute("data-field", "photo");
  });

  it("autosaves field values with the text", async () => {
    const user = userEvent.setup();
    mount();
    const p = await panel();
    await user.type(within(p).getByLabelText(/Tagline/), "Bright");
    await waitFor(
      () =>
        expect(vi.mocked(api.autosave)).toHaveBeenCalledWith(
          "12",
          expect.objectContaining({ fields: expect.objectContaining({ tagline: "Bright", price: 10 }) }),
        ),
      { timeout: 4000 },
    );
  });

  it("loads a revision's values and says which ones could not come back", async () => {
    const user = userEvent.setup();
    vi.mocked(api.listRevisions).mockResolvedValue([
      {
        id: "r1",
        post_id: "12",
        title: "Lamp",
        content: { schema_version: 1, blocks: [] },
        is_autosave: false,
        created_at: "2026-08-01T00:00:00Z",
        fields: { price: 8, old_sku: "L-1" },
      },
    ] as never);
    mount();
    const p = await panel();
    await user.click(screen.getByRole("button", { name: /show settings/i }));
    await user.click(await screen.findByRole("button", { name: "Load" }));
    await waitFor(() => expect(within(p).getByLabelText(/Price/)).toHaveValue(8));
    expect(vi.mocked(notify).info).toHaveBeenCalledWith(
      "Some field values were not loaded",
      expect.stringContaining("old_sku"),
    );
  });

  it("never sends a key the type no longer defines, even from a loaded working copy", async () => {
    const user = userEvent.setup();
    vi.mocked(api.listRevisions).mockResolvedValue([
      {
        id: "r2", post_id: "12", title: "Lamp", content: { schema_version: 1, blocks: [] }, is_autosave: false,
        created_at: "2026-09-02T00:00:00Z",
        fields: { price: 9, old_sku: "L-1" },
      },
    ] as never);
    const update = vi.spyOn(api, "updatePost").mockImplementation(async () => structuredClone(post) as never);
    mount();
    const p = await screen.findByTestId("fields-panel");
    await waitFor(() => expect(within(p).getByLabelText(/Price/)).toHaveValue(9));
    await user.type(within(p).getByLabelText(/Tagline/), "Bright");
    await waitFor(
      () => expect(vi.mocked(api.autosave)).toHaveBeenCalledWith("12", expect.objectContaining({ fields: expect.objectContaining({ tagline: "Bright" }) })),
      { timeout: 4000 },
    );
    expect(vi.mocked(api.autosave).mock.calls.at(-1)![1].fields).not.toHaveProperty("old_sku");
    await user.click(screen.getAllByRole("button", { name: "Save draft" })[0]!);
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
    expect(update.mock.calls[0]![1].fields).toEqual({ price: 9, tagline: "Bright" });
  });

  it("holds back a publish while a required field is empty, saying so next to the field", async () => {
    const user = userEvent.setup();
    const update = vi.spyOn(api, "updatePost").mockImplementation(async () => structuredClone(post) as never);
    mount();
    const p = await panel();
    await user.click(screen.getAllByRole("button", { name: "Publish" })[0]!);
    const tagline = (await within(p).findByText("Tagline is required to publish or schedule")).closest("[data-field]");
    expect(tagline).toHaveAttribute("data-field", "tagline");
    expect(vi.mocked(notify).error).toHaveBeenCalledWith("Fill in the required fields to publish", "Tagline");
    expect(update).not.toHaveBeenCalled();
    // A draft save still goes through.
    await user.click(screen.getAllByRole("button", { name: "Save draft" })[0]!);
    await waitFor(() => expect(update).toHaveBeenCalledTimes(1));
  });

  it("treats a working copy or autosave that differs only by a key the type no longer defines as the same entry", async () => {
    vi.mocked(api.getPost).mockImplementation(async () => ({ ...structuredClone(post), status: "published" }) as never);
    const phantom = { ...structuredClone(post.fields), old_sku: "L-1" };
    vi.mocked(api.listRevisions).mockResolvedValue([
      { id: "r3", post_id: "12", title: "Lamp", content: { schema_version: 1, blocks: [] }, is_autosave: false, created_at: "2026-09-02T00:00:00Z", fields: phantom },
      { id: "r4", post_id: "12", title: "Lamp", content: { schema_version: 1, blocks: [] }, is_autosave: true, created_at: "2026-09-03T00:00:00Z", fields: phantom },
    ] as never);
    mount();
    await panel();
    await act(async () => {
      await new Promise((r) => setTimeout(r, 100));
    });
    expect(screen.queryByTestId("unpublished-banner")).not.toBeInTheDocument();
    expect(screen.queryByTestId("draft-offer")).not.toBeInTheDocument();
    expect(screen.queryByText("Unsaved changes")).not.toBeInTheDocument();
  });

  it("loads a revision's values as they are when the type's fields could not be read, without claiming any were dropped", async () => {
    const user = userEvent.setup();
    vi.mocked(api.listFields).mockRejectedValue(new ApiError(500, { code: "internal", message: "down" }));
    vi.mocked(api.listRevisions).mockResolvedValue([
      { id: "r1", post_id: "12", title: "Lamp", content: { schema_version: 1, blocks: [] }, is_autosave: false, created_at: "2026-08-01T00:00:00Z", fields: { price: 8, old_sku: "L-1" } },
    ] as never);
    mount();
    await screen.findByPlaceholderText("Post title");
    await user.click(screen.getByRole("button", { name: /show settings/i }));
    await user.click(await screen.findByRole("button", { name: "Load" }));
    await waitFor(() => expect(vi.mocked(notify).success).toHaveBeenCalledWith("Revision loaded", expect.any(String)));
    expect(vi.mocked(notify).info).not.toHaveBeenCalled();
  });

  it("names the verb and the field on the media and entry buttons", async () => {
    mount();
    const p = await panel();
    expect(within(p).getByRole("button", { name: "Change Photo" })).toBeInTheDocument();
    expect(within(p).getByRole("button", { name: "Change Maker" })).toBeInTheDocument();
  });

  it("says Choose for an empty media or entry field and Close while its picker is open", async () => {
    const user = userEvent.setup();
    vi.mocked(api.getPost).mockImplementation(async (id: string) =>
      id === "5"
        ? ({ ...structuredClone(post), id: "5", title: "Acme", type: "brand", fields: {} } as never)
        : ({ ...structuredClone(post), fields: { price: 10, colour: "red" } } as never),
    );
    mount();
    const p = await panel();
    const photo = within(p).getByRole("button", { name: "Choose Photo" });
    expect(photo).toHaveTextContent("Choose");
    const maker = within(p).getByRole("button", { name: "Choose Maker" });
    await user.click(photo);
    expect(within(p).getByRole("button", { name: "Close Photo" })).toHaveTextContent("Close");
    await user.click(maker);
    expect(within(p).getByRole("button", { name: "Close Maker" })).toHaveTextContent("Close");
    await user.click(within(p).getByRole("button", { name: "Close Maker" }));
    expect(within(p).getByRole("button", { name: "Choose Maker" })).toBeInTheDocument();
  });

  it("shows nothing for a type without fields", async () => {
    vi.mocked(api.listFields).mockResolvedValue([]);
    mount();
    await screen.findByPlaceholderText("Post title");
    await act(async () => {
      await new Promise((r) => setTimeout(r, 50));
    });
    expect(screen.queryByTestId("fields-panel")).not.toBeInTheDocument();
  });
});
