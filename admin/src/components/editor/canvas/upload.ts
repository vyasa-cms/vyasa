import type { Editor } from "@tiptap/core";
import type { Node as PMNode } from "@tiptap/pm/model";
import { api } from "@/api/client";
import { notify } from "@/components/ui/toast";
import { mediaUrl } from "../blocks";
import { isImageFile, prepareImage } from "../prepareImage";
export { isImageFile } from "../prepareImage";

/**
 * Image uploads from the canvas: drop a file, paste one, or pick one in the
 * image block. The node appears immediately with a local preview and its
 * `url` is filled in when the upload lands; undo never reverts the URL,
 * because "the image I just added is suddenly blank" is not an undo anyone
 * asked for.
 */

export function imageFilesFrom(transfer: DataTransfer | null): File[] {
  if (transfer === null) return [];
  return Array.from(transfer.files).filter(isImageFile);
}

function uid(): string {
  return Math.random().toString(36).slice(2, 10);
}

function findByUploadId(doc: PMNode, uploadId: string): { pos: number; node: PMNode } | null {
  let found: { pos: number; node: PMNode } | null = null;
  doc.descendants((node, pos) => {
    if (found !== null) return false;
    if (node.type.name === "vyImage" && node.attrs["uploadId"] === uploadId) {
      found = { pos, node };
      return false;
    }
    return true;
  });
  return found;
}

function patch(editor: Editor, uploadId: string, attrs: Record<string, unknown>): boolean {
  const hit = findByUploadId(editor.state.doc, uploadId);
  if (hit === null) return false;
  editor.view.dispatch(
    editor.state.tr
      .setNodeMarkup(hit.pos, undefined, { ...hit.node.attrs, ...attrs })
      .setMeta("addToHistory", false),
  );
  return true;
}

async function upload(editor: Editor, uploadId: string, file: File, preview: string) {
  try {
    const media = await api.uploadMedia(await prepareImage(file));
    const hit = findByUploadId(editor.state.doc, uploadId);
    const currentAlt = typeof hit?.node.attrs["alt"] === "string" ? hit.node.attrs["alt"] : "";
    patch(editor, uploadId, {
      url: mediaUrl(media.id),
      pending: null,
      alt: currentAlt !== "" ? currentAlt : (media.alt ?? ""),
    });
  } catch (e) {
    notify.error(`Couldn't upload ${file.name}`, e);
    const hit = findByUploadId(editor.state.doc, uploadId);
    if (hit !== null && hit.node.attrs["url"] === "") {
      // Nothing to show: take the placeholder out rather than leave a
      // broken figure behind.
      editor.view.dispatch(
        editor.state.tr.delete(hit.pos, hit.pos + hit.node.nodeSize).setMeta("addToHistory", false),
      );
    } else {
      patch(editor, uploadId, { pending: null });
    }
  } finally {
    // Give the swapped-in URL a moment to paint before the preview goes.
    setTimeout(() => URL.revokeObjectURL(preview), 1000);
  }
}

/** Inserts one image node per file at `at` (default: the selection). */
export function insertImages(editor: Editor, files: File[], at?: number): void {
  let pos = at ?? editor.state.selection.to;
  for (const file of files) {
    if (!isImageFile(file)) continue;
    const uploadId = uid();
    const preview = URL.createObjectURL(file);
    editor
      .chain()
      .insertContentAt(pos, {
        type: "vyImage",
        attrs: { url: "", alt: "", pending: preview, uploadId },
      })
      .run();
    const hit = findByUploadId(editor.state.doc, uploadId);
    if (hit !== null) pos = hit.pos + hit.node.nodeSize;
    void upload(editor, uploadId, file, preview);
  }
}

/** Uploads into an existing image node, replacing whatever it showed. */
export function replaceImage(editor: Editor, pos: number, file: File): void {
  if (!isImageFile(file)) {
    notify.error("That isn't an image", `${file.name} is ${file.type || "an unknown type"}.`);
    return;
  }
  const node = editor.state.doc.nodeAt(pos);
  if (node === null || node.type.name !== "vyImage") return;
  const uploadId = typeof node.attrs["uploadId"] === "string" ? node.attrs["uploadId"] : uid();
  const preview = URL.createObjectURL(file);
  editor.view.dispatch(
    editor.state.tr.setNodeMarkup(pos, undefined, { ...node.attrs, uploadId, pending: preview }),
  );
  void upload(editor, uploadId, file, preview);
}
