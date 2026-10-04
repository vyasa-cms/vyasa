import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { RegistryBrowser } from "@/components/RegistryBrowser";
import { ConfirmProvider } from "@/components/ui/dialog";
import { api } from "@/api/client";

vi.mock("@/api/client", () => ({
  api: { browseRegistry: vi.fn(), installFromRegistry: vi.fn() },
}));

const mocked = api as unknown as {
  browseRegistry: ReturnType<typeof vi.fn>;
  installFromRegistry: ReturnType<typeof vi.fn>;
};

function entry(overrides: Record<string, unknown> = {}) {
  return {
    kind: "plugin",
    name: "storefront",
    title: "Storefront",
    summary: "Sell things.",
    author: "Vyasa",
    homepage: null,
    versions: [
      { version: "1.2.0", capabilities: ["db:read:posts", "net:fetch:api.stripe.com"], released_at: null },
    ],
    installed_version: null,
    update_available: false,
    new_capabilities: [],
    ...overrides,
  };
}

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <ConfirmProvider>
        <RegistryBrowser kind="plugin" />
      </ConfirmProvider>
    </QueryClientProvider>,
  );
}

describe("registry browser", () => {
  beforeEach(() => vi.clearAllMocks());

  it("shows what a plugin will be able to do before installing it", async () => {
    mocked.browseRegistry.mockResolvedValue({
      configured: true,
      state: "official",
      error: null,
      entries: [entry()],
    });
    mount();
    expect(await screen.findByText("Storefront")).toBeInTheDocument();
    expect(await screen.findByText(/This plugin will be able to/)).toBeInTheDocument();
    expect(screen.getByText("db:read:posts")).toBeInTheDocument();
    expect(screen.getByText("net:fetch:api.stripe.com")).toBeInTheDocument();
  });

  it("sends the capabilities it displayed back as the accepted set", async () => {
    mocked.browseRegistry.mockResolvedValue({
      configured: true,
      state: "official",
      error: null,
      entries: [entry()],
    });
    mocked.installFromRegistry.mockResolvedValue({
      kind: "plugin",
      name: "storefront",
      version: "1.2.0",
      capabilities: ["db:read:posts", "net:fetch:api.stripe.com"],
      needs_enabling: true,
    });
    mount();
    await userEvent.click(await screen.findByRole("button", { name: "Install" }));
    // The confirm dialog names the powers being granted before it proceeds.
    expect(await screen.findByText(/It will be able to: db:read:posts, net:fetch:api.stripe.com/)).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "Confirm" }));
    await waitFor(() => {
      expect(mocked.installFromRegistry).toHaveBeenCalledWith({
        kind: "plugin",
        name: "storefront",
        accept_capabilities: ["db:read:posts", "net:fetch:api.stripe.com"],
      });
    });
  });

  it("marks an update's new capabilities and offers Update, not Install", async () => {
    mocked.browseRegistry.mockResolvedValue({
      configured: true,
      state: "official",
      error: null,
      entries: [
        entry({
          installed_version: "1.0.0",
          update_available: true,
          new_capabilities: ["net:fetch:api.stripe.com"],
        }),
      ],
    });
    mount();
    expect(await screen.findByRole("button", { name: "Update" })).toBeEnabled();
    expect(screen.getByText("net:fetch:api.stripe.com (new)")).toBeInTheDocument();
  });

  it("says so plainly when the operator turned the marketplace off", async () => {
    mocked.browseRegistry.mockResolvedValue({ configured: false, state: "off", error: null, entries: [] });
    mount();
    expect(await screen.findByText(/turned off on this server/)).toBeInTheDocument();
  });
});
