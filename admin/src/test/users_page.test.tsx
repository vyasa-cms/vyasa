import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { notify } from "@/components/ui/toast";
import { Route } from "@/routes/_auth/users/index";

vi.mock("@/components/ui/toast", () => ({
  notify: { success: vi.fn(), error: vi.fn(), info: vi.fn(), undo: vi.fn() },
}));

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: { ...original.api, me: vi.fn(), listUsers: vi.fn(), createUser: vi.fn(), setUserRole: vi.fn(), suspendUser: vi.fn(), sendResetLink: vi.fn(), deleteUser: vi.fn(), listRoles: vi.fn(), myCaps: vi.fn(), confirmUser: vi.fn(), resendUserConfirmation: vi.fn() },
  };
});
const mocked = vi.mocked(api);
const Page = Route.options.component as React.ComponentType;

const ALL_CAPS = [
  "edit_posts", "publish_posts", "edit_others", "delete_posts", "manage_categories",
  "moderate_comments", "upload_media", "manage_users", "manage_themes", "manage_plugins",
  "manage_options", "view_admin",
];

const admin = { id: "1", email: "a@x", username: "a", display_name: "Ada", role: "admin", role_name: "Administrator", custom_role: null, bio: "", created_at: new Date().toISOString(), last_login_at: new Date().toISOString(), suspended_at: null, has_password: true, email_verified: true };
const invited = { id: "2", email: "b@x", username: "b", display_name: "Bo", role: "author", role_name: "Author", custom_role: null, bio: "", created_at: new Date().toISOString(), last_login_at: null, suspended_at: null, has_password: false, email_verified: true };
const charlie = { id: "3", email: "c@x", username: "c", display_name: "Charlie", role: "author", role_name: "Author", custom_role: null, bio: "", created_at: new Date().toISOString(), last_login_at: null, suspended_at: null, has_password: true, email_verified: true };
const unconfirmed = { id: "4", email: "dana@x", username: "member-ab12cd", display_name: "Dana", role: "subscriber", role_name: "Subscriber", custom_role: null, bio: "", created_at: new Date().toISOString(), last_login_at: null, suspended_at: null, has_password: true, email_verified: false };

const BUILT_IN_ROLES = [
  { slug: "admin", name: "Administrator", description: "", capabilities: [], built_in: true, users: 1 },
  { slug: "editor", name: "Editor", description: "", capabilities: [], built_in: true, users: 0 },
  { slug: "author", name: "Author", description: "", capabilities: [], built_in: true, users: 2 },
  { slug: "contributor", name: "Contributor", description: "", capabilities: [], built_in: true, users: 0 },
  { slug: "subscriber", name: "Subscriber", description: "", capabilities: [], built_in: true, users: 0 },
];

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><ConfirmProvider><Page /></ConfirmProvider></QueryClientProvider>);
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.me.mockResolvedValue(admin as never);
  mocked.listUsers.mockResolvedValue({ items: [admin, invited] as never, total: 2 });
  mocked.listRoles.mockResolvedValue(BUILT_IN_ROLES as never);
  // Full capabilities by default, so every role in the control is grantable
  // unless a test narrows this to exercise the disabled-with-a-hint path.
  mocked.myCaps.mockResolvedValue([...ALL_CAPS]);
});

describe("users page", () => {
  it("shows status, last sign-in, all five roles, and resends an invitation", async () => {
    const user = userEvent.setup();
    mocked.sendResetLink.mockResolvedValue({ sent: "invitation" });
    mount();
    await screen.findByText("Ada");
    expect(screen.getByText("Invited")).toBeInTheDocument();
    expect(screen.getByText("never")).toBeInTheDocument();
    const roles = Array.from((screen.getAllByRole("combobox")[0] as HTMLSelectElement).options).map((o) => o.value);
    expect(roles).toEqual(["admin", "editor", "author", "contributor", "subscriber"]);
    await user.click(screen.getByRole("button", { name: "Resend invite" }));
    await waitFor(() => expect(mocked.sendResetLink).toHaveBeenCalledWith("2"));
  });

  it("asks the server to validate a demotion even when only one admin is visible", async () => {
    const user = userEvent.setup();
    mocked.setUserRole.mockRejectedValue(new Error("Cannot remove the last administrator"));
    mount();
    await screen.findByText("Ada");
    await user.selectOptions(screen.getAllByRole("combobox")[0]!, "editor");
    await user.click(screen.getByRole("button", { name: "Change my role" }));
    await waitFor(() => expect(mocked.setUserRole).toHaveBeenCalledWith("1", "editor"));
    await waitFor(() => expect(mocked.listUsers.mock.calls.length).toBeGreaterThan(1));
  });

  it("suspends someone else from the row, never yourself", async () => {
    const user = userEvent.setup();
    mocked.suspendUser.mockResolvedValue(undefined);
    mount();
    await screen.findByText("Ada");
    const suspendButtons = screen.getAllByRole("button", { name: "Suspend" });
    expect(suspendButtons).toHaveLength(1);
    await user.click(suspendButtons[0]!);
    await waitFor(() => expect(mocked.suspendUser).toHaveBeenCalledWith("2", true));
  });

  it("never offers to delete your own account (the server refuses it)", async () => {
    mount();
    await screen.findByText("Ada");
    // Only Bo's row has a Delete action; the signed-in user's own row has none.
    expect(screen.getAllByRole("button", { name: "Delete" })).toHaveLength(1);
  });

  it("creates an invitation when the password is left blank", async () => {
    const user = userEvent.setup();
    mocked.createUser.mockResolvedValue({} as never);
    mount();
    await user.click(await screen.findByRole("button", { name: "Add user" }));
    await user.type(screen.getByLabelText("Email address"), "c@x");
    await user.click(screen.getByRole("button", { name: "Send invitation" }));
    await waitFor(() => expect(mocked.createUser).toHaveBeenCalledWith({ email: "c@x", password: null, role: "subscriber", display_name: null }));
  });

  describe("deleting a user who owns content", () => {
    const CONFLICT_MESSAGE = "this user owns 3 post(s) and 1 media item(s); choose a user to reassign them to before deleting";

    function conflictOnce() {
      mocked.deleteUser.mockImplementationOnce(() => {
        throw new ApiError(409, { code: "conflict", message: CONFLICT_MESSAGE });
      });
    }

    it("shows the server's message and a select, then retries the delete with reassign_to", async () => {
      const user = userEvent.setup();
      conflictOnce();
      mocked.deleteUser.mockResolvedValueOnce(undefined);
      mocked.listUsers
        .mockResolvedValueOnce({ items: [admin, invited], total: 2 }) // page load
        .mockResolvedValueOnce({ items: [admin, invited], total: 2 }) // dialog's own candidate query
        .mockResolvedValueOnce({ items: [admin], total: 1 }); // page refetch after the delete succeeds
      mount();
      await screen.findByText("Ada");
      await screen.findByText("Bo");

      await user.click(screen.getByRole("button", { name: "Delete" }));
      await user.click(await screen.findByRole("button", { name: "Delete account" }));

      expect(await screen.findByText(CONFLICT_MESSAGE)).toBeInTheDocument();
      const select = await screen.findByLabelText("New owner") as HTMLSelectElement;
      // Defaults to the signed-in user (Ada) and offers no way to reassign to Bo herself.
      expect(select.value).toBe("1");
      expect(Array.from(select.options).map((o) => o.value)).toEqual(["1"]);

      await user.click(screen.getByRole("button", { name: "Reassign and delete" }));

      await waitFor(() => expect(mocked.deleteUser).toHaveBeenCalledTimes(2));
      expect(mocked.deleteUser).toHaveBeenNthCalledWith(1, "2", undefined);
      expect(mocked.deleteUser).toHaveBeenNthCalledWith(2, "2", "1");
      await waitFor(() => expect(screen.queryByText("Bo")).not.toBeInTheDocument());
      expect(screen.queryByText(CONFLICT_MESSAGE)).not.toBeInTheDocument();
    });

    it("deletes nothing when the reassign dialog is cancelled", async () => {
      const user = userEvent.setup();
      conflictOnce();
      mount();
      await screen.findByText("Ada");

      await user.click(screen.getByRole("button", { name: "Delete" }));
      await user.click(await screen.findByRole("button", { name: "Delete account" }));
      await screen.findByText(CONFLICT_MESSAGE);

      await user.click(screen.getByRole("button", { name: "Cancel" }));

      expect(screen.queryByText(CONFLICT_MESSAGE)).not.toBeInTheDocument();
      expect(mocked.deleteUser).toHaveBeenCalledTimes(1);
      expect(screen.getByText("Bo")).toBeInTheDocument();
    });

    it("deletes a user with no content in a single call, no dialog", async () => {
      const user = userEvent.setup();
      mocked.deleteUser.mockResolvedValueOnce(undefined);
      mount();
      await screen.findByText("Ada");

      await user.click(screen.getByRole("button", { name: "Delete" }));
      await user.click(await screen.findByRole("button", { name: "Delete account" }));

      await waitFor(() => expect(mocked.deleteUser).toHaveBeenCalledWith("2", undefined));
      expect(mocked.deleteUser).toHaveBeenCalledTimes(1);
      expect(screen.queryByText("Reassign their content to…")).not.toBeInTheDocument();
    });

    it("still offers the signed-in user as the default target when the page's own search has narrowed to just the target", async () => {
      const user = userEvent.setup();
      conflictOnce();
      mocked.deleteUser.mockResolvedValueOnce(undefined);
      // The PAGE's search is scoped to "bo" and returns only Bo — the classic
      // trap: if the dialog reused these rows as its candidates, there would
      // be nobody left to reassign to and Confirm would be stuck disabled.
      // The dialog's own query (always searching "" first) must still see
      // everyone else.
      mocked.listUsers.mockImplementation(async (params?: { q?: string }) => {
        if (params?.q === "bo") return { items: [invited], total: 1 };
        return { items: [admin, invited], total: 2 };
      });
      mount();
      await screen.findByText("Ada");

      await user.type(screen.getByPlaceholderText("Search by name, email or username"), "bo");
      await waitFor(() => expect(screen.queryByText("Ada")).not.toBeInTheDocument());
      await screen.findByText("Bo");

      await user.click(screen.getByRole("button", { name: "Delete" }));
      await user.click(await screen.findByRole("button", { name: "Delete account" }));

      const select = await screen.findByLabelText("New owner") as HTMLSelectElement;
      expect(select.value).toBe("1");
      expect(Array.from(select.options).map((o) => o.textContent)).toEqual(["Ada (you)"]);

      await user.click(screen.getByRole("button", { name: "Reassign and delete" }));
      await waitFor(() => expect(mocked.deleteUser).toHaveBeenNthCalledWith(2, "2", "1"));
    });

    it("queries the API when typing in the dialog's own search box, independent of the page's search", async () => {
      const user = userEvent.setup();
      conflictOnce();
      mocked.listUsers.mockImplementation(async (params?: { q?: string }) => {
        if (params?.q === "charlie") return { items: [charlie], total: 1 };
        return { items: [admin, invited], total: 2 };
      });
      mount();
      await screen.findByText("Ada");

      await user.click(screen.getByRole("button", { name: "Delete" }));
      await user.click(await screen.findByRole("button", { name: "Delete account" }));
      await screen.findByLabelText("Search for a user");

      await user.type(screen.getByLabelText("Search for a user"), "charlie");
      await waitFor(() => expect(mocked.listUsers).toHaveBeenCalledWith({ q: "charlie", page: 1, per_page: 50 }));
      const select = await screen.findByLabelText("New owner") as HTMLSelectElement;
      expect(Array.from(select.options).map((o) => o.textContent)).toContain("Charlie");
    });

    it("shows a message instead of a silently disabled button when there is no one else to reassign to", async () => {
      const user = userEvent.setup();
      // A signed-in identity the app doesn't (yet) know, and a directory
      // that, once the target is excluded, has nobody left.
      mocked.me.mockRejectedValue(new Error("not signed in"));
      mocked.listUsers.mockResolvedValue({ items: [invited], total: 1 });
      conflictOnce();
      mount();
      await screen.findByText("Bo");

      await user.click(screen.getByRole("button", { name: "Delete" }));
      await user.click(await screen.findByRole("button", { name: "Delete account" }));

      expect(await screen.findByText("There's no one else to reassign this to.")).toBeInTheDocument();
      expect(screen.queryByLabelText("New owner")).not.toBeInTheDocument();
      expect(screen.getByRole("button", { name: "Reassign and delete" })).toBeDisabled();
    });

    it("keeps the dialog open and usable when the retry itself fails", async () => {
      const user = userEvent.setup();
      conflictOnce();
      mocked.deleteUser.mockImplementationOnce(() => {
        throw new ApiError(404, { code: "user_not_found", message: "user not found: 1" });
      });
      mocked.deleteUser.mockResolvedValueOnce(undefined);
      mocked.listUsers.mockResolvedValue({ items: [admin, invited, charlie], total: 3 });
      mount();
      await screen.findByText("Ada");

      // Bo's row: index 0 among the (self excluded) Delete buttons.
      await user.click(screen.getAllByRole("button", { name: "Delete" })[0]!);
      await user.click(await screen.findByRole("button", { name: "Delete account" }));
      await screen.findByText(CONFLICT_MESSAGE);

      // First retry: the chosen target no longer exists.
      await user.click(screen.getByRole("button", { name: "Reassign and delete" }));
      expect(await screen.findByText("user not found: 1")).toBeInTheDocument();
      // The dialog is still open and usable: pick someone else and retry.
      expect(screen.getByText(CONFLICT_MESSAGE)).toBeInTheDocument();
      const select = screen.getByLabelText("New owner") as HTMLSelectElement;
      await user.selectOptions(select, "3");
      await user.click(screen.getByRole("button", { name: "Reassign and delete" }));

      await waitFor(() => expect(mocked.deleteUser).toHaveBeenCalledTimes(3));
      expect(mocked.deleteUser).toHaveBeenNthCalledWith(3, "2", "3");
      await waitFor(() => expect(screen.queryByText(CONFLICT_MESSAGE)).not.toBeInTheDocument());
    });

    it("keeps the dialog's search text and chosen target across a failing retry", async () => {
      const user = userEvent.setup();
      conflictOnce();
      mocked.deleteUser.mockImplementationOnce(() => {
        throw new ApiError(404, { code: "user_not_found", message: "user not found: 3" });
      });
      mocked.deleteUser.mockResolvedValueOnce(undefined);
      mocked.listUsers.mockImplementation(async (params?: { q?: string }) => {
        if (params?.q === "char") return { items: [charlie], total: 1 };
        return { items: [admin, invited, charlie], total: 3 };
      });
      mount();
      await screen.findByText("Ada");

      // Bo's row: index 0 among the (self excluded) Delete buttons.
      await user.click(screen.getAllByRole("button", { name: "Delete" })[0]!);
      await user.click(await screen.findByRole("button", { name: "Delete account" }));
      await screen.findByText(CONFLICT_MESSAGE);

      const search = await screen.findByLabelText("Search for a user");
      await user.type(search, "char");
      await waitFor(() => expect(mocked.listUsers).toHaveBeenCalledWith({ q: "char", page: 1, per_page: 50 }));
      const select = (await screen.findByLabelText("New owner")) as HTMLSelectElement;
      await user.selectOptions(select, "3");
      expect(select.value).toBe("3");

      // A retry that fails must not wipe what was already typed and chosen.
      await user.click(screen.getByRole("button", { name: "Reassign and delete" }));
      expect(await screen.findByText("user not found: 3")).toBeInTheDocument();
      expect(screen.getByLabelText("Search for a user")).toHaveValue("char");
      expect(screen.getByLabelText("New owner")).toHaveValue("3");
      expect(screen.getByText(CONFLICT_MESSAGE)).toBeInTheDocument();

      // Retrying the same target again sends the same reassign_to.
      await user.click(screen.getByRole("button", { name: "Reassign and delete" }));
      await waitFor(() => expect(mocked.deleteUser).toHaveBeenCalledTimes(3));
      expect(mocked.deleteUser).toHaveBeenNthCalledWith(2, "2", "3");
      expect(mocked.deleteUser).toHaveBeenNthCalledWith(3, "2", "3");
    });

    it("starts clean when reopened for a different user", async () => {
      const user = userEvent.setup();
      const OTHER_MESSAGE = "this user owns 1 post(s) and 0 media item(s); choose a user to reassign them to before deleting";
      mocked.deleteUser.mockImplementationOnce(() => {
        throw new ApiError(409, { code: "conflict", message: CONFLICT_MESSAGE });
      });
      mocked.deleteUser.mockImplementationOnce(() => {
        throw new ApiError(409, { code: "conflict", message: OTHER_MESSAGE });
      });
      mocked.listUsers.mockResolvedValue({ items: [admin, invited, charlie], total: 3 });
      mount();
      await screen.findByText("Ada");

      // Open for Bo: type a search, pick a non-default target, then cancel.
      await user.click(screen.getAllByRole("button", { name: "Delete" })[0]!);
      await user.click(await screen.findByRole("button", { name: "Delete account" }));
      await screen.findByText(CONFLICT_MESSAGE);
      await user.type(screen.getByLabelText("Search for a user"), "char");
      const select1 = (await screen.findByLabelText("New owner")) as HTMLSelectElement;
      await user.selectOptions(select1, "3");
      await user.click(screen.getByRole("button", { name: "Cancel" }));
      expect(screen.queryByText(CONFLICT_MESSAGE)).not.toBeInTheDocument();

      // Open for Charlie: the leftovers from Bo's attempt are gone.
      await user.click(screen.getAllByRole("button", { name: "Delete" })[1]!);
      await user.click(await screen.findByRole("button", { name: "Delete account" }));
      await screen.findByText(OTHER_MESSAGE);
      expect(screen.getByLabelText("Search for a user")).toHaveValue("");
      const select2 = (await screen.findByLabelText("New owner")) as HTMLSelectElement;
      expect(select2.value).toBe("1");
    });
  });

  describe("custom roles", () => {
    const DRAFTER = { slug: "drafter", name: "Drafter", description: "", capabilities: ["edit_posts", "upload_media"], built_in: false, users: 1 };

    it("offers custom roles in the role control alongside the built-in ones", async () => {
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      mount();
      await screen.findByText("Ada");
      const select = screen.getAllByRole("combobox")[0] as HTMLSelectElement;
      const options = Array.from(select.options).map((o) => ({ value: o.value, label: o.textContent }));
      expect(options).toEqual(expect.arrayContaining([{ value: "drafter", label: "Drafter" }]));
      // Still every built-in, unchanged.
      expect(options.map((o) => o.value)).toEqual(expect.arrayContaining(["admin", "editor", "author", "contributor", "subscriber"]));
    });

    it("shows the custom role's name on the row, distinguished from a built-in role", async () => {
      const drafterUser = { ...invited, id: "4", display_name: "Deepa", role: "subscriber", custom_role: "drafter", role_name: "Drafter" };
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      mocked.listUsers.mockResolvedValue({ items: [admin, drafterUser] as never, total: 2 });
      mount();
      await screen.findByText("Deepa");
      expect(screen.getByText("Drafter (custom)")).toBeInTheDocument();
      // The row's select shows the custom role selected, not "Subscriber".
      const rows = screen.getAllByRole("combobox");
      const deepaSelect = rows.find((el) => (el as HTMLSelectElement).value === "drafter") as HTMLSelectElement;
      expect(deepaSelect).toBeDefined();
    });

    it("sends the custom role's slug when it is chosen from the row", async () => {
      const user = userEvent.setup();
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      mocked.setUserRole.mockResolvedValue(undefined);
      mount();
      await screen.findByText("Bo");
      const boSelect = screen.getAllByRole("combobox")[1]!;
      await user.selectOptions(boSelect, "drafter");
      await waitFor(() => expect(mocked.setUserRole).toHaveBeenCalledWith("2", "drafter"));
    });

    it("refreshes the signed-in user's own capabilities after a role assignment", async () => {
      // The account whose role changed may be the one signed in.
      const user = userEvent.setup();
      const invalidated = vi.spyOn(QueryClient.prototype, "invalidateQueries");
      const myCapsInvalidations = () => invalidated.mock.calls.filter(([filters]) => JSON.stringify(filters?.queryKey) === '["my-caps"]').length;
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      mocked.setUserRole.mockResolvedValue(undefined);
      mount();
      await screen.findByText("Bo");
      expect(myCapsInvalidations()).toBe(0);
      await user.selectOptions(screen.getAllByRole("combobox")[1]!, "drafter");
      await waitFor(() => expect(mocked.setUserRole).toHaveBeenCalledWith("2", "drafter"));
      await waitFor(() => expect(myCapsInvalidations()).toBe(1));
      invalidated.mockRestore();
    });

    it("shows the server's message, not a generic one, and leaves the control on the real role, when a role assignment is refused", async () => {
      const user = userEvent.setup();
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      mocked.setUserRole.mockRejectedValue(new ApiError(403, { code: "forbidden", message: "this role holds capabilities you do not have: manage_options" }));
      mount();
      await screen.findByText("Bo");
      const boSelect = screen.getAllByRole("combobox")[1] as HTMLSelectElement;
      await user.selectOptions(boSelect, "drafter");
      await waitFor(() => expect(vi.mocked(notify).error).toHaveBeenCalledWith(
        "Couldn't change the role",
        expect.objectContaining({ message: "this role holds capabilities you do not have: manage_options" }),
      ));
      // No optimistic state left behind: once the refusal settles, the
      // control shows Bo's real, unchanged role ("author"), not "drafter".
      await waitFor(() => expect(boSelect.value).toBe("author"));
    });

    it("refetches roles too after a role change, successful or refused, so user counts stay current", async () => {
      const user = userEvent.setup();
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      // listUsers always answers with Bo still at "author" (the mock data
      // doesn't actually move), so the control reverts to "author" after
      // each attempt — meaning picking "drafter" again is a genuine change
      // both times, not a no-op the second time around.
      mocked.setUserRole.mockResolvedValueOnce(undefined).mockRejectedValueOnce(new Error("nope"));
      mount();
      await screen.findByText("Bo");
      const initialRoleCalls = mocked.listRoles.mock.calls.length;

      await user.selectOptions(screen.getAllByRole("combobox")[1]!, "drafter");
      await waitFor(() => expect(mocked.setUserRole).toHaveBeenCalledTimes(1));
      await waitFor(() => expect(mocked.listRoles.mock.calls.length).toBeGreaterThan(initialRoleCalls));

      const afterSuccess = mocked.listRoles.mock.calls.length;
      await waitFor(() => expect((screen.getAllByRole("combobox")[1] as HTMLSelectElement).value).toBe("author"));
      await user.selectOptions(screen.getAllByRole("combobox")[1]!, "drafter");
      await waitFor(() => expect(mocked.setUserRole).toHaveBeenCalledTimes(2));
      await waitFor(() => expect(mocked.listRoles.mock.calls.length).toBeGreaterThan(afterSuccess));
    });

    it("disables a role the signed-in user can't grant, with a hint, in the row control", async () => {
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      // Holds neither edit_posts nor upload_media, so Drafter is out of reach.
      mocked.myCaps.mockResolvedValue(["view_admin", "manage_users"]);
      mount();
      await screen.findByText("Bo");
      const boSelect = screen.getAllByRole("combobox")[1] as HTMLSelectElement;
      const drafterOption = Array.from(boSelect.options).find((o) => o.value === "drafter") as HTMLOptionElement;
      expect(drafterOption.disabled).toBe(true);
      expect(drafterOption.title).toMatch(/can.t grant it/);
      // A role fully within reach stays selectable.
      const authorOption = Array.from(boSelect.options).find((o) => o.value === "author") as HTMLOptionElement;
      expect(authorOption.disabled).toBe(false);
    });

    it("offers custom roles in the new-user form too, and disables ones the signed-in user can't grant", async () => {
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      mocked.myCaps.mockResolvedValue(["view_admin", "manage_users"]);
      const user = userEvent.setup();
      mount();
      await user.click(await screen.findByRole("button", { name: "Add user" }));
      const roleSelect = screen.getByLabelText("Role") as HTMLSelectElement;
      const drafterOption = Array.from(roleSelect.options).find((o) => o.value === "drafter") as HTMLOptionElement;
      expect(drafterOption).toBeDefined();
      expect(drafterOption.disabled).toBe(true);
      expect(drafterOption.title).toMatch(/can.t grant it/);
    });

    it("sends a custom role's slug from the new-user form", async () => {
      const user = userEvent.setup();
      mocked.listRoles.mockResolvedValue([...BUILT_IN_ROLES, DRAFTER] as never);
      mocked.createUser.mockResolvedValue({} as never);
      mount();
      await user.click(await screen.findByRole("button", { name: "Add user" }));
      await user.type(screen.getByLabelText("Email address"), "d@x");
      await user.selectOptions(screen.getByLabelText("Role"), "drafter");
      await user.click(screen.getByRole("button", { name: "Send invitation" }));
      await waitFor(() => expect(mocked.createUser).toHaveBeenCalledWith({ email: "d@x", password: null, role: "drafter", display_name: null }));
    });
  });
});

describe("email confirmation", () => {
  it("badges an unconfirmed account and offers Confirm and Resend confirmation only for it", async () => {
    mocked.listUsers.mockResolvedValue({ items: [admin, unconfirmed], total: 2 } as never);
    mount();
    await screen.findByText("Dana");
    expect(screen.getByText("Unconfirmed")).toBeInTheDocument();
    // Ada is confirmed: no badge, no row actions for it.
    expect(screen.getAllByText("Confirm")).toHaveLength(1);
    expect(screen.getAllByText("Resend confirmation")).toHaveLength(1);
  });

  it("asks before confirming, and only confirms on a yes", async () => {
    const user = userEvent.setup();
    mocked.listUsers.mockResolvedValue({ items: [admin, unconfirmed], total: 2 } as never);
    mocked.confirmUser.mockResolvedValue({ ...unconfirmed, email_verified: true } as never);
    mount();
    await screen.findByText("Dana");
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    expect(await screen.findByText("Confirm Dana's email?")).toBeInTheDocument();
    // Says what confirming now does to the password, and no longer warns
    // that a stranger's password is kept (it is not).
    expect(screen.getByText(/the person will be sent a link to set their password/)).toBeInTheDocument();
    expect(screen.getByText(/password they signed up with is removed/)).toBeInTheDocument();
    expect(screen.queryByText(/keeps whatever password/)).toBeNull();
    expect(mocked.confirmUser).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Confirm email" }));
    await waitFor(() => expect(mocked.confirmUser).toHaveBeenCalledWith("4"));
    expect(notify.success).toHaveBeenCalledWith("Email confirmed", "They've been sent a link to set their password.");
  });

  it("says so when the account was confirmed but the set-password email could not be sent", async () => {
    const user = userEvent.setup();
    mocked.listUsers.mockResolvedValue({ items: [admin, unconfirmed], total: 2 } as never);
    // Confirmed, password removed, and the link not queued.
    mocked.confirmUser.mockResolvedValue({ ...unconfirmed, email_verified: true, has_password: false, link_sent: false } as never);
    mount();
    await screen.findByText("Dana");
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    await user.click(await screen.findByRole("button", { name: "Confirm email" }));
    await waitFor(() => expect(mocked.confirmUser).toHaveBeenCalledWith("4"));
    // Not the success that promises a link: the account has no password and
    // no link, and the row action that sends one is named.
    await waitFor(() => expect(notify.error).toHaveBeenCalledWith(
      "Confirmed, but the set-password email could not be sent",
      "Use “Resend invite” to send them a link to set their password.",
    ));
    expect(notify.success).not.toHaveBeenCalled();
  });

  it("does not confirm when the dialog is cancelled", async () => {
    const user = userEvent.setup();
    mocked.listUsers.mockResolvedValue({ items: [admin, unconfirmed], total: 2 } as never);
    mount();
    await screen.findByText("Dana");
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    await screen.findByText("Confirm Dana's email?");
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(mocked.confirmUser).not.toHaveBeenCalled();
  });

  it("resends a confirmation email without a dialog, and shows the server's 403 on reach", async () => {
    const user = userEvent.setup();
    mocked.listUsers.mockResolvedValue({ items: [admin, unconfirmed], total: 2 } as never);
    mocked.resendUserConfirmation.mockRejectedValue(new ApiError(403, { code: "forbidden", message: "that account is beyond your reach" }));
    mount();
    await screen.findByText("Dana");
    await user.click(screen.getByRole("button", { name: "Resend confirmation" }));
    await waitFor(() => expect(mocked.resendUserConfirmation).toHaveBeenCalledWith("4"));
    expect(notify.error).toHaveBeenCalledWith("Couldn't resend the confirmation email", expect.any(ApiError));
  });
});
