import { createRootRoute, createRoute, createRouter, createMemoryHistory, Outlet, RouterProvider, useNavigate } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it } from "vitest";

import { ConfirmProvider } from "@/components/ui/dialog";
import { useUnsavedChanges } from "@/lib/use-unsaved-changes";

function Editor() {
  const navigate = useNavigate();
  const canLeave = useUnsavedChanges(() => true);
  return (
    <button type="button" onClick={() => void canLeave.leave(() => void navigate({ to: "/login" }))}>
      Back
    </button>
  );
}

function makeRouter() {
  const root = createRootRoute({ component: () => <ConfirmProvider><Outlet /></ConfirmProvider> });
  const editor = createRoute({ getParentRoute: () => root, path: "/", component: Editor });
  const away = createRoute({ getParentRoute: () => root, path: "/login", component: () => <p>Away</p> });
  return createRouter({
    routeTree: root.addChildren([editor, away]),
    history: createMemoryHistory({ initialEntries: ["/"] }),
  });
}

describe("leaving with unsaved changes", () => {
  it("asks once when an explicit exit already confirmed", async () => {
    const user = userEvent.setup();
    const router = makeRouter();
    render(<RouterProvider router={router} />);
    await user.click(await screen.findByRole("button", { name: "Back" }));
    expect(await screen.findByText("Leave without saving?")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Leave" }));
    expect(await screen.findByText("Away")).toBeInTheDocument();
    // The route blocker did not ask a second time.
    await waitFor(() => expect(screen.queryByText("Leave without saving?")).toBeNull());
  });

  it("still asks on ordinary navigation", async () => {
    const user = userEvent.setup();
    const router = makeRouter();
    render(<RouterProvider router={router} />);
    await screen.findByRole("button", { name: "Back" });
    void router.navigate({ to: "/login" });
    expect(await screen.findByText("Leave without saving?")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Leave" }));
    expect(await screen.findByText("Away")).toBeInTheDocument();
  });
});
