//! What a provider failure is allowed to say to the person who sees it.
//!
//! Providers describe the same few situations at wildly different lengths.
//! OpenRouter answers a rate limit with several hundred characters of
//! nested JSON, and every surface that showed it — the studio, the drafts
//! list, a toast — cut it off mid-word. These pin the classification that
//! replaced it, and the rule that the raw body never comes back out.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use vyasa_ai::{Cause, ProviderError};

/// The body that actually landed on screen, verbatim and then some.
const OPENROUTER_429: &str = r#"{"error":{"message":"Provider returned error","code":429,"metadata":{"raw":"google/gemma-4-31b-it:free is temporarily rate-limited upstream. Please retry shortly, or add your own key to accumulate your rate limits: https://openrouter.ai/settings/integrations","provider_name":"Google AI Studio","is_byok":false,"provider_error_code":"429","limit_source":"upstream_provider_shared_pool"}}}"#;

#[test]
fn a_status_is_classified_by_what_it_means() {
    let cases = [
        (401, Cause::Auth),
        (403, Cause::Auth),
        (402, Cause::Credits),
        (404, Cause::UnknownModel),
        (408, Cause::Timeout),
        (429, Cause::RateLimited),
        (400, Cause::Rejected),
        (500, Cause::Overloaded),
        (503, Cause::Overloaded),
    ];
    for (code, expected) in cases {
        let err = ProviderError::Status(code, "body".to_owned());
        assert_eq!(err.cause(), expected, "status {code}");
    }
}

#[test]
fn the_provider_body_never_reaches_the_reader() {
    let err = ProviderError::Status(429, OPENROUTER_429.to_owned());
    let shown = err.to_string();

    // Short enough to read, and nothing quoted from the provider.
    assert!(shown.len() < 120, "{} chars: {shown}", shown.len());
    for leak in [
        "{",
        "\"",
        "metadata",
        "is_byok",
        "upstream_provider_shared_pool",
    ] {
        assert!(!shown.contains(leak), "leaked {leak}: {shown}");
    }
    // It still says the thing worth knowing, and what to do.
    assert!(shown.contains("Rate limited"), "{shown}");
    assert!(shown.contains("Try again"), "{shown}");

    // And an operator loses nothing: the body is intact for the log.
    assert!(err.detail().contains("is_byok"), "detail dropped the body");
}

#[test]
fn a_chain_reports_the_failure_that_needs_a_person() {
    // Two models rate limited and one key rejected: the rate limits clear
    // on their own, the key does not. Reporting the wrong one sends someone
    // away to wait for a problem that will still be there afterwards.
    let cause = Cause::most_actionable([Cause::RateLimited, Cause::Auth, Cause::RateLimited]);
    assert_eq!(cause, Some(Cause::Auth));

    assert_eq!(Cause::most_actionable([]), None);
    assert_eq!(
        Cause::most_actionable([Cause::Network, Cause::Overloaded]),
        Some(Cause::Overloaded),
    );
}

#[test]
fn one_model_failing_does_not_get_counted_like_a_chain() {
    let single = ProviderError::AllFailed {
        cause: Cause::RateLimited,
        tried: 1,
    };
    // "No model could answer (1 tried)" reads like a bug report.
    assert!(!single.to_string().contains("tried"), "{single}");
    assert_eq!(single.to_string(), Cause::RateLimited.message());

    let chain = ProviderError::AllFailed {
        cause: Cause::RateLimited,
        tried: 3,
    };
    assert!(chain.to_string().contains("3 tried"), "{chain}");
}

#[test]
fn a_body_that_would_not_parse_is_not_the_same_as_an_unreachable_provider() {
    // Only one of these is worth another attempt, and the circuit breaker
    // and the advice both depend on telling them apart.
    let garbage = ProviderError::Transport("decode: expected value at line 1".to_owned());
    assert_eq!(garbage.cause(), Cause::Unusable);

    let unreachable = ProviderError::Transport("connection refused".to_owned());
    assert_eq!(unreachable.cause(), Cause::Network);
}

#[test]
fn every_cause_offers_something_to_do_about_it() {
    for cause in [
        Cause::RateLimited,
        Cause::Auth,
        Cause::Credits,
        Cause::UnknownModel,
        Cause::Rejected,
        Cause::Overloaded,
        Cause::Timeout,
        Cause::Network,
        Cause::Unusable,
    ] {
        let m = cause.message();
        assert!(m.len() < 100, "{cause:?} is too long to read: {m}");
        assert!(m.ends_with('.'), "{cause:?} is not a sentence: {m}");
        assert!(
            m.chars().next().is_some_and(char::is_uppercase),
            "{cause:?} does not start a sentence: {m}"
        );
    }
}
