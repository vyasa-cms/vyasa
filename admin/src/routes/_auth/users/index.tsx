import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { UserPlus, Users as UsersIcon } from "lucide-react";
import { api, ApiError, type RoleResponse, type UserResponse } from "@/api/client";
import { useMe } from "@/components/auth";
import { useCapabilities } from "@/lib/capabilities";
import { Button } from "@/components/ui/button";
import { DataList, type Column } from "@/components/ui/data-list";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { Chip, EmptyState, ErrorNote, Field, PageHeader } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_auth/users/")({
  component: UsersPage,
});

/** Every role the server knows, in order of power. */
const ROLES = [
  { value: "admin", label: "Administrator", blurb: "everything, including users and settings" },
  { value: "editor", label: "Editor", blurb: "publish and manage everyone's content" },
  { value: "author", label: "Author", blurb: "write and publish their own posts" },
  { value: "contributor", label: "Contributor", blurb: "write drafts an editor publishes" },
  { value: "subscriber", label: "Subscriber", blurb: "read and comment only" },
];

function roleLabel(role: string, customRoles: RoleResponse[] = []): string {
  return ROLES.find((r) => r.value === role)?.label
    ?? customRoles.find((r) => r.slug === role)?.name
    ?? role;
}

/** What the role control's value is, and what the row's badge shows: the
 * custom role's slug when the user has one, the built-in role otherwise. */
function roleValue(u: UserResponse): string {
  return u.custom_role ?? u.role;
}

interface RoleOption {
  value: string;
  label: string;
  /** Whether the signed-in user holds every capability the role holds —
   * the same subset the server's grant guard checks, so a role that would
   * only 403 is disabled before the attempt, not after it. */
  grantable: boolean;
}

const GRANT_HINT = "You don't have all of this role's permissions, so you can't grant it.";

/**
 * Every role the role control can offer: the five built-ins (in order of
 * power) plus any custom roles, each marked whether the signed-in user
 * could actually grant it. Shared by the row control and the new-user
 * form so the two never disagree about which roles are offered.
 */
function roleOptions(allRoles: RoleResponse[], can: (cap: string) => boolean): { builtIn: RoleOption[]; custom: RoleOption[] } {
  const bySlug = new Map(allRoles.map((r) => [r.slug, r]));
  const builtIn = ROLES.map((r) => {
    const full = bySlug.get(r.value);
    // Until GET /roles has loaded, nothing is disabled rather than
    // everything: a longer wait than a wrong flash of "you can't grant
    // any of these".
    const grantable = full === undefined || full.capabilities.every((c) => can(c));
    return { value: r.value, label: r.label, grantable };
  });
  const custom = allRoles
    .filter((r) => !r.built_in)
    .map((r) => ({ value: r.slug, label: r.name, grantable: r.capabilities.every((c) => can(c)) }));
  return { builtIn, custom };
}

function roleOptionElements(options: RoleOption[]) {
  return options.map((o) => (
    <option key={o.value} value={o.value} disabled={!o.grantable} title={o.grantable ? undefined : GRANT_HINT}>
      {o.label}
    </option>
  ));
}

function when(iso: string | null | undefined): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const days = Math.floor((Date.now() - d.getTime()) / 86_400_000);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 30) return `${days} days ago`;
  return d.toLocaleDateString();
}

type Status = "active" | "invited" | "suspended";
function statusOf(u: UserResponse): Status {
  if (u.suspended_at) return "suspended";
  if (!u.has_password) return "invited";
  return "active";
}
const STATUS_LABEL: Record<Status, string> = { active: "Active", invited: "Invited", suspended: "Suspended" };
const STATUS_TONE = { active: "success", invited: "info", suspended: "warning" } as const;

const PAGE = 50;

function UsersPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const me = useMe();
  const caps = useCapabilities();
  const [term, setTerm] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  const [pages, setPages] = React.useState(1);
  const [creating, setCreating] = React.useState(false);
  const [editing, setEditing] = React.useState<UserResponse | null>(null);
  /** Set when a delete comes back 409: the user owns content that needs a new owner first. */
  const [conflict, setConflict] = React.useState<{ user: UserResponse; message: string; retryError?: string } | null>(null);

  React.useEffect(() => {
    const t = setTimeout(() => { setDebounced(term.trim()); setPages(1); }, 250);
    return () => clearTimeout(t);
  }, [term]);

  // Server-side search, one page at a time; "Show more" appends the next.
  const users = useQuery({
    queryKey: ["users", debounced, pages],
    queryFn: async () => {
      const all: UserResponse[] = [];
      let total = 0;
      for (let p = 1; p <= pages; p += 1) {
        const r = await api.listUsers({ q: debounced, page: p, per_page: PAGE });
        all.push(...r.items);
        total = r.total;
      }
      return { items: all, total };
    },
  });
  const invalidate = () => void queryClient.invalidateQueries({ queryKey: ["users"] });

  // The role control offers every custom role alongside the five built-in
  // ones; built-ins already come from the hard-coded ROLES list below.
  const roles = useQuery({ queryKey: ["roles"], queryFn: () => api.listRoles() });
  const allRoles = roles.data ?? [];
  const customRoles = allRoles.filter((r) => !r.built_in);
  const { builtIn: builtInRoleOptions, custom: customRoleOptions } = roleOptions(allRoles, caps.can);

  const remove = useMutation({
    mutationFn: ({ id, reassignTo }: { id: string; reassignTo?: string }) => api.deleteUser(id, reassignTo),
    onSuccess: () => { invalidate(); notify.success("Account deleted"); setConflict(null); },
  });

  /**
   * A user who owns posts or media can't be deleted outright: the server
   * answers 409 with a message and we ask who inherits their content, then
   * retry the same delete with `reassign_to`.
   */
  const attemptDelete = async (u: UserResponse, reassignTo?: string) => {
    try {
      await remove.mutateAsync({ id: u.id, reassignTo });
    } catch (e) {
      if (reassignTo === undefined && e instanceof ApiError && e.status === 409) {
        setConflict({ user: u, message: e.message });
        return;
      }
      // A retry that still fails (e.g. the chosen target no longer exists)
      // keeps the dialog open with the reason, instead of losing the
      // context and making them start the delete over.
      if (reassignTo !== undefined) {
        const reason = e instanceof Error ? e.message : "Something went wrong";
        setConflict((c) => (c ? { ...c, retryError: reason } : c));
      }
      notify.error("Couldn't delete the account", e);
    }
  };
  // A role's user count (shown on the Roles page) changes with every
  // assignment — including a refused one, since the caller finding out
  // "no" is still reason to see today's real counts, not stale ones.
  const invalidateRoles = () => void queryClient.invalidateQueries({ queryKey: ["roles"] });
  const setRole = useMutation({
    mutationFn: ({ id, role }: { id: string; role: string }) => api.setUserRole(id, role),
    // The account whose role changed may be the one signed in: its grants
    // ("my-caps") are read again rather than trusted for another minute.
    onSuccess: (_r, { role }) => { invalidate(); invalidateRoles(); void queryClient.invalidateQueries({ queryKey: ["my-caps"] }); notify.success(`Role changed to ${roleLabel(role, customRoles)}`); },
    // A grant or reach guard answers 403 with its own sentence (e.g. which
    // capabilities the role would exceed, or that the account is beyond
    // reach); shown as-is, not swallowed into a generic failure. Refetching
    // the user here (via invalidate()) is also what puts the control back
    // to the real, unchanged role — nothing here holds an optimistic value.
    onError: (e) => { invalidate(); invalidateRoles(); notify.error("Couldn't change the role", e); },
  });
  const suspend = useMutation({
    mutationFn: ({ id, suspended }: { id: string; suspended: boolean }) => api.suspendUser(id, suspended),
    onSuccess: (_r, { suspended }) => { invalidate(); notify.success(suspended ? "Account suspended" : "Account reinstated", suspended ? "They are signed out everywhere and cannot sign in." : "They can sign in again."); },
    onError: (e) => notify.error("Couldn't change the account", e),
  });
  const resetMfa = useMutation({
    mutationFn: (id: string) => api.resetUserMfa(id),
    onSuccess: () => { invalidate(); notify.success("Second factor removed", "They sign in with their password alone until they set it up again."); },
    onError: (e) => notify.error("Couldn't remove the second factor", e),
  });
  const link = useMutation({
    mutationFn: (id: string) => api.sendResetLink(id),
    onSuccess: (r) => notify.success(r.sent === "invitation" ? "Invitation sent" : "Reset link sent", "It works for one hour."),
    onError: (e) => notify.error("Couldn't send the link", e),
  });
  const signOut = useMutation({
    mutationFn: (id: string) => api.revokeUserSessions(id),
    onSuccess: (r) => notify.success("Signed out everywhere", `${r.ended} ${r.ended === 1 ? "session" : "sessions"} ended.`),
    onError: (e) => notify.error("Couldn't end their sessions", e),
  });
  const confirmEmail = useMutation({
    mutationFn: (id: string) => api.confirmUser(id),
    onSuccess: (r) => {
      invalidate();
      // Confirmed with its password removed, but the link to set one never
      // went out: the person has no way in until someone sends it, so say
      // so and name the row action that does.
      if (r.link_sent === false) {
        const resend = statusOf(r) === "invited" ? "Resend invite" : "Send reset link";
        notify.error("Confirmed, but the set-password email could not be sent", `Use “${resend}” to send them a link to set their password.`);
        return;
      }
      notify.success("Email confirmed", "They've been sent a link to set their password.");
    },
    // Reach-guarded like every other account action: a 403 here names
    // exactly why (not in reach, or the caller lacks manage_users),
    // straight from the server, not a generic failure.
    onError: (e) => notify.error("Couldn't confirm the account", e),
  });
  const resendConfirmation = useMutation({
    mutationFn: (id: string) => api.resendUserConfirmation(id),
    onSuccess: () => notify.success("Confirmation email sent"),
    onError: (e) => notify.error("Couldn't resend the confirmation email", e),
  });
  // Confirming vouches for the mailbox without the usual proof (the link
  // being followed). The server removes whatever password the account was
  // registered with (anyone can register anyone's address, so it may be a
  // stranger's) and mails a link to set one; the dialog says so, since the
  // person will not be able to sign in with the password they typed.
  const askConfirm = async (u: UserResponse) => {
    const ok = await confirm({
      title: `Confirm ${u.display_name || u.email}'s email?`,
      description: "This vouches that the mailbox is theirs, without the confirmation link being followed. The password they signed up with is removed, and the person will be sent a link to set their password. Only confirm if you know this person controls the address; otherwise resend the confirmation link.",
      confirmLabel: "Confirm email",
    });
    if (ok) confirmEmail.mutate(u.id);
  };

  const rows = users.data?.items ?? [];
  const total = users.data?.total ?? 0;

  // The server refuses to delete your own account outright (400
  // validation_failed), so the row action isn't offered for it — the same
  // way Suspend is hidden on your own row.
  const askDelete = async (u: UserResponse) => {
    const ok = await confirm({
      title: `Delete ${u.display_name || u.email}?`,
      description: "Their posts stay published, but the account is removed for good. Suspending keeps the account and just blocks sign-in.",
      confirmLabel: "Delete account",
      destructive: true,
    });
    if (ok) void attemptDelete(u);
  };

  const askRole = async (u: UserResponse, role: string) => {
    if (role === roleValue(u)) return;
    const isSelf = me.data?.id === u.id;
    if (isSelf && u.role === "admin" && role !== "admin") {
      const ok = await confirm({ title: "Give up your administrator access?", description: "You'll lose access to users, settings and plugins straight away. Another administrator would have to restore it.", confirmLabel: "Change my role", destructive: true });
      if (!ok) { invalidate(); return; }
    } else if (role === "admin") {
      const ok = await confirm({ title: `Make ${u.display_name || u.email} an administrator?`, description: "Administrators can change every setting, every user, and every plugin.", confirmLabel: "Make administrator" });
      if (!ok) { invalidate(); return; }
    }
    setRole.mutate({ id: u.id, role });
  };

  const columns: Column<UserResponse>[] = [
    {
      key: "user",
      header: "User",
      primary: true,
      render: (u) => (
        <span className="min-w-0">
          <span className="block truncate font-medium">
            {u.display_name || u.username}
            {me.data?.id === u.id ? <span className="ml-1.5 text-xs font-normal text-muted-foreground">(you)</span> : null}
          </span>
          <span className="block truncate text-[11px] text-muted-foreground">{u.email} · @{u.username}</span>
        </span>
      ),
    },
    {
      key: "status",
      header: "Status",
      width: "8rem",
      render: (u) => {
        const s = statusOf(u);
        return (
          <span className="flex flex-wrap gap-1">
            <Chip tone={STATUS_TONE[s]} dot={false}>{STATUS_LABEL[s]}</Chip>
            {u.mfa_enabled ? <Chip tone="info" dot={false}>2FA</Chip> : null}
            {u.email_verified === false ? <Chip tone="warning" dot={false}>Unconfirmed</Chip> : null}
          </span>
        );
      },
    },
    {
      key: "role",
      header: "Role",
      width: "11rem",
      render: (u) => (
        <label className="min-w-0 block">
          <span className="sr-only">Role for {u.display_name || u.email}</span>
          <select
            value={roleValue(u)}
            onChange={(e) => void askRole(u, e.target.value)}
            className="h-8 w-full rounded-md border border-input bg-background px-2 text-xs focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
          >
            <optgroup label="Built-in roles">{roleOptionElements(builtInRoleOptions)}</optgroup>
            {customRoleOptions.length > 0 ? (
              <optgroup label="Custom roles">{roleOptionElements(customRoleOptions)}</optgroup>
            ) : null}
          </select>
          {/* The row's own display beyond the collapsed <select>: which
              custom role, distinguished from a built-in one, and a text
              unlikely to collide with any option's own rendered text. */}
          {u.custom_role !== null && u.custom_role !== undefined ? (
            <span className="mt-1 block truncate text-[11px] text-muted-foreground">{u.role_name} (custom)</span>
          ) : null}
        </label>
      ),
    },
    { key: "created", header: "Joined", width: "8rem", render: (u) => <span className="text-xs text-muted-foreground">{when(u.created_at)}</span> },
    { key: "seen", header: "Last sign-in", width: "8rem", render: (u) => <span className="text-xs text-muted-foreground">{u.last_login_at ? when(u.last_login_at) : "never"}</span> },
  ];

  const action = (label: string, onClick: () => void, opts: { disabled?: boolean; danger?: boolean } = {}) => (
    <button type="button" onClick={onClick} disabled={opts.disabled} className={cn("rounded-md px-2 py-1 text-xs font-medium hover:bg-accent disabled:opacity-50", opts.danger ? "text-destructive hover:bg-destructive-subtle" : "text-muted-foreground hover:text-foreground")}>
      {label}
    </button>
  );

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Users"
        description="Who can sign in, and what each of them is allowed to do."
        actions={<Button size="sm" onClick={() => setCreating(true)}><UserPlus className="h-4 w-4" aria-hidden="true" />Add user</Button>}
      />

      <DataList
        rows={rows}
        columns={columns}
        rowKey={(u) => u.id}
        isLoading={users.isPending}
        error={users.error}
        search={{ value: term, onChange: setTerm, placeholder: "Search by name, email or username" }}
        rowActions={(u) => {
          const s = statusOf(u);
          const isSelf = me.data?.id === u.id;
          return (
            <span className="flex flex-wrap items-center gap-0.5">
              {action("Edit", () => setEditing(u))}
              {action(s === "invited" ? "Resend invite" : "Send reset link", () => link.mutate(u.id), { disabled: link.isPending })}
              {!isSelf ? action(s === "suspended" ? "Reinstate" : "Suspend", () => suspend.mutate({ id: u.id, suspended: s !== "suspended" }), { disabled: suspend.isPending }) : null}
              {action("Sign out everywhere", () => signOut.mutate(u.id), { disabled: signOut.isPending })}
              {u.mfa_enabled ? action("Reset 2FA", async () => { if (await confirm({ title: `Remove ${u.display_name || u.email}'s second factor?`, description: "For when the phone and the recovery codes are both gone. They sign in with the password alone until they set it up again.", confirmLabel: "Remove" })) resetMfa.mutate(u.id); }, { disabled: resetMfa.isPending }) : null}
              {u.email_verified === false ? action("Confirm", () => void askConfirm(u), { disabled: confirmEmail.isPending }) : null}
              {u.email_verified === false ? action("Resend confirmation", () => resendConfirmation.mutate(u.id), { disabled: resendConfirmation.isPending }) : null}
              {!isSelf ? action("Delete", () => void askDelete(u), { danger: true }) : null}
            </span>
          );
        }}
        empty={<EmptyState icon={UsersIcon} title="No users match" description="Try a different search term." />}
      />

      {total > rows.length ? (
        <div className="flex items-center gap-3 text-sm text-muted-foreground">
          <span>Showing {rows.length} of {total}.</span>
          <Button size="sm" variant="outline" onClick={() => setPages((p) => p + 1)} disabled={users.isFetching}>Show more</Button>
        </div>
      ) : total > 0 ? (
        <p className="text-sm text-muted-foreground">{total} {total === 1 ? "user" : "users"}.</p>
      ) : null}

      <p className="text-xs text-muted-foreground">
        {ROLES.map((r, i) => <span key={r.value}>{i > 0 ? " · " : ""}<b className="font-medium text-foreground">{r.label}</b> {r.blurb}</span>)}
      </p>

      <UserForm
        open={creating || editing !== null}
        user={editing}
        roleOptions={{ builtIn: builtInRoleOptions, custom: customRoleOptions }}
        onClose={() => { setCreating(false); setEditing(null); }}
        onSaved={invalidate}
      />

      <ReassignDialog
        conflict={conflict}
        me={me.data}
        onClose={() => setConflict(null)}
        onConfirm={(target) => { if (conflict) void attemptDelete(conflict.user, target); }}
        pending={remove.isPending}
      />
    </div>
  );
}

/**
 * Shown when a delete comes back 409: the user owns posts or media, so the
 * server needs to know who inherits them before it will remove the account.
 *
 * Candidates come from the dialog's own search, not the users page's
 * currently loaded/searched rows: the page might be filtered down to just
 * the one person being deleted, which must never leave this with nobody to
 * offer. The signed-in user is always an option and the default target,
 * whether or not they're in the fetched page.
 */
function ReassignDialog({
  conflict,
  me,
  onClose,
  onConfirm,
  pending,
}: {
  conflict: { user: UserResponse; message: string; retryError?: string } | null;
  me: UserResponse | undefined;
  onClose: () => void;
  onConfirm: (target: string) => void;
  pending: boolean;
}) {
  const [search, setSearch] = React.useState("");
  const [debounced, setDebounced] = React.useState("");
  const [target, setTarget] = React.useState("");

  React.useEffect(() => {
    const t = setTimeout(() => setDebounced(search.trim()), 250);
    return () => clearTimeout(t);
  }, [search]);

  // A fresh conflict (a new person being deleted) starts its own search and
  // its own pick of who to reassign to; it does not carry over the previous
  // one's leftovers. Keyed on the user's id, not the conflict object itself:
  // a failed retry replaces that object (to attach `retryError`) without
  // starting a new delete flow, and must not wipe the search text or the
  // target the person already chose while the error is showing.
  const conflictUserId = conflict?.user.id;
  React.useEffect(() => {
    if (conflictUserId !== undefined) {
      setSearch("");
      setDebounced("");
      setTarget(me?.id ?? "");
    }
  }, [conflictUserId]);

  const candidatesQuery = useQuery({
    queryKey: ["reassign-candidates", debounced],
    queryFn: () => api.listUsers({ q: debounced, page: 1, per_page: 50 }),
    enabled: conflict !== null,
  });

  const targetId = conflict?.user.id;
  const meOption = me !== undefined && me.id !== targetId ? me : undefined;
  const fetched = (candidatesQuery.data?.items ?? []).filter(
    (c) => c.id !== targetId && c.id !== meOption?.id,
  );
  // The signed-in user is offered regardless of what the search matched, so
  // narrowing the search to nothing (or to just the person being deleted)
  // never leaves the dialog with no way forward.
  const options = meOption ? [meOption, ...fetched] : fetched;
  const validTarget = options.some((o) => o.id === target) ? target : (meOption?.id ?? options[0]?.id ?? "");

  return (
    <Modal
      open={conflict !== null}
      onClose={onClose}
      title="Reassign their content to…"
      description={conflict?.message}
      footer={
        <>
          <Button variant="outline" onClick={onClose}>Cancel</Button>
          <Button variant="destructive" disabled={validTarget === "" || pending} onClick={() => onConfirm(validTarget)}>
            {pending ? "Reassigning…" : "Reassign and delete"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        {conflict?.retryError !== undefined ? (
          <p role="alert" className="rounded-md border border-destructive/40 bg-destructive-subtle px-3 py-2 text-sm text-destructive">
            {conflict.retryError}
          </p>
        ) : null}
        <Field label="Search for a user" htmlFor="reassign-search">
          <Input
            id="reassign-search"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            placeholder="Search by name, email or username"
          />
        </Field>

        {candidatesQuery.isPending ? (
          <p className="text-sm text-muted-foreground" role="status">Loading users…</p>
        ) : candidatesQuery.isError ? (
          <ErrorNote title="Couldn't load users" error={candidatesQuery.error} onRetry={() => void candidatesQuery.refetch()} />
        ) : options.length === 0 ? (
          <p className="text-sm text-muted-foreground">There's no one else to reassign this to.</p>
        ) : (
          <Field label="New owner" htmlFor="reassign-target">
            <select
              id="reassign-target"
              value={validTarget}
              onChange={(e) => setTarget(e.target.value)}
              className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
            >
              {options.map((o) => (
                <option key={o.id} value={o.id}>{o.display_name || o.email}{o.id === me?.id ? " (you)" : ""}</option>
              ))}
            </select>
          </Field>
        )}
      </div>
    </Modal>
  );
}

/** One form for create and edit. Create can set a password or send an invitation. */
function UserForm({
  open,
  user,
  roleOptions: options,
  onClose,
  onSaved,
}: {
  open: boolean;
  user: UserResponse | null;
  roleOptions: { builtIn: RoleOption[]; custom: RoleOption[] };
  onClose: () => void;
  onSaved: () => void;
}) {
  const [v, setV] = React.useState({ email: "", username: "", display_name: "", bio: "", password: "", role: "subscriber" });
  React.useEffect(() => {
    if (open) setV({ email: user?.email ?? "", username: user?.username ?? "", display_name: user?.display_name ?? "", bio: user?.bio ?? "", password: "", role: user?.role ?? "subscriber" });
  }, [open, user]);
  const set = (k: keyof typeof v) => (e: React.ChangeEvent<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>) => setV({ ...v, [k]: e.target.value });

  const save = useMutation({
    mutationFn: async () => {
      if (user) {
        await api.updateUser(user.id, { email: v.email.trim(), username: v.username.trim(), display_name: v.display_name.trim(), bio: v.bio });
        return "updated" as const;
      }
      await api.createUser({ email: v.email.trim(), password: v.password === "" ? null : v.password, role: v.role, display_name: v.display_name.trim() === "" ? null : v.display_name.trim() });
      return v.password === "" ? ("invited" as const) : ("created" as const);
    },
    onSuccess: (what) => {
      notify.success(what === "updated" ? "User updated" : what === "invited" ? `Invitation sent to ${v.email.trim()}` : "Account created", what === "invited" ? "They set their own password from the link; it works for one hour." : undefined);
      onSaved();
      onClose();
    },
    onError: (e) => notify.error(user ? "Couldn't update the user" : "Couldn't create the account", e),
  });

  const tooShort = v.password !== "" && v.password.length < 8;
  return (
    <Modal
      open={open}
      onClose={onClose}
      title={user ? "Edit user" : "Add a user"}
      description={user ? "Role and access are changed from the list." : "They'll sign in with their email address."}
      footer={<><Button variant="outline" onClick={onClose}>Cancel</Button><Button disabled={v.email.trim() === "" || tooShort || save.isPending} onClick={() => save.mutate()}>{save.isPending ? "Saving…" : user ? "Save changes" : v.password === "" ? "Send invitation" : "Create account"}</Button></>}
    >
      <div className="space-y-4">
        <Field label="Email address" htmlFor="user-email"><Input id="user-email" type="email" autoFocus autoCapitalize="none" value={v.email} onChange={set("email")} placeholder="person@example.com" /></Field>
        {user ? <Field label="Username" htmlFor="user-username" hint="Letters, digits, dots, dashes, underscores."><Input id="user-username" value={v.username} onChange={set("username")} className="font-mono text-sm" /></Field> : null}
        <Field label="Display name" htmlFor="user-name" hint="Shown as the byline on their posts."><Input id="user-name" value={v.display_name} onChange={set("display_name")} placeholder="Alex Rivera" /></Field>
        {user ? <Field label="Bio" htmlFor="user-bio"><textarea id="user-bio" rows={2} value={v.bio} onChange={set("bio")} className="w-full resize-y rounded-md border border-input bg-background p-2 text-sm" /></Field> : null}
        {!user ? (
          <>
            <Field label="Password" htmlFor="user-password" hint="Leave blank to email them an invitation with a set-password link instead." error={tooShort ? "Use at least 8 characters." : null}>
              <Input id="user-password" type="password" autoComplete="new-password" value={v.password} onChange={set("password")} />
            </Field>
            <Field label="Role" htmlFor="user-role">
              <select id="user-role" value={v.role} onChange={set("role")} className="h-9 w-full rounded-md border border-input bg-background px-3 text-sm focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring">
                <optgroup label="Built-in roles">{roleOptionElements(options.builtIn)}</optgroup>
                {options.custom.length > 0 ? <optgroup label="Custom roles">{roleOptionElements(options.custom)}</optgroup> : null}
              </select>
            </Field>
          </>
        ) : null}
      </div>
    </Modal>
  );
}
