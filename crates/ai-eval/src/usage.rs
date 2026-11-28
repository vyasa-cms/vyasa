//! What one run cost, from the rows a site would write to `ai_logs`.

use std::sync::{Mutex, MutexGuard, PoisonError};

use sarathi_eval::metrics::TokenUsage;
use vyasa_ai::AiUsageSink;

#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    prompt: u64,
    completion: u64,
    cache_write: u64,
    cache_read: u64,
    calls: u32,
}

/// The usage sink of one run. A "call" is a reply the provider billed: a
/// successful one, or one the harness could not use. A row with no tokens
/// counts for nothing; a real reply without usage is caught at the model
/// seam ([`crate::probe`]).
#[derive(Debug, Default)]
pub struct RunSink {
    tally: Mutex<Tally>,
}

impl RunSink {
    fn tally(&self) -> MutexGuard<'_, Tally> {
        self.tally.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn billed(&self, prompt: u32, completion: u32) {
        if prompt == 0 && completion == 0 {
            return;
        }
        let mut t = self.tally();
        t.calls = t.calls.saturating_add(1);
        t.prompt = t.prompt.saturating_add(prompt.into());
        t.completion = t.completion.saturating_add(completion.into());
    }

    /// The run's tokens. Input excludes what was written to or read from
    /// the prompt cache, which is priced on its own.
    #[must_use]
    pub fn usage(&self) -> TokenUsage {
        let t = *self.tally();
        TokenUsage {
            input_tokens: t
                .prompt
                .saturating_sub(t.cache_write)
                .saturating_sub(t.cache_read),
            output_tokens: t.completion,
            cached_input_tokens: t.cache_read,
            cache_write_input_tokens: t.cache_write,
        }
    }

    /// Billed model calls.
    #[must_use]
    pub fn calls(&self) -> u32 {
        self.tally().calls
    }
}

impl AiUsageSink for RunSink {
    fn log_success(&self, _: &str, _: &str, _: &str, prompt: u32, completion: u32) {
        self.billed(prompt, completion);
    }

    fn log_cache(&self, _: &str, _: &str, _: &str, write: u32, read: u32) {
        let mut t = self.tally();
        t.cache_write = t.cache_write.saturating_add(write.into());
        t.cache_read = t.cache_read.saturating_add(read.into());
    }

    fn log_failure(&self, _: &str, _: &str, _: &str, _: &str) {}

    fn log_spend(&self, _: &str, _: &str, _: &str, prompt: u32, completion: u32) {
        self.billed(prompt, completion);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cache_split_comes_out_of_the_input_and_empty_rows_count_for_nothing() {
        let sink = RunSink::default();
        sink.log_success("p", "m", "u", 100, 10);
        sink.log_cache("p", "m", "u", 30, 50);
        // A billed reply the harness could not use, then rows with no tokens.
        sink.log_spend("p", "m", "u", 7, 3);
        sink.log_spend("p", "m", "u", 0, 0);
        sink.log_success("p", "m", "u", 0, 0);
        sink.log_failure("p", "m", "u", "down");
        assert_eq!(sink.calls(), 2);
        assert_eq!(
            sink.usage(),
            TokenUsage {
                input_tokens: 27,
                output_tokens: 13,
                cached_input_tokens: 50,
                cache_write_input_tokens: 30,
            }
        );
    }
}
