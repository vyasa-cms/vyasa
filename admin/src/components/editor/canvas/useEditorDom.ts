import * as React from "react";
import type { Editor } from "@tiptap/core";

function mountedDom(editor: Editor): HTMLElement | null {
  try {
    return editor.view.dom as HTMLElement;
  } catch {
    // TipTap throws until `EditorContent` has mounted the view.
    return null;
  }
}

/**
 * The editor's DOM element, or null until the view exists.
 *
 * `useEditor` hands out the editor before `EditorContent` mounts its view,
 * and sibling effects run before that mount — so `editor.view.dom` in an
 * effect throws on first render. Subscribing to `mount` means listeners
 * attach exactly once the element is there.
 */
export function useEditorDom(editor: Editor): HTMLElement | null {
  const [dom, setDom] = React.useState<HTMLElement | null>(() => mountedDom(editor));

  React.useEffect(() => {
    setDom(mountedDom(editor));
    const onMount = () => setDom(mountedDom(editor));
    const onUnmount = () => setDom(null);
    editor.on("mount", onMount);
    editor.on("unmount", onUnmount);
    return () => {
      editor.off("mount", onMount);
      editor.off("unmount", onUnmount);
    };
  }, [editor]);

  return dom;
}
