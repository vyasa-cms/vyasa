import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { EntryTree, type EntryNode } from "@/components/entries/EntryTree";

afterEach(cleanup);

const n = (id: string, parent: string | null, position: number, title: string, status = "published"): EntryNode => ({ id, parent_id: parent, position, title, status });
// Guides (10)
//   Install (10)
// Intro (20)
// Draft (30, draft)
const nodes = [n("g", null, 10, "Guides"), n("i", "g", 10, "Install"), n("t", null, 20, "Intro"), n("d", null, 30, "Draft", "draft")];

const dataTransfer = () => ({ data: {} as Record<string, string>, setData(k: string, v: string) { this.data[k] = v; }, getData(k: string) { return this.data[k] ?? ""; }, effectAllowed: "" });

/** jsdom has no DragEvent, so `fireEvent.dragOver` drops `clientY`; build the event by hand. */
function dragOverAt(el: Element, clientY: number, dt: unknown) {
  const ev = new Event("dragover", { bubbles: true, cancelable: true });
  Object.assign(ev, { clientY, dataTransfer: dt });
  fireEvent(el, ev);
}

function setup(overrides: Partial<Parameters<typeof EntryTree>[0]> = {}) {
  const onMoves = vi.fn();
  render(<EntryTree nodes={nodes} onMoves={onMoves} href={(id) => `/posts/${id}`} {...overrides} />);
  return onMoves;
}

describe("EntryTree", () => {
  it("lists entries in reading order with their level and status", () => {
    setup();
    const items = screen.getAllByRole("treeitem");
    expect(items.map((i) => i.getAttribute("aria-level"))).toEqual(["1", "2", "1", "1"]);
    expect(items.map((i) => i.querySelector("a")?.textContent)).toEqual(["Guides", "Install", "Intro", "Draft"]);
    expect(items[3]).toHaveTextContent("draft");
    expect(items[0]?.querySelector("a")).toHaveAttribute("href", "/posts/g");
  });

  it("indents with the button and reports exactly the moves the server expects", () => {
    const onMoves = setup();
    fireEvent.click(screen.getByRole("button", { name: "Indent Intro" }));
    expect(onMoves).toHaveBeenCalledWith([{ id: "t", parent_id: "g", position: 20 }]);
  });

  it("disables moves that do nothing", () => {
    setup();
    expect(screen.getByRole("button", { name: "Move Guides up" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Outdent Guides" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Indent Guides" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Move Draft down" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Outdent Install" })).toBeEnabled();
  });

  it("moves up and down among siblings", () => {
    const onMoves = setup();
    fireEvent.click(screen.getByRole("button", { name: "Move Intro up" }));
    expect(onMoves).toHaveBeenLastCalledWith([
      { id: "t", parent_id: null, position: 10 },
      { id: "g", parent_id: null, position: 20 },
    ]);
  });

  it("drops a dragged entry inside another by the middle of the row", () => {
    const onMoves = setup();
    const items = screen.getAllByRole("treeitem");
    const [guides, , , draft] = items;
    if (guides === undefined || draft === undefined) throw new Error("rows");
    const dt = dataTransfer();
    guides.getBoundingClientRect = () => ({ top: 0, height: 100, bottom: 100, left: 0, right: 0, width: 0, x: 0, y: 0, toJSON: () => ({}) });
    fireEvent.dragStart(draft, { dataTransfer: dt });
    dragOverAt(guides, 50, dt);
    expect(guides).toHaveAttribute("data-drop", "inside");
    fireEvent.drop(guides, { dataTransfer: dt });
    expect(onMoves).toHaveBeenCalledWith([{ id: "d", parent_id: "g", position: 20 }]);
  });

  it("drops before a row by its top quarter and refuses a drop onto its own child", () => {
    const onMoves = setup();
    const [guides, install, , draft] = screen.getAllByRole("treeitem");
    if (guides === undefined || install === undefined || draft === undefined) throw new Error("rows");
    const dt = dataTransfer();
    const rect = { top: 0, height: 100, bottom: 100, left: 0, right: 0, width: 0, x: 0, y: 0, toJSON: () => ({}) };
    guides.getBoundingClientRect = () => rect;
    install.getBoundingClientRect = () => rect;
    fireEvent.dragStart(draft, { dataTransfer: dt });
    dragOverAt(guides, 5, dt);
    expect(guides).toHaveAttribute("data-drop", "before");
    fireEvent.drop(guides, { dataTransfer: dt });
    expect(onMoves).toHaveBeenLastCalledWith([
      { id: "d", parent_id: null, position: 10 },
      { id: "g", parent_id: null, position: 20 },
      { id: "t", parent_id: null, position: 30 },
    ]);
    onMoves.mockClear();
    fireEvent.dragStart(guides, { dataTransfer: dt });
    dragOverAt(install, 50, dt);
    fireEvent.drop(install, { dataTransfer: dt });
    expect(onMoves).not.toHaveBeenCalled();
  });

  it("moves with alt+arrows and ignores plain arrows", () => {
    const onMoves = setup();
    const intro = screen.getAllByRole("treeitem")[2];
    if (intro === undefined) throw new Error("row");
    fireEvent.keyDown(intro, { key: "ArrowUp" });
    expect(onMoves).not.toHaveBeenCalled();
    fireEvent.keyDown(intro, { key: "ArrowRight", altKey: true });
    expect(onMoves).toHaveBeenCalledWith([{ id: "t", parent_id: "g", position: 20 }]);
  });

  it("does nothing while a move is in flight", () => {
    const onMoves = setup({ busy: true });
    expect(screen.getByRole("button", { name: "Indent Intro" })).toBeDisabled();
    fireEvent.keyDown(screen.getAllByRole("treeitem")[2] as HTMLElement, { key: "ArrowRight", altKey: true });
    expect(onMoves).not.toHaveBeenCalled();
  });
});
