//! multilingual reference plugin: locale variants via post meta and
//! Accept-Language negotiation.

/// Meta key prefix: `ml:{locale}` holds translated block-document JSON.
pub const META_PREFIX: &str = "ml:";

fn quality_of(part: &str) -> f32 {
    part.split(";q=")
        .nth(1)
        .and_then(|q| q.trim().parse::<f32>().ok())
        .unwrap_or(1.0)
}

/// Negotiates the best locale from Accept-Language against available
/// translations (exact match > prefix match > default "").
#[must_use]
pub fn negotiate(accept_language: Option<&str>, available: &[String]) -> String {
    let Some(al) = accept_language else {
        return String::new();
    };
    let mut wanted: Vec<(String, f32)> = al
        .split(',')
        .filter_map(|part| {
            let first = part.trim().split(";q=").next()?;
            Some((first.trim().to_ascii_lowercase(), quality_of(part)))
        })
        .collect();
    wanted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (tag, _q) in wanted {
        if let Some(hit) = available.iter().find(|a| a.to_ascii_lowercase() == tag) {
            return hit.clone();
        }
        if let Some(base) = tag.split('-').next() {
            if let Some(hit) = available.iter().find(|a| a.to_ascii_lowercase() == base) {
                return hit.clone();
            }
        }
    }
    String::new()
}

/// Picks the translated content JSON for `locale`, falling back to the
/// original document.
#[must_use]
pub fn translate(
    original: &serde_json::Value,
    meta: &dyn Fn(&str) -> Option<serde_json::Value>,
    locale: &str,
) -> serde_json::Value {
    if locale.is_empty() {
        return original.clone();
    }
    meta(&format!("{META_PREFIX}{locale}")).unwrap_or_else(|| original.clone())
}

/// hreflang link tags for the SEO stage.
#[must_use]
pub fn hreflang_tags(base_url: &str, path: &str, locales: &[String]) -> String {
    let mut out = String::new();
    for l in locales {
        out.push_str(&format!(
            "<link rel=\"alternate\" hreflang=\"{l}\" href=\"{base_url}/{l}{path}\">\n"
        ));
    }
    out.push_str(&format!(
        "<link rel=\"alternate\" hreflang=\"x-default\" href=\"{base_url}{path}\">\n"
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation_prefers_exact_then_base() {
        let avail = vec![String::from("en"), String::from("de")];
        assert_eq!(negotiate(Some("de-DE,de;q=0.9,en;q=0.8"), &avail), "de");
        assert_eq!(negotiate(Some("fr-FR,fr;q=0.9,en-US;q=0.5"), &avail), "en");
        assert_eq!(negotiate(Some("ja"), &avail), "");
        assert_eq!(negotiate(None, &avail), "");
    }

    #[test]
    fn q_values_break_ties() {
        let avail = vec![String::from("en"), String::from("de")];
        assert_eq!(
            negotiate(Some("en;q=0.3,de;q=0.9"), &avail),
            "de",
            "higher q wins even when listed later"
        );
    }

    #[test]
    fn translate_falls_back_to_original() {
        let original = serde_json::json!({"blocks": []});
        let meta_de = |k: &str| {
            if k == "ml:de" {
                Some(serde_json::json!({"blocks": [1]})
                )
            } else {
                None
            }
        };
        assert_eq!(translate(&original, &meta_de, "de"), serde_json::json!({"blocks": [1]}));
        assert_eq!(translate(&original, &meta_de, "fr"), original);
    }

    #[test]
    fn hreflang_includes_x_default() {
        let tags = hreflang_tags("https://x.example", "/post/a", &[String::from("de")]);
        assert!(tags.contains("hreflang=\"de\""));
        assert!(tags.contains("hreflang=\"x-default\""));
    }
}
