import { NodeViewContent, NodeViewWrapper, type ReactNodeViewProps } from "@tiptap/react";
import { CODE_LANGUAGES } from "./languages";

/**
 * A code block with its language picker in the corner. The picker sits
 * outside the editable region so arrow keys and typing never land on it.
 */
export function CodeBlockView({ node, updateAttributes }: ReactNodeViewProps) {
  const language = typeof node.attrs["language"] === "string" ? node.attrs["language"] : "";
  return (
    <NodeViewWrapper className="vy-codeblock relative">
      <select
        contentEditable={false}
        value={language}
        onChange={(e) => updateAttributes({ language: e.target.value === "" ? null : e.target.value })}
        onMouseDown={(e) => e.stopPropagation()}
        aria-label="Code language"
        className="absolute right-2 top-2 z-10 h-6 rounded border bg-background px-1.5 font-sans text-[11px] text-muted-foreground focus-visible:outline-hidden focus-visible:ring-1 focus-visible:ring-ring"
      >
        {CODE_LANGUAGES.map((l) => (
          <option key={l.value} value={l.value}>
            {l.label}
          </option>
        ))}
      </select>
      <pre spellCheck={false}>
        <NodeViewContent<"code"> as="code" />
      </pre>
    </NodeViewWrapper>
  );
}
