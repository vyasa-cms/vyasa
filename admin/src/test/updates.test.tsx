import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { UpdatePanel } from "@/components/UpdatePanel";
import { api } from "@/api/client";

vi.mock("@/api/client", () => ({
  api: { updateStatus: vi.fn(), updateProgress: vi.fn(), applyUpdate: vi.fn() },
}));

const mocked = api as unknown as {
  updateStatus: ReturnType<typeof vi.fn>;
  updateProgress: ReturnType<typeof vi.fn>;
  applyUpdate: ReturnType<typeof vi.fn>;
};

function status(overrides: Record<string, unknown> = {}) {
  return {
    current: "1.0.0",
    latest: "1.1.0",
    update_available: true,
    releases: [
      {
        version: "1.1.0",
        summary: "Faster archives",
        notes_url: null,
        requires_attention: false,
      },
    ],
    environment: {
      mode: "standalone",
      binary_path: "/opt/vyasa/vyasa",
      binary_replaceable: true,
      restart_supervised: false,
    },
    instructions: [],
    preflight: {
      can_proceed: true,
      findings: [{ name: "media", status: "ok", detail: "media is writable" }],
    },
    channel_error: null,
    ...overrides,
  };
}

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <UpdatePanel />
    </QueryClientProvider>,
  );
}

describe("update panel", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    mocked.updateProgress.mockResolvedValue(null);
  });

  it("offers the button on a self-managed install", async () => {
    mocked.updateStatus.mockResolvedValue(status());
    mount();
    const button = await screen.findByRole("button", { name: /upgrade to 1\.1\.0/i });
    expect(button).toBeEnabled();
    expect(await screen.findByText(/media is writable/)).toBeInTheDocument();
  });

  it("shows commands instead of a button inside a container", async () => {
    mocked.updateStatus.mockResolvedValue(
      status({
        environment: {
          mode: "docker",
          binary_path: "/usr/local/bin/vyasa",
          binary_replaceable: false,
          restart_supervised: false,
        },
        instructions: ["docker compose pull", "docker compose up -d"],
      }),
    );
    mount();
    expect(await screen.findByText(/docker compose up -d/)).toBeInTheDocument();
    expect(
      screen.queryByRole("button", { name: /upgrade to/i }),
      "a container cannot replace its own image, so no button may imply it can",
    ).toBeNull();
  });

  it("refuses to start when the preflight failed", async () => {
    mocked.updateStatus.mockResolvedValue(
      status({
        preflight: {
          can_proceed: false,
          findings: [{ name: "media", status: "fail", detail: "media is not writable" }],
        },
      }),
    );
    mount();
    const button = await screen.findByRole("button", { name: /upgrade to 1\.1\.0/i });
    expect(button).toBeDisabled();
  });

  it("says it is up to date when nothing is newer", async () => {
    mocked.updateStatus.mockResolvedValue(
      status({ latest: "1.0.0", update_available: false, releases: [] }),
    );
    mount();
    expect(await screen.findByText(/up to date/)).toBeInTheDocument();
  });
});
