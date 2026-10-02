import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { api, ApiError } from "@/api/client";
import { ProfilePage } from "@/routes/_auth/profile/index";
import { TestRouter } from "./TestRouter";

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <TestRouter><ProfilePage /></TestRouter>
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.restoreAllMocks();
  vi.spyOn(api, "me").mockResolvedValue({ id: "1", display_name: "Ann", bio: "", role: "author" } as never);
  vi.spyOn(api, "listApiKeys").mockResolvedValue([]);
  vi.spyOn(api, "myCaps").mockResolvedValue([] as never);
  vi.spyOn(api, "mfaStatus").mockResolvedValue({ enabled: false } as never);
});

describe("profile password change", () => {
  it("sends the current password with a new one", async () => {
    const user = userEvent.setup();
    const update = vi.spyOn(api, "updateMe").mockResolvedValue(undefined);
    mount();
    await waitFor(() => expect(screen.getByLabelText("Display name")).toHaveValue("Ann"));
    await user.type(screen.getByLabelText("New password"), "brand-new-pass");
    await user.type(screen.getByLabelText("Again"), "brand-new-pass");
    const save = screen.getByRole("button", { name: "Save profile" });
    // Not without the current one.
    expect(save).toBeDisabled();
    await user.type(screen.getByLabelText("Current password"), "old-pass-123");
    await user.click(save);
    await waitFor(() => expect(update).toHaveBeenCalled());
    expect(update.mock.calls[0]?.[0]).toMatchObject({
      password: "brand-new-pass",
      current_password: "old-pass-123",
    });
  });

  it("shows a refused current password at the field", async () => {
    const user = userEvent.setup();
    vi.spyOn(api, "updateMe").mockRejectedValue(
      new ApiError(403, { code: "forbidden", message: "current password is incorrect" }),
    );
    mount();
    await waitFor(() => expect(screen.getByLabelText("Display name")).toHaveValue("Ann"));
    await user.type(screen.getByLabelText("Current password"), "wrong-pass");
    await user.type(screen.getByLabelText("New password"), "brand-new-pass");
    await user.type(screen.getByLabelText("Again"), "brand-new-pass");
    await user.click(screen.getByRole("button", { name: "Save profile" }));
    expect(await screen.findByText("current password is incorrect")).toBeInTheDocument();
  });

  it("does not send a current password when the password is unchanged", async () => {
    const user = userEvent.setup();
    const update = vi.spyOn(api, "updateMe").mockResolvedValue(undefined);
    mount();
    const name = await screen.findByLabelText("Display name");
    await waitFor(() => expect(name).toHaveValue("Ann"));
    await user.type(name, "e");
    await user.click(screen.getByRole("button", { name: "Save profile" }));
    await waitFor(() => expect(update).toHaveBeenCalled());
    expect(update.mock.calls[0]?.[0]).not.toHaveProperty("current_password");
    expect(update.mock.calls[0]?.[0]).not.toHaveProperty("password");
  });
});
