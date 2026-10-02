//! Budget caps evaluation tests.

#![allow(clippy::pedantic, clippy::unwrap_used)]

use vyasa_ai::{evaluate_budget, BudgetCaps, BudgetDecision};

fn caps() -> BudgetCaps {
    let mut per = std::collections::HashMap::new();
    per.insert(String::from("theme-gen"), 5.0);
    BudgetCaps {
        total_month_usd: Some(20.0),
        per_purpose_month_usd: per,
    }
}

#[test]
fn under_caps_allows() {
    assert_eq!(
        evaluate_budget(&caps(), 10.0, "theme-gen", 1.0),
        BudgetDecision::Allowed
    );
}

#[test]
fn purpose_cap_denies_only_that_purpose() {
    assert_eq!(
        evaluate_budget(&caps(), 10.0, "theme-gen", 5.0),
        BudgetDecision::PurposeExceeded
    );
    // Another purpose unaffected by theme-gen's cap (but still under total).
    assert_eq!(
        evaluate_budget(&caps(), 10.0, "editor-assist", 4.9),
        BudgetDecision::Allowed
    );
}

#[test]
fn total_cap_overrides_everything() {
    assert_eq!(
        evaluate_budget(&caps(), 25.0, "editor-assist", 0.0),
        BudgetDecision::TotalExceeded
    );
}
