//! Design-token schema v1: types, JSON Schema, parsing, and validation.
//!
//! Themes are data, never executable code. A [`TokenSet`] describes every
//! visual knob a theme may turn: colors (light + dark), typography, spacing,
//! radius, shadow, layout metrics, and text direction.
//!
//! Parsing rejects unknown keys; [`validate_token_set`] enforces ranges and
//! reports non-fatal warnings (e.g. low contrast) alongside hard errors.

use schemars::gen::SchemaGenerator;
use schemars::schema::{InstanceType, Metadata, Schema, SchemaObject, StringValidation};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize};

/// The token schema version this crate understands.
pub const TOKEN_SCHEMA_VERSION: u32 = 1;

/// Minimum accepted value for [`Spacing::unit_px`].
pub const MIN_UNIT_PX: f32 = 2.0;
/// Maximum accepted value for [`Spacing::unit_px`].
pub const MAX_UNIT_PX: f32 = 16.0;
/// Minimum accepted value for [`TokenSet::radius_px`].
pub const MIN_RADIUS_PX: f32 = 0.0;
/// Maximum accepted value for [`TokenSet::radius_px`].
pub const MAX_RADIUS_PX: f32 = 32.0;
/// Minimum accepted value for [`LayoutMetrics::content_width_px`].
pub const MIN_CONTENT_WIDTH_PX: f32 = 480.0;
/// Maximum accepted value for [`LayoutMetrics::content_width_px`].
pub const MAX_CONTENT_WIDTH_PX: f32 = 1920.0;
/// Minimum accepted value for [`LayoutMetrics::sidebar_width_px`].
pub const MIN_SIDEBAR_WIDTH_PX: f32 = 160.0;
/// Maximum accepted value for [`LayoutMetrics::sidebar_width_px`].
pub const MAX_SIDEBAR_WIDTH_PX: f32 = 420.0;
/// Minimum accepted value for either breakpoint.
pub const MIN_BREAKPOINT_PX: f32 = 320.0;
/// Maximum accepted value for either breakpoint.
pub const MAX_BREAKPOINT_PX: f32 = 1920.0;
/// Minimum accepted value for [`Typography::base_size_px`].
pub const MIN_BASE_SIZE_PX: f32 = 12.0;
/// Maximum accepted value for [`Typography::base_size_px`].
pub const MAX_BASE_SIZE_PX: f32 = 24.0;
/// Minimum accepted value for [`ScaleRatio::Custom`].
pub const MIN_SCALE_RATIO: f64 = 1.0;
/// Maximum accepted value for [`ScaleRatio::Custom`].
pub const MAX_SCALE_RATIO: f64 = 2.5;
/// WCAG AA minimum contrast ratio enforced as a warning.
pub const CONTRAST_THRESHOLD: f64 = 4.5;

/// A complete design-token document.
///
/// Every field has a sane default so partial documents (for example
/// truncated LLM output) can be completed automatically; unknown keys are
/// rejected at parse time so typos never silently disappear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct TokenSet {
    /// Schema version; must equal [`TOKEN_SCHEMA_VERSION`].
    pub version: u32,
    /// Light/dark color roles.
    pub colors: ColorPalette,
    /// Font choices, base size, and type-scale ratio.
    pub typography: Typography,
    /// Spacing unit and section rhythm multipliers.
    pub spacing: Spacing,
    /// Global corner radius in px.
    pub radius_px: f32,
    /// Elevation preset for cards and overlays.
    pub shadow: ShadowLevel,
    /// Layout metrics (content column, sidebar, density).
    pub layout: LayoutMetrics,
    /// Text direction for the whole site.
    pub direction: Direction,
}

impl Default for TokenSet {
    fn default() -> Self {
        Self {
            version: TOKEN_SCHEMA_VERSION,
            colors: ColorPalette::default(),
            typography: Typography::default(),
            spacing: Spacing::default(),
            radius_px: 8.0,
            shadow: ShadowLevel::Small,
            layout: LayoutMetrics::default(),
            direction: Direction::Ltr,
        }
    }
}

/// One color role with its light value and optional dark override.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ColorSlot {
    /// Value used in the light palette (`:root`).
    pub light: HexColor,
    /// Value used in dark contexts; `None` keeps the light value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dark: Option<HexColor>,
}

impl ColorSlot {
    /// A slot that only defines a light value.
    #[must_use]
    pub const fn light_only(light: HexColor) -> Self {
        Self { light, dark: None }
    }
}

/// The seven color roles Vyasa themes understand.
///
/// Defaults form an accessible neutral palette with blue accent; every role
/// ships a dark variant so `[data-theme=dark]` works out of the box.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct ColorPalette {
    /// Page background.
    pub bg: ColorSlot,
    /// Cards and raised panels.
    pub surface: ColorSlot,
    /// Primary text color.
    pub text: ColorSlot,
    /// Secondary text color.
    pub text_muted: ColorSlot,
    /// Hairlines and separators.
    pub border: ColorSlot,
    /// Brand / accent color.
    pub primary: ColorSlot,
    /// Text placed on top of `primary`.
    pub on_primary: ColorSlot,
}

fn hex(raw: &str) -> HexColor {
    HexColor(raw.to_owned())
}

impl Default for ColorPalette {
    fn default() -> Self {
        Self {
            bg: ColorSlot {
                light: hex("#ffffff"),
                dark: Some(hex("#101013")),
            },
            surface: ColorSlot {
                light: hex("#f4f4f5"),
                dark: Some(hex("#1b1b1f")),
            },
            text: ColorSlot {
                light: hex("#18181b"),
                dark: Some(hex("#f4f4f5")),
            },
            text_muted: ColorSlot {
                light: hex("#52525b"),
                dark: Some(hex("#a1a1aa")),
            },
            border: ColorSlot {
                light: hex("#e4e4e7"),
                dark: Some(hex("#27272a")),
            },
            primary: ColorSlot {
                light: hex("#2563eb"),
                dark: Some(hex("#60a5fa")),
            },
            on_primary: ColorSlot {
                light: hex("#ffffff"),
                dark: Some(hex("#0b1220")),
            },
        }
    }
}

/// An `#rgb` or `#rrggbb` color string, stored lowercased.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
pub struct HexColor(pub String);

impl HexColor {
    /// Parses a hex color; rejects anything that is not `#rgb`/`#rrggbb`.
    ///
    /// # Errors
    /// Returns a message naming the offending input, suitable for surfacing
    /// as a field-path error.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let bad = || format!("invalid hex color \"{raw}\" (expected #rgb or #rrggbb)");
        let Some(body) = raw.strip_prefix('#') else {
            return Err(bad());
        };
        if !matches!(body.len(), 3 | 6) || !body.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(bad());
        }
        Ok(Self(raw.to_ascii_lowercase()))
    }

    /// Normalized lowercase `#rrggbb` form (expands 3-digit shorthand).
    #[must_use]
    pub fn normalized(&self) -> String {
        let body = self.0.trim_start_matches('#');
        if body.len() == 3 {
            let mut full = String::with_capacity(7);
            full.push('#');
            for c in body.chars() {
                full.push(c);
                full.push(c);
            }
            full
        } else {
            format!("#{body}")
        }
    }

    /// 8-bit RGB channels of [`Self::normalized`].
    #[must_use]
    pub fn channels(&self) -> [u8; 3] {
        let body = self.normalized();
        let byte = |i: usize| u8::from_str_radix(&body[1 + i * 2..1 + i * 2 + 2], 16).unwrap_or(0);
        [byte(0), byte(1), byte(2)]
    }
}

impl<'de> Deserialize<'de> for HexColor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl JsonSchema for HexColor {
    fn schema_name() -> String {
        "HexColor".to_owned()
    }

    fn json_schema(_gen: &mut SchemaGenerator) -> Schema {
        SchemaObject {
            metadata: Some(Box::new(Metadata {
                description: Some("RGB hex color, `#rgb` or `#rrggbb`.".to_owned()),
                ..Metadata::default()
            })),
            instance_type: Some(InstanceType::String.into()),
            string: Some(Box::new(StringValidation {
                pattern: Some(r"^#([0-9a-fA-F]{3}|[0-9a-fA-F]{6})$".to_owned()),
                ..StringValidation::default()
            })),
            ..SchemaObject::default()
        }
        .into()
    }
}

/// Typography tokens: font stacks, base size, and the modular-scale ratio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Typography {
    /// Heading font.
    pub heading: FontChoice,
    /// Body font.
    pub body: FontChoice,
    /// Base font size in px (headings scale from this).
    pub base_size_px: f32,
    /// Named or numeric type-scale ratio.
    pub scale_ratio: ScaleRatio,
    /// `@font-face` declarations backing [`FontChoice::Custom`] families.
    pub font_faces: Vec<FontFace>,
}

impl Default for Typography {
    fn default() -> Self {
        Self {
            heading: FontChoice::SystemUi,
            body: FontChoice::SystemUi,
            base_size_px: 16.0,
            scale_ratio: ScaleRatio::MajorThird,
            font_faces: Vec::new(),
        }
    }
}

/// A font selection: a built-in system-safe stack or a custom family name.
///
/// Custom families should be backed by an entry in
/// [`Typography::font_faces`] when they are web fonts; system generics fall
/// back gracefully in every browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FontChoice {
    /// `system-ui` stack (default; always available offline).
    SystemUi,
    /// Classic serif stack.
    Serif,
    /// Monospace stack for code-heavy themes.
    Mono,
    /// A custom family name (e.g. `"Inter"`), optionally served via
    /// `@font-face`.
    Custom(String),
}

/// One `@font-face` declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FontFace {
    /// Family name this face registers; must match a
    /// [`FontChoice::Custom`] usage to have any effect.
    pub family: String,
    /// Web font source; `https://` or a bundled `/theme-assets/fonts/` path, so themes can never
    /// inject insecure or `javascript:` URLs into rendered pages.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src: Option<String>,
}

/// Modular scale ratios with well-known names plus a numeric escape hatch.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScaleRatio {
    /// 1.125
    MajorSecond,
    /// 1.2
    MinorThird,
    /// 1.25
    MajorThird,
    /// 1.333…
    PerfectFourth,
    /// 1.618…
    Golden,
    /// Any ratio in `[1.0, 2.5]`.
    Custom(f64),
}

impl ScaleRatio {
    /// The numeric value of this ratio.
    #[must_use]
    pub const fn value(self) -> f64 {
        match self {
            Self::MajorSecond => 1.125,
            Self::MinorThird => 1.2,
            Self::MajorThird => 1.25,
            Self::PerfectFourth => 1.333_333_333_333_333_3,
            Self::Golden => 1.618_033_988_749_895,
            Self::Custom(v) => v,
        }
    }
}

/// Spacing tokens: the base unit and the vertical rhythm multipliers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct Spacing {
    /// Base spacing unit in px; utilities use multiples of it.
    pub unit_px: f32,
    /// Ascending section-padding multipliers of [`Spacing::unit_px`].
    pub section_scale: Vec<f32>,
}

impl Default for Spacing {
    fn default() -> Self {
        Self {
            unit_px: 4.0,
            section_scale: vec![1.0, 2.0, 4.0, 8.0],
        }
    }
}

/// Layout metric tokens: content column width, sidebar width, and UI density.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields, default)]
pub struct LayoutMetrics {
    /// Main content column width in px.
    pub content_width_px: f32,
    /// Sidebar column width in px (0 disables the sidebar slot).
    pub sidebar_width_px: f32,
    /// Whitespace density preset for lists and cards.
    pub density: Density,
    /// Width below which columns collapse and type steps down, in px.
    ///
    /// A breakpoint is a theme decision like any other metric, but CSS
    /// media queries cannot read custom properties, so these are
    /// interpolated into the generated stylesheets rather than emitted as
    /// variables. They were two magic numbers in the stylesheet source
    /// until they were named here.
    pub breakpoint_sm_px: f32,
    /// Width below which the sidebar drops under the content, in px.
    pub breakpoint_md_px: f32,
}

impl Default for LayoutMetrics {
    fn default() -> Self {
        Self {
            content_width_px: 960.0,
            sidebar_width_px: 260.0,
            density: Density::Comfortable,
            // The values that were hard-coded in the stylesheets, so a
            // theme written before breakpoints existed renders identically.
            breakpoint_sm_px: 640.0,
            breakpoint_md_px: 900.0,
        }
    }
}

/// UI whitespace density.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Density {
    /// Tighter paddings (`×0.85`).
    Compact,
    /// Default rhythm (`×1`).
    Comfortable,
    /// Airy paddings (`×1.25`).
    Spacious,
}

impl Density {
    /// Numeric multiplier used by layout code.
    #[must_use]
    pub const fn multiplier(self) -> f32 {
        match self {
            Self::Compact => 0.85,
            Self::Comfortable => 1.0,
            Self::Spacious => 1.25,
        }
    }
}

/// Box-shadow elevation presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ShadowLevel {
    /// No shadow at all.
    None,
    /// Subtle card lift.
    Small,
    /// Standard dropdown / card shadow.
    Medium,
    /// Modal / popover shadow.
    Large,
}

impl ShadowLevel {
    /// The CSS `box-shadow` value for this level.
    #[must_use]
    pub fn css_value(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Small => "0 1px 2px rgb(0 0 0 / 0.05)",
            Self::Medium => "0 2px 8px rgb(0 0 0 / 0.08)",
            Self::Large => "0 8px 24px rgb(0 0 0 / 0.12)",
        }
    }
}

/// Site-wide text direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    /// Left-to-right (default).
    Ltr,
    /// Right-to-left; flips the layout engine's logical properties.
    Rtl,
}

impl Direction {
    /// The CSS `direction` value.
    #[must_use]
    pub const fn css_value(self) -> &'static str {
        match self {
            Self::Ltr => "ltr",
            Self::Rtl => "rtl",
        }
    }
}

/// A single validation outcome: a hard [`TokenDiagnostic::Error`] blocks
/// compilation, a [`TokenDiagnostic::Warning`] does not.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenDiagnostic {
    /// Hard failure; the token set cannot be compiled safely.
    Error {
        /// Dot-separated field path, e.g. `colors.primary.light`.
        path: String,
        /// What is wrong at that path.
        message: String,
    },
    /// Non-fatal observation surfaced to editors and the AI builder.
    Warning {
        /// Dot-separated field path the note applies to.
        path: String,
        /// What was observed.
        message: String,
    },
}

impl TokenDiagnostic {
    /// The dot-separated field path this diagnostic points at.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::Error { path, .. } | Self::Warning { path, .. } => path,
        }
    }

    /// Whether this diagnostic blocks compilation.
    #[must_use]
    pub const fn is_error(&self) -> bool {
        matches!(self, Self::Error { .. })
    }
}

impl std::fmt::Display for TokenDiagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Error { path, message } | Self::Warning { path, message } => {
                write!(
                    f,
                    "{}: {}",
                    if self.is_error() { "error" } else { "warning" },
                    path
                )?;
                write!(f, ": {message}")
            }
        }
    }
}

fn err(path: &str, message: String) -> TokenDiagnostic {
    TokenDiagnostic::Error {
        path: path.to_owned(),
        message,
    }
}

fn warn(path: &str, message: String) -> TokenDiagnostic {
    TokenDiagnostic::Warning {
        path: path.to_owned(),
        message,
    }
}

/// Parses a token JSON document.
///
/// Unknown keys are rejected; deserialization failures are reported with
/// their precise field path (via `serde_path_to_error`).
///
/// # Errors
/// Returns one [`TokenDiagnostic::Error`] per parse failure with the exact
/// field path attached.
pub fn parse_token_set(json: &str) -> Result<TokenSet, Vec<TokenDiagnostic>> {
    let mut de = serde_json::Deserializer::from_str(json);
    let deserializer = serde_path_to_error::deserialize(&mut de);
    match deserializer {
        Ok(tokens) => Ok(tokens),
        // serde_path_to_error returns its own error wrapper for *parse* errors;
        // unexpected EOF has no path segment.
        Err(e) => Err(vec![TokenDiagnostic::Error {
            path: e.path().to_string(),
            message: e.inner().to_string(),
        }]),
    }
}

/// Validates a parsed [`TokenSet`].
///
/// Returns hard errors (out-of-range numbers, bad font sources, wrong
/// schema version) and non-fatal warnings (low WCAG contrast pairs,
/// incomplete dark palette, unused/duplicate font faces).
#[must_use]
pub fn validate_token_set(tokens: &TokenSet) -> Vec<TokenDiagnostic> {
    let mut out = Vec::new();
    if tokens.version != TOKEN_SCHEMA_VERSION {
        out.push(err(
            "version",
            format!(
                "unsupported schema version {} (expected {TOKEN_SCHEMA_VERSION})",
                tokens.version
            ),
        ));
    }
    check_range(
        &mut out,
        "radius_px",
        tokens.radius_px,
        MIN_RADIUS_PX,
        MAX_RADIUS_PX,
    );
    validate_typography(&tokens.typography, &mut out);
    validate_spacing(&tokens.spacing, &mut out);
    validate_layout_metrics(&tokens.layout, &mut out);
    for w in contrast_warnings(tokens) {
        out.push(w);
    }
    out
}

fn check_range(out: &mut Vec<TokenDiagnostic>, path: &str, v: f32, min: f32, max: f32) {
    if !v.is_finite() || !(min..=max).contains(&v) {
        out.push(err(path, format!("{v} is out of range [{min}, {max}]")));
    }
}

fn validate_typography(t: &Typography, out: &mut Vec<TokenDiagnostic>) {
    check_range(
        out,
        "typography.base_size_px",
        t.base_size_px,
        MIN_BASE_SIZE_PX,
        MAX_BASE_SIZE_PX,
    );
    if let ScaleRatio::Custom(v) = t.scale_ratio {
        if !v.is_finite() || !(MIN_SCALE_RATIO..=MAX_SCALE_RATIO).contains(&v) {
            out.push(err(
                "typography.scale_ratio",
                format!("{v} is out of range [{MIN_SCALE_RATIO}, {MAX_SCALE_RATIO}]"),
            ));
        }
    }
    let mut seen: Vec<&str> = Vec::new();
    for (i, face) in t.font_faces.iter().enumerate() {
        let base = format!("typography.font_faces[{i}]");
        if face.family.is_empty()
            || !face
                .family
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_')
        {
            out.push(err(
                &format!("{base}.family"),
                format!(
                    "\"{}\" must be non-empty and contain only letters, digits, spaces, '-', '_'",
                    face.family
                ),
            ));
        }
        if let Some(src) = &face.src {
            // Either a remote https:// font or one the package bundles
            // under assets/fonts/, served from this origin.
            if !src.starts_with("https://") && !src.starts_with("/theme-assets/fonts/") {
                out.push(err(
                    &format!("{base}.src"),
                    format!("\"{src}\" must be an https:// URL or a /theme-assets/fonts/ path"),
                ));
            }
            // The stylesheet is inlined into a <style> element. A source
            // that starts with https:// can still carry a quote to close
            // the url() or a `<` to open a tag; nothing a URL needs is
            // among these.
            if src
                .chars()
                .any(|c| matches!(c, '"' | '\'' | '\\' | '<' | '>') || c.is_whitespace())
            {
                out.push(err(
                    &format!("{base}.src"),
                    format!("\"{src}\" contains a character a URL cannot"),
                ));
            }
        }
        if seen.contains(&face.family.as_str()) {
            out.push(warn(
                &base,
                format!("duplicate @font-face for family \"{}\"", face.family),
            ));
        }
        seen.push(face.family.as_str());
    }
    let used = |c: &FontChoice| matches!(c, FontChoice::Custom(name) if t.font_faces.iter().any(|f| &f.family == name));
    for (slot, choice) in [("heading", &t.heading), ("body", &t.body)] {
        if let FontChoice::Custom(name) = choice {
            if !used(choice) && t.font_faces.iter().all(|f| &f.family != name) {
                out.push(warn(
                    &format!("typography.{slot}"),
                    format!(
                        "custom family \"{name}\" has no matching @font-face; browsers will fall back"
                    ),
                ));
            }
        }
    }
}

fn validate_spacing(s: &Spacing, out: &mut Vec<TokenDiagnostic>) {
    check_range(out, "spacing.unit_px", s.unit_px, MIN_UNIT_PX, MAX_UNIT_PX);
    let scale = &s.section_scale;
    if scale.is_empty() {
        out.push(err("spacing.section_scale", "must not be empty".to_owned()));
        return;
    }
    if scale.len() > 8 {
        out.push(err(
            "spacing.section_scale",
            format!("has {} steps; at most 8 are supported", scale.len()),
        ));
    }
    for (i, step) in scale.iter().enumerate() {
        if !step.is_finite() || !(0.25..=20.0).contains(step) {
            out.push(err(
                &format!("spacing.section_scale[{i}]"),
                format!("{step} is out of range [0.25, 20]"),
            ));
        }
        if i > 0 && *step < scale[i - 1] {
            out.push(err(
                &format!("spacing.section_scale[{i}]"),
                format!("{step} must be ascending (previous step {})", scale[i - 1]),
            ));
        }
    }
}

fn validate_layout_metrics(l: &LayoutMetrics, out: &mut Vec<TokenDiagnostic>) {
    check_range(
        out,
        "layout.content_width_px",
        l.content_width_px,
        MIN_CONTENT_WIDTH_PX,
        MAX_CONTENT_WIDTH_PX,
    );
    for (path, value) in [
        ("layout.breakpoint_sm_px", l.breakpoint_sm_px),
        ("layout.breakpoint_md_px", l.breakpoint_md_px),
    ] {
        check_range(out, path, value, MIN_BREAKPOINT_PX, MAX_BREAKPOINT_PX);
    }
    if l.breakpoint_sm_px >= l.breakpoint_md_px {
        out.push(TokenDiagnostic::Error {
            path: "layout.breakpoint_md_px".to_owned(),
            message: format!(
                "must be wider than breakpoint_sm_px ({})",
                l.breakpoint_sm_px
            ),
        });
    }
    if l.sidebar_width_px != 0.0 {
        check_range(
            out,
            "layout.sidebar_width_px",
            l.sidebar_width_px,
            MIN_SIDEBAR_WIDTH_PX,
            MAX_SIDEBAR_WIDTH_PX,
        );
    }
}

/// WCAG relative luminance of a color.
fn rel_luminance(hex: &HexColor) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.039_28 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let [r, g, b] = hex.channels();
    0.212_6 * lin(r) + 0.715_2 * lin(g) + 0.072_2 * lin(b)
}

/// Contrast ratio between two colors (1.0 ..= 21.0).
#[must_use]
pub fn contrast_ratio(fg: &HexColor, bg: &HexColor) -> f64 {
    let (lf, lb) = (rel_luminance(fg), rel_luminance(bg));
    let (hi, lo) = if lf >= lb { (lf, lb) } else { (lb, lf) };
    (hi + 0.05) / (lo + 0.05)
}

fn pick(dark: bool, s: &ColorSlot) -> &HexColor {
    if dark {
        s.dark.as_ref().unwrap_or(&s.light)
    } else {
        &s.light
    }
}

fn contrast_pair(
    out: &mut Vec<TokenDiagnostic>,
    tokens: &TokenSet,
    fg_role: &str,
    bg_role: &str,
    dark: bool,
) -> bool {
    let slot = |role: &str| -> Option<&ColorSlot> {
        match role {
            "bg" => Some(&tokens.colors.bg),
            "surface" => Some(&tokens.colors.surface),
            "text" => Some(&tokens.colors.text),
            "text_muted" => Some(&tokens.colors.text_muted),
            "primary" => Some(&tokens.colors.primary),
            "on_primary" => Some(&tokens.colors.on_primary),
            _ => None,
        }
    };
    let (Some(fg), Some(bg)) = (slot(fg_role), slot(bg_role)) else {
        return false;
    };
    let ratio = contrast_ratio(pick(dark, fg), pick(dark, bg));
    if ratio < CONTRAST_THRESHOLD {
        let mode = if dark { "dark" } else { "light" };
        out.push(warn(
            &format!("colors.{fg_role}"),
            format!(
                "{mode} contrast {ratio:.2} on {bg_role} is below {CONTRAST_THRESHOLD} (WCAG AA)"
            ),
        ));
        return true;
    }
    false
}

fn contrast_warnings(tokens: &TokenSet) -> Vec<TokenDiagnostic> {
    let mut out = Vec::new();
    for dark in [false, true] {
        for (fg, bg) in [
            ("text", "bg"),
            ("text", "surface"),
            ("text_muted", "surface"),
            ("on_primary", "primary"),
        ] {
            contrast_pair(&mut out, tokens, fg, bg, dark);
        }
    }
    // Partial dark palettes: warn once per missing role when at least one
    // role overrides its dark value.
    let slots = [
        ("bg", &tokens.colors.bg),
        ("surface", &tokens.colors.surface),
        ("text", &tokens.colors.text),
        ("text_muted", &tokens.colors.text_muted),
        ("border", &tokens.colors.border),
        ("primary", &tokens.colors.primary),
        ("on_primary", &tokens.colors.on_primary),
    ];
    let any_dark = slots.iter().any(|(_, s)| s.dark.is_some());
    if any_dark {
        for (name, s) in slots {
            if s.dark.is_none() {
                out.push(warn(
                    &format!("colors.{name}.dark"),
                    "dark palette is partially overridden; this role keeps its light value"
                        .to_owned(),
                ));
            }
        }
    }
    out
}

/// The JSON Schema for [`TokenSet`], exported to
/// `docs/theme-token-schema.json` for editors and the AI theme-builder.
#[must_use]
pub fn json_schema() -> serde_json::Value {
    let mut schema = schemars::schema_for!(TokenSet);
    schema
        .schema
        .metadata
        .get_or_insert_with(Box::default)
        .description = Some("Vyasa design-token schema v1.".to_owned());
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

/// One-shot pipeline: parse → validate → compile to CSS.
///
/// # Errors
/// Returns all hard [`TokenDiagnostic::Error`]s (parse or validation);
/// warnings are dropped when errors exist, otherwise returned alongside
/// the compiled CSS is not — this returns only the diagnostics on failure
/// and never emits CSS for an invalid set.
pub fn compile_tokens(json: &str) -> Result<(String, Vec<TokenDiagnostic>), Vec<TokenDiagnostic>> {
    let tokens = parse_token_set(json)?;
    let diags = validate_token_set(&tokens);
    if diags.iter().any(TokenDiagnostic::is_error) {
        return Err(diags);
    }
    let warnings = diags;
    Ok((crate::css::tokens_to_css(&tokens), warnings))
}
