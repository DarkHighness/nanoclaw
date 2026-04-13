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

    #[must_use]
    pub const fn prefix_cache_eligible_tokens(&self) -> u64 {
        self.input_tokens
    }

    #[must_use]
    pub fn prefix_cache_hit_rate(&self) -> Option<f64> {
        let total = self.prefix_cache_eligible_tokens();
        if total == 0 {
            return None;
        }
        Some(self.cache_read_tokens as f64 / total as f64)
    }

    #[must_use]
    pub fn prefix_cache_hit_rate_basis_points(&self) -> Option<u32> {
        let total = self.prefix_cache_eligible_tokens();
        if total == 0 {
            return None;
        }
        let numerator = self
            .cache_read_tokens
            .saturating_mul(10_000)
            .saturating_add(total / 2);
        Some((numerator / total).min(u64::from(u32::MAX)) as u32)
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

#[cfg(test)]
mod tests {
    use super::TokenUsage;

    #[test]
    fn prefix_cache_hit_rate_is_none_without_input_tokens() {
        let usage = TokenUsage::default();

        assert_eq!(usage.prefix_cache_hit_rate(), None);
        assert_eq!(usage.prefix_cache_hit_rate_basis_points(), None);
    }

    #[test]
    fn prefix_cache_hit_rate_uses_input_tokens_as_denominator() {
        let usage = TokenUsage::from_input_output(120, 30, 20);

        assert_eq!(usage.prefix_cache_eligible_tokens(), 120);
        assert_eq!(usage.prefix_cache_hit_rate_basis_points(), Some(1667));
        assert!(
            usage
                .prefix_cache_hit_rate()
                .is_some_and(|ratio| (ratio - (1.0 / 6.0)).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn prefix_cache_hit_rate_accumulates_across_turns() {
        let mut aggregate = TokenUsage::from_input_output(120, 30, 20);
        aggregate.accumulate(&TokenUsage::from_input_output(80, 20, 40));

        assert_eq!(aggregate.input_tokens, 200);
        assert_eq!(aggregate.cache_read_tokens, 60);
        assert_eq!(aggregate.prefix_cache_hit_rate_basis_points(), Some(3000));
    }
}
