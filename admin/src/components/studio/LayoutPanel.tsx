import * as React from "react";

import {
  TEMPLATE_TYPES,
  type Layout,
  type Section,
  type TemplateType,
  type TokenSet,
  type Vocabulary,
} from "@/api/themes";
import { cn } from "@/lib/utils";
import { SectionTree } from "./SectionTree";

const TEMPLATE_LABELS: Record<TemplateType, string> = {
  index: "Home",
  single: "Post",
  archive: "Archive",
  page: "Page",
  search: "Search",
  "not-found": "Not found",
};

/**
 * A theme's layout: one section tree per template type.
 *
 * The tree editor itself is [`SectionTree`], shared with the page editor —
 * a template and a single page compose the same way, so they get the same
 * controls. All this adds is which of the six trees you are looking at.
 */
export function LayoutPanel({
  layout,
  vocab,
  tokens,
  onChange,
  template: controlledTemplate,
  onTemplateChange,
  selected,
  onSelectedChange,
  inlineProperties,
  showAdd,
}: {
  layout: Layout;
  vocab: Vocabulary;
  tokens: TokenSet;
  onChange: (template: TemplateType, blocks: Section[]) => void;
  /**
   * Which tree to show, when the canvas decides.
   *
   * Section ids are only unique within a template — every template has a
   * `header` — so a click on the page has to say which tree it came from
   * or the wrong `header` opens.
   */
  template?: TemplateType;
  onTemplateChange?: (t: TemplateType) => void;
  selected?: string | null;
  onSelectedChange?: (id: string | null) => void;
  /** False when the properties live somewhere else, as in the studio. */
  inlineProperties?: boolean;
  /** False when inserting lives somewhere else, as in the studio. */
  showAdd?: boolean;
}) {
  const [own, setOwn] = React.useState<TemplateType>("index");
  const template = controlledTemplate ?? own;
  const setTemplate = (t: TemplateType) => {
    setOwn(t);
    onTemplateChange?.(t);
  };

  return (
    <div className="space-y-3" data-testid="layout-panel">
      <div role="tablist" aria-label="Template" className="flex flex-wrap gap-1">
        {TEMPLATE_TYPES.map((t) => (
          <button
            key={t}
            role="tab"
            aria-selected={template === t}
            onClick={() => setTemplate(t)}
            className={cn(
              "rounded-md px-2 py-1 text-xs",
              template === t
                ? "bg-primary text-primary-foreground font-medium"
                : "text-muted-foreground hover:bg-accent hover:text-foreground",
            )}
          >
            {TEMPLATE_LABELS[t]}
          </button>
        ))}
      </div>

      <SectionTree
        key={template}
        sections={layout[template]}
        vocab={vocab}
        tokens={tokens}
        selected={selected}
        onSelectedChange={onSelectedChange}
        inlineProperties={inlineProperties}
        showAdd={showAdd}
        onChange={(next) => onChange(template, next)}
        label={`${TEMPLATE_LABELS[template]} sections`}
        emptyHint="No sections yet. Add one to start composing this template."
      />
    </div>
  );
}
