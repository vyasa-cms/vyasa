//! Compares Vyasa's two agent harnesses on a fixed set of Theme Studio
//! briefs, through `sarathi-eval`'s passes: spend caps, saved records and
//! reports.
//!
//! Both arms get the same starting theme, prompts, toolbox, offline
//! renderer and step budget; each runs the way a site runs it (the old
//! arm through the registry's failover chain, the new one through
//! Sarathi). Nothing here calls a provider unless a pass is run live.
//!
//! # Where the arms differ
//!
//! Each arm behaves as it does in production, so a report has to be read
//! with these in mind:
//!
//! - **Answer limit.** The new arm sends 4096 output tokens. The old arm
//!   sends 4096 to Anthropic and 8192 to `OpenAI` and Gemini. A reasoning
//!   model counts its reasoning against the limit.
//! - **Malformed steps.** The old arm repairs a bad reply inside one step
//!   with a repair prompt. A bad reply costs the new arm one of its steps.
//! - **Tool choice.** Where a model refuses a forced tool call, the old arm
//!   asks for the call in the system prompt and reads a text answer as a
//!   step.
//! - **Retries.** Sarathi retries a call whose connection fails or times
//!   out. A reply cut off that way and then retried is billed by the
//!   provider and is not counted here.

pub mod cli;
pub mod harness;
pub mod probe;
pub mod site;
pub mod usage;
