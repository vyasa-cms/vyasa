import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { ContainerControls, type PaneBlock } from "@/components/editor/canvas/ContainerControls";
import { PluginAttrsForm } from "@/components/editor/canvas/PluginAttrsForm";

afterEach(cleanup);

const paramsSchema = {
  type: "object",
  properties: {
    rows: {
      type: "array",
      title: "Parameters",
      items: {
        type: "object",
        properties: {
          name: { type: "string" },
          type: { type: "string" },
          required: { type: "boolean" },
          default: { type: "string" },
          description: { type: "string", format: "inline-html" },
        },
      },
    },
  },
};

describe("PluginAttrsForm", () => {
  it("renders nothing without a schema", () => {
    const { container } = render(<PluginAttrsForm schema={undefined} attrs={{}} onChange={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });

  it("adds a blank row to a rows field", () => {
    const onChange = vi.fn();
    render(<PluginAttrsForm schema={paramsSchema} attrs={{}} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Add Parameters" }));
    expect(onChange).toHaveBeenCalledWith({ rows: [{ name: "", type: "", required: false, default: "", description: "" }] });
  });

  it("edits one cell and toggles a boolean without touching other rows", () => {
    const onChange = vi.fn();
    const rows = [
      { name: "id", type: "string", required: false, default: "", description: "" },
      { name: "n", type: "number", required: false, default: "", description: "" },
    ];
    render(<PluginAttrsForm schema={paramsSchema} attrs={{ rows }} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Parameters 2 Name"), { target: { value: "limit" } });
    expect(onChange).toHaveBeenLastCalledWith({ rows: [rows[0], { ...rows[1], name: "limit" }] });
    fireEvent.click(screen.getByLabelText("Parameters 1 Required"));
    expect(onChange).toHaveBeenLastCalledWith({ rows: [{ ...rows[0], required: true }, rows[1]] });
  });

  it("removes a row, and removes the attribute when the last row goes", () => {
    const onChange = vi.fn();
    render(<PluginAttrsForm schema={paramsSchema} attrs={{ rows: [{ name: "id" }] }} onChange={onChange} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove Parameters 1" }));
    expect(onChange).toHaveBeenCalledWith({});
  });

  it("edits a list of strings and a code field", () => {
    const onChange = vi.fn();
    const schema = {
      type: "object",
      properties: { labels: { type: "array", title: "Tab labels", items: { type: "string" } }, tree: { type: "string", format: "code", title: "Tree" } },
    };
    render(<PluginAttrsForm schema={schema} attrs={{ labels: ["npm"], tree: "a/" }} onChange={onChange} />);
    fireEvent.change(screen.getByLabelText("Tab labels 1"), { target: { value: "pnpm" } });
    expect(onChange).toHaveBeenLastCalledWith({ labels: ["pnpm"], tree: "a/" });
    fireEvent.click(screen.getByRole("button", { name: "Add Tab labels" }));
    expect(onChange).toHaveBeenLastCalledWith({ labels: ["npm", ""], tree: "a/" });
    fireEvent.change(screen.getByLabelText("Tree"), { target: { value: "b/\n  c" } });
    expect(onChange).toHaveBeenLastCalledWith({ labels: ["npm"], tree: "b/\n  c" });
    expect(screen.getByLabelText("Tree").tagName).toBe("TEXTAREA");
  });

  it("applies a JSON field on blur and refuses bad JSON without changing anything", () => {
    const onChange = vi.fn();
    const schema = { type: "object", properties: { extra: { type: "object", title: "Extra" } } };
    render(<PluginAttrsForm schema={schema} attrs={{}} onChange={onChange} />);
    const box = screen.getByLabelText("Extra");
    fireEvent.change(box, { target: { value: "{oops" } });
    fireEvent.blur(box);
    expect(screen.getByRole("alert")).toHaveTextContent("Not valid JSON");
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.change(box, { target: { value: '{"a": 1}' } });
    fireEvent.blur(box);
    expect(onChange).toHaveBeenCalledWith({ extra: { a: 1 } });
  });
});

const pane = (n: number, filled = false): PaneBlock => ({ kind: "group", attrs: {}, children: filled ? [{ kind: "paragraph", attrs: { text: `p${n}` }, children: [] }] : [] });

describe("ContainerControls", () => {
  const tabs = { child: "group", labels: "labels" };

  it("adds a label and a pane together and keeps the first pane as it was", () => {
    const onChange = vi.fn();
    const first = pane(1, true);
    render(<ContainerControls spec={tabs} attrs={{ labels: ["npm"] }} panes={[first]} onChange={onChange} confirm={async () => true} />);
    fireEvent.click(screen.getByRole("button", { name: "Add pane" }));
    const [attrs, panes] = onChange.mock.calls[0] as [Record<string, unknown>, PaneBlock[]];
    expect(attrs).toEqual({ labels: ["npm", "Tab 2"] });
    expect(panes).toHaveLength(2);
    expect(panes[0]).toBe(first);
  });

  it("asks before removing a pane that has content, and does nothing on no", async () => {
    const onChange = vi.fn();
    const confirm = vi.fn().mockResolvedValue(false);
    render(<ContainerControls spec={tabs} attrs={{ labels: ["a", "b"] }} panes={[pane(1, true), pane(2)]} onChange={onChange} confirm={confirm} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove pane 1" }));
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1));
    expect(onChange).not.toHaveBeenCalled();
    confirm.mockResolvedValue(true);
    fireEvent.click(screen.getByRole("button", { name: "Remove pane 1" }));
    await waitFor(() => expect(onChange).toHaveBeenCalledTimes(1));
    const [attrs, panes] = onChange.mock.calls[0] as [Record<string, unknown>, PaneBlock[]];
    expect(attrs).toEqual({ labels: ["b"] });
    expect(panes).toHaveLength(1);
  });

  it("removes an empty pane without asking", async () => {
    const onChange = vi.fn();
    const confirm = vi.fn();
    render(<ContainerControls spec={tabs} attrs={{ labels: ["a", "b"] }} panes={[pane(1, true), pane(2)]} onChange={onChange} confirm={confirm} />);
    fireEvent.click(screen.getByRole("button", { name: "Remove pane 2" }));
    await waitFor(() => expect(onChange).toHaveBeenCalledTimes(1));
    expect(confirm).not.toHaveBeenCalled();
  });

  it("renames a label without touching the panes", () => {
    const onChange = vi.fn();
    const panes = [pane(1)];
    render(<ContainerControls spec={tabs} attrs={{ labels: ["a"] }} panes={panes} onChange={onChange} confirm={async () => true} />);
    fireEvent.change(screen.getByLabelText("Pane 1 label"), { target: { value: "pnpm" } });
    expect(onChange).toHaveBeenCalledWith({ labels: ["pnpm"] }, panes);
  });

  it("steps have no labels: add and remove-last act on the panes alone", async () => {
    const onChange = vi.fn();
    const confirm = vi.fn().mockResolvedValue(true);
    render(<ContainerControls spec={{ child: "group" }} attrs={{}} panes={[pane(1), pane(2, true)]} onChange={onChange} confirm={confirm} />);
    expect(screen.queryByLabelText("Pane 1 label")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Add pane" }));
    expect(onChange.mock.calls[0]?.[0]).toEqual({});
    expect((onChange.mock.calls[0]?.[1] as PaneBlock[]).length).toBe(3);
    fireEvent.click(screen.getByRole("button", { name: "Remove last pane" }));
    await waitFor(() => expect(confirm).toHaveBeenCalled());
    await waitFor(() => expect(onChange).toHaveBeenCalledTimes(2));
    expect((onChange.mock.calls[1]?.[1] as PaneBlock[]).length).toBe(1);
  });
});
