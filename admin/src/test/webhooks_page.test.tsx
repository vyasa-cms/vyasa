import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { Route } from "@/routes/_auth/webhooks/index";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return {
    ...original,
    api: {
      ...original.api,
      listWebhooks: vi.fn(),
      createWebhook: vi.fn(),
      updateWebhook: vi.fn(),
      testWebhook: vi.fn(),
      webhookDeliveries: vi.fn(),
      redeliverWebhook: vi.fn(),
    },
  };
});
const mocked = vi.mocked(api);
const Page = Route.options.component as React.ComponentType;

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><ConfirmProvider><Page /></ConfirmProvider></QueryClientProvider>);
}

beforeEach(() => vi.clearAllMocks());

describe("webhooks page", () => {
  it("shows the true state, the last delivery, and a failing banner", async () => {
    mocked.listWebhooks.mockResolvedValue([
      { id: "1", url: "https://a.example/hook", events: ["post.published"], enabled: true, last_delivery: { status: "failed", at: new Date().toISOString() } },
      { id: "2", url: "https://b.example/hook", events: [], enabled: false, last_delivery: null },
    ]);
    mount();
    await screen.findByText("https://a.example/hook");
    expect(screen.getByText("Active")).toBeInTheDocument();
    expect(screen.getByText("Paused")).toBeInTheDocument();
    expect(screen.getByText("Never")).toBeInTheDocument();
    expect(screen.getByTestId("webhooks-failing")).toHaveTextContent("1 webhook's last delivery failed");
  });

  it("shows the secret once after creating, with the header name", async () => {
    const user = userEvent.setup();
    mocked.listWebhooks.mockResolvedValue([]);
    mocked.createWebhook.mockResolvedValue({ id: "9", url: "https://c.example/h", events: ["post.published"], enabled: true, secret: "whsec_abc", signature_header: "X-Vyasa-Signature" });
    mount();
    await user.click((await screen.findAllByRole("button", { name: "Add webhook" }))[0]!);
    await user.type(screen.getByLabelText("Endpoint URL"), "https://c.example/h");
    const buttons = screen.getAllByRole("button", { name: "Add webhook" });
    await user.click(buttons[buttons.length - 1]!);
    await waitFor(() => expect(mocked.createWebhook).toHaveBeenCalledWith({ url: "https://c.example/h", events: ["post.published"] }));
    expect(await screen.findByTestId("webhook-secret-value")).toHaveTextContent("whsec_abc");
    expect(screen.getByTestId("webhook-secret")).toHaveTextContent("X-Vyasa-Signature");
  });

  it("pauses from the row and offers redelivery for a failed attempt", async () => {
    const user = userEvent.setup();
    mocked.listWebhooks.mockResolvedValue([
      { id: "1", url: "https://a.example/hook", events: ["post.published"], enabled: true, last_delivery: null },
    ]);
    mocked.updateWebhook.mockResolvedValue({ id: "1", url: "https://a.example/hook", events: ["post.published"], enabled: false, last_delivery: null });
    mocked.webhookDeliveries.mockResolvedValue([
      { id: "d1", event: "post.published", status: "failed", response_code: 500, attempts: 2, at: new Date().toISOString(), payload: "{}", response_body: "boom", error: "HTTP 500" },
    ]);
    mocked.redeliverWebhook.mockResolvedValue({ queued: true });
    mount();
    await user.click(await screen.findByRole("button", { name: "Pause" }));
    await waitFor(() => expect(mocked.updateWebhook).toHaveBeenCalledWith("1", { enabled: false }));
    await user.click(screen.getByRole("button", { name: "History" }));
    await user.click(await screen.findByRole("button", { name: "Redeliver" }));
    await waitFor(() => expect(mocked.redeliverWebhook).toHaveBeenCalledWith("1", "d1"));
    expect(screen.getAllByText("A post is published").length).toBeGreaterThan(1);
  });
});
