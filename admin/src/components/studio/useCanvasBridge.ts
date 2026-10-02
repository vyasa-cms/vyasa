import * as React from "react";

/**
 * The canvas bridge: everything that turns a same-origin preview iframe
 * into a design surface — click to select, drag to reorder, double-click
 * to edit marked text, Delete/Escape, hover outlines, scroll kept across
 * re-renders.
 *
 * One implementation for the theme studio and the page editor, because
 * "the canvas behaves differently over there" is a bug class, not a
 * feature. It wires each *document* exactly once (a data flag on the
 * root element), so toggling the preview's colour scheme — which used to
 * re-run the wiring and stack duplicate listeners — now only flips the
 * theme attribute.
 */

const PICK_CSS = `
[data-vy-hover]{outline:1.5px dashed rgba(99,102,241,.7);outline-offset:-2px;}
[data-vy-selected]{outline:2px solid rgb(99,102,241);outline-offset:-2px;}
[data-vy-dragging]{opacity:.35;}
[data-vy-drop="before"]{box-shadow:inset 0 3px 0 0 rgb(99,102,241);}
[data-vy-drop="after"]{box-shadow:inset 0 -3px 0 0 rgb(99,102,241);}
body[data-vy-grabbing]{cursor:grabbing;user-select:none;}
[data-vy-edit]{cursor:text;}
[data-vy-edit][contenteditable]{outline:2px solid rgb(99,102,241);outline-offset:2px;border-radius:2px;cursor:text;}
[data-section] a,[data-section] img{-webkit-user-drag:none;user-drag:none;}
[data-vy-empty]{min-height:56px;display:flex;align-items:center;justify-content:center;outline:1.5px dashed rgba(99,102,241,.45);outline-offset:-6px;margin:4px 0;}
[data-vy-empty]::after{content:attr(data-vy-empty) " — nothing to show here yet";font:11px/1 system-ui,sans-serif;color:rgba(99,102,241,.85);letter-spacing:.02em;}
[data-vy-slot]{cursor:default;}
`;

/**
 * The `sandbox` every preview iframe carries.
 *
 * `allow-same-origin` without `allow-scripts`: the rendered page (theme
 * templates, theme JS, plugin output) never executes, so it cannot act
 * with the admin's session; the parent can still reach `contentDocument`
 * and attach the canvas listeners below, which run in the parent's realm.
 * Never add `allow-scripts` here — together with `allow-same-origin` the
 * frame could remove its own sandbox.
 *
 * Consequence: behaviour a theme implements in script (a mobile menu
 * drawer toggle, carousels, theme-side dark-mode switches) is inert in
 * the preview; the page renders in its initial, no-JS state.
 */
export const PREVIEW_SANDBOX = "allow-same-origin";

/** How far the pointer travels before a press counts as a drag, not a click. */
const DRAG_THRESHOLD_PX = 4;

/** The drag payload type the insert library writes. */
export const INSERT_MIME = "application/x-vyasa-section";

export interface CanvasBridge {
  /** Attach to the iframe's `onLoad`. */
  onFrameLoad: () => void;
  /** Section id under the pointer, for callers that surface it. */
  hovered: string | null;
}

export function useCanvasBridge({
  frameRef,
  html,
  dark,
  selectedId = null,
  onSelect,
  onMove,
  onInlineEdit,
  onDelete,
  onInsertKind,
}: {
  frameRef: React.RefObject<HTMLIFrameElement | null>;
  /** The srcDoc content; a change means a fresh document to wire. */
  html: string | null | undefined;
  dark: boolean;
  selectedId?: string | null;
  onSelect?: (id: string | null) => void;
  onMove?: (id: string, targetId: string, place: "before" | "after") => void;
  onInlineEdit?: (id: string, key: string, value: string) => void;
  onDelete?: (id: string) => void;
  /**
   * A kind dropped from the insert library. `targetId` is the section the
   * insertion line was drawn against, `null` for an empty stretch (append).
   */
  onInsertKind?: (kind: string, targetId: string | null, place: "before" | "after") => void;
}): CanvasBridge {
  const [hovered, setHovered] = React.useState<string | null>(null);
  const scrollRef = React.useRef(0);

  // Callbacks are usually inline arrows; refs stop the iframe being
  // re-wired on every render of the tool around it.
  const selectRef = React.useRef(onSelect);
  selectRef.current = onSelect;
  const moveRef = React.useRef(onMove);
  moveRef.current = onMove;
  const inlineEditRef = React.useRef(onInlineEdit);
  inlineEditRef.current = onInlineEdit;
  const deleteRef = React.useRef(onDelete);
  deleteRef.current = onDelete;
  const insertKindRef = React.useRef(onInsertKind);
  insertKindRef.current = onInsertKind;
  const selectedIdRef = React.useRef(selectedId);
  selectedIdRef.current = selectedId;
  const darkRef = React.useRef(dark);
  darkRef.current = dark;
  // A drag ends with a click event too. Without this the section you just
  // dropped would also be selected by the release that dropped it.
  const draggedRef = React.useRef(false);
  // The element being edited in place, if any. Lives on the hook, not in
  // the load handler, because a preview re-render replaces the document
  // and must not leave a stale editing session behind.
  const editingRef = React.useRef<{
    el: HTMLElement;
    sid: string;
    key: string;
    original: string;
  } | null>(null);

  /** Wires one document. Called once per document, guarded by a flag. */
  const wire = React.useCallback(() => {
    try {
      const win = frameRef.current?.contentWindow;
      const doc = frameRef.current?.contentDocument;
      if (win === null || win === undefined || doc === null || doc === undefined) return;
      // A fresh document loads with no theme attribute; stamp it before
      // anything else, whether or not this document still needs wiring —
      // the toggle effect below only covers changes *between* loads.
      doc.documentElement.setAttribute("data-theme", darkRef.current ? "dark" : "light");
      doc.documentElement.style.colorScheme = darkRef.current ? "dark" : "light";
      if (doc.documentElement.dataset["vyWired"] === "1") return;
      doc.documentElement.dataset["vyWired"] = "1";

      // The document was just replaced; any editing session died with it.
      editingRef.current = null;

      if (scrollRef.current > 0) win.scrollTo(0, scrollRef.current);
      win.addEventListener("scroll", () => {
        scrollRef.current = win.scrollY;
      });

      const style = doc.createElement("style");
      style.textContent = PICK_CSS;
      doc.head.append(style);

      // Links and images are natively draggable, and a native drag eats
      // every mousemove — which is why link- and image-heavy sections
      // felt undraggable while plain text bands worked. The canvas owns
      // dragging; the browser does not.
      doc.addEventListener("dragstart", (e) => e.preventDefault());

      const sectionAt = (target: EventTarget | null): string | null => {
        if (target === null || !(target instanceof doc.defaultView!.Element)) return null;
        return target.closest("[data-section]")?.getAttribute("data-section") ?? null;
      };

      // Editing text where it sits. The renderer marks what may be edited
      // (`data-vy-edit` names the settings key), so the canvas never
      // guesses which element is which setting.
      const finishEdit = () => {
        const editing = editingRef.current;
        if (editing === null) return;
        editingRef.current = null;
        editing.el.removeAttribute("contenteditable");
        const value = (editing.el.textContent ?? "").trim();
        if (value !== editing.original.trim()) {
          inlineEditRef.current?.(editing.sid, editing.key, value);
        }
      };
      const cancelEdit = () => {
        const editing = editingRef.current;
        if (editing === null) return;
        editingRef.current = null;
        editing.el.removeAttribute("contenteditable");
        editing.el.textContent = editing.original;
      };
      const startEdit = (el: HTMLElement, sid: string, key: string) => {
        finishEdit();
        editingRef.current = { el, sid, key, original: el.textContent ?? "" };
        // Plain text only: pasting markup into a headline must not become
        // markup in the settings. Browsers without the value fall back to
        // rich mode, and the commit reads textContent either way.
        el.setAttribute("contenteditable", "plaintext-only");
        if (!el.isContentEditable) el.setAttribute("contenteditable", "true");
        el.focus();
        const range = doc.createRange();
        range.selectNodeContents(el);
        const selection = win.getSelection();
        selection?.removeAllRanges();
        selection?.addRange(range);
        el.addEventListener("blur", finishEdit, { once: true });
      };

      // Dragging a section to a new place. Tracked by hand rather than
      // with HTML5 drag-and-drop, which cannot draw an insertion line
      // between two arbitrary elements and drags a ghost image nobody
      // asked for.
      let dragId: string | null = null;
      let dragging = false;
      let from = { x: 0, y: 0 };
      let drop: { id: string; place: "before" | "after" } | null = null;

      const el = (id: string) => doc.querySelector(`[data-section="${CSS.escape(id)}"]`);
      const clearMarks = () => {
        for (const m of doc.querySelectorAll("[data-vy-drop],[data-vy-dragging]")) {
          m.removeAttribute("data-vy-drop");
          m.removeAttribute("data-vy-dragging");
        }
        doc.body.removeAttribute("data-vy-grabbing");
      };
      const cancel = () => {
        dragId = null;
        dragging = false;
        drop = null;
        clearMarks();
      };

      doc.addEventListener("mousedown", (e) => {
        // Clear the suppression flag on every fresh press. A drag whose
        // release lands on a different element fires no click at all, so
        // a flag only cleared by that click stays set and eats the *next*
        // real one.
        draggedRef.current = false;
        // A press inside the text being edited moves the caret; it must
        // not begin a section drag.
        const editing = editingRef.current;
        if (
          editing !== null &&
          e.target instanceof doc.defaultView!.Node &&
          editing.el.contains(e.target)
        ) {
          return;
        }
        const id = sectionAt(e.target);
        if (id === null) return;
        // A slot-placed section (chrome, flat-theme blocks) is positioned
        // by the template, so dragging it on the canvas would be a lie.
        // It still selects; the layers rail still reorders the tree.
        if (
          e.target instanceof doc.defaultView!.Element &&
          e.target.closest("[data-vy-slot]") !== null
        ) {
          return;
        }
        dragId = id;
        dragging = false;
        from = { x: e.clientX, y: e.clientY };
      });

      doc.addEventListener("mousemove", (e) => {
        if (dragId === null) {
          setHovered(sectionAt(e.target));
          return;
        }
        if (!dragging) {
          if (Math.hypot(e.clientX - from.x, e.clientY - from.y) < DRAG_THRESHOLD_PX) return;
          dragging = true;
          setHovered(null);
          doc.body.setAttribute("data-vy-grabbing", "");
          el(dragId)?.setAttribute("data-vy-dragging", "");
        }
        for (const m of doc.querySelectorAll("[data-vy-drop]")) m.removeAttribute("data-vy-drop");
        drop = null;
        const over = sectionAt(e.target);
        if (over === null || over === dragId) return;
        const box = el(over)?.getBoundingClientRect();
        if (box === undefined) return;
        // Which half of the target the pointer is over decides whether
        // the section lands above it or below it.
        const place = e.clientY < box.top + box.height / 2 ? "before" : "after";
        el(over)?.setAttribute("data-vy-drop", place);
        drop = { id: over, place };
      });

      doc.addEventListener("mouseup", () => {
        const moved = dragging && dragId !== null && drop !== null;
        const [id, target] = [dragId, drop];
        draggedRef.current = dragging;
        cancel();
        if (moved && id !== null && target !== null) {
          moveRef.current?.(id, target.id, target.place);
        }
      });

      doc.addEventListener("mouseleave", () => {
        cancel();
        setHovered(null);
      });

      doc.addEventListener("click", (e) => {
        // A canvas selects; it does not browse. Without this, clicking a
        // heading in the preview navigates the iframe away from the page
        // being designed, and the author has to reload to get back.
        e.preventDefault();
        e.stopPropagation();
        if (draggedRef.current) {
          draggedRef.current = false;
          return;
        }
        const editing = editingRef.current;
        if (
          editing !== null &&
          e.target instanceof doc.defaultView!.Node &&
          editing.el.contains(e.target)
        ) {
          return;
        }
        selectRef.current?.(sectionAt(e.target));
      });

      // Dropping a card from the insert library. The drag starts in the
      // parent document; only the types are readable during dragover, the
      // payload arrives at drop. The insertion line reuses the reorder
      // markers, so dropping reads exactly like moving.
      doc.addEventListener("dragover", (e) => {
        if (!e.dataTransfer?.types.includes(INSERT_MIME)) return;
        e.preventDefault();
        e.dataTransfer.dropEffect = "copy";
        for (const m of doc.querySelectorAll("[data-vy-drop]")) m.removeAttribute("data-vy-drop");
        const over = sectionAt(e.target);
        if (over === null) return;
        const box = el(over)?.getBoundingClientRect();
        if (box === undefined) return;
        const place = e.clientY < box.top + box.height / 2 ? "before" : "after";
        el(over)?.setAttribute("data-vy-drop", place);
      });
      doc.addEventListener("drop", (e) => {
        const kind = e.dataTransfer?.getData(INSERT_MIME);
        if (kind === undefined || kind === "") return;
        e.preventDefault();
        let target: { id: string; place: "before" | "after" } | null = null;
        const over = sectionAt(e.target);
        if (over !== null) {
          const box = el(over)?.getBoundingClientRect();
          if (box !== undefined) {
            target = {
              id: over,
              place: e.clientY < box.top + box.height / 2 ? "before" : "after",
            };
          }
        }
        clearMarks();
        insertKindRef.current?.(kind, target?.id ?? null, target?.place ?? "after");
      });

      doc.addEventListener("dblclick", (e) => {
        if (!(e.target instanceof doc.defaultView!.Element)) return;
        const target = e.target.closest("[data-vy-edit]");
        const key = target?.getAttribute("data-vy-edit");
        const sid = sectionAt(e.target);
        if (
          !(target instanceof doc.defaultView!.HTMLElement) ||
          key === null ||
          key === undefined ||
          key === "" ||
          sid === null
        ) {
          return;
        }
        e.preventDefault();
        e.stopPropagation();
        selectRef.current?.(sid);
        startEdit(target, sid, key);
      });

      doc.addEventListener("keydown", (e) => {
        const editing = editingRef.current;
        if (editing !== null) {
          if (e.key === "Enter") {
            e.preventDefault();
            editing.el.blur();
          } else if (e.key === "Escape") {
            e.preventDefault();
            cancelEdit();
          }
          return;
        }
        if (e.key === "Escape") {
          selectRef.current?.(null);
          return;
        }
        if ((e.key === "Delete" || e.key === "Backspace") && selectedIdRef.current !== null) {
          e.preventDefault();
          deleteRef.current?.(selectedIdRef.current);
        }
      });
    } catch {
      // Not reachable same-origin — the canvas simply has no effect.
    }
  }, [frameRef]);

  // The colour scheme is an attribute, not wiring: flipping it must not
  // stack a second set of listeners (it used to).
  React.useEffect(() => {
    try {
      const doc = frameRef.current?.contentDocument;
      if (doc === null || doc === undefined) return;
      doc.documentElement.setAttribute("data-theme", dark ? "dark" : "light");
      doc.documentElement.style.colorScheme = dark ? "dark" : "light";
    } catch {
      // Cross-origin only; the toggle simply has no effect.
    }
  }, [frameRef, dark, html]);

  // Outlines follow state rather than being painted at click time, so the
  // canvas agrees with the tree no matter which of them started it.
  React.useEffect(() => {
    const doc = frameRef.current?.contentDocument;
    if (doc === null || doc === undefined) return;
    for (const [attr, id] of [
      ["data-vy-selected", selectedId],
      ["data-vy-hover", hovered],
    ] as const) {
      for (const marked of doc.querySelectorAll(`[${attr}]`)) marked.removeAttribute(attr);
      if (id === null) continue;
      doc.querySelector(`[data-section="${CSS.escape(id)}"]`)?.setAttribute(attr, "");
    }
    // A selection made in the tree (or a fresh insert) may point below
    // the fold. `nearest` keeps a click on the canvas from moving
    // anything — what was clicked is already in view.
    if (selectedId !== null) {
      doc
        .querySelector(`[data-section="${CSS.escape(selectedId)}"]`)
        ?.scrollIntoView?.({ block: "nearest", behavior: "smooth" });
    }
  }, [frameRef, selectedId, hovered, html]);

  // A new document may finish loading before React re-renders; wiring on
  // both the load event and the html change covers either order.
  React.useEffect(() => {
    wire();
  }, [wire, html, dark]);

  return { onFrameLoad: wire, hovered };
}
