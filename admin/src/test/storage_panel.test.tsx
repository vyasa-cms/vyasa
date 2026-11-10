import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { api, type StorageSettings } from "@/api/client";
import { StoragePanel } from "@/components/StoragePanel";

vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return { ...original, api: { ...original.api, storageSettings: vi.fn(), storageSettingsSave: vi.fn(), storageTest: vi.fn(), storageMigrate: vi.fn() } };
});
const mocked = vi.mocked(api);

const base: StorageSettings = {
  provider: "local", bucket: "", region: "auto", endpoint: "", path_style: true, access_key_id_hint: "", has_secret: false, keys_unreadable: false,
  source: "none", encrypted: true, counts: { local: 2, s3: 0 }, ephemeral_disk: true, migration: null,
};

function mount() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><StoragePanel /></QueryClientProvider>);
}

beforeEach(() => vi.clearAllMocks());

describe("storage panel", () => {
  it("is read-only when the environment owns the configuration", async () => {
    mocked.storageSettings.mockResolvedValue({ ...base, provider: "s3", bucket: "env-bucket", endpoint: "https://s3.example", source: "environment", has_secret: true, access_key_id_hint: "AB12" });
    mount();
    expect(await screen.findByText(/Set by the operator/)).toBeInTheDocument();
    expect(screen.getByLabelText("Bucket")).toBeDisabled();
    expect(screen.queryByRole("button", { name: "Save storage" })).not.toBeInTheDocument();
    expect(screen.getByText(/ending in AB12/)).toBeInTheDocument();
  });

  it("warns about an ephemeral disk, tests, then saves with the keys only when typed", async () => {
    const user = userEvent.setup();
    mocked.storageSettings.mockResolvedValue(base);
    mocked.storageTest.mockResolvedValue(undefined);
    mocked.storageSettingsSave.mockResolvedValue({ ...base, provider: "s3", bucket: "b", endpoint: "https://e", source: "options", has_secret: true, access_key_id_hint: "1234", counts: { local: 2, s3: 0 } });
    mount();
    expect(await screen.findByTestId("storage-ephemeral")).toBeInTheDocument();
    await user.click(screen.getByLabelText("Object storage (S3-compatible)"));
    await user.click(screen.getByRole("button", { name: "Cloudflare R2" }));
    await user.clear(screen.getByLabelText("Endpoint"));
    await user.type(screen.getByLabelText("Endpoint"), "https://e");
    await user.type(screen.getByLabelText("Bucket"), "b");
    expect(screen.getByRole("button", { name: "Test connection" })).toBeDisabled();
    await user.type(screen.getByLabelText("Access key id"), "k");
    await user.type(screen.getByLabelText("Secret access key"), "s");
    await user.click(screen.getByRole("button", { name: "Test connection" }));
    await waitFor(() => expect(mocked.storageTest).toHaveBeenCalledWith({ provider: "s3", bucket: "b", region: "auto", endpoint: "https://e", path_style: true, access_key_id: "k", secret_access_key: "s" }));
    await user.click(screen.getByRole("button", { name: "Save storage" }));
    await waitFor(() => expect(mocked.storageSettingsSave).toHaveBeenCalledWith({ provider: "s3", bucket: "b", region: "auto", endpoint: "https://e", path_style: true, access_key_id: "k", secret_access_key: "s" }));
    // After the save the keys are stored and the old files can be moved.
    expect(await screen.findByRole("button", { name: "Move 2 files to object storage" })).toBeEnabled();
    expect((screen.getByLabelText("Secret access key") as HTMLInputElement).value).toBe("");
  });

  it("offers no move without a usable object store, and says when the keys cannot be read", async () => {
    mocked.storageSettings.mockResolvedValue({ ...base, provider: "local", bucket: "b", source: "options", has_secret: false, keys_unreadable: true, counts: { local: 1, s3: 3 } });
    mount();
    expect(await screen.findByTestId("storage-keys-unreadable")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Move/ })).not.toBeInTheDocument();
  });

  it("starts the move and shows its progress", async () => {
    const user = userEvent.setup();
    mocked.storageSettings
      .mockResolvedValueOnce({ ...base, provider: "s3", bucket: "b", endpoint: "https://e", source: "options", has_secret: true, counts: { local: 3, s3: 1 }, migration: null })
      .mockResolvedValue({ ...base, provider: "s3", source: "options", has_secret: true, counts: { local: 0, s3: 4 }, migration: { state: "done", total: 3, done: 3, failed: 0, to: "s3", started_at: "now", finished_at: "later", last_error: null } });
    mocked.storageMigrate.mockResolvedValue({ state: "running", total: 3, done: 0, failed: 0, to: "s3", started_at: "now", finished_at: null, last_error: null });
    mount();
    await user.click(await screen.findByRole("button", { name: "Move 3 files to object storage" }));
    await waitFor(() => expect(mocked.storageMigrate).toHaveBeenCalled());
    expect(await screen.findByText(/Last move: 3 moved/)).toBeInTheDocument();
  });
});
