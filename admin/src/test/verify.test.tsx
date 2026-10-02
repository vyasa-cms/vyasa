import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api, ApiError } from "@/api/client";

vi.mock("@/api/client", () => ({
  api: {
    verifyEmail: vi.fn(),
    resendConfirmation: vi.fn(),
    me: vi.fn(),
    myCaps: vi.fn(),
  },
  ApiError: class ApiError extends Error {
    status: number;
    code: string;
    constructor(status: number, body: { message?: string; code?: string } | null) {
      super(body?.message ?? `request failed with ${status}`);
      this.status = status;
      this.code = body?.code ?? "unknown";
    }
  },
}));

const mocked = api as unknown as {
  verifyEmail: ReturnType<typeof vi.fn>;
  resendConfirmation: ReturnType<typeof vi.fn>;
  me: ReturnType<typeof vi.fn>;
  myCaps: ReturnType<typeof vi.fn>;
};

function mountAt(path: string) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: [path] }),
    context: { queryClient },
    basepath: "/admin",
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} context={{ queryClient }} />
    </QueryClientProvider>,
  );
}

const SECONDARY = "I don't know the password — email me a link to set one";

beforeEach(() => {
  vi.clearAllMocks();
  mocked.me.mockRejectedValue(new Error("not signed in"));
});

describe("verify page", () => {
  it("posts nothing on arrival: a link scanner that opens the page confirms nothing", async () => {
    mountAt("/admin/verify?token=abc123");
    expect(await screen.findByRole("heading", { name: "Confirm your email" })).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Confirm" })).toBeDisabled();
    expect(screen.getByRole("button", { name: SECONDARY })).toBeInTheDocument();
    // Someone who did not sign up is not asked to confirm anything.
    expect(screen.getByTestId("verify-not-me")).toHaveTextContent("If you didn't sign up, you can simply close this page.");
    expect(screen.queryByText(/I didn't sign up/)).toBeNull();
    // Give any effect a chance to fire.
    await new Promise((r) => setTimeout(r, 50));
    expect(mocked.verifyEmail).not.toHaveBeenCalled();
  });

  it("confirms with the password chosen at sign-up, and shows a generic success", async () => {
    const user = userEvent.setup();
    mocked.verifyEmail.mockResolvedValue(undefined);
    mountAt("/admin/verify?token=abc123");

    await user.type(await screen.findByLabelText("Password"), "my-own-password");
    await user.click(screen.getByRole("button", { name: "Confirm" }));

    await waitFor(() => expect(mocked.verifyEmail).toHaveBeenCalledWith("abc123", "my-own-password"));
    expect(mocked.verifyEmail).toHaveBeenCalledTimes(1);
    expect(await screen.findByText("Your email is confirmed")).toBeInTheDocument();
    expect(screen.getByTestId("verify-done")).toHaveTextContent(
      "If you confirmed with your password, you can sign in now. Otherwise, check your inbox: we've sent a link to set a password.",
    );
    expect(screen.getByRole("button", { name: "Sign in" })).toBeInTheDocument();
  });

  it("confirms without a password from the secondary action, with the same success copy", async () => {
    const user = userEvent.setup();
    mocked.verifyEmail.mockResolvedValue(undefined);
    mountAt("/admin/verify?token=abc123");

    await user.click(await screen.findByRole("button", { name: SECONDARY }));

    await waitFor(() => expect(mocked.verifyEmail).toHaveBeenCalledWith("abc123"));
    expect(mocked.verifyEmail.mock.calls[0]).toHaveLength(1);
    expect(await screen.findByText("Your email is confirmed")).toBeInTheDocument();
    expect(screen.getByTestId("verify-done")).toHaveTextContent("check your inbox: we've sent a link to set a password.");
  });

  it("shows a wrong password inline and lets the person try again", async () => {
    const user = userEvent.setup();
    mocked.verifyEmail
      .mockRejectedValueOnce(new ApiError(400, { code: "password_mismatch", message: "that is not the password this sign-up was made with" }))
      .mockResolvedValueOnce(undefined);
    mountAt("/admin/verify?token=abc123");

    const field = await screen.findByLabelText("Password");
    await user.type(field, "wrong-guess");
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    expect(await screen.findByTestId("verify-mismatch")).toHaveTextContent(
      "That isn't the password this account was created with. Try again, or use the option below.",
    );
    // Still the form, not the dead-link screen.
    expect(screen.queryByText("This confirmation link is invalid or has expired")).toBeNull();

    await user.clear(field);
    await user.type(field, "right-one");
    await user.click(screen.getByRole("button", { name: "Confirm" }));
    await waitFor(() => expect(mocked.verifyEmail).toHaveBeenLastCalledWith("abc123", "right-one"));
    expect(await screen.findByText("Your email is confirmed")).toBeInTheDocument();
  });

  it("shows a failure message and a resend form for a bad token, and sends a generic resend", async () => {
    const user = userEvent.setup();
    mocked.verifyEmail.mockRejectedValue(new ApiError(400, { code: "validation_failed", message: "this confirmation link is invalid or has expired" }));
    mocked.resendConfirmation.mockResolvedValue({ message: "sent" });
    mountAt("/admin/verify?token=expired");

    await user.click(await screen.findByRole("button", { name: SECONDARY }));
    expect(await screen.findByText("This confirmation link is invalid or has expired")).toBeInTheDocument();
    await user.type(screen.getByLabelText("Email address"), "person@example.com");
    await user.click(screen.getByRole("button", { name: "Send a new link" }));

    await waitFor(() => expect(mocked.resendConfirmation).toHaveBeenCalledWith("person@example.com"));
    expect(await screen.findByTestId("resend-sent")).toHaveTextContent(
      "If that address needs confirming, we've sent a new link. It works for 24 hours.",
    );
  });

  it("shows the resend form directly when there is no token at all, without calling verify", async () => {
    mountAt("/admin/verify");
    expect(await screen.findByText("This confirmation link is invalid or has expired")).toBeInTheDocument();
    expect(screen.getByLabelText("Email address")).toBeInTheDocument();
    expect(mocked.verifyEmail).not.toHaveBeenCalled();
  });
});
