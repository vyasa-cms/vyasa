import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import * as pluginsApi from "@/api/plugins";
import { api } from "@/api/client";
import { ConfirmProvider } from "@/components/ui/dialog";
import { notify } from "@/components/ui/toast";

import { PluginsPage } from "@/routes/_auth/plugins";
import { TestRouter } from "./TestRouter";

vi.mock("@/api/plugins", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  listPlugins: vi.fn(),
  setPluginEnabled: vi.fn(),
  inspectPlugin: vi.fn(),
  installPlugin: vi.fn(),
  pluginAudit: vi.fn(),
}));
vi.mock("@/api/client", async (importOriginal) => {
  const original = await importOriginal<{ api: typeof api }>();
  return { ...original, api: { ...original.api, browseRegistry: vi.fn(), getPluginSurface: vi.fn() } };
});

const mocked = vi.mocked(pluginsApi);
const client = vi.mocked(api);

const sample = [
  {
    id: "1",
    name: "hello",
    version: "0.2.0",
    enabled: false,
    status: "loaded",
    status_reason: "",
    capabilities: ["log:write", "net:fetch:*.example.com"],
    description: "Says hello.",
    author: "Ada",
    versions: ["0.2.0", "0.1.0"],
    audit: { deny: 2 },
  },
  { id: "2", name: "flaky", version: "1.0.0", enabled: true, status: "degraded", status_reason: "timed out rendering block", capabilities: [], versions: ["1.0.0"] },
];

function makeApp() {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return <QueryClientProvider client={qc}><ConfirmProvider><TestRouter><PluginsPage /></TestRouter></ConfirmProvider></QueryClientProvider>;
}

beforeEach(() => {
  vi.clearAllMocks();
  mocked.listPlugins.mockResolvedValue(sample as never);
  mocked.pluginAudit.mockResolvedValue([]);
  client.browseRegistry.mockResolvedValue({ configured: true, entries: [{ kind: "plugin", name: "hello", title: "Hello", summary: "", author: "Ada", installed_version: "0.2.0", update_available: true, new_capabilities: [], versions: [{ version: "0.3.0", capabilities: [] }] }] } as never);
  client.getPluginSurface.mockResolvedValue({ blocks: [], routes: [], forms: [], postTypes: [], taxonomies: [], tasks: [] } as never);
});

describe("plugin manager", () => {
  it("lists plugins with human capability labels, status reasons, and updates", async () => {
    render(makeApp());
    expect(await screen.findByTestId("plugin-row-hello")).toBeInTheDocument();
    expect(screen.getByText(/Fetch from hosts matching \*\.example\.com/)).toBeInTheDocument();
    expect(screen.getByText(/Write log messages/)).toBeInTheDocument();
    expect(screen.getByText("timed out rendering block")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("0.3.0 available")).toBeInTheDocument());
    // Roll back only where a prior version exists.
    expect(screen.getAllByRole("button", { name: "Roll back" })).toHaveLength(1);
  });

  it("reads the package first and only installs a trusted one", async () => {
    const user = userEvent.setup();
    mocked.inspectPlugin.mockResolvedValue({ name: "p", version: "1.0.0", description: "", author: "", homepage: "", license: "", capabilities: ["log:write"], trusted: false, signature_prefix: "abcd1234", wasm_bytes: 2048, installed_version: null, new_capabilities: ["log:write"] });
    render(makeApp());
    const fileInput = await screen.findByLabelText("plugin file");
    await user.upload(fileInput, new File(["x"], "p.vyplugin", { type: "application/zip" }));
    await screen.findByTestId("signature-bad");
    expect(screen.getByTestId("install-button")).toBeDisabled();

    mocked.inspectPlugin.mockResolvedValue({ name: "p", version: "1.0.0", description: "", author: "", homepage: "", license: "", capabilities: [], trusted: true, signature_prefix: "abcd1234", wasm_bytes: 2048, installed_version: null, new_capabilities: [] });
    mocked.installPlugin.mockResolvedValue({ id: "9", name: "p", version: "1.0.0" });
    await user.upload(fileInput, new File(["y"], "q.vyplugin", { type: "application/zip" }));
    await screen.findByTestId("signature-ok");
    await user.click(screen.getByTestId("install-button"));
    await waitFor(() => expect(mocked.installPlugin).toHaveBeenCalledTimes(1));
  });

  it("toggle calls enable API", async () => {
    const user = userEvent.setup();
    mocked.setPluginEnabled.mockResolvedValue(undefined);
    render(makeApp());
    const btn = (await screen.findAllByRole("button", { name: "Enable" }))[0]!;
    await user.click(btn);
    await waitFor(() => expect(mocked.setPluginEnabled).toHaveBeenCalledWith("1", true));
  });

  it("opens a detail view with versions to switch to", async () => {
    const user = userEvent.setup();
    render(makeApp());
    await user.click(await screen.findByRole("button", { name: "hello" }));
    const versions = await screen.findByTestId("plugin-versions");
    expect(versions).toHaveTextContent("0.1.0");
    expect(screen.getByRole("button", { name: "Switch to this" })).toBeInTheDocument();
  });

  it("explains a plugin turned off because an administrator's content type holds its slug", async () => {
    mocked.listPlugins.mockResolvedValue([
      {
        id: "3", name: "bookshelf", version: "1.0.0", enabled: false, status: "errored", capabilities: [], versions: ["1.0.0"],
        status_reason: "the plugin \"bookshelf\" declares the post type \"book\", but \"book\" is a content type created by an administrator; delete or rename that content type, or use a version of the plugin with another slug",
      },
    ] as never);
    render(makeApp());
    const row = await screen.findByTestId("plugin-row-bookshelf");
    // Off, but the reason is not hidden behind "Disabled".
    expect(row).toHaveTextContent(/is a content type created by an administrator/);
    expect(within(row).getByRole("link", { name: "Content types" })).toHaveAttribute("href", "/content-types");
  });

  it("says a plugin degraded by a slug clash keeps running without the type, and only turning it on or rolling back is refused", async () => {
    const user = userEvent.setup();
    mocked.listPlugins.mockResolvedValue([
      {
        id: "3", name: "bookshelf", version: "1.0.0", enabled: true, status: "degraded", capabilities: [], versions: ["1.0.0"],
        status_reason: "post type \"book\" not loaded: an administrator's content type holds the slug",
      },
    ] as never);
    render(makeApp());
    const row = await screen.findByTestId("plugin-row-bookshelf");
    expect(row).toHaveTextContent(/not loaded: an administrator.s content type holds the slug/);
    await user.click(screen.getByRole("button", { name: "Details" }));
    const detail = await screen.findByTestId("plugin-detail");
    expect(detail).toHaveTextContent(/running without that type/);
    expect(detail).toHaveTextContent(/Saving its settings or upgrading it keeps it on/);
    expect(detail).toHaveTextContent(/Turning it off and on again, or rolling it back, is refused/);
    expect(detail).not.toHaveTextContent(/turns it off/);
  });

  it("says when an installed or upgraded plugin arrives degraded", async () => {
    const user = userEvent.setup();
    const warn = vi.spyOn(notify, "info");
    mocked.inspectPlugin.mockResolvedValue({ name: "bookshelf", version: "1.1.0", description: "", author: "", homepage: "", license: "", capabilities: [], trusted: true, signature_prefix: "abcd1234", wasm_bytes: 2048, installed_version: "1.0.0", new_capabilities: [] });
    mocked.installPlugin.mockResolvedValue({ id: "3", name: "bookshelf", version: "1.1.0", status: "degraded" } as never);
    render(makeApp());
    await user.upload(await screen.findByLabelText("plugin file"), new File(["y"], "b.vyplugin", { type: "application/zip" }));
    await screen.findByTestId("signature-ok");
    await user.click(screen.getByTestId("install-button"));
    await waitFor(() => expect(warn).toHaveBeenCalledWith("bookshelf 1.1.0 installed, but not all of it is loaded", expect.stringContaining("plugin list")));
  });
});
