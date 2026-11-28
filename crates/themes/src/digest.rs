//! A rendered page as an agent reads it: the section order and the visible
//! text, not a wall of markup.

/// Longest digest handed back; the harness truncates further anyway.
const MAX_TEXT: usize = 2_800;

/// Removes everything between `open`…`close` pairs, inclusive.
fn strip_blocks(html: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        match rest[start..].find(close) {
            Some(end) => rest = &rest[start + end + close.len()..],
            None => {
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// Reduces rendered HTML to what a reader would take in.
#[must_use]
pub fn page_digest(html: &str) -> String {
    // Section order first: it answers "did my edit land where I meant".
    let mut order: Vec<&str> = Vec::new();
    for chunk in html.split("data-section=\"").skip(1) {
        if let Some(end) = chunk.find('"') {
            order.push(&chunk[..end]);
        }
    }

    // Strip style/script wholesale, then tags, then collapse whitespace.
    let source = strip_blocks(
        &strip_blocks(html, "<style", "</style>"),
        "<script",
        "</script>",
    );
    let source = source.as_str();
    let mut text = String::new();
    let mut in_tag = false;
    let mut last_space = true;
    for c in source.chars() {
        match c {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                if !last_space {
                    text.push(' ');
                    last_space = true;
                }
            }
            c if in_tag => {
                let _ = c;
            }
            c if c.is_whitespace() => {
                if !last_space {
                    text.push(' ');
                    last_space = true;
                }
            }
            c => {
                text.push(c);
                last_space = false;
            }
        }
    }
    let text: String = text.chars().take(MAX_TEXT).collect();

    format!(
        "sections in order: {}\n\nvisible text:\n{}",
        if order.is_empty() {
            "(none carried markers)".to_owned()
        } else {
            order.join(" → ")
        },
        text.trim()
    )
}

#[cfg(test)]
mod tests {
    use super::page_digest;

    #[test]
    fn a_digest_reads_like_the_page_not_like_its_markup() {
        let html = r#"<html><head><style>.x{color:red}</style></head><body>
            <div data-section="hero"><h1>Objects for slow homes</h1>
            <script>evil()</script><p>Hand-thrown stoneware.</p></div>
            <div data-section="grid"><a href="/p">Clay mug</a></div>
        </body></html>"#;
        let digest = page_digest(html);
        assert!(digest.contains("hero → grid"), "{digest}");
        assert!(digest.contains("Objects for slow homes"), "{digest}");
        assert!(digest.contains("Clay mug"), "{digest}");
        assert!(!digest.contains("color:red"), "styles stripped: {digest}");
        assert!(!digest.contains("evil"), "scripts stripped: {digest}");
        assert!(!digest.contains("<h1>"), "tags stripped: {digest}");
    }
}
