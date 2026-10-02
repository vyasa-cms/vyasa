import * as React from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { api, type RegistryEntry } from "@/api/client";
import { useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, Panel, Skeleton } from "@/components/ui/primitives";
import { Input } from "@/components/ui/input";
import { notify } from "@/components/ui/toast";

/**
 * Browse and install from the marketplace.
 *
 * The capability list is the point of this component, not decoration:
 * a plugin can only do what its manifest declares and the broker grants,
 * so what is shown here is what the plugin will actually be able to do.
 * Installing sends those same capabilities back as `accept_capabilities`,
 * and the server refuses if the package asks for anything else — a
 * catalogue edited between reading and clicking cannot grant powers
 * nobody agreed to.
 */
export function RegistryBrowser({ kind }: { kind: "plugin" | "theme" }) {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [search, setSearch] = React.useState("");

  const listings = useQuery({
    queryKey: ["registry", kind, search],
    queryFn: () => api.browseRegistry({ kind, q: search || undefined }),
  });

  const install = useMutation({
    mutationFn: (entry: RegistryEntry) =>
      api.installFromRegistry({
        kind,
        name: entry.name,
        accept_capabilities: entry.versions[0]?.capabilities ?? [],
      }),
    onSuccess: (done) => {
      void queryClient.invalidateQueries({ queryKey: ["registry"] });
      void queryClient.invalidateQueries({ queryKey: [kind === "plugin" ? "plugins" : "themes"] });
      notify.success(
        `Installed ${done.name} ${done.version}`,
        done.needs_enabling
          ? kind === "plugin"
            ? "Enable it when you're ready — it does nothing until you do."
            : "Activate it when you're ready."
          : undefined,
      );
    },
    onError: (e) => notify.error("Couldn't install it", e),
  });

  const title = kind === "plugin" ? "Browse plugins" : "Browse themes";

  if (listings.isLoading) {
    return (
      <Panel title={title}>
        <Skeleton className="h-24 w-full" />
      </Panel>
    );
  }
  if (listings.data?.configured === false) {
    return (
      <Panel title={title}>
        <EmptyState
          title="No marketplace configured"
          description="Set a marketplace index URL in Settings to browse and install from one."
        />
      </Panel>
    );
  }
  if (listings.data?.error != null) {
    return (
      <Panel title={title}>
        <p className="p-4 text-sm text-muted-foreground">
          Marketplace unavailable: {listings.data.error}
        </p>
      </Panel>
    );
  }

  const entries = listings.data?.entries ?? [];

  return (
    <Panel title={title}>
      <div className="space-y-3 p-4">
        <Input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={`Search ${kind}s…`}
          aria-label={`Search ${kind}s`}
        />
        {entries.length === 0 ? (
          <p className="text-sm text-muted-foreground">
            {search === "" ? "Nothing listed yet." : "Nothing matches that."}
          </p>
        ) : (
          <ul className="space-y-3" data-testid="registry-entries">
            {entries.map((entry) => {
              const latest = entry.versions[0];
              const capabilities = latest?.capabilities ?? [];
              const installing = install.isPending && install.variables?.name === entry.name;
              return (
                <li key={`${entry.kind}-${entry.name}`} className="rounded border p-3 text-sm">
                  <div className="flex flex-wrap items-baseline gap-x-2">
                    <span className="font-medium">{entry.title || entry.name}</span>
                    <span className="text-xs text-muted-foreground">
                      {latest?.version ?? "—"}
                      {entry.author !== "" ? ` · ${entry.author}` : ""}
                    </span>
                    {entry.installed_version !== null ? (
                      <Chip tone={entry.update_available ? "warning" : "success"} dot={false}>
                        {entry.update_available
                          ? `installed ${entry.installed_version} — update available`
                          : `installed ${entry.installed_version}`}
                      </Chip>
                    ) : null}
                  </div>
                  {entry.summary !== "" ? (
                    <p className="mt-1 text-muted-foreground">{entry.summary}</p>
                  ) : null}

                  {capabilities.length > 0 ? (
                    <div className="mt-2">
                      <p className="text-xs font-medium text-muted-foreground">
                        This plugin will be able to:
                      </p>
                      <p className="mt-1 flex flex-wrap gap-1">
                        {capabilities.map((c) => (
                          <Chip
                            key={c}
                            tone={entry.new_capabilities.includes(c) ? "warning" : "neutral"}
                            dot={false}
                          >
                            {c}
                            {entry.new_capabilities.includes(c) ? " (new)" : ""}
                          </Chip>
                        ))}
                      </p>
                    </div>
                  ) : kind === "plugin" ? (
                    <p className="mt-2 text-xs text-muted-foreground">
                      Requests no capabilities.
                    </p>
                  ) : null}

                  <div className="mt-3 flex items-center gap-2">
                    <button
                      type="button"
                      disabled={
                        installing ||
                        (entry.installed_version !== null && !entry.update_available)
                      }
                      className="rounded border px-2 py-1 text-xs hover:bg-accent disabled:opacity-50"
                      onClick={() => {
                        const newOnes = entry.new_capabilities;
                        void confirm({
                          title:
                            entry.installed_version === null
                              ? `Install ${entry.title || entry.name}?`
                              : `Update to ${latest?.version ?? ""}?`,
                          description:
                            newOnes.length > 0
                              ? `This version asks for capabilities the installed one does not have: ${newOnes.join(", ")}.`
                              : capabilities.length > 0
                                ? `It will be able to: ${capabilities.join(", ")}.`
                                : undefined,
                        }).then((ok) => {
                          if (ok) install.mutate(entry);
                        });
                      }}
                    >
                      {installing
                        ? "Installing…"
                        : entry.installed_version === null
                          ? "Install"
                          : entry.update_available
                            ? "Update"
                            : "Installed"}
                    </button>
                    {entry.homepage !== null ? (
                      <a
                        href={entry.homepage}
                        target="_blank"
                        rel="noreferrer"
                        className="text-xs text-primary underline"
                      >
                        Details
                      </a>
                    ) : null}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </Panel>
  );
}
