//! Slug generation: unicode-folding, URL-safe, and uniqueness helpers.

/// Maximum slug length before truncation.
const MAX_LEN: usize = 200;

/// Converts `input` into a URL-safe slug.
///
/// Non-ASCII characters are folded to ASCII (e.g. `Café` → `cafe`), all
/// letters are lowercased, and runs of non-alphanumeric characters become
/// single hyphens. The result is truncated to 200 characters. Empty or
/// punctuation-only input yields an empty string — callers decide the
/// fallback (see [`unique_slug`]).
///
/// Note that folding can make distinct titles collide (`café` and `cafe`
/// both produce `cafe`); use [`unique_slug`] to disambiguate.
#[must_use]
pub fn slugify(input: &str) -> String {
    let folded = fold_ascii(input);
    let mut out = String::with_capacity(folded.len());
    let mut prev_dash = true; // suppress leading hyphens
    for ch in folded.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    if out.len() > MAX_LEN {
        let mut end = MAX_LEN;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
        while out.ends_with('-') {
            out.pop();
        }
    }
    out
}

/// Folds non-ASCII characters to their ASCII transliteration.
///
/// Emoji and other symbols whose transliteration is a multi-word name
/// (deunicode renders `🌍` as `"Earth Africa"`) are dropped instead of
/// polluting slugs with unrelated words.
fn fold_ascii(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii() {
            out.push(ch);
        } else if let Some(replacement) = deunicode::deunicode_char(ch) {
            let trimmed = replacement.trim();
            if !trimmed.is_empty() && !trimmed.contains(char::is_whitespace) {
                out.push_str(trimmed);
            }
        }
    }
    out
}

/// The slugs [`unique_slug`] tries for `base`, in order: `base`, then
/// `base-2` through `base-999`.
///
/// Callers that check a whole batch at once (one database round trip for
/// every candidate) use this to stay on the same suffix scheme.
pub fn slug_candidates(base: &str) -> impl Iterator<Item = String> + '_ {
    std::iter::once(base.to_string()).chain((2..1000_u32).map(move |n| format!("{base}-{n}")))
}

/// Returns `base` if `is_taken` reports it as unused; otherwise appends
/// `-2`, `-3`, ... until an unused slug is found.
///
/// `base` must be non-empty (see [`slugify`]). If absurdly many candidates
/// are taken, a millisecond-timestamp suffix guarantees termination.
#[must_use]
pub fn unique_slug(base: &str, is_taken: impl Fn(&str) -> bool) -> String {
    if let Some(free) = slug_candidates(base).find(|candidate| !is_taken(candidate)) {
        return free;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(0));
    format!("{base}-{now}")
}

#[cfg(test)]
mod tests {
    use super::{slugify, unique_slug};

    #[test]
    fn basic_slugification() {
        assert_eq!(slugify("Hello World"), "hello-world");
        assert_eq!(slugify("Café Crème 101!"), "cafe-creme-101");
        assert_eq!(slugify("Hello,   World!!"), "hello-world");
        assert_eq!(
            slugify("  --leading and trailing--  "),
            "leading-and-trailing"
        );
        assert_eq!(slugify("snake_case-stays"), "snake-case-stays");
    }

    #[test]
    fn empty_and_punctuation_only_input() {
        assert_eq!(slugify(""), "");
        assert_eq!(slugify("   "), "");
        assert_eq!(slugify("!!!???---"), "");
    }

    #[test]
    fn unicode_folding_collisions() {
        // Distinct titles folding to the same slug is the documented
        // collision unique_slug() exists to resolve.
        assert_eq!(slugify("café"), slugify("cafe"));
        assert_eq!(slugify("café"), "cafe");
    }

    #[test]
    fn non_latin_input_folds_to_something() {
        let slug = slugify("日本語のテキスト");
        assert!(!slug.is_empty(), "expected romanization, got empty");
    }

    #[test]
    fn emoji_are_stripped() {
        assert_eq!(slugify("Hello 🌍 World"), "hello-world");
    }

    #[test]
    fn long_input_is_truncated() {
        let long = "a".repeat(500);
        let slug = slugify(&long);
        assert!(slug.len() <= 200, "len {}", slug.len());
        assert!(!slug.ends_with('-'));
    }

    #[test]
    fn unique_slug_appends_counter() {
        let taken: Vec<&str> = vec!["hello", "hello-2", "hello-3"];
        let slug = unique_slug("hello", |c| taken.contains(&c));
        assert_eq!(slug, "hello-4");
    }

    #[test]
    fn unique_slug_returns_base_when_free() {
        let slug = unique_slug("free-slug", |_| false);
        assert_eq!(slug, "free-slug");
    }

    #[test]
    fn slug_candidates_follow_the_suffix_scheme() {
        let all: Vec<String> = super::slug_candidates("a").collect();
        assert_eq!(all.len(), 999);
        assert_eq!(all[0], "a");
        assert_eq!(all[1], "a-2");
        assert_eq!(all[998], "a-999");
    }

    #[test]
    fn unique_slug_terminates_when_everything_is_taken() {
        let slug = unique_slug("x", |_| true);
        assert!(slug.starts_with("x-"), "slug: {slug}");
        assert_ne!(slug, "x");
    }
}
