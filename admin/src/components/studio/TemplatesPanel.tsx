import * as React from "react";
import { Code2, RotateCcw } from "lucide-react";

import type { Vocabulary } from "@/api/themes";
import { Button } from "@/components/ui/button";
import { MonacoPane } from "@/components/editor/MonacoPane";
import { Chip } from "@/components/ui/primitives";
import { cn } from "@/lib/utils";

/**
 * Tera overrides for the seven page templates. The built-in source is
 * always visible, so "customise" starts from the real thing rather than a
 * blank file; edits apply on demand, because a template mid-edit is
 * usually not a valid template yet.
 */
export function TemplatesPanel({
  templates,
  vocab,
  onApply,
  onRemove,
}: {
  templates: Record<string, string>;
  vocab: Vocabulary;
  onApply: (name: string, source: string) => void;
  onRemove: (name: string) => void;
}) {
  const [selected, setSelected] = React.useState("single.html");
  const builtin = vocab.template_files.find((f) => f.name === selected)?.builtin_source ?? "";
  const override = templates[selected];
  const saved = override ?? builtin;
  const [source, setSource] = React.useState(saved);
  // Follow the selection and outside changes (a revert, the assistant).
  React.useEffect(() => setSource(saved), [saved, selected]);
  const dirty = source !== saved;

  return (
    <div className="flex h-full flex-col gap-2" data-testid="templates-panel">
      <ul className="flex flex-wrap gap-1" aria-label="Template files">
        {vocab.template_files.map((f) => {
          const custom = templates[f.name] !== undefined;
          return (
            <li key={f.name}>
              <button
                type="button"
                aria-pressed={selected === f.name}
                onClick={() => setSelected(f.name)}
                className={cn(
                  "inline-flex items-center gap-1 rounded-md px-2 py-1 font-mono text-[11px]",
                  selected === f.name
                    ? "bg-primary text-primary-foreground"
                    : "text-muted-foreground hover:bg-accent hover:text-foreground",
                )}
              >
                {f.name}
                {custom ? (
                  <span
                    className={cn(
                      "h-1.5 w-1.5 rounded-full",
                      selected === f.name ? "bg-primary-foreground" : "bg-primary",
                    )}
                    aria-label="customised"
                  />
                ) : null}
              </button>
            </li>
          );
        })}
      </ul>

      <div className="flex items-center gap-2 text-xs">
        {override === undefined ? (
          <Chip tone="neutral">Built-in</Chip>
        ) : (
          <Chip tone="info">Customised</Chip>
        )}
        <span className="text-muted-foreground">
          {override === undefined
            ? "Edit to override this template for the theme."
            : "This theme overrides the built-in template."}
        </span>
      </div>

      <div className="min-h-0 flex-1 overflow-hidden rounded-md border">
        <MonacoPane
          value={source}
          language="html"
          height={420}
          onChange={setSource}
          ariaLabel={`${selected} source`}
        />
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button
          size="sm"
          disabled={!dirty}
          onClick={() => onApply(selected, source)}
          data-testid="apply-template"
        >
          <Code2 className="h-3.5 w-3.5" aria-hidden="true" />
          Apply
        </Button>
        {dirty ? (
          <Button size="sm" variant="ghost" onClick={() => setSource(saved)}>
            Discard
          </Button>
        ) : null}
        {override !== undefined ? (
          <Button
            size="sm"
            variant="outline"
            className="ml-auto"
            onClick={() => onRemove(selected)}
          >
            <RotateCcw className="h-3.5 w-3.5" aria-hidden="true" />
            Use built-in
          </Button>
        ) : null}
      </div>
      <p className="text-[11px] text-muted-foreground">
        Templates run in a sandbox: no environment access, and includes are
        limited to this theme&rsquo;s files. Compile errors show above the preview.
      </p>
    </div>
  );
}
