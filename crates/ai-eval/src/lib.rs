//! Compares Vyasa's two agent harnesses on a fixed set of Theme Studio
//! briefs, through `sarathi-eval`'s passes: spend caps, saved records and
//! reports.
//!
//! Both arms get the same starting theme, prompts, toolbox, offline
//! renderer and step budget; each runs the way a site runs it (the old
//! arm through the registry's failover chain, the new one through
//! Sarathi). Nothing here calls a provider unless a pass is run live.

pub mod cli;
pub mod harness;
pub mod probe;
pub mod site;
pub mod usage;
