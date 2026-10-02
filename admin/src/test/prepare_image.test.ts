import { describe, expect, it } from "vitest";
import { isHeic, isImageFile, needsPreparation, prepareImage, MAX_UPLOAD_BYTES } from "@/components/editor/prepareImage";

const file = (name: string, type: string, size = 10) =>
  new File([new Uint8Array(size)], name, { type });

describe("preparing phone photos", () => {
  it("recognises pictures the phone hands over without a type", () => {
    expect(isImageFile(file("IMG_0001.HEIC", ""))).toBe(true);
    expect(isImageFile(file("a.jpg", ""))).toBe(true);
    expect(isImageFile(file("notes.txt", ""))).toBe(false);
    expect(isHeic(file("IMG_0001.HEIC", ""))).toBe(true);
    expect(isHeic(file("a.heif", "image/heif"))).toBe(true);
  });

  it("leaves an ordinary photo alone", async () => {
    const f = file("a.jpg", "image/jpeg");
    expect(needsPreparation(f)).toBe(false);
    expect(await prepareImage(f)).toBe(f);
  });

  it("wants to shrink a photo past the cap and convert a HEIC", () => {
    expect(needsPreparation(file("big.jpg", "image/jpeg", MAX_UPLOAD_BYTES + 1))).toBe(true);
    expect(needsPreparation(file("a.heic", "image/heic"))).toBe(true);
  });

  it("says why when the browser cannot convert", async () => {
    // jsdom has no createImageBitmap, which is exactly a browser that cannot.
    await expect(prepareImage(file("a.heic", "image/heic"))).rejects.toThrow(/Most Compatible/);
    await expect(prepareImage(file("big.jpg", "image/jpeg", MAX_UPLOAD_BYTES + 1))).rejects.toThrow(
      /larger than 10 MB/,
    );
  });
});
