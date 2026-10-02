import { describe, expect, it } from "vitest";
import { cleanPastedHtml } from "@/components/editor/paste";

describe("cleanPastedHtml", () => {
  it("turns Google Docs styled spans into real marks", () => {
    // The shape Docs actually puts on the clipboard: a bold-in-name-only
    // wrapper, and emphasis expressed only as inline styles.
    const docs =
      '<b style="font-weight:normal" id="docs-internal-guid-abc123">' +
      '<p><span style="font-weight:700">Bold words</span> and ' +
      '<span style="font-style:italic">italic ones</span> and ' +
      '<span style="font-weight:400">plain ones</span>.</p></b>';
    const out = cleanPastedHtml(docs);
    expect(out).toContain("<strong>Bold words</strong>");
    expect(out).toContain("<em>italic ones</em>");
    expect(out).toContain("plain ones");
    expect(out).not.toContain("<b");
    expect(out).not.toContain("docs-internal-guid");
  });

  it("handles combined weight+style and strikethrough", () => {
    const out = cleanPastedHtml(
      '<span style="font-weight:600;font-style:italic">both</span>' +
        '<span style="text-decoration:line-through">gone</span>',
    );
    expect(out).toContain("<em><strong>both</strong></em>");
    expect(out).toContain("<s>gone</s>");
  });

  it("drops style and meta junk but keeps real structure", () => {
    const out = cleanPastedHtml(
      "<meta charset='utf-8'><style>p{color:red}</style>" +
        "<h2>Heading</h2><ul><li>one</li></ul><p><a href='/x'>link</a></p>",
    );
    expect(out).not.toContain("color:red");
    expect(out).toContain("<h2>Heading</h2>");
    expect(out).toContain("<li>one</li>");
    expect(out).toContain('<a href="/x">link</a>');
  });

  it("leaves honest bold tags alone", () => {
    const out = cleanPastedHtml("<p><b>really bold</b> and <strong>this</strong></p>");
    expect(out).toContain("<b>really bold</b>");
    expect(out).toContain("<strong>this</strong>");
  });
});
