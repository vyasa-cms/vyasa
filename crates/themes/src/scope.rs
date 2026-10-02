//! Per-section style scopes.
//!
//! A scope is a small overlay on the theme's colour roles that applies to one
//! section and everything inside it. It is what makes the most ordinary
//! device in modern page design — a dark call-to-action band between two
//! light ones — expressible at all, and it does so without letting authors
//! write CSS.
//!
//! The important property is that a scope is *derived*. A value may be a
//! literal hex, but it may also name another role (`"$text"`), so "background
//! becomes the text colour, text becomes the background" stays correct when
//! the palette changes underneath it. Literals do not.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::tokens::{ColorSlot, HexColor, TokenDiagnostic, TokenSet};

/// The colour roles a scope may override, in compile order.
pub const SCOPED_ROLES: [&str; 7] = [
    "bg",
    "surface",
    "text",
    "text-muted",
    "border",
    "primary",
    "on-primary",
];

/// A colour in a scope: either a literal, or a reference to a theme role.
///
/// References are the reason scopes survive a palette change. `"$text"`
/// resolves against the *theme's* roles, not the enclosing scope, so nesting
/// cannot produce a cycle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ColorRef {
    /// `"#0b0b0c"` or `"$text"`, distinguished at validation time.
    Value(String),
}

impl ColorRef {
    /// The role name when this is a reference (`"$text"` → `"text"`).
    #[must_use]
    pub fn role(&self) -> Option<&str> {
        let Self::Value(raw) = self;
        raw.strip_prefix('$')
    }

    /// The raw string as written.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let Self::Value(raw) = self;
        raw
    }

    /// Resolves to a concrete light/dark pair against the theme's palette.
    ///
    /// A literal has no dark variant of its own — it is the same colour in
    /// both themes, which is usually what an author means by "this band is
    /// this exact colour". A reference inherits the referenced role's dark
    /// variant, so it keeps tracking the palette.
    #[must_use]
    pub fn resolve(&self, tokens: &TokenSet) -> Option<ColorSlot> {
        match self.role() {
            Some(role) => role_slot(tokens, role).cloned(),
            None => HexColor::parse(self.as_str())
                .ok()
                .map(ColorSlot::light_only),
        }
    }
}

/// Looks a colour role up on a token set by its kebab-case name.
#[must_use]
pub fn role_slot<'a>(tokens: &'a TokenSet, role: &str) -> Option<&'a ColorSlot> {
    let c = &tokens.colors;
    match role {
        "bg" => Some(&c.bg),
        "surface" => Some(&c.surface),
        "text" => Some(&c.text),
        "text-muted" | "text_muted" => Some(&c.text_muted),
        "border" => Some(&c.border),
        "primary" => Some(&c.primary),
        "on-primary" | "on_primary" => Some(&c.on_primary),
        _ => None,
    }
}

/// Colour overrides applied to one section and its children.
///
/// Every field is optional: a scope states only what it changes, and
/// everything else keeps inheriting.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct StyleScope {
    /// Section background.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<ColorRef>,
    /// Cards and panels inside the section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<ColorRef>,
    /// Body text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<ColorRef>,
    /// Secondary text.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_muted: Option<ColorRef>,
    /// Hairlines inside the section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub border: Option<ColorRef>,
    /// Accent within the section.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<ColorRef>,
    /// Text placed on the accent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub on_primary: Option<ColorRef>,
    /// Vertical padding in spacing units; `None` uses the section default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub padding_y: Option<f32>,
    /// Constrain content to the theme's reading width rather than full bleed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contained: Option<bool>,
    /// Optional entrance motion: fade or rise. Respects reduced-motion preferences.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion: Option<String>,
}

impl StyleScope {
    /// Whether this scope changes nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The colour overrides as `(role, value)` pairs, in compile order.
    #[must_use]
    pub fn colors(&self) -> Vec<(&'static str, &ColorRef)> {
        [
            ("bg", self.bg.as_ref()),
            ("surface", self.surface.as_ref()),
            ("text", self.text.as_ref()),
            ("text-muted", self.text_muted.as_ref()),
            ("border", self.border.as_ref()),
            ("primary", self.primary.as_ref()),
            ("on-primary", self.on_primary.as_ref()),
        ]
        .into_iter()
        .filter_map(|(role, value)| value.map(|v| (role, v)))
        .collect()
    }

    /// The colours this scope resolves to, for contrast checking.
    ///
    /// Roles the scope does not set fall through to the theme, so a band that
    /// only darkens its background is still checked against the *inherited*
    /// text colour — which is exactly the case that produces unreadable
    /// output.
    #[must_use]
    pub fn resolved(&self, tokens: &TokenSet, role: &str) -> Option<ColorSlot> {
        let own = match role {
            "bg" => self.bg.as_ref(),
            "surface" => self.surface.as_ref(),
            "text" => self.text.as_ref(),
            "text-muted" => self.text_muted.as_ref(),
            "border" => self.border.as_ref(),
            "primary" => self.primary.as_ref(),
            "on-primary" => self.on_primary.as_ref(),
            _ => None,
        };
        match own {
            Some(value) => value.resolve(tokens),
            None => role_slot(tokens, role).cloned(),
        }
    }
}

/// Validates one scope against the theme it will render inside.
///
/// `path` prefixes every diagnostic so the editor can point at the offending
/// section.
#[must_use]
pub fn validate_scope(scope: &StyleScope, tokens: &TokenSet, path: &str) -> Vec<TokenDiagnostic> {
    let mut out = Vec::new();
    if scope
        .motion
        .as_deref()
        .is_some_and(|m| !matches!(m, "fade" | "rise"))
    {
        out.push(TokenDiagnostic::Error {
            path: format!("{path}.scope.motion"),
            message: "motion must be fade or rise".into(),
        });
    }

    for (role, value) in scope.colors() {
        match value.role() {
            Some(referenced) => {
                if role_slot(tokens, referenced).is_none() {
                    out.push(TokenDiagnostic::Error {
                        path: format!("{path}.scope.{role}"),
                        message: format!(
                            "unknown role \"${referenced}\" (expected one of: {})",
                            SCOPED_ROLES.join(", ")
                        ),
                    });
                }
            }
            None => {
                if let Err(message) = HexColor::parse(value.as_str()) {
                    out.push(TokenDiagnostic::Error {
                        path: format!("{path}.scope.{role}"),
                        message,
                    });
                }
            }
        }
    }

    if let Some(padding) = scope.padding_y {
        if !(0.0..=24.0).contains(&padding) {
            out.push(TokenDiagnostic::Error {
                path: format!("{path}.scope.padding_y"),
                message: format!("{padding} is outside 0..=24 spacing units"),
            });
        }
    }

    // The whole point of a scope is that the result stays readable, so the
    // same contrast rule the root palette obeys applies here too.
    if !out.iter().any(TokenDiagnostic::is_error) {
        out.extend(contrast_notes(scope, tokens, path));
    }
    out
}

/// AA contrast checks over a scope's effective colours.
fn contrast_notes(scope: &StyleScope, tokens: &TokenSet, path: &str) -> Vec<TokenDiagnostic> {
    let mut out = Vec::new();
    // `text-muted` is here because overriding only `bg` and `text` — which
    // is what a dark band usually is — leaves secondary text at its
    // light-mode grey on a near-black ground. The root palette has always
    // been checked for this; inside a scope it was not, and the result was
    // a sub-heading nobody could read.
    let pairs = [
        ("text", "bg"),
        ("text-muted", "bg"),
        ("on-primary", "primary"),
    ];

    for (fg_role, bg_role) in pairs {
        let (Some(fg), Some(bg)) = (
            scope.resolved(tokens, fg_role),
            scope.resolved(tokens, bg_role),
        ) else {
            continue;
        };
        for dark in [false, true] {
            let f = variant(&fg, dark);
            let b = variant(&bg, dark);
            let ratio = crate::tokens::contrast_ratio(&f, &b);
            if ratio < 4.5 {
                out.push(TokenDiagnostic::Warning {
                    path: format!("{path}.scope.{fg_role}"),
                    message: format!(
                        "{fg_role} on {bg_role} is {ratio:.1}:1 in {} — below the 4.5:1 needed for body text",
                        if dark { "dark" } else { "light" }
                    ),
                });
            }
        }
    }
    out
}

fn variant(slot: &ColorSlot, dark: bool) -> HexColor {
    if dark {
        slot.dark.clone().unwrap_or_else(|| slot.light.clone())
    } else {
        slot.light.clone()
    }
}
