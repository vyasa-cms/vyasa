import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createMemoryHistory, createRouter, RouterProvider } from "@tanstack/react-router";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { routeTree } from "@/routeTree.gen";
import { api } from "@/api/client";

vi.mock("@/api/client", () => ({
  api: {
    me: vi.fn(),
    login: vi.fn(),
    loginMfa: vi.fn(),
    logout: vi.fn(),
    setupStatus: vi.fn(),
    registrationInfo: vi.fn(),
    resendConfirmation: vi.fn(),
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

const mocked = api as unknown as Record<string, ReturnType<typeof vi.fn>>;

function makeApp() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const router = createRouter({
    routeTree,
    history: createMemoryHistory({ initialEntries: ["/admin/login"] }),
    context: { queryClient },
    basepath: "/admin",
  });
  render(
    <QueryClientProvider client={queryClient}>
      <RouterProvider router={router} context={{ queryClient }} />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.me!.mockRejectedValue(new Error("not signed in"));
  mocked.setupStatus!.mockResolvedValue({ needs_admin: false });
});

describe("sign-in page and registration", () => {
  it("offers Create an account only once GET /auth/registration says available", async () => {
    mocked.registrationInfo!.mockResolvedValue({ enabled: true, available: true, password_min_length: 8 });
    makeApp();
    expect(await screen.findByRole("link", { name: "Create an account" })).toHaveAttribute("href", "/admin/register");
  });

  it("offers no link and no confirm hint when registration is on but cannot work (no mail relay or site address)", async () => {
    const { ApiError } = await import("@/api/client");
    const user = userEvent.setup();
    mocked.registrationInfo!.mockResolvedValue({ enabled: true, available: false, password_min_length: 8 });
    mocked.login!.mockRejectedValue(new ApiError(401, { code: "unauthorized", message: "wrong email or password" }));
    makeApp();
    await user.type(await screen.findByTestId("login-email"), "new@example.com");
    await user.type(screen.getByTestId("login-password"), "whatever-1");
    await user.click(screen.getByRole("button", { name: /sign in/i }));
    expect(await screen.findByTestId("login-error")).toBeInTheDocument();
    expect(screen.queryByRole("link", { name: "Create an account" })).toBeNull();
    expect(screen.queryByTestId("confirm-hint")).toBeNull();
  });

  it("offers no such link when registration is disabled", async () => {
    mocked.registrationInfo!.mockResolvedValue({ enabled: false, available: false, password_min_length: 8 });
    makeApp();
    await screen.findByTestId("login-email");
    expect(screen.queryByRole("link", { name: "Create an account" })).toBeNull();
  });

  it("hints at confirming an unconfirmed account after a 401, and resends on request", async () => {
    const { ApiError } = await import("@/api/client");
    const user = userEvent.setup();
    mocked.registrationInfo!.mockResolvedValue({ enabled: true, available: true, password_min_length: 8 });
    mocked.login!.mockRejectedValue(new ApiError(401, { code: "unauthorized", message: "wrong email or password" }));
    mocked.resendConfirmation!.mockResolvedValue({ message: "sent" });
    makeApp();

    await user.type(await screen.findByTestId("login-email"), "new@example.com");
    await user.type(screen.getByTestId("login-password"), "whatever-1");
    await user.click(screen.getByRole("button", { name: /sign in/i }));

    expect(await screen.findByTestId("login-error")).toHaveTextContent("That email and password don't match an account.");
    expect(screen.getByTestId("confirm-hint")).toHaveTextContent("Created an account recently? Confirm your email first.");

    await user.click(screen.getByRole("button", { name: "Resend confirmation" }));
    const resendInput = screen.getByLabelText("Email to resend the confirmation to");
    expect(resendInput).toHaveValue("new@example.com");
    await user.click(screen.getByRole("button", { name: "Send" }));

    await waitFor(() => expect(mocked.resendConfirmation).toHaveBeenCalledWith("new@example.com"));
    expect(await screen.findByTestId("resend-confirmation-sent")).toHaveTextContent(
      "If that address needs confirming, we've sent a new link.",
    );
  });

  it("never lets an opened, emptied resend panel block the sign-in submit", async () => {
    // Regression: the resend panel's email input is `required`. It used to
    // share the sign-in form with the Sign in button, so emptying it made
    // that button's native validation silently refuse to submit at all.
    const { ApiError } = await import("@/api/client");
    const user = userEvent.setup();
    mocked.registrationInfo!.mockResolvedValue({ enabled: true, available: true, password_min_length: 8 });
    mocked.login!.mockRejectedValue(new ApiError(401, { code: "unauthorized", message: "wrong email or password" }));
    makeApp();

    await user.type(await screen.findByTestId("login-email"), "new@example.com");
    await user.type(screen.getByTestId("login-password"), "whatever-1");
    await user.click(screen.getByRole("button", { name: /sign in/i }));
    await screen.findByTestId("login-error");

    await user.click(screen.getByRole("button", { name: "Resend confirmation" }));
    const resendInput = screen.getByLabelText("Email to resend the confirmation to");
    await user.clear(resendInput);
    expect(resendInput).toHaveValue("");

    await user.click(screen.getByRole("button", { name: /sign in/i }));
    await waitFor(() => expect(mocked.login).toHaveBeenCalledTimes(2));
    expect(mocked.resendConfirmation).not.toHaveBeenCalled();
  });

  it("closes the resend panel on Cancel, back to the plain link", async () => {
    const { ApiError } = await import("@/api/client");
    const user = userEvent.setup();
    mocked.registrationInfo!.mockResolvedValue({ enabled: true, available: true, password_min_length: 8 });
    mocked.login!.mockRejectedValue(new ApiError(401, { code: "unauthorized", message: "wrong email or password" }));
    makeApp();

    await user.type(await screen.findByTestId("login-email"), "new@example.com");
    await user.type(screen.getByTestId("login-password"), "whatever-1");
    await user.click(screen.getByRole("button", { name: /sign in/i }));
    await screen.findByTestId("login-error");

    await user.click(screen.getByRole("button", { name: "Resend confirmation" }));
    expect(screen.getByLabelText("Email to resend the confirmation to")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByLabelText("Email to resend the confirmation to")).toBeNull();
    expect(screen.getByRole("button", { name: "Resend confirmation" })).toBeInTheDocument();
  });

  it("shows no confirm hint when registration is disabled, even on a 401", async () => {
    const { ApiError } = await import("@/api/client");
    const user = userEvent.setup();
    mocked.registrationInfo!.mockResolvedValue({ enabled: false, available: false, password_min_length: 8 });
    mocked.login!.mockRejectedValue(new ApiError(401, { code: "unauthorized", message: "wrong email or password" }));
    makeApp();

    await user.type(await screen.findByTestId("login-email"), "new@example.com");
    await user.type(screen.getByTestId("login-password"), "whatever-1");
    await user.click(screen.getByRole("button", { name: /sign in/i }));

    expect(await screen.findByTestId("login-error")).toBeInTheDocument();
    expect(screen.queryByTestId("confirm-hint")).toBeNull();
  });

  it("shows no confirm hint for a 429 (too many attempts is not an unconfirmed-account signal)", async () => {
    const { ApiError } = await import("@/api/client");
    const user = userEvent.setup();
    mocked.registrationInfo!.mockResolvedValue({ enabled: true, available: true, password_min_length: 8 });
    mocked.login!.mockRejectedValue(new ApiError(429, { code: "rate_limited", message: "too many attempts" }));
    makeApp();

    await user.type(await screen.findByTestId("login-email"), "new@example.com");
    await user.type(screen.getByTestId("login-password"), "whatever-1");
    await user.click(screen.getByRole("button", { name: /sign in/i }));

    expect(await screen.findByTestId("login-error")).toHaveTextContent("Too many attempts. Wait a minute and try again.");
    expect(screen.queryByTestId("confirm-hint")).toBeNull();
  });
});
