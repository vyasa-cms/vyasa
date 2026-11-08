//! The MDX-like whitelist, as a line-based pre-pass.
//!
//! Real MDX is JavaScript; this is not. Only the seven tag names below are
//! recognised, only at the start of a line and outside code fences, and
//! attribute values are plain strings (`{...}` is read as its text). A tag
//! must open and close on a single line. Everything else is Markdown.

/// The recognised component names.
pub const NAMES: &[&str] = &["Callout", "Tabs", "Tab", "Steps", "Step", "Params", "Param"];

/// A parsed fragment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    /// Markdown text, to be parsed by the Markdown pass.
    Md(String),
    /// A recognised component.
    Comp {
        /// Tag name.
        name: String,
        /// Attributes, in source order.
        attrs: Vec<(String, String)>,
        /// Nested content.
        children: Vec<Node>,
    },
}

impl Node {
    /// An attribute's value on a component.
    #[must_use]
    pub fn attr(&self, key: &str) -> Option<&str> {
        match self {
            Node::Comp { attrs, .. } => attrs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.as_str()),
            Node::Md(_) => None,
        }
    }
}

struct Open {
    name: String,
    attrs: Vec<(String, String)>,
    children: Vec<Node>,
    buf: String,
}

impl Open {
    fn flush(&mut self) {
        if !self.buf.trim().is_empty() {
            self.children.push(Node::Md(std::mem::take(&mut self.buf)));
        } else {
            self.buf.clear();
        }
    }
}

/// Parses `src` into nodes plus warnings.
#[must_use]
pub fn parse(src: &str) -> (Vec<Node>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut stack: Vec<Open> = vec![Open {
        name: String::new(),
        attrs: Vec::new(),
        children: Vec::new(),
        buf: String::new(),
    }];
    let mut fence: Option<(char, usize)> = None;

    for line in src.split_inclusive('\n') {
        let trimmed = line.trim_start();
        // Code fences hide everything inside them.
        if let Some((ch, len)) = fence {
            let run = trimmed.chars().take_while(|c| *c == ch).count();
            if run >= len && trimmed[run..].trim().is_empty() {
                fence = None;
            }
            push_line(&mut stack, line);
            continue;
        }
        for ch in ['`', '~'] {
            let run = trimmed.chars().take_while(|c| *c == ch).count();
            if run >= 3 {
                fence = Some((ch, run));
            }
        }
        if fence.is_some() {
            push_line(&mut stack, line);
            continue;
        }
        match classify(trimmed.trim_end()) {
            Line::Text => push_line(&mut stack, line),
            Line::Bad(why) => {
                warnings.push(why);
                push_line(&mut stack, line);
            }
            Line::Close(name) => {
                if !stack.iter().skip(1).any(|o| o.name == name) {
                    warnings.push(format!("</{name}> closes nothing; kept as text"));
                    push_line(&mut stack, line);
                    continue;
                }
                while stack.len() > 1 {
                    let top = stack.last().map(|o| o.name.clone()).unwrap_or_default();
                    close_top(&mut stack);
                    if top == name {
                        break;
                    }
                    warnings.push(format!("<{top}> was not closed before </{name}>"));
                }
            }
            Line::Open {
                name,
                attrs,
                self_closing,
                rest,
            } => {
                if let Some(top) = stack.last_mut() {
                    top.flush();
                }
                if self_closing {
                    if let Some(top) = stack.last_mut() {
                        top.children.push(Node::Comp {
                            name,
                            attrs,
                            children: Vec::new(),
                        });
                    }
                    continue;
                }
                let closing = format!("</{name}>");
                if let Some(inner) = rest.trim_end().strip_suffix(closing.as_str()) {
                    let mut children = Vec::new();
                    if !inner.trim().is_empty() {
                        children.push(Node::Md(format!("{}\n", inner.trim())));
                    }
                    if let Some(top) = stack.last_mut() {
                        top.children.push(Node::Comp {
                            name,
                            attrs,
                            children,
                        });
                    }
                    continue;
                }
                let mut open = Open {
                    name,
                    attrs,
                    children: Vec::new(),
                    buf: String::new(),
                };
                if !rest.trim().is_empty() {
                    open.buf.push_str(rest.trim());
                    open.buf.push('\n');
                }
                stack.push(open);
            }
        }
    }
    while stack.len() > 1 {
        let top = stack.last().map(|o| o.name.clone()).unwrap_or_default();
        warnings.push(format!(
            "<{top}> was never closed; closed at the end of the page"
        ));
        close_top(&mut stack);
    }
    let mut root = stack.pop().unwrap_or(Open {
        name: String::new(),
        attrs: Vec::new(),
        children: Vec::new(),
        buf: String::new(),
    });
    root.flush();
    (root.children, warnings)
}

fn push_line(stack: &mut [Open], line: &str) {
    if let Some(top) = stack.last_mut() {
        top.buf.push_str(line);
    }
}

fn close_top(stack: &mut Vec<Open>) {
    if stack.len() < 2 {
        return;
    }
    let Some(mut done) = stack.pop() else { return };
    done.flush();
    if let Some(parent) = stack.last_mut() {
        parent.flush();
        parent.children.push(Node::Comp {
            name: done.name,
            attrs: done.attrs,
            children: done.children,
        });
    }
}

enum Line {
    Text,
    Bad(String),
    Close(String),
    Open {
        name: String,
        attrs: Vec<(String, String)>,
        self_closing: bool,
        rest: String,
    },
}

fn classify(line: &str) -> Line {
    if let Some(rest) = line.strip_prefix("</") {
        if let Some(name) = rest.strip_suffix('>') {
            if NAMES.contains(&name.trim()) {
                return Line::Close(name.trim().to_owned());
            }
        }
        return Line::Text;
    }
    let Some(rest) = line.strip_prefix('<') else {
        return Line::Text;
    };
    let name: String = rest
        .chars()
        .take_while(char::is_ascii_alphanumeric)
        .collect();
    if !NAMES.contains(&name.as_str()) {
        return Line::Text;
    }
    let after = &rest[name.len()..];
    match after.chars().next() {
        Some(c) if c.is_whitespace() || c == '>' || c == '/' => {}
        None => {
            return Line::Bad(format!(
                "<{name}> tag does not end on its line; kept as text"
            ))
        }
        Some(_) => return Line::Text,
    }
    match scan_tag(after) {
        Some((attr_src, self_closing, remainder)) => Line::Open {
            name,
            attrs: parse_attrs(attr_src),
            self_closing,
            rest: remainder.to_owned(),
        },
        None => Line::Bad(format!(
            "<{name}> tag does not end on its line; kept as text"
        )),
    }
}

/// Finds the `>` that ends the tag, honouring quotes and braces.
/// Returns (attribute text, self-closing, text after the tag).
fn scan_tag(after: &str) -> Option<(&str, bool, &str)> {
    let mut quote: Option<char> = None;
    let mut depth = 0usize;
    for (i, c) in after.char_indices() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                '{' => depth += 1,
                '}' => depth = depth.saturating_sub(1),
                '>' if depth == 0 => {
                    let attrs = &after[..i];
                    let (attrs, self_closing) = match attrs.trim_end().strip_suffix('/') {
                        Some(a) => (a, true),
                        None => (attrs, false),
                    };
                    return Some((attrs, self_closing, &after[i + 1..]));
                }
                _ => {}
            },
        }
    }
    None
}

fn parse_attrs(src: &str) -> Vec<(String, String)> {
    let chars: Vec<char> = src.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    while i < chars.len() {
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let start = i;
        while i < chars.len()
            && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | '-' | ':'))
        {
            i += 1;
        }
        if i == start {
            i += 1;
            continue;
        }
        let key: String = chars[start..i].iter().collect();
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        if i >= chars.len() || chars[i] != '=' {
            out.push((key, "true".to_owned()));
            continue;
        }
        i += 1;
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        let value = match chars.get(i) {
            Some(&q) if q == '"' || q == '\'' => {
                i += 1;
                let s = i;
                while i < chars.len() && chars[i] != q {
                    i += 1;
                }
                let v: String = chars[s..i].iter().collect();
                i += 1;
                v
            }
            Some('{') => {
                let mut depth = 0usize;
                let s = i + 1;
                let mut q: Option<char> = None;
                while i < chars.len() {
                    let c = chars[i];
                    match q {
                        Some(qq) => {
                            if c == qq {
                                q = None;
                            }
                        }
                        None => match c {
                            '"' | '\'' => q = Some(c),
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        },
                    }
                    i += 1;
                }
                let v: String = chars[s..i.min(chars.len())].iter().collect();
                i += 1;
                unwrap_string_literal(v.trim())
            }
            _ => {
                let s = i;
                while i < chars.len() && !chars[i].is_whitespace() {
                    i += 1;
                }
                chars[s..i].iter().collect()
            }
        };
        out.push((key, value));
    }
    out
}

fn unwrap_string_literal(v: &str) -> String {
    let b = v.as_bytes();
    if b.len() >= 2 && (b[0] == b'"' || b[0] == b'\'') && b[b.len() - 1] == b[0] {
        return v[1..v.len() - 1].to_owned();
    }
    v.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(name: &str, attrs: &[(&str, &str)], children: Vec<Node>) -> Node {
        Node::Comp {
            name: name.to_owned(),
            attrs: attrs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
                .collect(),
            children,
        }
    }

    #[test]
    fn plain_markdown_is_one_node() {
        let (nodes, w) = parse("# Hi\n\ntext\n");
        assert_eq!(nodes, vec![Node::Md("# Hi\n\ntext\n".into())]);
        assert!(w.is_empty());
    }

    #[test]
    fn a_callout_with_attributes_and_a_body() {
        let (nodes, w) = parse("Before\n\n<Callout type=\"warning\" title='Heads up'>\n\nCareful.\n\n</Callout>\n\nAfter\n");
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(nodes.len(), 3);
        assert_eq!(
            nodes[1],
            comp(
                "Callout",
                &[("type", "warning"), ("title", "Heads up")],
                vec![Node::Md("\nCareful.\n\n".into())]
            )
        );
    }

    #[test]
    fn single_line_components_and_self_closing_tags() {
        let (nodes, _) = parse("<Callout>Short note</Callout>\n<Params>\n<Param name=\"id\" type=\"string\" required />\n</Params>\n");
        assert_eq!(
            nodes[0],
            comp("Callout", &[], vec![Node::Md("Short note\n".into())])
        );
        assert_eq!(
            nodes[1],
            comp(
                "Params",
                &[],
                vec![comp(
                    "Param",
                    &[("name", "id"), ("type", "string"), ("required", "true")],
                    vec![]
                )]
            )
        );
    }

    #[test]
    fn brace_attributes_are_read_as_text() {
        let (nodes, _) = parse("<Tabs items={[\"npm\", \"pnpm\"]}>\n<Tab>\nx\n</Tab>\n</Tabs>\n");
        assert_eq!(nodes[0].attr("items"), Some("[\"npm\", \"pnpm\"]"));
    }

    #[test]
    fn tags_inside_code_fences_are_text() {
        let src = "```mdx\n<Callout>\nnot a component\n</Callout>\n```\n";
        let (nodes, w) = parse(src);
        assert_eq!(nodes, vec![Node::Md(src.into())]);
        assert!(w.is_empty());
    }

    #[test]
    fn unknown_tags_are_markdown() {
        let (nodes, _) = parse("<Banner>hi</Banner>\n<details>\n</details>\n");
        assert!(matches!(nodes.as_slice(), [Node::Md(_)]));
    }

    #[test]
    fn an_unclosed_component_is_closed_with_a_warning() {
        let (nodes, w) = parse("<Steps>\n\n<Step>\none\n");
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(matches!(&nodes[0], Node::Comp { name, .. } if name == "Steps"));
    }

    #[test]
    fn a_stray_closing_tag_is_kept_as_text() {
        let (nodes, w) = parse("</Callout>\n");
        assert_eq!(w.len(), 1);
        assert_eq!(nodes, vec![Node::Md("</Callout>\n".into())]);
    }

    #[test]
    fn a_tag_that_does_not_end_on_its_line_is_text() {
        let (nodes, w) = parse("<Callout\n  type=\"tip\">\n");
        assert_eq!(w.len(), 1);
        assert!(matches!(nodes.as_slice(), [Node::Md(_)]));
    }
}
