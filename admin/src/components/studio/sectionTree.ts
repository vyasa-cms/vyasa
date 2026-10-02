/**
 * Pure operations on a section tree.
 *
 * The layout is now nested, so "move this down" and "nest this under the one
 * above" are tree edits rather than array index swaps. Keeping them here —
 * pure, and away from React — is what makes them testable, and they are the
 * part most likely to lose a subtree if they are wrong.
 *
 * Every function returns a new tree; nothing mutates in place.
 */
import type { Section, Vocabulary } from "@/api/themes";

/** Every section in a tree, depth-first, parents before children. */
export function flatten(sections: Section[]): { section: Section; depth: number }[] {
  const out: { section: Section; depth: number }[] = [];
  const walk = (list: Section[], depth: number) => {
    for (const section of list) {
      out.push({ section, depth });
      walk(section.children ?? [], depth + 1);
    }
  };
  walk(sections, 0);
  return out;
}

/** Every id in a tree — ids must be unique across the whole template. */
export function allIds(sections: Section[]): string[] {
  return flatten(sections).map((n) => n.section.id);
}

/** An id derived from a kind that no section in the tree is using. */
export function uniqueId(kind: string, taken: string[]): string {
  const base = kind.replace(/[^a-z0-9-]/g, "-").replace(/^-+|-+$/g, "") || "section";
  if (!taken.includes(base)) return base;
  let n = 2;
  while (taken.includes(`${base}-${n}`)) n += 1;
  return `${base}-${n}`;
}

/** Whether a kind may hold children, per the server's vocabulary. */
export function isContainer(kind: string, vocab: Vocabulary): boolean {
  return vocab.blocks.find((b) => b.kind === kind)?.container === true;
}

/** Replaces the section with `id`, leaving the rest of the tree alone. */
export function updateSection(
  sections: Section[],
  id: string,
  update: (section: Section) => Section,
): Section[] {
  return sections.map((section) => {
    if (section.id === id) return update(section);
    if (section.children === undefined) return section;
    return { ...section, children: updateSection(section.children, id, update) };
  });
}

/** Removes the section with `id` and returns the tree without it. */
export function removeSection(sections: Section[], id: string): Section[] {
  return sections
    .filter((s) => s.id !== id)
    .map((s) =>
      s.children === undefined ? s : { ...s, children: removeSection(s.children, id) },
    );
}

/** The section with `id`, or undefined. */
export function findSection(sections: Section[], id: string): Section | undefined {
  for (const section of sections) {
    if (section.id === id) return section;
    const found = findSection(section.children ?? [], id);
    if (found !== undefined) return found;
  }
  return undefined;
}

/** The ids of the section's ancestors, outermost first. */
export function pathTo(sections: Section[], id: string, trail: string[] = []): string[] | null {
  for (const section of sections) {
    if (section.id === id) return trail;
    const deeper = pathTo(section.children ?? [], id, [...trail, section.id]);
    if (deeper !== null) return deeper;
  }
  return null;
}

/** The list a section belongs to, and its index within it. */
function locate(
  sections: Section[],
  id: string,
): { siblings: Section[]; index: number } | null {
  const index = sections.findIndex((s) => s.id === id);
  if (index !== -1) return { siblings: sections, index };
  for (const section of sections) {
    const found = locate(section.children ?? [], id);
    if (found !== null) return found;
  }
  return null;
}

/**
 * Moves a section up or down among its own siblings.
 *
 * Deliberately does not hop between parents: a move that silently changed a
 * section's container would be very hard to undo by eye.
 */
export function moveSection(
  sections: Section[],
  id: string,
  direction: -1 | 1,
): Section[] {
  const found = locate(sections, id);
  if (found === null) return sections;
  const to = found.index + direction;
  if (to < 0 || to >= found.siblings.length) return sections;

  const reorder = (list: Section[]): Section[] => {
    if (list !== found.siblings) {
      return list.map((s) =>
        s.children === undefined ? s : { ...s, children: reorder(s.children) },
      );
    }
    const next = [...list];
    const [item] = next.splice(found.index, 1);
    if (item === undefined) return list;
    next.splice(to, 0, item);
    return next;
  };
  return reorder(sections);
}

/**
 * Nests a section inside the container immediately above it.
 *
 * Returns the tree unchanged when there is nowhere to nest into — the caller
 * shows the control as disabled rather than failing silently.
 */
export function indentSection(
  sections: Section[],
  id: string,
  vocab: Vocabulary,
): Section[] {
  const found = locate(sections, id);
  if (found === null || found.index === 0) return sections;
  const target = found.siblings[found.index - 1];
  const moving = found.siblings[found.index];
  if (target === undefined || moving === undefined) return sections;
  if (!isContainer(target.kind, vocab)) return sections;

  const without = removeSection(sections, id);
  return updateSection(without, target.id, (s) => ({
    ...s,
    children: [...(s.children ?? []), moving],
  }));
}

/** Whether `indentSection` would change anything. */
export function canIndent(sections: Section[], id: string, vocab: Vocabulary): boolean {
  const found = locate(sections, id);
  if (found === null || found.index === 0) return false;
  const target = found.siblings[found.index - 1];
  return target !== undefined && isContainer(target.kind, vocab);
}

/**
 * Lifts a section out of its container, placing it just after that container.
 */
export function outdentSection(sections: Section[], id: string): Section[] {
  const parents = pathTo(sections, id);
  if (parents === null || parents.length === 0) return sections;
  const parentId = parents[parents.length - 1];
  const moving = findSection(sections, id);
  if (parentId === undefined || moving === undefined) return sections;

  const without = removeSection(sections, id);
  const insertAfter = (list: Section[]): Section[] =>
    list.flatMap((s) => {
      if (s.id === parentId) return [s, moving];
      if (s.children === undefined) return [s];
      return [{ ...s, children: insertAfter(s.children) }];
    });
  return insertAfter(without);
}

/** Whether `outdentSection` would change anything. */
export function canOutdent(sections: Section[], id: string): boolean {
  const parents = pathTo(sections, id);
  return parents !== null && parents.length > 0;
}

/**
 * Inserts a new section after `afterId`, or inside it when it is an empty
 * container the author just added.
 */
export function insertSection(
  sections: Section[],
  section: Section,
  afterId: string | null,
): Section[] {
  if (afterId === null) return [...sections, section];
  const insert = (list: Section[]): Section[] =>
    list.flatMap((s) => {
      if (s.id === afterId) return [s, section];
      if (s.children === undefined) return [s];
      return [{ ...s, children: insert(s.children) }];
    });
  return insert(sections);
}

/**
 * Inserts a new section beside `targetId`, on the given side.
 *
 * The drop-from-library case: the insertion line the canvas draws is a
 * promise about *where*, and "after the selection" would break it.
 */
export function insertRelativeTo(
  sections: Section[],
  section: Section,
  targetId: string,
  place: "before" | "after",
): Section[] {
  const insert = (list: Section[]): Section[] =>
    list.flatMap((s) => {
      if (s.id === targetId) return place === "before" ? [section, s] : [s, section];
      if (s.children === undefined) return [s];
      return [{ ...s, children: insert(s.children) }];
    });
  return insert(sections);
}

/** Adds a section as the last child of a container. */
export function appendChild(
  sections: Section[],
  parentId: string,
  child: Section,
): Section[] {
  return updateSection(sections, parentId, (s) => ({
    ...s,
    children: [...(s.children ?? []), child],
  }));
}

/**
 * Moves a section to sit immediately before or after another one.
 *
 * Unlike [`moveSection`], this does cross parents. That function refuses to,
 * on the grounds that a section silently changing container is hard to undo
 * by eye — but a drag onto a spot on the canvas is not silent, it is the
 * whole gesture, so here it is exactly what was asked for.
 *
 * Returns the tree unchanged when the move cannot mean anything: onto
 * itself, onto a section that is not there, or into its own subtree, which
 * would take the moving section's descendants out of the tree with it.
 */
export function moveRelativeTo(
  sections: Section[],
  id: string,
  targetId: string,
  place: "before" | "after",
): Section[] {
  if (id === targetId) return sections;
  const moving = findSection(sections, id);
  if (moving === undefined) return sections;
  const trail = pathTo(sections, targetId);
  if (trail === null || trail.includes(id)) return sections;

  const insert = (list: Section[]): Section[] => {
    const index = list.findIndex((s) => s.id === targetId);
    if (index !== -1) {
      const next = [...list];
      next.splice(place === "before" ? index : index + 1, 0, moving);
      return next;
    }
    return list.map((s) =>
      s.children === undefined ? s : { ...s, children: insert(s.children) },
    );
  };
  return insert(removeSection(sections, id));
}
