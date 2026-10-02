//! Usage/cost accounting traits + helpers.

use std::hash::BuildHasher;

use super::provider::{compute_cost, Usage};

/// Where usage rows land. Implemented over the ai_logs repository in api;
/// tests use an in-memory recorder.
pub trait AiUsageSink: Send + Sync {
    /// Records a successful structured completion (cost computed from the
    /// provider's cost table).
    fn log_success(
        &self,
        provider: &str,
        model: &str,
        purpose: &str,
        prompt_tokens: u32,
        completion_tokens: u32,
    );

    /// Records a failed/invalid attempt (zero tokens).
    fn log_failure(&self, provider: &str, model: &str, purpose: &str, error: &str);

    /// Records tokens a provider charged for a reply that was not usable.
    ///
    /// A structured-output attempt that fails validation still cost what
    /// the provider billed; recording it as a zero-token failure made the
    /// month's spend read lower than the invoice. Default is to ignore,
    /// so a recorder that only cares about outcomes need not change.
    fn log_spend(
        &self,
        _provider: &str,
        _model: &str,
        _purpose: &str,
        _prompt_tokens: u32,
        _completion_tokens: u32,
    ) {
    }
}

/// Computes USD cost for one completion against `table`.
#[must_use]
pub fn cost_usd<S: BuildHasher>(
    table: &std::collections::HashMap<String, super::provider::ModelCost, S>,
    model: &str,
    usage: Usage,
) -> f64 {
    compute_cost(table, model, usage)
}
