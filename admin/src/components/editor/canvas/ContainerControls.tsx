
import { syncChildren, type Attrs, type ContainerSpec } from "@/lib/pluginSchema";

/** The part of a block this control touches. */
export interface PaneBlock {
  kind: string;
  attrs: Attrs;
  children: PaneBlock[];
}

const smallButton = "rounded border px-2 py-1 text-xs hover:bg-muted disabled:opacity-50";

const emptyPane = (spec: ContainerSpec): PaneBlock => ({ kind: spec.child, attrs: {}, children: [] });

/**
 * Adds and removes the panes of a plugin container block (tabs, steps).
 *
 * A block with a `labels` attribute keeps one label per pane; one without
 * just has panes. Growing never touches existing panes; shrinking drops a
 * pane that has content only after `confirm` says yes.
 */
export function ContainerControls({
  spec,
  attrs,
  panes,
  onChange,
  confirm,
}: {
  spec: ContainerSpec;
  attrs: Attrs;
  panes: PaneBlock[];
  onChange: (attrs: Attrs, panes: PaneBlock[]) => void;
  confirm: (message: string) => Promise<boolean>;
}) {
  const labels = spec.labels === undefined ? undefined : Array.isArray(attrs[spec.labels]) ? (attrs[spec.labels] as string[]) : [];

  const add = () => {
    const next = syncChildren(panes, panes.length + 1, () => emptyPane(spec));
    onChange(spec.labels === undefined ? attrs : { ...attrs, [spec.labels]: [...(labels ?? []), `Tab ${(labels?.length ?? 0) + 1}`] }, next);
  };

  const removeAt = async (index: number) => {
    const doomed = panes[index];
    if (doomed !== undefined && doomed.children.length > 0 && !(await confirm("This pane has content. Remove it anyway?"))) return;
    const keep = panes.filter((_, i) => i !== index);
    onChange(spec.labels === undefined ? attrs : { ...attrs, [spec.labels]: (labels ?? []).filter((_, i) => i !== index) }, keep);
  };

  return (
    <div className="space-y-1" data-testid="container-controls">
      {labels === undefined
        ? null
        : labels.map((label, i) => (
            <div key={i} className="flex gap-1">
              <input
                className="h-8 w-full rounded-md border border-input bg-background px-2 text-sm"
                aria-label={`Pane ${i + 1} label`}
                value={label}
                onChange={(e) => onChange({ ...attrs, [spec.labels as string]: labels.map((l, j) => (j === i ? e.target.value : l)) }, panes)}
              />
              <button type="button" className={smallButton} aria-label={`Remove pane ${i + 1}`} onClick={() => void removeAt(i)}>
                Remove
              </button>
            </div>
          ))}
      <div className="flex gap-1">
        <button type="button" className={smallButton} aria-label="Add pane" onClick={add}>
          Add pane
        </button>
        {labels === undefined ? (
          <button type="button" className={smallButton} aria-label="Remove last pane" disabled={panes.length === 0} onClick={() => void removeAt(panes.length - 1)}>
            Remove last pane
          </button>
        ) : null}
      </div>
    </div>
  );
}
