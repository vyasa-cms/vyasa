//! Card text: cleaning and the allow-list check.

use ab_glyph::FontVec;
use unicode_bidi::BidiClass;
use unicode_general_category::{get_general_category, GeneralCategory};
use unicode_normalization::UnicodeNormalization;
use unicode_script::{Script, UnicodeScript};

use super::has_glyph;

/// Longest title drawn, in characters after cleaning.
pub const MAX_TITLE_CHARS: usize = 200;
/// Longest label drawn, in characters after cleaning.
pub const MAX_LABEL_CHARS: usize = 60;

/// Cleans card text, in this order: drop every Cc and Cf character that
/// is not White_Space; collapse each run of White_Space to one space; trim;
/// NFC; cut to `max_chars` characters and trim the end again.
#[must_use]
pub fn clean(s: &str, max_chars: usize) -> String {
    let mut collapsed = String::with_capacity(s.len());
    let mut in_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            in_space = true;
            continue;
        }
        if matches!(
            get_general_category(c),
            GeneralCategory::Control | GeneralCategory::Format
        ) {
            continue;
        }
        if in_space && !collapsed.is_empty() {
            collapsed.push(' ');
        }
        in_space = false;
        collapsed.push(c);
    }
    let cut: String = collapsed.nfc().take(max_chars).collect();
    cut.trim_end().to_owned()
}

/// Whether every character of `s` can be drawn left to right, one glyph
/// per character: an allowed script (or Common), no combining mark, no
/// variation selector, no right-to-left or Arabic-number bidi class, and a
/// glyph in the font.
pub(super) fn supported(font: &FontVec, s: &str) -> bool {
    s.chars().all(|c| char_supported(font, c))
}

fn char_supported(font: &FontVec, c: char) -> bool {
    let script_ok = matches!(
        c.script(),
        Script::Latin
            | Script::Greek
            | Script::Cyrillic
            | Script::Armenian
            | Script::Georgian
            | Script::Han
            | Script::Hiragana
            | Script::Katakana
            | Script::Hangul
            | Script::Common
    );
    let mark = matches!(
        get_general_category(c),
        GeneralCategory::NonspacingMark
            | GeneralCategory::SpacingMark
            | GeneralCategory::EnclosingMark
    );
    let selector =
        matches!(c, '\u{FE00}'..='\u{FE0F}' | '\u{E0100}'..='\u{E01EF}' | '\u{180B}'..='\u{180F}');
    let rtl = matches!(
        unicode_bidi::bidi_class(c),
        BidiClass::R | BidiClass::AL | BidiClass::AN
    );
    script_ok && !mark && !selector && !rtl && has_glyph(font, c)
}

/// Whether a break may fall between `a` and `b` without a space: both are
/// Han, Hiragana, Katakana or Hangul.
pub(super) fn cjk_break(a: char, b: char) -> bool {
    is_cjk(a) && is_cjk(b)
}

fn is_cjk(c: char) -> bool {
    matches!(
        c.script(),
        Script::Han | Script::Hiragana | Script::Katakana | Script::Hangul
    )
}
