//! Custom-CSS namespacing.
//!
//! Author CSS is sanitized upstream (at-rules stripped, urls restricted to
//! https by [`crate::tokens`] sibling module in core) and then **scoped**
//! here so it can never leak onto admin chrome or plugin surfaces: every
//! top-level selector is prefixed with `#vy-site`, and the whole sheet is
//! additionally wrapped so specificity beats theme defaults.

use std::fmt::Write as _;

/// Wraps sanitized author CSS in a `#vy-site` scope.
///
/// Selectors at top level (outside any block) each get the prefix; nested
/// selectors inside blocks are left alone (they are already scoped by their
/// parent). At-rules were removed during sanitization, so none appear here.
#[must_use]
pub fn namespace_custom_css(css: &str) -> String {
    let mut out = String::from("#vy-site {\n");
    let mut depth = 0usize;
    let mut line = String::new();
    for ch in css.chars() {
        match ch {
            '{' => {
                if depth == 0 {
                    let _ =
                        std::fmt::Write::write_fmt(&mut out, format_args!("  {} {{", line.trim()));
                    line.clear();
                } else {
                    let _ = std::fmt::Write::write_fmt(&mut out, format_args!("{line}{{"));
                    line.clear();
                }
                depth += 1;
            }
            '}' => {
                depth = depth.saturating_sub(1);
                out.push('}');
                if depth == 0 {
                    out.push('\n');
                }
            }
            '\n' => {
                if depth == 0 {
                    // Top-level newline ends a pending selector chunk.
                    if !line.trim().is_empty() {
                        let _ = writeln!(out, "  {}", line.trim());
                    }
                    line.clear();
                } else {
                    out.push('\n');
                }
            }
            ';' => {
                if depth == 0 && !line.trim().is_empty() {
                    // Bare declaration at top level: wrap as #vy-site rule.
                    let _ = writeln!(out, "#vy-site {{ {}}}", line.trim());
                    line.clear();
                } else {
                    line.push(';');
                }
            }
            other => {
                if depth == 0 {
                    line.push(other);
                } else {
                    out.push(other);
                }
            }
        }
    }
    if !line.trim().is_empty() {
        let _ = writeln!(out, "#vy-site {{ {}}}", line.trim());
    }
    out.push_str("}\n");
    out
}
