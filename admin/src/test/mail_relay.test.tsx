import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";
import { MailRelayPanel } from "@/components/MailRelayPanel";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return { ...original, api: { ...original.api, mailSettings: vi.fn(), mailSettingsSave: vi.fn(), mailTest: vi.fn() } };
});
const mocked = vi.mocked(api);

function mount(props: { testTo?: string } = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><MailRelayPanel {...props} /></QueryClientProvider>);
}

beforeEach(() => vi.clearAllMocks());

describe("mail relay panel", () => {
  it("shows the environment relay without a password and lets a preset fill the host", async () => {
    const user = userEvent.setup();
    mocked.mailSettings.mockResolvedValue({ host: "smtp.env.example", port: 587, username: "u", from: "a@b.c", has_password: true, source: "environment", encrypted: true });
    mount();
    expect(await screen.findByText(/Set in the environment/)).toBeInTheDocument();
    expect((screen.getByLabelText("Relay host") as HTMLInputElement).value).toBe("smtp.env.example");
    expect((screen.getByLabelText("Password") as HTMLInputElement).value).toBe("");
    await user.click(screen.getByRole("button", { name: "SendGrid" }));
    expect((screen.getByLabelText("Relay host") as HTMLInputElement).value).toBe("smtp.sendgrid.net");
    expect((screen.getByLabelText("Username") as HTMLInputElement).value).toBe("apikey");
  });

  it("saves with the password only when one was typed, then allows a test", async () => {
    const user = userEvent.setup();
    mocked.mailSettings.mockResolvedValue({ host: "", port: 587, username: "", from: "", has_password: false, source: "none", encrypted: true });
    mocked.mailSettingsSave.mockResolvedValue({ host: "smtp.x", port: 587, username: "", from: "me@x", has_password: false, source: "options", encrypted: true });
    mocked.mailTest.mockResolvedValue(undefined);
    mount({ testTo: "me@x" });
    await screen.findByText(/No relay yet/);
    expect(screen.getByRole("button", { name: "Send test" })).toBeDisabled();
    await user.type(screen.getByLabelText("Relay host"), "smtp.x");
    await user.type(screen.getByLabelText("From address"), "me@x");
    await user.click(screen.getByRole("button", { name: "Save relay" }));
    await waitFor(() => expect(mocked.mailSettingsSave).toHaveBeenCalledWith({ host: "smtp.x", port: 587, username: "", from: "me@x" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Send test" })).toBeEnabled());
    await user.click(screen.getByRole("button", { name: "Send test" }));
    await waitFor(() => expect(mocked.mailTest).toHaveBeenCalledWith("me@x"));
  });
});
