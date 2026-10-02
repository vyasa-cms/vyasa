//! spam-guard reference plugin: scores incoming comments and flags spam.
//!
//! Host-side logic lives in `heuristics`; the wasm wrapper (built with
//! plugin-sdk tooling) forwards comment bodies through [`is_spam`].

pub mod heuristics;

/// Default auto-spam threshold.
pub const DEFAULT_THRESHOLD: u8 = 60;

/// Returns `(score, is_spam)` for a comment body.
#[must_use]
pub fn evaluate(body: &str, threshold: u8) -> (u8, bool) {
    let signals = heuristics::extract(body);
    let s = heuristics::score(&signals, threshold.max(100));
    (s, s >= threshold)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::heuristics::extract;

    #[test]
    fn clean_comment_scores_low() {
        let (s, spam) = evaluate("Great article, thanks for sharing!", DEFAULT_THRESHOLD);
        assert!(s < DEFAULT_THRESHOLD);
        assert!(!spam);
    }

    #[test]
    fn link_farm_is_flagged() {
        let body = "check https://a.io https://b.io https://c.io https://d.io";
        let (_, spam) = evaluate(body, DEFAULT_THRESHOLD);
        assert!(spam);
    }

    #[test]
    fn shouting_is_flagged() {
        let (_, spam) = evaluate("BUY NOW CHEAP MEDS ONLINE STORE", DEFAULT_THRESHOLD);
        assert!(spam);
    }

    #[test]
    fn repeated_word_spam_detected_by_extractor() {
        let s = extract("wonderful wonderful wonderful offers inside");
        assert!(s.repeated_words);
    }

    #[test]
    fn caps_ratio_math() {
        let s = extract("AAAA");
        assert!((s.caps_ratio - 1.0).abs() < 1e-9);
        let s2 = extract("");
        assert_eq!(s2.caps_ratio, 0.0);
    }

    #[test]
    fn short_bodies_get_small_penalty() {
        let plain = heuristics::score(&extract("nice post thank you"), 100);
        let short = heuristics::score(&extract("hi"), 100);
        assert!(short > plain);
    }
}

