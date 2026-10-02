import type { Editor } from "@tiptap/core";
import { notify } from "@/components/ui/toast";
import { inlineToHtml, type InlineNode } from "./inline";
import { docToBlocks, type PMNode } from "./serialize";

/**
 * Writing assistance that acts where the caret is: rewrite, shorten, expand
 * or fix a selection, or continue from the end. The server does the
 * prompting (`POST /api/v1/ai/assist/{kind}`); this file turns editor state
 * into a request and the reply back into a document edit.
 */

export type TextAssistKind = "rewrite" | "shorten" | "expand" | "fix" | "continue";

export const SELECTION_ASSISTS: { kind: TextAssistKind; label: string; hint: string }[] = [
  { kind: "rewrite", label: "Improve", hint: "Clearer, same meaning and length" },
  { kind: "shorten", label: "Shorten", hint: "About half as long" },
  { kind: "expand", label: "Expand", hint: "A sentence or two more" },
  { kind: "fix", label: "Fix grammar", hint: "Spelling and punctuation only" },
];

function friendly(message: string): string {
  if (message.includes("not_enough_content")) {
    return "Write a little more first — the assistant needs some context.";
  }
  if (message.includes("selection_too_short")) return "Select a few words first.";
  if (message.toLowerCase().includes("api key") || message.includes("no text model")) {
    return "AI isn't set up yet: register a text model under AI models.";
  }
  return message;
}

export async function assistText(
  kind: TextAssistKind,
  content: unknown,
  selection?: string,
): Promise<string> {
  const response = await fetch(`/api/v1/ai/assist/${kind}`, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ content, selection }),
  });
  const body = (await response.json().catch(() => null)) as
    | { text?: string; message?: string }
    | null;
  if (!response.ok) {
    throw new Error(friendly(body?.message ?? `assist failed (${response.status})`));
  }
  if (typeof body?.text !== "string" || body.text.trim() === "") {
    throw new Error("The assistant returned nothing usable.");
  }
  return body.text;
}

function documentOf(editor: Editor): unknown {
  return {
    schema_version: 1,
    blocks: docToBlocks(editor.getJSON() as unknown as PMNode),
  };
}

/**
 * The selection as the HTML the server understands, so marks inside it
 * survive the round trip when the selection stays within one block. Across
 * blocks it degrades to plain text.
 */
function selectionHtml(editor: Editor): string {
  const { from, to, $from, $to } = editor.state.selection;
  if ($from.sameParent($to)) {
    const slice = editor.state.doc.cut(from, to);
    const block = slice.firstChild;
    if (block !== null && block.isTextblock) {
      return inlineToHtml(block.content.toJSON() as InlineNode[] | undefined);
    }
  }
  return editor.state.doc.textBetween(from, to, "\n", " ");
}

/** Replaces the selection with the assistant's version of it. */
export async function runSelectionAssist(editor: Editor, kind: TextAssistKind): Promise<boolean> {
  const { from, to, empty } = editor.state.selection;
  if (empty) {
    notify.error("Select some text first");
    return false;
  }
  // What was selected when the request left. The reply lands where the
  // selection was; if the author kept typing meanwhile, those positions
  // now cover different words, and replacing them would eat the new text.
  const original = editor.state.doc.textBetween(from, to, "\n", " ");
  try {
    const text = await assistText(kind, documentOf(editor), selectionHtml(editor));
    const now = editor.state.doc;
    if (to > now.content.size || now.textBetween(from, to, "\n", " ") !== original) {
      notify.error("The text changed while the assistant was working", "Select it again and retry.");
      return false;
    }
    // The reply is inline HTML (or plain text, which is also valid HTML);
    // ProseMirror keeps the marks the schema knows and drops the rest.
    editor
      .chain()
      .focus()
      .insertContentAt({ from, to }, text.replace(/\n+/g, "<br>"))
      .run();
    return true;
  } catch (e) {
    notify.error("The assistant couldn't help with that", e);
    return false;
  }
}

/** Appends a paragraph continuing from the end of the current block. */
export async function runContinue(editor: Editor): Promise<boolean> {
  try {
    const text = await assistText("continue", documentOf(editor));
    const paragraphs = text
      .split(/\n{2,}/)
      .map((p) => p.trim())
      .filter((p) => p !== "")
      .map((p) => ({
        type: "paragraph",
        content: [{ type: "text", text: p.replace(/\s*\n\s*/g, " ") }],
      }));
    if (paragraphs.length === 0) return false;
    const { $to } = editor.state.selection;
    const after = $to.after(Math.max(1, $to.depth));
    editor.chain().focus().insertContentAt(after, paragraphs).run();
    return true;
  } catch (e) {
    notify.error("The assistant couldn't continue", e);
    return false;
  }
}
