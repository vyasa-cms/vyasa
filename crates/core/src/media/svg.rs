//! Safe SVG uploads: an allow-list sanitiser.
//!
//! An SVG is a document that can carry script, embedded HTML (`foreignObject`), styles with
//! `@import`, and references to other origins. The sanitiser parses the upload as XML and writes
//! back only what is on the allow-lists below: drawing elements, presentation attributes, and
//! references to an id inside the same file (`#id`). Everything else is dropped with its subtree.
//! A document that is not well-formed XML, has a DOCTYPE (entities), or whose root is not
//! `<svg>` is refused. The result is what gets stored; the original bytes are never kept.

use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer};
use vyasa_common::AppError;

const SVG_NS: &str = "http://www.w3.org/2000/svg";
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";
/// Deepest element nesting kept; deeper is refused.
const MAX_DEPTH: usize = 64;
/// Most `<use>` elements kept: references can nest, so their number bounds the expansion.
const MAX_USE: usize = 16;

/// Elements kept. Anything else (script, foreignObject, style, image, a, animate, set, iframe,
/// filter, pattern, marker ...) is removed with its children.
const ELEMENTS: &[&str] = &[
    "svg",
    "g",
    "defs",
    "symbol",
    "use",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "title",
    "desc",
    "linearGradient",
    "radialGradient",
    "stop",
    "clipPath",
    "mask",
];

/// Attributes kept (presentation and geometry). No `style`, no `on*`, no `class`.
const ATTRIBUTES: &[&str] = &[
    "id",
    "viewBox",
    "preserveAspectRatio",
    "width",
    "height",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "fx",
    "fy",
    "d",
    "points",
    "transform",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "stroke-dasharray",
    "stroke-dashoffset",
    "opacity",
    "color",
    "clip-path",
    "clip-rule",
    "clipPathUnits",
    "mask",
    "maskUnits",
    "maskContentUnits",
    "offset",
    "stop-color",
    "stop-opacity",
    "gradientUnits",
    "gradientTransform",
    "spreadMethod",
    "display",
    "visibility",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "letter-spacing",
    "dx",
    "dy",
    "version",
    "role",
    "aria-label",
    "aria-hidden",
    "aria-labelledby",
];

fn refuse(why: &str) -> AppError {
    AppError::validation(format!("The SVG cannot be used: {why}."))
}

/// Functions an attribute value may call. Anything else (`image-set()`, `image()`, `element()`,
/// `src()`, `cross-fade()`, `paint()` ...) can load a resource or is not needed for a logo.
const FUNCTIONS: &[&str] = &[
    "url",
    "rgb",
    "rgba",
    "hsl",
    "hsla",
    "hwb",
    "lab",
    "lch",
    "oklab",
    "oklch",
    "color",
    "matrix",
    "translate",
    "translatex",
    "translatey",
    "scale",
    "scalex",
    "scaley",
    "rotate",
    "skew",
    "skewx",
    "skewy",
    "calc",
];

/// A value that may only point inside the file: no scheme, no external url(), no script, no
/// CSS escape (`u\72 l(` is `url(` to a browser), only known functions.
fn value_is_safe(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    if value.contains('\\')
        || lower.contains("javascript:")
        || lower.contains("data:")
        || lower.contains("expression(")
        || lower.contains("@import")
        || lower.contains('<')
    {
        return false;
    }
    // Every function call is on the list, and every url(...) is a fragment reference.
    let bytes = lower.as_bytes();
    for (i, _) in lower.match_indices('(') {
        let name_start = bytes[..i]
            .iter()
            .rposition(|b| !(b.is_ascii_alphanumeric() || *b == b'-'))
            .map_or(0, |p| p + 1);
        let name = &lower[name_start..i];
        if !FUNCTIONS.contains(&name) {
            return false;
        }
        if name == "url" {
            let inner = lower[i + 1..]
                .trim_start()
                .trim_start_matches(['\'', '"'])
                .trim_start();
            if !inner.starts_with('#') {
                return false;
            }
        }
    }
    true
}

/// Rebuilds a start tag with only the allowed attributes; `None` when the element is not allowed.
fn clean_start(e: &BytesStart<'_>, root: bool) -> Result<Option<BytesStart<'static>>, AppError> {
    let name = e.name().as_ref().to_owned();
    if !ELEMENTS.contains(&name.as_str()) {
        return Ok(None);
    }
    let mut out = BytesStart::new(name.clone());
    for attr in e.attributes() {
        let attr = attr.map_err(|_| refuse("an attribute is malformed"))?;
        let key = attr.key.as_ref().to_owned();
        let value = attr
            .normalized_value(quick_xml::XmlVersion::Implicit1_0)
            .map_err(|_| refuse("an attribute value is malformed"))?
            .into_owned();
        let keep = match key.as_str() {
            // Written once, below, on the root.
            "xmlns" | "xmlns:xlink" => false,
            // A reference to something in the same file, on the elements that take one.
            "href" | "xlink:href" => {
                matches!(name.as_str(), "use" | "linearGradient" | "radialGradient")
                    && value.starts_with('#')
            }
            k => ATTRIBUTES.contains(&k) && value_is_safe(&value),
        };
        if keep {
            out.push_attribute((key.as_str(), value.as_str()));
        }
    }
    if root {
        // A standalone SVG needs its namespace to render; always the SVG one.
        out.push_attribute(("xmlns", SVG_NS));
        out.push_attribute(("xmlns:xlink", XLINK_NS));
    }
    Ok(Some(out.into_owned()))
}

/// The sanitised bytes of an SVG upload.
///
/// # Errors
///
/// `AppError::Validation` when the file is not well-formed XML, declares a DOCTYPE, is not
/// UTF-8, nests deeper than 64 elements, or its root element is not `<svg>`.
pub fn sanitize(bytes: &[u8]) -> Result<Vec<u8>, AppError> {
    let text = std::str::from_utf8(bytes).map_err(|_| refuse("it is not UTF-8"))?;
    let mut reader = Reader::from_str(text);
    let mut writer = Writer::new(Vec::with_capacity(bytes.len()));
    // Depth of kept elements, and how many levels of a dropped subtree we are inside.
    let mut depth = 0usize;
    let mut skipping = 0usize;
    let mut saw_root = false;
    let mut uses = 0usize;
    loop {
        let event = reader
            .read_event()
            .map_err(|_| refuse("it is not well-formed XML"))?;
        match event {
            Event::Eof => break,
            Event::DocType(_) => return Err(refuse("a DOCTYPE is not allowed")),
            Event::Start(e) => {
                if skipping > 0 {
                    skipping += 1;
                    continue;
                }
                if depth == 0 && (saw_root || e.name().as_ref() != "svg") {
                    return Err(refuse("the root element must be <svg>"));
                }
                match clean_start(&e, depth == 0)? {
                    Some(start) => {
                        if start.name().as_ref() == "use" {
                            uses += 1;
                        }
                        if uses > MAX_USE || depth >= MAX_DEPTH {
                            return Err(refuse("it is nested too deeply or uses <use> too often"));
                        }
                        saw_root = true;
                        depth += 1;
                        write(&mut writer, Event::Start(start))?;
                    }
                    None => skipping = 1,
                }
            }
            Event::Empty(e) => {
                if skipping > 0 {
                    continue;
                }
                if depth == 0 && (saw_root || e.name().as_ref() != "svg") {
                    return Err(refuse("the root element must be <svg>"));
                }
                if let Some(start) = clean_start(&e, depth == 0)? {
                    if start.name().as_ref() == "use" {
                        uses += 1;
                        if uses > MAX_USE {
                            return Err(refuse("it uses <use> too often"));
                        }
                    }
                    saw_root = true;
                    write(&mut writer, Event::Empty(start))?;
                }
            }
            Event::End(e) => {
                if skipping > 0 {
                    skipping -= 1;
                    continue;
                }
                depth = depth.saturating_sub(1);
                let name = e.name().as_ref().to_owned();
                write(&mut writer, Event::End(BytesEnd::new(name)))?;
            }
            Event::Text(t) if skipping == 0 && depth > 0 => {
                let s = t.xml10_content();
                write(&mut writer, Event::Text(BytesText::new(&s)))?;
            }
            Event::GeneralRef(r) if skipping == 0 && depth > 0 => {
                let name = r.xml10_content();
                let ch = match name.as_ref() {
                    "amp" => "&",
                    "lt" => "<",
                    "gt" => ">",
                    "quot" => "\"",
                    "apos" => "'",
                    _ => match r.resolve_char_ref() {
                        Ok(Some(c)) => {
                            write(&mut writer, Event::Text(BytesText::new(&c.to_string())))?;
                            continue;
                        }
                        _ => return Err(refuse("it uses an entity")),
                    },
                };
                write(&mut writer, Event::Text(BytesText::new(ch)))?;
            }
            Event::CData(c) if skipping == 0 && depth > 0 => {
                let s = c.xml10_content();
                write(&mut writer, Event::Text(BytesText::new(&s)))?;
            }
            // Comments, processing instructions, the XML declaration, text outside the root.
            _ => {}
        }
    }
    if !saw_root || depth != 0 {
        return Err(refuse("it has no <svg> element"));
    }
    Ok(writer.into_inner())
}

fn write(w: &mut Writer<Vec<u8>>, e: Event<'_>) -> Result<(), AppError> {
    w.write_event(e)
        .map_err(|_| refuse("it could not be written back"))
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    fn clean(s: &str) -> String {
        String::from_utf8(sanitize(s.as_bytes()).expect("sanitised")).expect("utf8")
    }

    const LOGO: &str = r##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><defs><linearGradient id="g"><stop offset="0" stop-color="#f00"/></linearGradient></defs><title>Logo</title><path d="M0 0h10v10z" fill="url(#g)"/><use href="#g"/></svg>"##;

    #[test]
    fn a_plain_logo_survives() {
        let out = clean(LOGO);
        for kept in [
            "viewBox=\"0 0 10 10\"",
            "<path d=\"M0 0h10v10z\"",
            "fill=\"url(#g)\"",
            "<use href=\"#g\"",
            "<title>Logo</title>",
        ] {
            assert!(out.contains(kept), "{kept} in {out}");
        }
        assert!(!out.contains("<?xml"));
    }

    #[test]
    fn script_foreign_object_and_style_go_with_their_children() {
        let out = clean(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><foreignObject><div xmlns="http://www.w3.org/1999/xhtml"><b>x</b></div></foreignObject><style>@import url(http://evil/x.css);</style><g><circle r="1"/></g></svg>"#,
        );
        assert_eq!(
            out,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><g><circle r="1"/></g></svg>"#
        );
    }

    #[test]
    fn handlers_styles_and_external_references_are_dropped() {
        let out = clean(
            r#"<svg xmlns="http://www.w3.org/2000/svg" onload="x()"><rect onclick="x()" style="fill:red" class="a" width="1" fill="url(http://evil/a.svg#x)" stroke="javascript:alert(1)"/><use href="http://evil/a.svg#x"/><use xlink:href="data:image/svg+xml,x"/><image href="a.png"/><a href="javascript:x()"><rect width="2"/></a></svg>"#,
        );
        assert_eq!(
            out,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><rect width="1"/><use/><use/></svg>"#
        );
    }

    #[test]
    fn animation_and_filters_that_load_things_are_removed() {
        let out = clean(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1"><set attributeName="href" to="javascript:x()"/><animate attributeName="fill" values="a;b"/></rect><filter id="f"><feImage href="http://evil/x"/></filter></svg>"#,
        );
        assert_eq!(
            out,
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><rect width="1"></rect></svg>"#
        );
    }

    #[test]
    fn doctype_entities_and_wrong_roots_are_refused() {
        for bad in [
            r#"<!DOCTYPE svg [<!ENTITY x "y">]><svg xmlns="http://www.w3.org/2000/svg">&x;</svg>"#,
            "<html><body/></html>",
            r#"<svg xmlns="http://www.w3.org/2000/svg"><g></svg>"#,
            r#"<svg xmlns="http://www.w3.org/2000/svg"/><svg xmlns="http://www.w3.org/2000/svg"/>"#,
            "not xml at all",
            "",
        ] {
            assert!(sanitize(bad.as_bytes()).is_err(), "{bad}");
        }
        assert!(sanitize(&[0xff, 0xfe, 0x00]).is_err(), "not UTF-8");
    }

    #[test]
    fn text_is_escaped_and_cdata_becomes_text() {
        let out = clean(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><text>a &amp; b &lt;<![CDATA[<script>]]></text></svg>"#,
        );
        assert!(out.contains("a &amp; b &lt;&lt;script&gt;"), "{out}");
        assert!(!out.contains("<script"));
    }

    #[test]
    fn deep_nesting_is_refused() {
        let deep = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg">{}{}</svg>"#,
            "<g>".repeat(100),
            "</g>".repeat(100)
        );
        assert!(sanitize(deep.as_bytes()).is_err());
    }

    #[test]
    fn css_escapes_and_image_functions_are_dropped() {
        let out = clean(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect fill="u\72 l(http://evil/x.svg#p)" stroke="u\rl(http://evil/y)" mask="image-set('https://evil/a.png' 1x)" clip-path="-webkit-image-set(x)" width="1"/><rect fill="rgb(1,2,3)" mask="url(#m)" transform="translate(1 2) rotate(3)"/></svg>"#,
        );
        assert!(!out.contains("evil"), "{out}");
        assert!(out.contains("fill=\"rgb(1,2,3)\""), "{out}");
        assert!(out.contains("mask=\"url(#m)\""), "{out}");
        assert!(
            out.contains("transform=\"translate(1 2) rotate(3)\""),
            "{out}"
        );
    }

    #[test]
    fn the_root_always_binds_both_namespaces_and_use_is_capped() {
        let out = clean(r##"<svg><use xlink:href="#a"/></svg>"##);
        assert!(
            out.contains("xmlns=\"http://www.w3.org/2000/svg\""),
            "{out}"
        );
        assert!(
            out.contains("xmlns:xlink=\"http://www.w3.org/1999/xlink\""),
            "{out}"
        );
        let many = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg">{}</svg>"#,
            r##"<use href="#a"/>"##.repeat(17)
        );
        assert!(sanitize(many.as_bytes()).is_err());
        let few = many.replacen(r##"<use href="#a"/>"##, "", 1);
        assert!(sanitize(few.as_bytes()).is_ok());
    }
}
