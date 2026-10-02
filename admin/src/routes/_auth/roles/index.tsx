import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { KeyRound, Plus } from "lucide-react";
import { api, type RoleResponse } from "@/api/client";
import { useCapabilities, CAPABILITIES, CAPABILITY_ORDER, capabilityLabel } from "@/lib/capabilities";
import { Button } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, Field, PageHeader } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/roles/")({
  component: RolesPage,
});

function action(label: string, onClick: () => void, opts: { disabled?: boolean; danger?: boolean } = {}) {
  return (
    <button type="button" onClick={onClick} disabled={opts.disabled} className={cn("rounded-md px-2 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50", opts.danger ? "text-destructive hover:bg-destructive-subtle" : "text-muted-foreground hover:text-foreground")}>
      {label}
    </button>
  );
}

function RolesPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const caps = useCapabilities();
  const [creating, setCreating] = React.useState(false);
  const [copySource, setCopySource] = React.useState<RoleResponse | null>(null);
  const [editing, setEditing] = React.useState<RoleResponse | null>(null);
  const formOpen = creating || copySource !== null || editing !== null;
  const closeForm = () => { setCreating(false); setCopySource(null); setEditing(null); };

  const roles = useQuery({ queryKey: ["roles"], queryFn: () => api.listRoles() });
  // A role mutation can change what a user's row shows (its role_name, or
  // whether the role still exists at all), so the Users page's cache needs
  // refreshing too, not just this page's own list.
  // And what the signed-in user may do: the role edited or deleted may be
  // their own, and the shell and this page's checklist read "my-caps".
  const invalidate = () => {
    void queryClient.invalidateQueries({ queryKey: ["roles"] });
    void queryClient.invalidateQueries({ queryKey: ["users"] });
    void queryClient.invalidateQueries({ queryKey: ["my-caps"] });
  };

  // The server refuses (403) to edit or delete a role whose STORED
  // capabilities include one the caller doesn't hold — otherwise a user
  // manager could strip a role down to size and take over the accounts
  // that held it. Offering Edit/Delete anyway would be a dead end: every
  // save would 403, and a capability already on the role can't be
  // unchecked (its checkbox is disabled) to get out of it. So such a role
  // is read-only here; Copy still works; it seeds only what's held.
  const exceedsOwnCaps = (r: RoleResponse) => r.capabilities.some((c) => !caps.can(c));

  const remove = useMutation({
    mutationFn: (slug: string) => api.deleteRole(slug),
    onSuccess: () => { invalidate(); notify.success("Role deleted"); },
    // A role still assigned to someone comes back 409 naming how many; a
    // capability the caller lacks comes back 403. Both carry the server's
    // own sentence, shown as-is rather than a generic failure.
    onError: (e) => notify.error("Couldn't delete the role", e),
  });

  const askDelete = async (r: RoleResponse) => {
    const ok = await confirm({
      title: `Delete ${r.name}?`,
      description: r.users > 0
        ? `${r.users} ${r.users === 1 ? "person holds" : "people hold"} this role today. Move them to another role first, or the server will refuse.`
        : "Nobody holds this role right now.",
      confirmLabel: "Delete role",
      destructive: true,
    });
    if (ok) remove.mutate(r.slug);
  };

  const rows = roles.data ?? [];

  const columns: Column<RoleResponse>[] = [
    {
      key: "role",
      header: "Role",
      primary: true,
      render: (r) => (
        <span className="min-w-0">
          <span className="flex flex-wrap items-center gap-1.5">
            <span className="truncate font-medium">{r.name}</span>
            {r.built_in ? <Chip tone="neutral" dot={false}>Built-in</Chip> : <Chip tone="info" dot={false}>Custom</Chip>}
          </span>
          <span className="block truncate font-mono text-[11px] text-muted-foreground">/{r.slug}</span>
          {r.description !== "" ? <span className="block truncate text-xs text-muted-foreground">{r.description}</span> : null}
        </span>
      ),
    },
    {
      key: "capabilities",
      header: "Capabilities",
      render: (r) => (
        <span className="flex flex-wrap gap-1">
          {r.capabilities.length === 0
            ? <span className="text-[11px] text-muted-foreground">None</span>
            : r.capabilities.map((c) => (
                <span key={c} className="rounded-full border px-1.5 py-px text-[11px] text-muted-foreground">{capabilityLabel(c)}</span>
              ))}
        </span>
      ),
    },
    {
      key: "users",
      header: "Users",
      width: "6rem",
      render: (r) => <span className="tabular-nums text-muted-foreground">{r.users}</span>,
    },
  ];

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Roles"
        description="What each kind of account is allowed to do. Assign a role to someone from the Users page."
        actions={<Button size="sm" onClick={() => setCreating(true)}><Plus className="h-4 w-4" aria-hidden="true" />New role</Button>}
      />

      <DataList
        rows={rows}
        columns={columns}
        rowKey={(r) => r.slug}
        isLoading={roles.isPending}
        error={roles.error}
        rowActions={(r) => {
          if (r.built_in) return action("Copy", () => setCopySource(r));
          if (exceedsOwnCaps(r)) {
            return (
              <span className="flex flex-wrap items-center gap-2">
                <span className="text-[11px] text-muted-foreground">This role includes permissions you don&rsquo;t have, so you can&rsquo;t change it.</span>
                {action("Copy", () => setCopySource(r))}
              </span>
            );
          }
          return (
            <span className="flex flex-wrap items-center gap-0.5">
              {action("Edit", () => setEditing(r))}
              {action("Delete", () => void askDelete(r), { danger: true })}
            </span>
          );
        }}
        empty={<EmptyState icon={KeyRound} title="No roles yet" description="Create a custom role to grant a narrower set of capabilities than the built-in roles." />}
      />

      <RoleForm open={formOpen} editing={editing} copyFrom={copySource} onClose={closeForm} onSaved={invalidate} />
    </div>
  );
}

/** Create, edit, or "copy from a built-in role" — one form, three starting points. */
function RoleForm({
  open,
  editing,
  copyFrom,
  onClose,
  onSaved,
}: {
  open: boolean;
  editing: RoleResponse | null;
  copyFrom: RoleResponse | null;
  onClose: () => void;
  onSaved: () => void;
}) {
  const caps = useCapabilities();
  const [v, setV] = React.useState({ slug: "", name: "", description: "", capabilities: [] as string[] });

  React.useEffect(() => {
    if (!open) return;
    if (editing) {
      // Only ever opened for a role within the caller's own capabilities
      // (see RolesPage's `exceedsOwnCaps`), so every stored capability
      // here is one the checklist can also show as checked.
      setV({ slug: editing.slug, name: editing.name, description: editing.description, capabilities: [...editing.capabilities] });
    } else if (copyFrom) {
      // Seed only what the signed-in user holds — a capability they lack
      // could never be sent anyway (its checkbox is disabled), and
      // pre-checking it would just make Create 403 immediately.
      setV({ slug: "", name: `${copyFrom.name} copy`, description: copyFrom.description, capabilities: copyFrom.capabilities.filter((c) => caps.can(c)) });
    } else {
      setV({ slug: "", name: "", description: "", capabilities: [] });
    }
    // `caps.can` is not a dependency: it is a fresh function every render
    // (see useCapabilities), and the capability list it reads is already
    // loaded by the time this form can be opened from the roles list.
  }, [open, editing, copyFrom]);

  const omittedFromCopy = copyFrom ? copyFrom.capabilities.filter((c) => !caps.can(c)) : [];

  const toggleCap = (cap: string) => setV((prev) => ({
    ...prev,
    capabilities: prev.capabilities.includes(cap) ? prev.capabilities.filter((c) => c !== cap) : [...prev.capabilities, cap],
  }));

  const slugTrimmed = v.slug.trim();
  const nameTrimmed = v.name.trim();
  const validSlug = /^[a-z0-9][a-z0-9-]{1,39}$/.test(slugTrimmed);
  const validName = nameTrimmed.length >= 1 && nameTrimmed.length <= 60;
  const showSlugError = slugTrimmed !== "" && !validSlug;

  const save = useMutation({
    mutationFn: async () => {
      const body = { slug: slugTrimmed, name: nameTrimmed, description: v.description.trim(), capabilities: v.capabilities };
      if (editing) return api.updateRole(editing.slug, body);
      return api.createRole(body);
    },
    onSuccess: () => {
      notify.success(editing ? "Role updated" : "Role created");
      onSaved();
      onClose();
    },
    // Escalation (403), a taken slug (409) or a bad slug/name (400) all come
    // back with the server's own sentence; shown verbatim, not swallowed.
    onError: (e) => notify.error(editing ? "Couldn't update the role" : "Couldn't create the role", e),
  });

  const title = editing ? `Edit ${editing.name}` : copyFrom ? `Copy of ${copyFrom.name}` : "New role";
  const description = editing
    ? "Renaming keeps every assignment. Changes to capabilities apply the next time each user's session makes a request."
    : copyFrom
      ? `Starts with ${copyFrom.name}'s capabilities — pick a new slug and adjust from there.`
      : "A name, a slug, and what its users are allowed to do.";

  return (
    <Modal
      open={open}
      onClose={onClose}
      title={title}
      description={description}
      size="lg"
      footer={
        <>
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button disabled={!validSlug || !validName || save.isPending} onClick={() => save.mutate()}>
            {save.isPending ? "Saving…" : editing ? "Save changes" : "Create role"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Slug" htmlFor="role-slug" hint="2 to 40 lowercase letters, digits and hyphens; can't start with a hyphen." error={showSlugError ? "Enter a valid slug." : null}>
          <Input id="role-slug" autoFocus={!editing} value={v.slug} onChange={(e) => setV({ ...v, slug: e.target.value })} className="font-mono text-sm" />
        </Field>
        <Field label="Name" htmlFor="role-name">
          <Input id="role-name" autoFocus={editing !== null} value={v.name} onChange={(e) => setV({ ...v, name: e.target.value })} placeholder="Drafter" />
        </Field>
        <Field label="Description" htmlFor="role-description" hint="Optional. Shown on the roles list.">
          <textarea id="role-description" rows={2} value={v.description} onChange={(e) => setV({ ...v, description: e.target.value })} className="w-full resize-y rounded-md border border-input bg-background p-2 text-sm" />
        </Field>

        {omittedFromCopy.length > 0 ? (
          <p role="status" className="rounded-md border border-warning/40 bg-warning-subtle px-3 py-2 text-sm text-warning">
            Left out because you don&rsquo;t have them: {omittedFromCopy.map((c) => capabilityLabel(c)).join(", ")}.
          </p>
        ) : null}

        <fieldset className="space-y-2">
          <legend className="text-sm font-medium">Capabilities</legend>
          <div className="grid gap-1.5 sm:grid-cols-2">
            {CAPABILITY_ORDER.map((cap) => {
              const info = CAPABILITIES[cap];
              const held = caps.can(cap);
              return (
                <label key={cap} className={cn("flex items-start gap-2 rounded-md border px-3 py-2 text-sm", held ? "cursor-pointer hover:bg-accent" : "cursor-not-allowed opacity-60")}>
                  <input
                    type="checkbox"
                    checked={v.capabilities.includes(cap)}
                    disabled={!held}
                    onChange={() => toggleCap(cap)}
                    className="mt-0.5 h-3.5 w-3.5 accent-primary"
                  />
                  <span className="min-w-0">
                    <span className="block truncate font-medium">{info?.label ?? cap}</span>
                    <span className="block text-xs text-muted-foreground">{info?.description}</span>
                    {info?.also && info.also.length > 0 ? (
                      <span className="block text-xs text-muted-foreground">Also: {info.also.join("; ")}.</span>
                    ) : null}
                    {info?.fullAdministratorOnly && info.fullAdministratorOnly.length > 0 ? (
                      <span className="block text-xs text-muted-foreground">Only a full administrator (every capability): {info.fullAdministratorOnly.join("; ")}.</span>
                    ) : null}
                    {!held ? <span className="block text-[11px] text-warning">You don&rsquo;t have this capability, so you can&rsquo;t grant it.</span> : null}
                  </span>
                </label>
              );
            })}
          </div>
        </fieldset>

        {CAPABILITY_ORDER.filter((cap) => v.capabilities.includes(cap) && CAPABILITIES[cap]?.administratorLevel !== undefined).map((cap) => (
          <p key={cap} role="alert" data-testid="admin-level-warning" className="rounded-md border border-warning/40 bg-warning-subtle px-3 py-2 text-sm text-warning">
            Administrator-level: with &ldquo;{capabilityLabel(cap)}&rdquo;, {CAPABILITIES[cap]?.administratorLevel}. Give it only to someone you would trust as an administrator.
          </p>
        ))}

        {!v.capabilities.includes("view_admin") ? (
          <p role="status" className="rounded-md border border-warning/40 bg-warning-subtle px-3 py-2 text-sm text-warning">
            Without &ldquo;{capabilityLabel("view_admin")}&rdquo;, people with this role can sign in and manage their own profile, but can&rsquo;t use the admin screens.
          </p>
        ) : null}
      </div>
    </Modal>
  );
}
