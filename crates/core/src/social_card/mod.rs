//! Social cards: a generated 1200x630 Open Graph image per entry.
//!
//! A theme declares a card layout in its manifest (`[social_card]`); this
//! module holds that layout, its validation, the text cleaning and check,
//! the line layout and the PNG render. It is pure: the caller hands it the
//! theme's background and font bytes and the entry's title and label.
//!
//! The design is `vyasa-internal specs/2026-11-22-social-card-renderer-design.md`.

mod draw;
mod layout;
mod text;

#[cfg(test)]
mod tests;

use ab_glyph::{Font, FontVec};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub use text::{clean, MAX_LABEL_CHARS, MAX_TITLE_CHARS};

/// Card width in pixels.
pub const WIDTH: u32 = 1200;
/// Card height in pixels.
pub const HEIGHT: u32 = 630;
/// Bumped with any change to layout, drawing, the label rule, or an
/// `ab_glyph` / `image` upgrade: it is part of every card key, so a bump
/// gives every card a new URL.
pub const RENDERER_REVISION: &str = "1";
/// Largest background file a layout may name.
pub const MAX_BACKGROUND_BYTES: usize = 4 * 1024 * 1024;
/// Largest font file a layout may name.
pub const MAX_FONT_BYTES: usize = 8 * 1024 * 1024;
/// Longest `alt` pattern, in characters.
pub const MAX_ALT_CHARS: usize = 300;
/// Largest title or label size, in pixels.
pub const MAX_SIZE: u32 = 200;

/// The `[social_card]` section of a theme manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardLayout {
    /// Post types whose entries get cards (non-empty).
    pub post_types: Vec<String>,
    /// Theme asset path of the 1200x630 PNG background (`images/...`).
    pub background: String,
    /// Theme asset path of the static TTF/OTF font (`fonts/...`).
    pub font: String,
    /// Title colour, `#RGB` or `#RRGGBB`.
    pub color: String,
    /// Label colour; the title colour when omitted.
    #[serde(default)]
    pub label_color: Option<String>,
    /// Where and how large the title is drawn.
    pub title: TitleBox,
    /// Where the label is drawn; no label when omitted.
    #[serde(default)]
    pub label: Option<LabelBox>,
    /// The `og:image:alt` pattern: `{title}`, `{label}` and `{site}`.
    #[serde(default = "default_alt")]
    pub alt: String,
}

fn default_alt() -> String {
    "{title}: {site}".to_owned()
}

/// The title box: top-left corner, width, height, sizes and line cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TitleBox {
    /// Left edge.
    pub x: u32,
    /// Top of the first line.
    pub y: u32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
    /// Starting font size in pixels.
    pub size: u32,
    /// Smallest font size the title steps down to.
    pub min_size: u32,
    /// Most lines the title may take.
    pub max_lines: u32,
}

/// The label box: one line, `size x 1.2` tall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LabelBox {
    /// Left edge.
    pub x: u32,
    /// Top of the line.
    pub y: u32,
    /// Width.
    pub w: u32,
    /// Font size in pixels.
    pub size: u32,
}

impl CardLayout {
    /// Checks every rule that needs neither the theme's files nor the
    /// site's post types.
    ///
    /// # Errors
    /// A message naming the first rule that fails.
    pub fn validate(&self) -> Result<(), String> {
        if self.post_types.is_empty() || self.post_types.iter().any(|t| t.trim().is_empty()) {
            return Err("social_card.post_types must list at least one post type".to_owned());
        }
        check_asset_path("background", &self.background, "images/", &["png"])?;
        check_asset_path("font", &self.font, "fonts/", &["ttf", "otf"])?;
        parse_color(&self.color).ok_or_else(|| {
            format!(
                "social_card.color \"{}\" is not #RGB or #RRGGBB",
                self.color
            )
        })?;
        if let Some(c) = &self.label_color {
            parse_color(c)
                .ok_or_else(|| format!("social_card.label_color \"{c}\" is not #RGB or #RRGGBB"))?;
        }
        let t = self.title;
        if !(0 < t.min_size && t.min_size <= t.size && t.size <= MAX_SIZE) {
            return Err(format!(
                "social_card.title needs 0 < min_size <= size <= {MAX_SIZE}"
            ));
        }
        if t.max_lines == 0 {
            return Err("social_card.title.max_lines must be at least 1".to_owned());
        }
        if !inside(t.x, t.w, WIDTH) || !inside(t.y, t.h, HEIGHT) {
            return Err("social_card.title box must lie inside 1200x630".to_owned());
        }
        if u64::from(t.min_size) * 6 > u64::from(t.h) * 5 || t.w < t.min_size {
            return Err("social_card.title box must hold at least one line at min_size".to_owned());
        }
        if let Some(l) = self.label {
            if !(0 < l.size && l.size <= MAX_SIZE) {
                return Err(format!("social_card.label needs 0 < size <= {MAX_SIZE}"));
            }
            // The label box is size x 1.2 tall: y + 6 x size / 5 <= 630.
            let bottom = u64::from(l.y) * 5 + u64::from(l.size) * 6;
            if !inside(l.x, l.w, WIDTH) || bottom > u64::from(HEIGHT) * 5 {
                return Err("social_card.label box must lie inside 1200x630".to_owned());
            }
            if l.w < l.size {
                return Err("social_card.label box must be at least size wide".to_owned());
            }
        }
        if self.alt.chars().count() > MAX_ALT_CHARS {
            return Err(format!(
                "social_card.alt is longer than {MAX_ALT_CHARS} characters"
            ));
        }
        expand_alt(&self.alt, "", "", "")
            .ok_or_else(|| "social_card.alt may use only {title}, {label} and {site}".to_owned())?;
        Ok(())
    }

    /// The alt text for a card: the pattern with its placeholders filled.
    #[must_use]
    pub fn alt_text(&self, title: &str, label: &str, site: &str) -> String {
        expand_alt(&self.alt, title, label, site).unwrap_or_default()
    }
}

fn inside(start: u32, len: u32, limit: u32) -> bool {
    u64::from(start) + u64::from(len) <= u64::from(limit)
}

/// A relative path under `dir` with one of `exts`, no `..`, no empty or
/// dot segment and no backslash.
fn check_asset_path(field: &str, path: &str, dir: &str, exts: &[&str]) -> Result<(), String> {
    let ok = path.starts_with(dir)
        && !path.contains('\\')
        && path
            .split('/')
            .all(|seg| !seg.is_empty() && !seg.starts_with('.'))
        && path
            .rsplit_once('.')
            .is_some_and(|(_, ext)| exts.contains(&ext.to_ascii_lowercase().as_str()));
    if ok {
        Ok(())
    } else {
        Err(format!(
            "social_card.{field} \"{path}\" must be a theme asset under {dir} ending in .{}",
            exts.join(" or .")
        ))
    }
}

/// `#RGB` or `#RRGGBB` as three bytes.
#[must_use]
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let hex = s.strip_prefix('#')?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digit = |i: usize| u8::from_str_radix(hex.get(i..=i)?, 16).ok();
    match hex.len() {
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17]),
        6 => {
            let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
            Some([byte(0)?, byte(2)?, byte(4)?])
        }
        _ => None,
    }
}

/// Fills `{title}`, `{label}` and `{site}` in one pass (a title holding
/// `{site}` stays as written); `None` for any other brace.
fn expand_alt(pattern: &str, title: &str, label: &str, site: &str) -> Option<String> {
    let mut out = String::with_capacity(pattern.len() + title.len() + site.len());
    let mut rest = pattern;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        let (value, len) = [("{title}", title), ("{label}", label), ("{site}", site)]
            .into_iter()
            .find(|(name, _)| tail.starts_with(name))
            .map(|(name, value)| (value, name.len()))?;
        out.push_str(value);
        rest = &tail[len..];
    }
    out.push_str(rest);
    Some(out)
}

/// A validated layout with its files loaded: what a render needs, built
/// once per theme version.
pub struct CardTheme {
    layout: CardLayout,
    color: [u8; 3],
    label_color: [u8; 3],
    font: FontVec,
    background: RgbaImage,
    digests: [String; 3],
}

impl std::fmt::Debug for CardTheme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CardTheme")
            .field("layout", &self.layout)
            .field("digests", &self.digests)
            .finish_non_exhaustive()
    }
}

impl CardTheme {
    /// Validates `layout` and loads its files. `section` is the stored
    /// text of the manifest section; its digest is part of every key.
    ///
    /// # Errors
    /// A message naming the rule or file that failed.
    pub fn load(
        layout: CardLayout,
        section: &str,
        background: &[u8],
        font: &[u8],
    ) -> Result<Self, String> {
        layout.validate()?;
        if background.len() > MAX_BACKGROUND_BYTES {
            return Err(format!(
                "social_card.background is {} bytes; the maximum is {MAX_BACKGROUND_BYTES}",
                background.len()
            ));
        }
        match png_dimensions(background) {
            Some((WIDTH, HEIGHT)) => {}
            Some((w, h)) => {
                return Err(format!(
                    "social_card.background is {w}x{h}; it must be {WIDTH}x{HEIGHT}"
                ))
            }
            None => return Err("social_card.background is not a PNG".to_owned()),
        }
        let digests = [
            hex::encode(Sha256::digest(section.as_bytes())),
            hex::encode(Sha256::digest(background)),
            hex::encode(Sha256::digest(font)),
        ];
        let background = image::load_from_memory_with_format(background, image::ImageFormat::Png)
            .map_err(|e| format!("social_card.background: {e}"))?
            .to_rgba8();
        if font.len() > MAX_FONT_BYTES {
            return Err(format!(
                "social_card.font is {} bytes; the maximum is {MAX_FONT_BYTES}",
                font.len()
            ));
        }
        let font = FontVec::try_from_vec(font.to_vec())
            .map_err(|_| "social_card.font is not a TrueType or OpenType font".to_owned())?;
        let color = parse_color(&layout.color).unwrap_or_default();
        let label_color = layout
            .label_color
            .as_deref()
            .and_then(parse_color)
            .unwrap_or(color);
        Ok(Self {
            layout,
            color,
            label_color,
            font,
            background,
            digests,
        })
    }

    /// The layout this theme declared.
    #[must_use]
    pub fn layout(&self) -> &CardLayout {
        &self.layout
    }

    /// Whether `post_type` is in the layout's scope.
    #[must_use]
    pub fn covers(&self, post_type: &str) -> bool {
        self.layout.post_types.iter().any(|t| t == post_type)
    }

    /// Whether a cleaned title and label can be drawn: every character
    /// passes the text check and the layout finds room (an ellipsis it
    /// needs must exist in the font). The title must not be empty.
    #[must_use]
    pub fn supports(&self, title: &str, label: &str) -> bool {
        self.lay_out(title, label).is_some()
    }

    /// The title lines the card would draw (the last one ends in "…" when
    /// the title was cut), or `None` when [`Self::supports`] is false.
    #[must_use]
    pub fn title_lines(&self, title: &str, label: &str) -> Option<Vec<String>> {
        self.lay_out(title, label).map(|p| p.title)
    }

    fn lay_out(&self, title: &str, label: &str) -> Option<layout::Placed> {
        if title.is_empty()
            || !text::supported(&self.font, title)
            || !text::supported(&self.font, label)
        {
            return None;
        }
        layout::place(&self.font, &self.layout, title, label)
    }

    /// The card key: 16 hex characters of SHA-256 over the renderer
    /// revision, the text, the theme's identity and its file digests.
    #[must_use]
    pub fn key(&self, theme_name: &str, theme_version: i64, title: &str, label: &str) -> String {
        let version = theme_version.to_string();
        card_key(&[
            RENDERER_REVISION.as_bytes(),
            title.as_bytes(),
            label.as_bytes(),
            theme_name.as_bytes(),
            version.as_bytes(),
            self.digests[0].as_bytes(),
            self.digests[1].as_bytes(),
            self.digests[2].as_bytes(),
        ])
    }

    /// Draws the card and encodes it as PNG.
    ///
    /// # Errors
    /// When the text cannot be drawn (the caller should have checked
    /// [`Self::supports`]) or the encoder fails.
    pub fn render(&self, title: &str, label: &str) -> Result<Vec<u8>, String> {
        let placed = self
            .lay_out(title, label)
            .ok_or_else(|| "the title or label cannot be drawn".to_owned())?;
        let mut img = self.background.clone();
        draw::lines(
            &mut img,
            &self.font,
            &placed.title,
            self.layout.title.x,
            self.layout.title.y,
            placed.title_size,
            self.color,
        );
        if let (Some(line), Some(b)) = (&placed.label, self.layout.label) {
            draw::lines(
                &mut img,
                &self.font,
                std::slice::from_ref(line),
                b.x,
                b.y,
                b.size,
                self.label_color,
            );
        }
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .map_err(|e| format!("card encode: {e}"))?;
        Ok(out.into_inner())
    }
}

/// SHA-256 over length-prefixed fields (each a u32 big-endian length,
/// then its bytes), first 16 hex characters.
#[must_use]
pub fn card_key(fields: &[&[u8]]) -> String {
    let mut h = Sha256::new();
    for f in fields {
        let len = u32::try_from(f.len()).unwrap_or(u32::MAX);
        h.update(len.to_be_bytes());
        h.update(f);
    }
    hex::encode(&h.finalize()[..8])
}

/// Width and height from a PNG header, without decoding.
#[must_use]
pub fn png_dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    const MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
    if bytes.get(..8)? != MAGIC || bytes.get(12..16)? != b"IHDR" {
        return None;
    }
    let w = u32::from_be_bytes(bytes.get(16..20)?.try_into().ok()?);
    let h = u32::from_be_bytes(bytes.get(20..24)?.try_into().ok()?);
    Some((w, h))
}

/// Whether the font has a glyph for `c` (glyph 0 is `.notdef`).
fn has_glyph(font: &FontVec, c: char) -> bool {
    font.glyph_id(c).0 != 0
}
