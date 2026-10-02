import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { notify } from "@/components/ui/toast";
import { canVisit, CAPABILITIES } from "@/lib/capabilities";
import { Route } from "@/routes/_auth/roles/index";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: { ...original.api, listRoles: vi.fn(), createRole: vi.fn(), updateRole: vi.fn(), deleteRole: vi.fn(), myCaps: vi.fn() },
  };
});
vi.mock("@/components/ui/toast", () => ({
  notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), undo: vi.fn() },
}));

const mocked = vi.mocked(api);
const Page = Route.options.component as React.ComponentType;

const ALL_CAPS = [
  "edit_posts", "publish_posts", "edit_others", "delete_posts", "manage_categories",
  "moderate_comments", "upload_media", "manage_users", "manage_themes", "manage_plugins",
  "manage_options", "view_admin",
];

const ADMIN_ROLE = { slug: "admin", name: "Administrator", description: "Everything.", capabilities: [...ALL_CAPS], built_in: true, users: 1 };
const EDITOR_ROLE = { slug: "editor", name: "Editor", description: "Publish and manage everyone's content.", capabilities: ["edit_posts", "publish_posts", "edit_others", "delete_posts", "manage_categories", "moderate_comments", "upload_media", "view_admin"], built_in: true, users: 0 };
const AUTHOR_ROLE = { slug: "author", name: "Author", description: "Write and publish their own posts.", capabilities: ["edit_posts", "publish_posts", "upload_media", "view_admin"], built_in: true, users: 0 };
const CONTRIBUTOR_ROLE = { slug: "contributor", name: "Contributor", description: "Write drafts an editor publishes.", capabilities: ["edit_posts", "view_admin"], built_in: true, users: 0 };
const SUBSCRIBER_ROLE = { slug: "subscriber", name: "Subscriber", description: "Read and comment only.", capabilities: ["view_admin"], built_in: true, users: 0 };
const BUILT_IN_ROLES = [ADMIN_ROLE, EDITOR_ROLE, AUTHOR_ROLE, CONTRIBUTOR_ROLE, SUBSCRIBER_ROLE];
const DRAFTER = { slug: "drafter", name: "Drafter", description: "Writes and uploads only.", capabilities: ["edit_posts", "upload_media"], built_in: false, users: 3 };

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><ConfirmProvider><Page /></ConfirmProvider></QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
  // Full capabilities by default, so checklist items are interactive unless a test narrows them.
  mocked.myCaps.mockResolvedValue([...ALL_CAPS]);
});

describe("roles page", () => {
  it("lists built-in roles as read-only with a Copy action, and custom roles with their user count", async () => {
    mount();
    await screen.findByText("Administrator");
    expect(screen.getByText("Drafter")).toBeInTheDocument();

    // Five built-ins, each offering only "Copy" — no Edit or Delete.
    expect(screen.getAllByRole("button", { name: "Copy" })).toHaveLength(5);
    expect(screen.queryAllByRole("button", { name: "Edit" })).toHaveLength(1);
    expect(screen.queryAllByRole("button", { name: "Delete" })).toHaveLength(1);

    expect(screen.getAllByText("Built-in")).toHaveLength(5);
    expect(screen.getByText("Custom")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument(); // Drafter's user count
  });

  it("keeps a role beyond the caller's own capabilities read-only, and Copy seeds only what's held", async () => {
    const user = userEvent.setup();
    // Holds neither edit_posts nor upload_media (Drafter's capabilities),
    // nor most of Editor's.
    mocked.myCaps.mockResolvedValue(["view_admin", "manage_users"]);
    mocked.createRole.mockResolvedValue({ ...DRAFTER } as never);
    mount();
    await screen.findByText("Administrator");

    // Drafter exceeds the caller: no Edit/Delete, a reason, Copy only.
    const drafterRow = screen.getByText("Drafter").closest("li") as HTMLElement;
    expect(within(drafterRow).queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(within(drafterRow).queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    expect(within(drafterRow).getByText(/includes permissions you don.t have/)).toBeInTheDocument();
    expect(within(drafterRow).getByRole("button", { name: "Copy" })).toBeInTheDocument();

    // Copying Editor (which holds far more than the caller) seeds only
    // view_admin, and names what was left out — never a capability the
    // caller lacks, so Save can never 403 on that account.
    await user.click(screen.getAllByRole("button", { name: "Copy" })[1]!); // Editor
    expect(screen.getByRole("checkbox", { name: /Use the admin screens/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Write drafts/ })).not.toBeChecked();
    const note = screen.getByText(/Left out because you don.t have them/);
    expect(note.textContent).toContain("Write drafts");
    expect(note.textContent).toContain("Upload media");

    await user.type(screen.getByLabelText("Slug"), "editor-lite");
    await user.click(screen.getByRole("button", { name: "Create role" }));

    await waitFor(() => expect(mocked.createRole).toHaveBeenCalledWith({
      slug: "editor-lite", name: "Editor copy", description: "Publish and manage everyone's content.", capabilities: ["view_admin"],
    }));
  });

  it("creates a custom role from a capability checklist, each capability labelled and described in plain words", async () => {
    const user = userEvent.setup();
    mocked.createRole.mockResolvedValue({ ...DRAFTER, slug: "reviewer" } as never);
    mount();
    await screen.findByText("Administrator");

    await user.click(screen.getByRole("button", { name: "New role" }));
    expect(screen.getByText("Write and edit their own posts and pages.")).toBeInTheDocument();
    expect(screen.getByText("Add images, files and other media to the library.")).toBeInTheDocument();

    await user.type(screen.getByLabelText("Slug"), "reviewer");
    await user.type(screen.getByLabelText("Name"), "Reviewer");
    await user.click(screen.getByRole("checkbox", { name: /Write drafts/ }));
    await user.click(screen.getByRole("checkbox", { name: /Upload media/ }));
    await user.click(screen.getByRole("button", { name: "Create role" }));

    await waitFor(() => expect(mocked.createRole).toHaveBeenCalledWith({
      slug: "reviewer", name: "Reviewer", description: "", capabilities: ["edit_posts", "upload_media"],
    }));
    expect(vi.mocked(notify).success).toHaveBeenCalledWith("Role created");
  });

  it("edits a custom role, prefilling its current fields, and saves the change", async () => {
    const user = userEvent.setup();
    mocked.updateRole.mockResolvedValue({ ...DRAFTER, capabilities: ["edit_posts"] } as never);
    mount();
    await screen.findByText("Drafter");

    await user.click(screen.getByRole("button", { name: "Edit" }));
    expect(screen.getByLabelText("Slug")).toHaveValue("drafter");
    expect(screen.getByLabelText("Name")).toHaveValue("Drafter");
    expect(screen.getByRole("checkbox", { name: /Write drafts/ })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: /Upload media/ })).toBeChecked();

    await user.click(screen.getByRole("checkbox", { name: /Upload media/ }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(mocked.updateRole).toHaveBeenCalledWith("drafter", {
      slug: "drafter", name: "Drafter", description: "Writes and uploads only.", capabilities: ["edit_posts"],
    }));
  });

  it("copies a built-in role into a fresh custom role, with a blank slug to fill in", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByText("Administrator");

    await user.click(screen.getAllByRole("button", { name: "Copy" })[1]!); // Editor
    expect(screen.getByText("Copy of Editor")).toBeInTheDocument();
    expect(screen.getByLabelText("Slug")).toHaveValue("");
    expect(screen.getByLabelText("Name")).toHaveValue("Editor copy");
    expect(screen.getByRole("checkbox", { name: /Moderate comments/ })).toBeChecked();
    // Editor does not hold manage_users.
    expect(screen.getByRole("checkbox", { name: /Manage people/ })).not.toBeChecked();
  });

  it("asks for confirmation before deleting, and shows the server's message when the role is still in use", async () => {
    const user = userEvent.setup();
    mocked.deleteRole.mockRejectedValue(new ApiError(409, { code: "conflict", message: "this role is held by 3 user(s); move them to another role first" }));
    mount();
    await screen.findByText("Drafter");

    await user.click(screen.getByRole("button", { name: "Delete" }));
    expect(await screen.findByText("Delete Drafter?")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Delete role" }));

    await waitFor(() => expect(mocked.deleteRole).toHaveBeenCalledWith("drafter"));
    await waitFor(() => expect(vi.mocked(notify).error).toHaveBeenCalledWith(
      "Couldn't delete the role",
      expect.objectContaining({ message: "this role is held by 3 user(s); move them to another role first" }),
    ));
  });

  it("surfaces the server's exact message, not a generic one, when creating a role is refused", async () => {
    const user = userEvent.setup();
    mocked.createRole.mockRejectedValue(new ApiError(403, { code: "forbidden", message: "this role holds capabilities you do not have: manage_options" }));
    mount();
    await screen.findByText("Administrator");

    await user.click(screen.getByRole("button", { name: "New role" }));
    await user.type(screen.getByLabelText("Slug"), "over-reach");
    await user.type(screen.getByLabelText("Name"), "Over reach");
    await user.click(screen.getByRole("button", { name: "Create role" }));

    await waitFor(() => expect(vi.mocked(notify).error).toHaveBeenCalledWith(
      "Couldn't create the role",
      expect.objectContaining({ message: "this role holds capabilities you do not have: manage_options" }),
    ));
    // The dialog stays open — a failed save is not treated as a success.
    expect(screen.getByRole("button", { name: "Create role" })).toBeInTheDocument();
  });

  it("surfaces the server's 403 message when editing a role that holds a capability beyond the caller's reach", async () => {
    // A role can hold a capability its stored caller no longer has (e.g. a
    // higher-capability administrator created it); PATCH refuses the edit
    // even when nothing about the capabilities is being changed.
    const user = userEvent.setup();
    mocked.updateRole.mockRejectedValue(new ApiError(403, { code: "forbidden", message: "this role holds capabilities you do not have: manage_options" }));
    mount();
    await screen.findByText("Drafter");

    await user.click(screen.getByRole("button", { name: "Edit" }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));

    await waitFor(() => expect(vi.mocked(notify).error).toHaveBeenCalledWith(
      "Couldn't update the role",
      expect.objectContaining({ message: "this role holds capabilities you do not have: manage_options" }),
    ));
    // The dialog stays open — a failed save is not treated as a success.
    expect(screen.getByRole("button", { name: "Save changes" })).toBeInTheDocument();
  });

  it("surfaces the server's 403 message when deleting a role that holds a capability beyond the caller's reach", async () => {
    const user = userEvent.setup();
    mocked.deleteRole.mockRejectedValue(new ApiError(403, { code: "forbidden", message: "this role holds capabilities you do not have: manage_options" }));
    mount();
    await screen.findByText("Drafter");

    await user.click(screen.getByRole("button", { name: "Delete" }));
    await user.click(await screen.findByRole("button", { name: "Delete role" }));

    await waitFor(() => expect(mocked.deleteRole).toHaveBeenCalledWith("drafter"));
    await waitFor(() => expect(vi.mocked(notify).error).toHaveBeenCalledWith(
      "Couldn't delete the role",
      expect.objectContaining({ message: "this role holds capabilities you do not have: manage_options" }),
    ));
  });

  it("disables a capability the signed-in user doesn't hold, with a hint, without blocking the rest of the form", async () => {
    const user = userEvent.setup();
    mocked.myCaps.mockResolvedValue(["edit_posts", "upload_media", "view_admin", "manage_users"]);
    mount();
    await screen.findByText("Administrator");
    await user.click(screen.getByRole("button", { name: "New role" }));

    const settings = await screen.findByRole("checkbox", { name: /Manage site settings/ });
    expect(settings).toBeDisabled();
    const settingsLabel = settings.closest("label") as HTMLLabelElement;
    expect(within(settingsLabel).getByText(/don.t have this capability/)).toBeInTheDocument();

    const drafts = screen.getByRole("checkbox", { name: /Write drafts/ });
    expect(drafts).toBeEnabled();
    await user.click(drafts);
    expect(drafts).toBeChecked();
  });

  it("shows a note when \"Use the admin screens\" is unchecked, and clears it once checked", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByText("Administrator");
    await user.click(screen.getByRole("button", { name: "New role" }));

    // Exactly what the shell does: the profile page and sign-out, nothing else.
    const note = /people with this role can sign in and manage their own profile, but can.t use the admin screens/;
    expect(screen.getByText(note)).toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: /Use the admin screens/ }));
    expect(screen.queryByText(note)).not.toBeInTheDocument();
  });

  it("warns that site settings, plugins and appearance are administrator-level as each is ticked", async () => {
    const user = userEvent.setup();
    mount();
    await screen.findByText("Administrator");
    await user.click(screen.getByRole("button", { name: "New role" }));

    // Nothing administrator-level is ticked yet.
    expect(screen.queryByTestId("admin-level-warning")).not.toBeInTheDocument();
    await user.click(screen.getByRole("checkbox", { name: /Write drafts/ }));
    expect(screen.queryByTestId("admin-level-warning")).not.toBeInTheDocument();

    for (const [name, label] of [
      ["manage_options", /Manage site settings/],
      ["manage_plugins", /Manage plugins/],
      ["manage_themes", /Manage appearance/],
    ] as const) {
      const box = screen.getByRole("checkbox", { name: label });
      await user.click(box);
      const warning = screen.getByTestId("admin-level-warning");
      expect(warning.textContent).toMatch(/^Administrator-level:/);
      // The warning says what this one could do, in the words of its description.
      expect(warning.textContent).toContain(CAPABILITIES[name]!.label);
      expect(warning.textContent).toContain(CAPABILITIES[name]!.administratorLevel);
      await user.click(box);
      expect(screen.queryByTestId("admin-level-warning")).not.toBeInTheDocument();
    }
  });

  it("describes the three administrator-level capabilities as such, and what still takes a full administrator", () => {
    const flagged = Object.entries(CAPABILITIES).filter(([, info]) => info.administratorLevel !== undefined).map(([name]) => name);
    expect(flagged.sort()).toEqual(["manage_options", "manage_plugins", "manage_themes"]);
    for (const name of flagged) {
      expect(CAPABILITIES[name]!.description).toMatch(/Administrator-level/);
    }
    // The takeover routes are not something manage_options hands out on its own.
    const options = CAPABILITIES.manage_options!;
    expect(options.fullAdministratorOnly?.join(" ")).toMatch(/mail relay/);
    expect(options.fullAdministratorOnly?.join(" ")).toMatch(/site address/);
    expect(options.fullAdministratorOnly?.join(" ")).toMatch(/updates/);
    expect(options.fullAdministratorOnly?.join(" ")).toMatch(/importing a site archive/);
    expect(options.also?.join(" ")).not.toMatch(/importing a site archive/);
  });

  it("refreshes the signed-in user's own capabilities after creating, editing or deleting a role", async () => {
    // Editing the role you hold changes what you can do; the shell and
    // this page must not keep acting on the grants from before.
    const user = userEvent.setup();
    const invalidated = vi.spyOn(QueryClient.prototype, "invalidateQueries");
    const myCapsInvalidations = () => invalidated.mock.calls.filter(([filters]) => JSON.stringify(filters?.queryKey) === '["my-caps"]').length;
    mocked.createRole.mockResolvedValue({ ...DRAFTER, slug: "reviewer" } as never);
    mocked.updateRole.mockResolvedValue({ ...DRAFTER } as never);
    mocked.deleteRole.mockResolvedValue(undefined as never);
    mount();
    await screen.findByText("Drafter");
    expect(myCapsInvalidations()).toBe(0);

    await user.click(screen.getByRole("button", { name: "New role" }));
    await user.type(screen.getByLabelText("Slug"), "reviewer");
    await user.type(screen.getByLabelText("Name"), "Reviewer");
    await user.click(screen.getByRole("button", { name: "Create role" }));
    await waitFor(() => expect(myCapsInvalidations()).toBe(1));

    await user.click(screen.getByRole("button", { name: "Edit" }));
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(myCapsInvalidations()).toBe(2));

    await user.click(screen.getByRole("button", { name: "Delete" }));
    await user.click(await screen.findByRole("button", { name: "Delete role" }));
    await waitFor(() => expect(myCapsInvalidations()).toBe(3));
    // And the capabilities were really asked for again.
    await waitFor(() => expect(mocked.myCaps.mock.calls.length).toBeGreaterThan(1));
    invalidated.mockRestore();
  });

  it("is only reachable with manage_users", () => {
    expect(canVisit("/roles", ["manage_users"])).toBe(true);
    expect(canVisit("/roles", ["edit_posts"])).toBe(false);
    expect(canVisit("/roles", [])).toBe(false);
  });
});
