import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError, type ContentField } from "@/api/client";
import { FieldEditor } from "@/components/content/FieldEditor";
import { ConfirmProvider } from "@/components/ui/dialog";
import { notify } from "@/components/ui/toast";
import { TestRouter } from "./TestRouter";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: {
      ...original.api,
      listContentTypes: vi.fn(),
      listFields: vi.fn(),
      createField: vi.fn(),
      updateField: vi.fn(),
      deleteField: vi.fn(),
      reorderFields: vi.fn(),
      listOrphans: vi.fn(),
      cleanUpOrphan: vi.fn(),
    },
  };
});
vi.mock("@/components/ui/toast", () => ({
  notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), undo: vi.fn() },
}));

const mocked = vi.mocked(api);

/** The dialog focuses itself on the next frame; typing before that would lose keystrokes. */
async function settle() {
  await act(async () => {
    await new Promise((r) => requestAnimationFrame(() => r(undefined)));
  });
}

function field(key: string, kind: ContentField["kind"], position: number, extra: Partial<ContentField> = {}): ContentField {
  return {
    type_slug: "product",
    key,
    label: key[0]!.toUpperCase() + key.slice(1),
    help: "",
    kind,
    required: false,
    options: {},
    position,
    created_at: "",
    updated_at: "",
    ...extra,
  };
}

const FIELDS = [
  field("price", "number", 0, { required: true, options: { min: 0, step: 0.01 } }),
  field("colour", "choice", 1, { options: { choices: ["red", "blue"], multiple: false } }),
  field("manual", "url", 2),
];

const base = { public: true, has_archive: true, description: "", count: 0, plugin: false };
const TYPES = [
  { ...base, slug: "post", singular: "Post", plural: "Posts", owner: "builtin" },
  { ...base, slug: "page", singular: "Page", plural: "Pages", owner: "builtin" },
  { ...base, slug: "product", singular: "Product", plural: "Products", owner: "admin" },
  { ...base, slug: "brand", singular: "Brand", plural: "Brands", owner: "admin" },
];

function mount(slug = "product") {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ConfirmProvider>
        <TestRouter>
          <FieldEditor slug={slug} />
        </TestRouter>
      </ConfirmProvider>
    </QueryClientProvider>,
  );
}

function rowOf(label: string): HTMLElement {
  return screen.getByText(label, { selector: "[data-testid=field-label]" }).closest("li") as HTMLElement;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.listContentTypes.mockResolvedValue(structuredClone(TYPES) as never);
  mocked.listFields.mockResolvedValue(structuredClone(FIELDS));
  mocked.listOrphans.mockResolvedValue([]);
  mocked.createField.mockImplementation(async (_slug, body) => field(body.key, body.kind, 9, body as Partial<ContentField>));
});

async function openAdd(user: ReturnType<typeof userEvent.setup>, kind: string, label: string) {
  await user.click(screen.getByRole("button", { name: "Add field" }));
  await settle();
  await user.type(screen.getByLabelText("Label"), label);
  await user.selectOptions(screen.getByLabelText("Kind"), kind);
}

describe("field editor", () => {
  it("lists the type's fields in order with their kind and a required marker", async () => {
    mount();
    expect(await screen.findByRole("heading", { name: /Products/ })).toBeInTheDocument();
    await screen.findByText("Price", { selector: "[data-testid=field-label]" });
    const labels = screen.getAllByTestId("field-label").map((n) => n.textContent);
    expect(labels).toEqual(["Price", "Colour", "Manual"]);
    expect(within(rowOf("Price")).getByText("Required")).toBeInTheDocument();
    expect(within(rowOf("Price")).getByText("Number")).toBeInTheDocument();
    expect(within(rowOf("Manual")).getByText("Link")).toBeInTheDocument();
  });

  type User = ReturnType<typeof userEvent.setup>;
  const KINDS: [string, string, string, (user: User) => Promise<void>, Record<string, unknown>][] = [
    ["text", "Short name", "short_name", async (user) => {
      await user.type(screen.getByLabelText("Maximum length"), "80");
      await user.click(screen.getByRole("checkbox", { name: /Required/ }));
    }, { max_length: 80 }],
    ["textarea", "Story", "story", async () => undefined, {}],
    ["number", "Weight", "weight", async (user) => {
      await user.type(screen.getByLabelText("Minimum"), "0");
      await user.type(screen.getByLabelText("Maximum"), "500");
      await user.type(screen.getByLabelText("Step"), "0.5");
    }, { min: 0, max: 500, step: 0.5 }],
    ["boolean", "In stock", "in_stock", async () => undefined, {}],
    ["date", "Launch date", "launch_date", async () => undefined, {}],
    ["url", "Manual link", "manual_link", async () => undefined, {}],
    ["media", "Photo", "photo", async () => undefined, {}],
    ["choice", "Sizes", "sizes", async (user) => {
      await user.type(screen.getByLabelText("Choices"), "S{enter}M{enter}L");
      await user.click(screen.getByRole("checkbox", { name: /more than one/i }));
    }, { choices: ["S", "M", "L"], multiple: true }],
    ["entry", "Maker", "maker", async (user) => {
      await user.selectOptions(screen.getByLabelText("Points at"), "brand");
    }, { entry_type: "brand" }],
  ];

  it.each(KINDS)("adds a %s field, its key following the label, sending only that kind's options", async (kind, label, key, fill, options) => {
    const user = userEvent.setup();
    mount();
    await screen.findByText("Price", { selector: "[data-testid=field-label]" });
    await openAdd(user, kind, label);
    expect(screen.getByLabelText("Key")).toHaveValue(key);
    await fill(user);
    await user.click(screen.getByRole("button", { name: "Create field" }));
    await waitFor(() =>
      expect(mocked.createField).toHaveBeenCalledWith("product", {
        key, label, help: "", kind, required: kind === "text", options,
      }),
    );
  });

  it("refuses a malformed key before sending it, and shows the server's message on a taken key", async () => {
    const user = userEvent.setup();
    mocked.createField.mockRejectedValue(new ApiError(409, { code: "conflict", message: "values for \"price\" are still stored; clean them up first" }));
    mount();
    await screen.findByText("Price", { selector: "[data-testid=field-label]" });
    await openAdd(user, "text", "X");
    await user.clear(screen.getByLabelText("Key"));
    await user.type(screen.getByLabelText("Key"), "1bad-key");
    expect(screen.getByRole("button", { name: "Create field" })).toBeDisabled();
    await user.clear(screen.getByLabelText("Key"));
    await user.type(screen.getByLabelText("Key"), "price");
    await user.click(screen.getByRole("button", { name: "Create field" }));
    await waitFor(() =>
      expect(vi.mocked(notify).error).toHaveBeenCalledWith(
        "Couldn't add the field",
        expect.objectContaining({ message: expect.stringContaining("clean them up first") }),
      ),
    );
  });

  it("edits a field, keeping its key, and replaces its options", async () => {
    const user = userEvent.setup();
    mocked.updateField.mockResolvedValue(FIELDS[1]!);
    mount();
    await screen.findByText("Colour", { selector: "[data-testid=field-label]" });
    await user.click(within(rowOf("Colour")).getByRole("button", { name: "Edit" }));
    await settle();
    expect(screen.getByLabelText("Key")).toBeDisabled();
    expect(screen.getByLabelText("Choices")).toHaveValue("red\nblue");
    await user.type(screen.getByLabelText("Choices"), "{enter}green");
    await user.click(screen.getByRole("button", { name: "Save field" }));
    await waitFor(() =>
      expect(mocked.updateField).toHaveBeenCalledWith("product", "colour", {
        label: "Colour", help: "", kind: "choice", required: false, options: { choices: ["red", "blue", "green"], multiple: false },
      }),
    );
  });

  it("shows the count when a kind change is refused", async () => {
    const user = userEvent.setup();
    mocked.updateField.mockRejectedValue(new ApiError(409, { code: "conflict", message: "2 entries hold a value for \"manual\"; the kind can't change" }));
    mount();
    await screen.findByText("Manual", { selector: "[data-testid=field-label]" });
    await user.click(within(rowOf("Manual")).getByRole("button", { name: "Edit" }));
    await settle();
    await user.selectOptions(screen.getByLabelText("Kind"), "text");
    await user.click(screen.getByRole("button", { name: "Save field" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("2 entries hold a value");
  });

  it("reorders by sending every key in the new order", async () => {
    const user = userEvent.setup();
    mocked.reorderFields.mockResolvedValue(FIELDS);
    mount();
    await screen.findByText("Colour", { selector: "[data-testid=field-label]" });
    await user.click(within(rowOf("Colour")).getByRole("button", { name: "Move Colour up" }));
    await waitFor(() => expect(mocked.reorderFields).toHaveBeenCalledWith("product", ["colour", "price", "manual"]));
    expect(within(rowOf("Price")).getByRole("button", { name: "Move Price up" })).toBeDisabled();
    expect(within(rowOf("Manual")).getByRole("button", { name: "Move Manual down" })).toBeDisabled();
  });

  it("deletes a field after saying stored values are kept, and cleans up left-over values on request", async () => {
    const user = userEvent.setup();
    mocked.deleteField.mockResolvedValue(undefined);
    mocked.listOrphans.mockResolvedValue([{ key: "old_sku", entries: 4 }]);
    mocked.cleanUpOrphan.mockResolvedValue({ entries: 4 });
    mount();
    await screen.findByText("Manual", { selector: "[data-testid=field-label]" });

    await user.click(within(rowOf("Manual")).getByRole("button", { name: "Delete" }));
    const dialog = await screen.findByRole("dialog");
    expect(dialog).toHaveTextContent(/values are kept/i);
    await user.click(within(dialog).getByRole("button", { name: "Delete field" }));
    await waitFor(() => expect(mocked.deleteField).toHaveBeenCalledWith("product", "manual"));

    const orphans = await screen.findByTestId("orphans");
    expect(orphans).toHaveTextContent("old_sku");
    expect(orphans).toHaveTextContent("4 entries");
    await user.click(within(orphans).getByRole("button", { name: "Clean up old_sku" }));
    const confirm = await screen.findByRole("dialog");
    await user.click(within(confirm).getByRole("button", { name: "Remove values" }));
    await waitFor(() => expect(mocked.cleanUpOrphan).toHaveBeenCalledWith("product", "old_sku"));
    expect(vi.mocked(notify).success).toHaveBeenCalledWith("Values removed", "From 4 entries.");
  });
});
