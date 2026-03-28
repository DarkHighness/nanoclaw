use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub prefill_tokens: u64,
    pub decode_tokens: u64,
    pub cache_read_tokens: u64,
}

impl TokenUsage {
    #[must_use]
    pub const fn from_input_output(
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
    ) -> Self {
        Self {
            input_tokens,
            output_tokens,
            prefill_tokens: input_tokens.saturating_sub(cache_read_tokens),
            decode_tokens: output_tokens,
            cache_read_tokens,
        }
    }

    #[must_use]
    pub const fn is_zero(&self) -> bool {
        self.input_tokens == 0
            && self.output_tokens == 0
            && self.prefill_tokens == 0
            && self.decode_tokens == 0
            && self.cache_read_tokens == 0
    }

    pub fn accumulate(&mut self, other: &Self) {
        self.input_tokens = self.input_tokens.saturating_add(other.input_tokens);
        self.output_tokens = self.output_tokens.saturating_add(other.output_tokens);
        self.prefill_tokens = self.prefill_tokens.saturating_add(other.prefill_tokens);
        self.decode_tokens = self.decode_tokens.saturating_add(other.decode_tokens);
        self.cache_read_tokens = self
            .cache_read_tokens
            .saturating_add(other.cache_read_tokens);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ContextWindowUsage {
    pub used_tokens: usize,
    pub max_tokens: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TokenLedgerSnapshot {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<ContextWindowUsage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<TokenUsage>,
    #[serde(default)]
    pub cumulative_usage: TokenUsage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TokenUsagePhase {
    RequestStarted,
    ResponseCompleted,
}
