//! Line layout: greedy wrapping, the size step-down and the cut.
//!
//! Line height is 1.2 x size; a size fits a box of height `h` when
//! `6 x lines x size <= 5 x h`, the integer form validation also uses, so
//! the two cannot disagree at the boundary.

use ab_glyph::{Font, FontVec, GlyphId, PxScale, ScaleFont};

use super::text::cjk_break;
use super::{has_glyph, CardLayout, TitleBox};

/// The size step of the title step-down.
const STEP: u32 = 4;

/// What [`place`] decided: the title lines and size, and the label line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Placed {
    /// Title lines, top to bottom.
    pub title: Vec<String>,
    /// Title size in pixels.
    pub title_size: u32,
    /// The label line, when the layout has a label box and a label.
    pub label: Option<String>,
}

/// Lays out a cleaned, checked title and label; `None` when a cut is
/// needed and the font has neither "…" nor ".".
pub(super) fn place(
    font: &FontVec,
    layout: &CardLayout,
    title: &str,
    label: &str,
) -> Option<Placed> {
    let (title, title_size) = place_title(font, &layout.title, title)?;
    let label = match layout.label {
        Some(b) if !label.is_empty() => {
            let m = Measure::new(font, b.size);
            Some(fit_one_line(&m, &tokens(label), b.w)?)
        }
        _ => None,
    };
    Some(Placed {
        title,
        title_size,
        label,
    })
}

/// The scale of a font size given in pixels per em.
pub(super) fn scale(font: &FontVec, size: u32) -> PxScale {
    let size = px(size);
    let factor = font
        .units_per_em()
        .map_or(1.0, |upem| font.height_unscaled() / upem);
    PxScale::from(size * factor)
}

#[allow(clippy::cast_precision_loss)] // sizes and widths are at most 1200
pub(super) fn px(v: u32) -> f32 {
    v as f32
}

/// Text widths at one size, kerning included.
struct Measure<'a> {
    font: ab_glyph::PxScaleFont<&'a FontVec>,
    ellipsis: Option<&'static str>,
}

impl<'a> Measure<'a> {
    fn new(font: &'a FontVec, size: u32) -> Self {
        let ellipsis = if has_glyph(font, '…') {
            Some("…")
        } else if has_glyph(font, '.') {
            Some("...")
        } else {
            None
        };
        Self {
            font: font.as_scaled(scale(font, size)),
            ellipsis,
        }
    }

    fn width(&self, s: &str) -> f32 {
        let mut w = 0.0;
        let mut prev: Option<GlyphId> = None;
        for c in s.chars() {
            let id = self.font.glyph_id(c);
            if let Some(p) = prev {
                w += self.font.kern(p, id);
            }
            w += self.font.h_advance(id);
            prev = Some(id);
        }
        w
    }

    fn fits(&self, s: &str, w: u32) -> bool {
        self.width(s) <= px(w)
    }
}

/// An unbreakable run and whether a space precedes it.
#[derive(Debug, Clone)]
struct Token {
    text: String,
    space_before: bool,
}

/// Splits cleaned text at its break opportunities: spaces, and between
/// two characters of the Han, Hiragana, Katakana or Hangul scripts.
fn tokens(s: &str) -> Vec<Token> {
    let mut out = Vec::new();
    for word in s.split(' ').filter(|w| !w.is_empty()) {
        let mut cur = String::new();
        let mut first = true;
        let mut prev: Option<char> = None;
        for c in word.chars() {
            if prev.is_some_and(|p| cjk_break(p, c)) {
                out.push(Token {
                    text: std::mem::take(&mut cur),
                    space_before: first && !out.is_empty(),
                });
                first = false;
            }
            cur.push(c);
            prev = Some(c);
        }
        out.push(Token {
            text: cur,
            space_before: first && !out.is_empty(),
        });
    }
    out
}

fn join(line: &mut String, t: &Token) {
    if !line.is_empty() && t.space_before {
        line.push(' ');
    }
    line.push_str(&t.text);
}

/// Greedy wrapping into width `w`: each line takes tokens while they fit.
/// Returns, per line, the index of its first token.
fn wrap(m: &Measure<'_>, toks: &[Token], w: u32) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut line = String::new();
    for (i, t) in toks.iter().enumerate() {
        if line.is_empty() {
            starts.push(i);
            line.push_str(&t.text);
            continue;
        }
        let mut cand = line.clone();
        join(&mut cand, t);
        if m.fits(&cand, w) {
            line = cand;
        } else {
            starts.push(i);
            line.clone_from(&t.text);
        }
    }
    starts
}

fn line_text(toks: &[Token]) -> String {
    let mut s = String::new();
    for t in toks {
        join(&mut s, t);
    }
    s
}

fn lines_of(toks: &[Token], starts: &[usize]) -> Vec<String> {
    starts
        .iter()
        .enumerate()
        .map(|(n, &a)| {
            let b = starts.get(n + 1).copied().unwrap_or(toks.len());
            line_text(&toks[a..b])
        })
        .collect()
}

/// Whether `lines` lines at `size` fit a box of height `h`.
fn tall_enough(lines: usize, size: u32, h: u32) -> bool {
    let lines = u64::try_from(lines).unwrap_or(u64::MAX);
    lines.saturating_mul(6).saturating_mul(u64::from(size)) <= u64::from(h) * 5
}

fn place_title(font: &FontVec, b: &TitleBox, title: &str) -> Option<(Vec<String>, u32)> {
    let toks = tokens(title);
    let max_lines = usize::try_from(b.max_lines).unwrap_or(usize::MAX);
    let mut size = b.size;
    loop {
        let m = Measure::new(font, size);
        if toks.iter().all(|t| m.fits(&t.text, b.w)) {
            let starts = wrap(&m, &toks, b.w);
            if starts.len() <= max_lines && tall_enough(starts.len(), size, b.h) {
                return Some((lines_of(&toks, &starts), size));
            }
        }
        if size <= b.min_size {
            break;
        }
        size = size.saturating_sub(STEP).max(b.min_size);
    }
    // Nothing fits: lay out at min_size with as many lines as the box
    // holds, break over-wide runs at characters, cut the last line.
    let size = b.min_size;
    let m = Measure::new(font, size);
    let by_height = u64::from(b.h) * 5 / (6 * u64::from(size).max(1));
    let allowed = usize::try_from(by_height)
        .unwrap_or(usize::MAX)
        .min(max_lines)
        .max(1);
    let toks = break_wide(&m, toks, b.w);
    let starts = wrap(&m, &toks, b.w);
    if starts.len() <= allowed {
        return Some((lines_of(&toks, &starts), size));
    }
    let mut lines = lines_of(&toks, &starts[..allowed - 1]);
    let rest = toks[starts[allowed - 1]..].to_vec();
    lines.truncate(allowed - 1);
    lines.push(cut(&m, &rest, b.w)?);
    Some((lines, size))
}

/// Splits every run wider than `w` at character boundaries.
fn break_wide(m: &Measure<'_>, toks: Vec<Token>, w: u32) -> Vec<Token> {
    let mut out = Vec::with_capacity(toks.len());
    for t in toks {
        if m.fits(&t.text, w) {
            out.push(t);
            continue;
        }
        let mut space_before = t.space_before;
        let mut piece = String::new();
        for c in t.text.chars() {
            let mut cand = piece.clone();
            cand.push(c);
            if !piece.is_empty() && !m.fits(&cand, w) {
                out.push(Token {
                    text: std::mem::take(&mut piece),
                    space_before,
                });
                space_before = false;
                piece.push(c);
            } else {
                piece = cand;
            }
        }
        if !piece.is_empty() {
            out.push(Token {
                text: piece,
                space_before,
            });
        }
    }
    out
}

/// One line: the text if it fits, else [`cut`].
fn fit_one_line(m: &Measure<'_>, toks: &[Token], w: u32) -> Option<String> {
    let whole = line_text(toks);
    if m.fits(&whole, w) {
        Some(whole)
    } else {
        cut(m, toks, w)
    }
}

/// The longest prefix of `toks` that, with the ellipsis, fits `w`: at a
/// break opportunity when one fits, else at a character. `None` when the
/// font has no ellipsis and no full stop.
fn cut(m: &Measure<'_>, toks: &[Token], w: u32) -> Option<String> {
    let ell = m.ellipsis?;
    let with_ell = |s: &str| {
        let mut s = s.trim_end().to_owned();
        s.push_str(ell);
        s
    };
    for n in (1..toks.len()).rev() {
        let cand = with_ell(&line_text(&toks[..n]));
        if m.fits(&cand, w) {
            return Some(cand);
        }
    }
    let whole: Vec<char> = line_text(toks).chars().collect();
    for n in (0..whole.len()).rev() {
        let prefix: String = whole[..n].iter().collect();
        let cand = with_ell(&prefix);
        if m.fits(&cand, w) {
            return Some(cand);
        }
    }
    Some(ell.to_owned())
}
