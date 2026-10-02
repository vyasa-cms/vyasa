import * as React from "react";
import { createFileRoute } from "@tanstack/react-router";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ChevronDown,
  ChevronUp,
  CornerDownRight,
  Menu as MenuIcon,
  Plus,
  Trash2,
} from "lucide-react";
import { api } from "@/api/client";
import { Button } from "@/components/ui/button";
import { Modal, useConfirm } from "@/components/ui/dialog";
import { EmptyState, ErrorNote, Field, PageHeader, Skeleton } from "@/components/ui/primitives";
import { notify } from "@/components/ui/toast";
import { Input } from "@/components/ui/input";
import { cn } from "@/lib/utils";

/**
 * A text field that saves when editing is finished, not on every key.
 *
 * The label and address fields were controlled straight from the query
 * cache and fired a PATCH per keystroke. React re-rendered with the old
 * server value before the response landed, so the field snapped back
 * after each character, the caret jumped to the end, and two quick keys
 * raced two PATCHes computed from the same stale base -- typing "ab"
 * saved "b". Editing lives here until blur or Enter; Escape puts the
 * server value back.
 */
function CommitInput({
  value,
  onCommit,
  ...rest
}: {
  value: string;
  onCommit: (next: string) => void;
} & Omit<React.InputHTMLAttributes<HTMLInputElement>, "value" | "onChange" | "onBlur">) {
  const [draft, setDraft] = React.useState(value);
  // Escape reverts and then blurs, and the blur's commit would otherwise
  // read the draft from before the revert -- state has not re-rendered
  // yet -- and save the very text Escape was meant to throw away.
  const discard = React.useRef(false);
  // A save elsewhere (a reorder, an undo) changes the server value while
  // this field is not being edited; follow it.
  React.useEffect(() => setDraft(value), [value]);
  const commit = () => {
    if (discard.current) {
      discard.current = false;
      return;
    }
    const next = draft.trim();
    if (next !== value && next !== "") onCommit(next);
    else setDraft(value);
  };
  return (
    <input
      {...rest}
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") e.currentTarget.blur();
        if (e.key === "Escape") {
          discard.current = true;
          setDraft(value);
          e.currentTarget.blur();
        }
      }}
    />
  );
}

export const Route = createFileRoute("/_auth/menus/")({
  component: MenusPage,
});

interface Menu {
  id: string;
  slug: string;
  name: string;
}

interface Item {
  id: string;
  parent_id: string | null;
  label: string;
  url: string;
  sort_order: number;
}

function slugify(value: string): string {
  return value
    .toLowerCase()
    .trim()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export function MenusPage() {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const [creating, setCreating] = React.useState(false);
  const [activeId, setActiveId] = React.useState<string | null>(null);

  const menus = useQuery({ queryKey: ["menus"], queryFn: () => api.listMenus() });
  const rows = (menus.data ?? []) as Menu[];

  // Land on the first menu so the page is never an empty shell.
  React.useEffect(() => {
    if (activeId === null && rows.length > 0 && rows[0] !== undefined) {
      setActiveId(rows[0].id);
    }
  }, [rows, activeId]);

  const remove = useMutation({
    mutationFn: (id: string) => api.deleteMenu(id),
    onSuccess: () => {
      setActiveId(null);
      void queryClient.invalidateQueries({ queryKey: ["menus"] });
      notify.success("Menu deleted");
    },
    onError: (e) => notify.error("Couldn't delete the menu", e),
  });

  const askDelete = async (m: Menu) => {
    const ok = await confirm({
      title: `Delete the “${m.name}” menu?`,
      description:
        "Its links are removed too, and any theme area showing this menu falls back to default navigation.",
      confirmLabel: "Delete menu",
      destructive: true,
    });
    if (ok) remove.mutate(m.id);
  };

  return (
    <div className="space-y-4 sm:space-y-5">
      <PageHeader
        title="Menus"
        description="The links your theme shows as site navigation."
        actions={
          <Button size="sm" onClick={() => setCreating(true)}>
            <Plus className="h-4 w-4" aria-hidden="true" />
            New menu
          </Button>
        }
      />

      {menus.isError ? <ErrorNote title="Couldn't load menus" error={menus.error} onRetry={() => void menus.refetch()} /> : menus.isPending ? (
        <Skeleton className="h-64 w-full rounded-lg" />
      ) : rows.length === 0 ? (
        <EmptyState
          icon={MenuIcon}
          title="No menus yet"
          description="Create a menu, then add links to posts, pages or anywhere else."
          action={
            <Button size="sm" variant="outline" onClick={() => setCreating(true)}>
              Create a menu
            </Button>
          }
        />
      ) : (
        <div className="grid gap-4 lg:grid-cols-[15rem_minmax(0,1fr)]">
          {/* Menu picker — a row of tabs on phones, a rail on desktop. */}
          <div className="scrollbar-thin -mx-3 flex gap-1.5 overflow-x-auto px-3 lg:mx-0 lg:flex-col lg:overflow-visible lg:px-0">
            {rows.map((m) => (
              <button
                key={m.id}
                type="button"
                onClick={() => setActiveId(m.id)}
                className={cn(
                  "touch-target shrink-0 rounded-lg border px-3 py-2 text-left text-sm transition-colors",
                  activeId === m.id
                    ? "border-primary bg-primary-subtle text-primary"
                    : "bg-card hover:bg-accent",
                )}
              >
                <span className="block truncate font-medium">{m.name}</span>
                <span className="block truncate font-mono text-[11px] opacity-70">
                  {m.slug}
                </span>
              </button>
            ))}
          </div>

          {activeId === null ? null : (
            <MenuItems
              menu={rows.find((m) => m.id === activeId) ?? rows[0]}
              onDeleteMenu={askDelete}
            />
          )}
        </div>
      )}

      <CreateMenuModal
        open={creating}
        onClose={() => setCreating(false)}
        onCreated={(id) => {
          setActiveId(id);
          void queryClient.invalidateQueries({ queryKey: ["menus"] });
        }}
      />
    </div>
  );
}

/** The ordered, nestable list of links inside one menu. */
function MenuItems({
  menu,
  onDeleteMenu,
}: {
  menu: Menu | undefined;
  onDeleteMenu: (m: Menu) => void;
}) {
  const queryClient = useQueryClient();
  const confirm = useConfirm();
  const menuId = menu?.id;

  const detail = useQuery({
    queryKey: ["menu", menuId],
    queryFn: () => api.getMenu(menuId as string),
    enabled: menuId !== undefined,
  });

  const items = (detail.data?.[1] ?? []) as Item[];
  const invalidate = () =>
    void queryClient.invalidateQueries({ queryKey: ["menu", menuId] });

  const add = useMutation({
    mutationFn: (body: { label: string; url: string }) =>
      api.addMenuItem(menuId as string, {
        ...body,
        sort_order: items.length,
      }),
    onSuccess: () => {
      invalidate();
      notify.success("Link added");
    },
    onError: (e) => notify.error("Couldn't add the link", e),
  });

  const update = useMutation({
    mutationFn: (args: {
      id: string;
      body: Parameters<typeof api.updateMenuItem>[1];
    }) => api.updateMenuItem(args.id, args.body),
    onSuccess: invalidate,
    onError: (e) => notify.error("Couldn't update the link", e),
  });

  const removeItem = useMutation({
    mutationFn: (id: string) => api.deleteMenuItem(id),
    onSuccess: () => {
      invalidate();
      notify.success("Link removed");
    },
    onError: (e) => notify.error("Couldn't remove the link", e),
  });

  const [label, setLabel] = React.useState("");
  const [url, setUrl] = React.useState("");

  // Parents first, each followed by its children, matching how it renders.
  const tree = React.useMemo(() => {
    const visited = new Set<string>();
    const walk = (parent: string | null, depth: number): { item: Item; depth: number }[] =>
      items.filter(i => i.parent_id === parent && !visited.has(i.id))
        .sort((a, b) => a.sort_order - b.sort_order)
        .flatMap(item => { visited.add(item.id); return [{ item, depth }, ...walk(item.id, depth + 1)]; });
    return walk(null, 0);
  }, [items]);

  const move = (item: Item, direction: -1 | 1) => {
    const siblings = items
      .filter((i) => i.parent_id === item.parent_id)
      .sort((a, b) => a.sort_order - b.sort_order);
    const index = siblings.findIndex((s) => s.id === item.id);
    const swapWith = siblings[index + direction];
    if (swapWith === undefined) return;
    // Swap the two sort_orders; the list re-sorts on refetch.
    update.mutate({ id: item.id, body: { sort_order: swapWith.sort_order } });
    update.mutate({ id: swapWith.id, body: { sort_order: item.sort_order } });
  };

  const descendants = (id: string): Item[] => items.filter(i => i.parent_id === id).flatMap(i => [i, ...descendants(i.id)]);
  const nestingParent = (item: Item) => {
    const siblings = items.filter(i => i.parent_id === item.parent_id).sort((a, b) => a.sort_order - b.sort_order);
    const parent = siblings[siblings.findIndex(i => i.id === item.id) - 1];
    const depth = tree.find(row => row.item.id === item.id)?.depth ?? 0;
    const maxDepth = Math.max(depth, ...descendants(item.id).map(child => tree.find(row => row.item.id === child.id)?.depth ?? depth));
    return maxDepth < 2 ? parent : undefined;
  };
  const indent = (item: Item) => {
    const parent = nestingParent(item);
    if (parent) update.mutate({ id: item.id, body: { parent_id: parent.id } });
  };

  const outdent = (item: Item) => {
    const parent = items.find(i => i.id === item.parent_id);
    update.mutate({ id: item.id, body: parent?.parent_id ? { parent_id: parent.parent_id } : { detach: true } });
  };

  const askRemove = async (item: Item) => {
    const children = descendants(item.id).length;
    const ok = await confirm({
      title: `Remove “${item.label}”?`,
      description:
        children > 0
          ? `Its ${children} nested ${children === 1 ? "link" : "links"} are removed too.`
          : "The link disappears from your site's navigation.",
      confirmLabel: "Remove",
      destructive: true,
    });
    if (ok) removeItem.mutate(item.id);
  };

  if (menu === undefined) return null;

  return (
    <div className="space-y-4">
      <div className="overflow-hidden rounded-lg border bg-card">
        <div className="flex items-center gap-2 border-b px-4 py-3">
          <div className="min-w-0 flex-1">
            <h2 className="truncate text-sm font-semibold">{menu.name}</h2>
            <p className="truncate text-xs text-muted-foreground">
              {items.length} {items.length === 1 ? "link" : "links"}
            </p>
          </div>
          <Button
            variant="ghost"
            size="sm"
            className="h-8 text-destructive hover:bg-destructive-subtle"
            onClick={() => onDeleteMenu(menu)}
          >
            <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
            Delete menu
          </Button>
        </div>

        {detail.isError ? <ErrorNote title="Couldn't load menu links" error={detail.error} onRetry={() => void detail.refetch()} /> : detail.isPending ? (
          <div className="p-4">
            <Skeleton className="h-24 w-full" />
          </div>
        ) : tree.length === 0 ? (
          <EmptyState
            icon={MenuIcon}
            title="No links yet"
            description="Add the first link below — it will appear in your site's navigation."
          />
        ) : (
          <ul className="divide-y">
            {tree.map(({ item, depth }, index) => (
              <li
                key={item.id}
                className="flex items-center gap-2 px-3 py-2"
                style={{ paddingLeft: `${0.75 + depth * 1.5}rem` }}
              >
                {depth > 0 ? (
                  <CornerDownRight
                    className="h-3.5 w-3.5 shrink-0 text-muted-foreground"
                    aria-hidden="true"
                  />
                ) : null}
                <div className="min-w-0 flex-1">
                  <CommitInput
                    value={item.label}
                    aria-label={`Label for ${item.label}`}
                    onCommit={(label) => update.mutate({ id: item.id, body: { label } })}
                    className="w-full truncate border-0 bg-transparent p-0 text-sm font-medium focus-visible:outline-none focus-visible:ring-0"
                  />
                  <CommitInput
                    value={item.url}
                    aria-label={`Address for ${item.label}`}
                    onCommit={(url) => update.mutate({ id: item.id, body: { url } })}
                    className="w-full truncate border-0 bg-transparent p-0 font-mono text-[11px] text-muted-foreground focus-visible:outline-none focus-visible:ring-0"
                  />
                </div>

                <div className="flex shrink-0 items-center gap-0.5">
                  <IconBtn
                    label={`Move ${item.label} up`}
                    disabled={index === 0}
                    onClick={() => move(item, -1)}
                  >
                    <ChevronUp className="h-3.5 w-3.5" aria-hidden="true" />
                  </IconBtn>
                  <IconBtn
                    label={`Move ${item.label} down`}
                    disabled={index === tree.length - 1}
                    onClick={() => move(item, 1)}
                  >
                    <ChevronDown className="h-3.5 w-3.5" aria-hidden="true" />
                  </IconBtn>
                  {nestingParent(item) ? (
                    <IconBtn
                      label={`Nest ${item.label} under the link above`}
                      onClick={() => indent(item)}
                    >
                      <span aria-hidden="true" className="text-xs">
                        →
                      </span>
                    </IconBtn>
                  ) : null}
                  {depth > 0 ? (
                    <IconBtn
                      label={`Move ${item.label} up one level`}
                      onClick={() => outdent(item)}
                    >
                      <span aria-hidden="true" className="text-xs">
                        ←
                      </span>
                    </IconBtn>
                  ) : null}
                  <IconBtn
                    label={`Remove ${item.label}`}
                    destructive
                    onClick={() => void askRemove(item)}
                  >
                    <Trash2 className="h-3.5 w-3.5" aria-hidden="true" />
                  </IconBtn>
                </div>
              </li>
            ))}
          </ul>
        )}

        <form
          className="flex flex-col gap-2 border-t bg-muted/40 p-3 sm:flex-row"
          onSubmit={(e) => {
            e.preventDefault();
            if (label.trim() === "" || url.trim() === "") return;
            add.mutate({ label: label.trim(), url: url.trim() });
            setLabel("");
            setUrl("");
          }}
        >
          <Input
            value={label}
            onChange={(e) => setLabel(e.target.value)}
            placeholder="Link text"
            aria-label="New link text"
            className="sm:max-w-[12rem]"
          />
          <Input
            value={url}
            onChange={(e) => setUrl(e.target.value)}
            placeholder="/about or https://…"
            aria-label="New link address"
            className="font-mono text-xs"
          />
          <Button
            type="submit"
            size="sm"
            disabled={label.trim() === "" || url.trim() === "" || add.isPending}
          >
            <Plus className="h-4 w-4" aria-hidden="true" />
            Add link
          </Button>
        </form>
      </div>
    </div>
  );
}

function IconBtn({
  label,
  onClick,
  disabled,
  destructive,
  children,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
  destructive?: boolean;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      title={label}
      className={cn(
        "inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground transition-colors disabled:opacity-30",
        destructive === true
          ? "hover:bg-destructive-subtle hover:text-destructive"
          : "hover:bg-accent hover:text-foreground",
      )}
    >
      {children}
    </button>
  );
}

function CreateMenuModal({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (id: string) => void;
}) {
  const [name, setName] = React.useState("");
  const [slug, setSlug] = React.useState("");
  const [slugTouched, setSlugTouched] = React.useState(false);
  const effectiveSlug = slugTouched ? slug : slugify(name);

  const create = useMutation({
    mutationFn: () => api.createMenu({ slug: effectiveSlug, name }),
    onSuccess: (created) => {
      notify.success(`“${name}” created`);
      setName("");
      setSlug("");
      setSlugTouched(false);
      onCreated(created.id);
      onClose();
    },
    onError: (e) => notify.error("Couldn't create the menu", e),
  });

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="New menu"
      description="Give it a name your team will recognise, like Main or Footer."
      footer={
        <>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button
            disabled={name.trim() === "" || effectiveSlug === "" || create.isPending}
            onClick={() => create.mutate()}
          >
            {create.isPending ? "Creating…" : "Create menu"}
          </Button>
        </>
      }
    >
      <div className="space-y-4">
        <Field label="Name" htmlFor="menu-name">
          <Input
            id="menu-name"
            autoFocus
            value={name}
            onChange={(e) => setName(e.target.value)}
            placeholder="Main navigation"
          />
        </Field>
        <Field
          label="Reference"
          htmlFor="menu-slug"
          hint="How your theme refers to this menu."
        >
          <Input
            id="menu-slug"
            value={effectiveSlug}
            onChange={(e) => {
              setSlugTouched(true);
              setSlug(e.target.value);
            }}
            placeholder="main"
            className="font-mono text-xs"
          />
        </Field>
      </div>
    </Modal>
  );
}
