//! Options domain tests: permalink grammar, custom-CSS sanitizer battery,
//! and OptionChanged-driven cache invalidation.

#![allow(clippy::pedantic)]

use vyasa_core::events;
use vyasa_core::options::{sanitize_custom_css, PermalinkPattern};
use vyasa_themes::cache::{keys, CacheService};

// --- Permalink grammar -------------------------------------------------------

#[test]
fn valid_patterns_parse() {
    for raw in [
        "/post/{slug}",
        "/{year}/{month}/{slug}",
        "/blog/{type}/{slug}",
        "/posts/{year}/{month}/{slug}",
    ] {
        assert!(PermalinkPattern::parse(raw).is_ok(), "{raw}");
    }
}

#[test]
fn invalid_patterns_are_rejected() {
    let cases = [
        ("/post/{author}", "unknown token"),
        ("/post/{slug", "unclosed"),
        ("/Post/{slug}", "uppercase literal"),
        ("post slug/{x}", "space"),
        ("//{slug}", "empty segment"),
        ("/{slug}/", "trailing slash"),
    ];
    for (raw, why) in cases {
        assert!(PermalinkPattern::parse(raw).is_err(), "{why}: {raw}");
    }
}

#[test]
fn pattern_builds_urls() {
    let p = PermalinkPattern::parse("/{year}/{month}/{slug}").expect("valid");
    assert_eq!(p.url_for("post", "hello", 2026, 8), "/2026/08/hello");
    let d = PermalinkPattern::parse(PermalinkPattern::DEFAULT).expect("default");
    assert_eq!(d.url_for("post", "abc", 2026, 1), "/post/abc");
}

// --- Custom CSS sanitizer battery --------------------------------------------

#[test]
fn malicious_css_fixtures_are_stripped() {
    const FIXTURES: &[(&str, &str)] = &[
        // @import exfiltration
        ("@import url(https://evil.example/x.css);\np{color:red}", ""),
        // expression()
        ("p{width:expression(alert(1))}", ""),
        // behavior / -moz-binding
        ("p{-moz-binding:url(https://evil.example/x.xml)}", ""),
        // javascript: url
        ("a{background:url(javascript:alert(1))}", ""),
        // nested at-rules
        (
            "@media screen{@supports(color:red){p{color:blue}}}\n.b{c:d}",
            ".b{c:d}",
        ),
        // data: urls
        ("p{background:url(data:text/html,<script>1</script>)}", ""),
    ];
    for (input, must_not_contain) in FIXTURES {
        let out = sanitize_custom_css(input);
        if !must_not_contain.is_empty() {
            assert!(
                out.contains(must_not_contain),
                "{input:?}: safe rule lost: {out:?}"
            );
        }
        let lower = out.to_ascii_lowercase();
        for banned in [
            "@import",
            "expression(",
            "-moz-binding",
            "javascript:",
            "data:",
            "@media",
            "@supports",
        ] {
            assert!(
                !lower.contains(banned),
                "{input:?}: leaked {banned}: {out:?}"
            );
        }
    }
}

#[test]
fn safe_rules_survive_with_https_urls() {
    let input = "body{color:#333}\na:hover{text-decoration:underline}\n\
                 .hero{background:url('https://cdn.example.com/bg.jpg')}";
    let out = sanitize_custom_css(input);
    assert!(out.contains("color:#333"), "{out:?}");
    assert!(out.contains("underline"));
    assert!(out.contains("https://cdn.example.com/bg.jpg"), "{out:?}");
    // http:// (non-secure) urls are dropped.
    let out2 = sanitize_custom_css("p{background:url(http://insecure.example/x.png)}");
    assert!(!out2.contains("http://insecure.example"), "{out2:?}");
}

#[test]
fn namespacing_scopes_selectors_under_vy_site() {
    use vyasa_themes::namespace_custom_css;
    let css = sanitize_custom_css("h1{font-size:2rem}\n.post .title{margin:0}");
    let scoped = namespace_custom_css(&css);
    assert!(scoped.starts_with("#vy-site {"), "{scoped}");
    assert!(
        scoped.contains("#vy-site { h1{") || scoped.contains("#vy-site {\n  h1 {"),
        "{scoped}"
    );
    assert!(scoped.contains(".post .title"), "{scoped}");
}

// --- Cache invalidation via OptionChanged ------------------------------------

#[test]
fn option_changed_event_purges_render_cache() {
    let cache = CacheService::new(1024 * 1024, 60);
    let version = "pubtest-v1";
    cache.insert(&keys::home(version), vyasa_themes::CachedPage::new("stale"));

    let mut rx = events::subscribe();
    events::emit_option_changed("site_title");

    let received = futures::executor::block_on(async {
        loop {
            match rx.try_recv() {
                Ok(ev) => break ev,
                Err(tokio::sync::broadcast::error::TryRecvError::Empty) => std::hint::spin_loop(),
                Err(e) => panic!("bus error: {e}"),
            }
        }
    });

    match received {
        events::Event::OptionChanged(changed) => {
            // Policy: any option change drops the whole render cache — the
            // site identity can appear on any page.
            cache.invalidate_theme();
            assert_eq!(changed.key, "site_title");
            assert!(cache.get(&keys::home(version)).is_none(), "cache purged");
        }
        other => panic!("unexpected event {other:?}"),
    }
}

// --- Brand kit ---------------------------------------------------------------

use serde_json::json;
use vyasa_core::options::{brand_kit_prompt_of, validate_brand_kit, BRAND_KIT_FIELDS};

#[test]
fn a_brand_kit_is_short_free_text_under_known_names() {
    assert!(validate_brand_kit(&json!({"voice": "Quiet and technical."})).is_ok());
    assert!(validate_brand_kit(&json!({})).is_ok());
    // Absent is fine: most sites will never write one.
    assert!(validate_brand_kit(&serde_json::Value::Null).is_ok());

    // A typo in a field name would otherwise be stored and silently never
    // reach a prompt.
    let err = validate_brand_kit(&json!({"vibe": "loud"})).expect_err("unknown field");
    assert!(err.to_string().contains("vibe"), "{err}");
    assert!(
        err.to_string().contains("voice"),
        "names the real fields: {err}"
    );

    assert!(
        validate_brand_kit(&json!({"voice": 42})).is_err(),
        "not text"
    );
    assert!(
        validate_brand_kit(&json!("just a string")).is_err(),
        "not an object"
    );
    let long = "x".repeat(601);
    assert!(
        validate_brand_kit(&json!({"voice": long})).is_err(),
        "too long"
    );
}

#[test]
fn the_prompt_lines_are_ordered_and_skip_blanks() {
    // Two sites with the same kit must produce the same prompt, whatever
    // order the JSON object happens to have.
    let kit = json!({
        "avoid": "Gradients.",
        "voice": "  Quiet.  ",
        "audience": "Engineers",
        "palette": "   ",
    });
    let prompt = brand_kit_prompt_of(Some(&kit));
    assert_eq!(
        prompt,
        "- audience: Engineers\n- voice: Quiet.\n- avoid: Gradients.\n"
    );
}

#[test]
fn nothing_configured_says_nothing() {
    assert_eq!(brand_kit_prompt_of(None), "");
    assert_eq!(brand_kit_prompt_of(Some(&json!({}))), "");
    assert_eq!(brand_kit_prompt_of(Some(&json!({"voice": "   "}))), "");
    // Not an object: stored by some earlier version, or by hand.
    assert_eq!(brand_kit_prompt_of(Some(&json!("oops"))), "");
}

#[test]
fn every_field_the_validator_accepts_can_reach_a_prompt() {
    // A field allowed on the way in but dropped on the way out would be a
    // setting that quietly does nothing.
    let kit = json!(BRAND_KIT_FIELDS
        .iter()
        .map(|f| ((*f).to_owned(), json!(format!("value for {f}"))))
        .collect::<serde_json::Map<_, _>>());
    assert!(validate_brand_kit(&kit).is_ok());
    let prompt = brand_kit_prompt_of(Some(&kit));
    for field in BRAND_KIT_FIELDS {
        assert!(prompt.contains(&format!("- {field}: ")), "{field} missing");
    }
}

#[test]
fn permalink_routes_require_safe_unambiguous_segments() {
    for pattern in [
        "/no-slug",
        "/api/{slug}",
        "/{slug}/{slug}",
        "/foo{slug}",
        "/{year}/{unknown}/{slug}",
    ] {
        assert!(PermalinkPattern::parse(pattern).is_err(), "{pattern}");
    }
    let pattern = PermalinkPattern::parse("/journal/{year}/{month}/{slug}").unwrap();
    assert_eq!(pattern.slug_from("/journal/2026/09/hello"), Some("hello"));
    assert_eq!(pattern.slug_from("/other/2026/09/hello"), None);
    assert_eq!(pattern.slug_from("/journal/not-a-year/09/hello"), None);
}

#[test]
fn menu_events_purge_every_cached_page() {
    let cache = CacheService::new(1_000_000, 300);
    for key in [
        "tone:home",
        "tone:single:a",
        "tone:archive:category",
        "ttwo:home",
    ] {
        cache.insert(key, vyasa_themes::cache::CachedPage::new("old navigation"));
    }
    cache.apply_domain_event(&events::Event::MenuChanged, "one");
    for key in [
        "tone:home",
        "tone:single:a",
        "tone:archive:category",
        "ttwo:home",
    ] {
        assert!(cache.get(key).is_none(), "stale {key}");
    }
}
