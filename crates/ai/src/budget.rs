//! Monthly spend caps with graceful degradation.
//!
//! Month boundary is the **UTC calendar month** (documented). The check
//! has a small race tolerance — a concurrent call may push spend slightly
//! past the cap (overrun by at most one completion), which is acceptable
//! for cost governance.

use vyasa_common::AppError;

/// Configured caps; `None` fields are uncapped.
#[derive(Debug, Clone, Default)]
pub struct BudgetCaps {
    /// Total USD per month.
    pub total_month_usd: Option<f64>,
    /// Per-purpose USD per month (purpose -> cap).
    pub per_purpose_month_usd: std::collections::HashMap<String, f64>,
}

/// Pre-call verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetDecision {
    /// Under every cap: proceed.
    Allowed,
    /// Total month cap reached (`ai_budget_exceeded`).
    TotalExceeded,
    /// Purpose month cap reached (`ai_budget_exceeded`).
    PurposeExceeded,
}

/// Evaluates the caps against current spend.
#[must_use]
pub fn evaluate(
    caps: &BudgetCaps,
    total_spend: f64,
    purpose: &str,
    purpose_spend: f64,
) -> BudgetDecision {
    if let Some(cap) = caps.total_month_usd {
        if total_spend >= cap {
            return BudgetDecision::TotalExceeded;
        }
    }
    if let Some(cap) = caps.per_purpose_month_usd.get(purpose) {
        if purpose_spend >= *cap {
            return BudgetDecision::PurposeExceeded;
        }
    }
    BudgetDecision::Allowed
}

/// Builds the [`AppError::External`] for a capped account with the
/// machine-readable code embedded so UIs can branch on it.
#[must_use]
pub fn budget_error(decision: BudgetDecision, purpose: &str) -> AppError {
    AppError::external(
        "ai-budget",
        format!("ai_budget_exceeded: {decision:?} (purpose={purpose})"),
    )
}
