import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, ApiError } from "@/api/client";
import { Route } from "@/routes/register";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: { ...original.api, registrationInfo: vi.fn(), register: vi.fn() },
  };
});
const mocked = vi.mocked(api);
const Page = Route.options.component as React.ComponentType;

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <Page />
    </QueryClientProvider>,
  );
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.registrationInfo.mockResolvedValue({ enabled: true, available: true, password_min_length: 8 });
});

describe("register page", () => {
  it("collects a display name, email and password, with a honeypot hidden from people and assistive tech", async () => {
    mount();
    expect(await screen.findByLabelText("Display name")).toBeInTheDocument();
    expect(screen.getByLabelText("Email address")).toBeInTheDocument();
    expect(screen.getByLabelText("Password")).toBeInTheDocument();
    expect(screen.getByLabelText("Confirm password")).toBeInTheDocument();

    const honeypot = screen.getByTestId("register-honeypot") as HTMLInputElement;
    expect(honeypot.name).toBe("website");
    expect(honeypot.getAttribute("aria-hidden")).toBe("true");
    expect(honeypot.tabIndex).toBe(-1);
    expect(honeypot.autocomplete).toBe("off");
    // Off-screen, not `display: none`: some bots skip filling a field that
    // isn't rendered at all, defeating the trap.
    expect(honeypot.style.position).toBe("absolute");
    expect(honeypot.style.display).not.toBe("none");
  });

  it("blocks submission on a short or mismatched password", async () => {
    const user = userEvent.setup();
    mount();
    await user.type(await screen.findByLabelText("Email address"), "a@example.com");
    await user.type(screen.getByLabelText("Password"), "short");
    await user.type(screen.getByLabelText("Confirm password"), "short");
    expect(screen.getByRole("button", { name: "Create account" })).toBeDisabled();

    await user.clear(screen.getByLabelText("Password"));
    await user.type(screen.getByLabelText("Password"), "longenough1");
    await user.clear(screen.getByLabelText("Confirm password"));
    await user.type(screen.getByLabelText("Confirm password"), "different1");
    expect(screen.getByText("The two passwords differ.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Create account" })).toBeDisabled();
    expect(mocked.register).not.toHaveBeenCalled();
  });

  it("always shows the same check-your-email screen on a well-formed submission, honeypot included", async () => {
    const user = userEvent.setup();
    mocked.register.mockResolvedValue({ message: "sent" });
    mount();
    await user.type(await screen.findByLabelText("Display name"), "Ada Lovelace");
    await user.type(screen.getByLabelText("Email address"), "ada@example.com");
    await user.type(screen.getByLabelText("Password"), "longenough1");
    await user.type(screen.getByLabelText("Confirm password"), "longenough1");
    // A bot would fill this too; a person never reaches it, so this is set
    // directly rather than through userEvent's visibility-checked typing.
    fireEvent.change(screen.getByTestId("register-honeypot"), { target: { value: "https://spam.example" } });
    await user.click(screen.getByRole("button", { name: "Create account" }));

    await waitFor(() =>
      expect(mocked.register).toHaveBeenCalledWith({
        email: "ada@example.com",
        display_name: "Ada Lovelace",
        password: "longenough1",
        website: "https://spam.example",
      }),
    );
    expect(await screen.findByText("Check your email")).toBeInTheDocument();
    expect(screen.getByText(/ada@example.com/)).toBeInTheDocument();
  });

  it("shows a closed message from GET /auth/registration without letting the form submit", async () => {
    mocked.registrationInfo.mockResolvedValue({ enabled: false, available: false, password_min_length: 8 });
    mount();
    expect(await screen.findByText("Not taking new accounts")).toBeInTheDocument();
    expect(screen.queryByLabelText("Email address")).toBeNull();
  });

  it("shows the closed message when registration is on but cannot work right now", async () => {
    mocked.registrationInfo.mockResolvedValue({ enabled: true, available: false, password_min_length: 8 });
    mount();
    expect(await screen.findByText("Not taking new accounts")).toBeInTheDocument();
    expect(screen.queryByLabelText("Email address")).toBeNull();
  });

  it("shows a distinct message for 403 registration_closed from a submit, even though GET said enabled", async () => {
    const user = userEvent.setup();
    mocked.register.mockRejectedValue(new ApiError(403, { code: "registration_closed", message: "this site does not take registrations" }));
    mount();
    await user.type(await screen.findByLabelText("Email address"), "a@example.com");
    await user.type(screen.getByLabelText("Password"), "longenough1");
    await user.type(screen.getByLabelText("Confirm password"), "longenough1");
    await user.click(screen.getByRole("button", { name: "Create account" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This site isn't taking new accounts right now.");
  });

  it("shows a distinct message for 503 registration_unavailable", async () => {
    const user = userEvent.setup();
    mocked.register.mockRejectedValue(new ApiError(503, { code: "registration_unavailable", message: "registration is not available right now; try again later" }));
    mount();
    await user.type(await screen.findByLabelText("Email address"), "a@example.com");
    await user.type(screen.getByLabelText("Password"), "longenough1");
    await user.type(screen.getByLabelText("Confirm password"), "longenough1");
    await user.click(screen.getByRole("button", { name: "Create account" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Registration isn't available right now. Try again later.");
  });

  it("shows a distinct message for 429 rate limiting", async () => {
    const user = userEvent.setup();
    mocked.register.mockRejectedValue(new ApiError(429, { code: "rate_limited", message: "too many registrations from here; try again in an hour" }));
    mount();
    await user.type(await screen.findByLabelText("Email address"), "a@example.com");
    await user.type(screen.getByLabelText("Password"), "longenough1");
    await user.type(screen.getByLabelText("Confirm password"), "longenough1");
    await user.click(screen.getByRole("button", { name: "Create account" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Too many attempts. Wait a while and try again.");
  });

  it("shows the server's own message for 400 validation_failed", async () => {
    // The client only pre-checks password length and agreement; a field
    // the server rejects for its own reasons (the display name here) still
    // reaches the request, and the server's exact wording is shown as-is.
    const user = userEvent.setup();
    mocked.register.mockRejectedValue(new ApiError(400, { code: "validation_failed", message: "the display name is too long" }));
    mount();
    await user.type(await screen.findByLabelText("Email address"), "a@example.com");
    await user.type(screen.getByLabelText("Password"), "longenough1");
    await user.type(screen.getByLabelText("Confirm password"), "longenough1");
    await user.click(screen.getByRole("button", { name: "Create account" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("the display name is too long");
  });
});
