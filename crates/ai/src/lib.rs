//! AI foundation: provider abstraction, Anthropic client with tool-use
//! structured outputs, validated retry runner, and usage/cost logging.
//!
//! Feature code must go through [`runner::run_structured`] — never call a
//! provider directly — so retries, validation, and audit stay uniform.

pub mod agent;
pub mod anthropic;
pub mod assist;
pub mod budget;
pub mod catalog;
pub mod circuit;
pub mod failover;
pub mod openai;
pub mod page_designer;
pub mod provider;
pub mod runner;
pub mod theme_studio;
pub mod usage;

pub use anthropic::AnthropicProvider;
pub use budget::{budget_error, evaluate as evaluate_budget, BudgetCaps, BudgetDecision};
pub use circuit::CircuitBreaker;
pub use failover::{complete_with_failover, FailoverProvider, ModelSlot, ProviderSlot};
pub use openai::{Moderation, OpenAiProvider};
pub use provider::{
    compute_cost, external_error, Attachment, Cause, Completion, LlmProvider, ModelCost,
    PromptSpec, ProviderError, Usage,
};
pub use runner::{run_structured, MAX_ATTEMPTS};

/// Default per-Mtok USD rates for supported models.
#[must_use]
pub fn default_cost_table() -> std::collections::HashMap<String, ModelCost> {
    std::collections::HashMap::from([
        (
            String::from("claude-sonnet-4-5"),
            ModelCost {
                input_per_mtok: 3.0,
                output_per_mtok: 15.0,
            },
        ),
        (
            String::from("claude-haiku-4-5"),
            ModelCost {
                input_per_mtok: 0.8,
                output_per_mtok: 4.0,
            },
        ),
    ])
}
pub use usage::AiUsageSink;
