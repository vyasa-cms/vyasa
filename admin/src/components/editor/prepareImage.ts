/**
 * Makes a picked or dropped photo uploadable.
 *
 * Phones hand the editor two things the server cannot take as they are: a
 * HEIC photo (the iPhone default, which the server refuses) and a photo past
 * the 10 MiB cap. Where the browser can decode the file — Safari reads HEIC;
 * every browser reads a large JPEG — it is redrawn to a JPEG that fits,
 * upright, with the camera metadata (and its location) left out. Where it
 * cannot, the file goes up as it is and the server's message says why.
 */

/** The server's cap, mirrored so a phone photo is shrunk rather than refused. */
export const MAX_UPLOAD_BYTES = 10 * 1024 * 1024;
/** Longest side after a shrink: plenty for any theme, a quarter of a 48 MP shot. */
const MAX_EDGE = 4000;

export function isHeic(file: File): boolean {
  return (
    file.type === "image/heic" ||
    file.type === "image/heif" ||
    (file.type === "" && /\.hei[cf]$/i.test(file.name))
  );
}

/** Whether a file is a picture, by type or, when the phone omits it, by name. */
export function isImageFile(file: File): boolean {
  return file.type.startsWith("image/") || (file.type === "" && /\.(jpe?g|png|gif|webp|avif|hei[cf])$/i.test(file.name));
}

export function needsPreparation(file: File): boolean {
  return isHeic(file) || (isImageFile(file) && file.size > MAX_UPLOAD_BYTES);
}

async function decode(file: File): Promise<ImageBitmap> {
  // `from-image` applies the EXIF rotation, so what is redrawn is upright.
  return createImageBitmap(file, { imageOrientation: "from-image" } as ImageBitmapOptions);
}

function toBlob(canvas: HTMLCanvasElement, quality: number): Promise<Blob | null> {
  return new Promise((resolve) => canvas.toBlob(resolve, "image/jpeg", quality));
}

/**
 * The file to upload: the original when it is fine, or a JPEG redrawn to
 * fit. Throws when a conversion was needed and the browser could not do it.
 */
export async function prepareImage(file: File): Promise<File> {
  if (!needsPreparation(file)) return file;
  if (typeof createImageBitmap === "undefined") {
    throw new Error(unpreparable(file));
  }
  let bitmap: ImageBitmap;
  try {
    bitmap = await decode(file);
  } catch {
    throw new Error(unpreparable(file));
  }
  try {
    let scale = Math.min(1, MAX_EDGE / Math.max(bitmap.width, bitmap.height));
    for (let attempt = 0; attempt < 4; attempt += 1) {
      const canvas = document.createElement("canvas");
      canvas.width = Math.max(1, Math.round(bitmap.width * scale));
      canvas.height = Math.max(1, Math.round(bitmap.height * scale));
      const ctx = canvas.getContext("2d");
      if (ctx === null) throw new Error(unpreparable(file));
      ctx.drawImage(bitmap, 0, 0, canvas.width, canvas.height);
      const blob = await toBlob(canvas, attempt === 0 ? 0.88 : 0.8);
      if (blob !== null && blob.size <= MAX_UPLOAD_BYTES) {
        const name = file.name.replace(/\.[^.]+$/, "") || "photo";
        return new File([blob], `${name}.jpg`, { type: "image/jpeg" });
      }
      scale *= 0.7;
    }
    throw new Error(`${file.name} is too large even after shrinking; the limit is 10 MB.`);
  } finally {
    bitmap.close();
  }
}

function unpreparable(file: File): string {
  return isHeic(file)
    ? `${file.name} is a HEIC photo this browser can't convert. Set the camera to Most Compatible, or export it as JPEG.`
    : `${file.name} is larger than 10 MB and this browser can't shrink it.`;
}
