//! Spam scoring heuristics: pure functions over comment content.

/// Individual signal scores; higher = more suspicious.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpamSignals {
    /// Number of hyperlinks in the body.
    pub links: usize,
    /// Ratio of uppercase letters to letters (0.0..=1.0).
    pub caps_ratio: f64,
    /// Whether the body repeats the same 6+ char word 3+ times.
    pub repeated_words: bool,
    /// Body length in characters.
    pub length: usize,
}

/// Extracts signals from raw text (links counted from markdown-ish and
/// bare URLs).
#[must_use]
pub fn extract(body: &str) -> SpamSignals {
    let lower = body.to_ascii_lowercase();
    let links = lower.matches("http://").count() + lower.matches("https://").count();
    let letters: Vec<char> = body.chars().filter(char::is_ascii_alphabetic).collect();
    let upper = letters.iter().filter(|c| c.is_ascii_uppercase()).count();
    let caps_ratio = if letters.is_empty() {
        0.0
    } else {
        f64::from(upper as u32) / f64::from(letters.len() as u32)
    };
    let repeated = repeated_words(&lower);
    SpamSignals {
        links,
        caps_ratio,
        repeated_words: repeated,
        length: body.chars().count(),
    }
}

fn repeated_words(lower: &str) -> bool {
    let mut counts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for word in lower.split_whitespace() {
        let w: String = word.chars().filter(char::is_ascii_alphanumeric).collect();
        if w.chars().count() >= 6 {
            let count = counts.entry(w).or_default();
            *count += 1;
            if *count >= 3 {
                return true;
            }
        }
    }
    false
}

/// Composite score 0..=100. >= threshold means auto-spam.
#[must_use]
pub fn score(s: &SpamSignals, auto_spam_threshold: u8) -> u8 {
    let mut score: u32 = 0;
    score += u32::try_from(s.links.min(4) * 15).unwrap_or(60);
    if s.caps_ratio > 0.7 {
        score += 60;
    } else if s.caps_ratio > 0.4 {
        score += 30;
    }
    if s.repeated_words {
        score += 25;
    }
    if s.length < 15 {
        score += 10;
    }
    let _ = auto_spam_threshold; // threshold applied by is_spam
    score.clamp(0, 100) as u8
}

/// Convenience: `true` when the composite score reaches the threshold.
#[must_use]
pub fn is_spam(body: &str, threshold: u8) -> bool {
    let s = extract(body);
    score(&s, threshold) >= threshold.min(100)
}
