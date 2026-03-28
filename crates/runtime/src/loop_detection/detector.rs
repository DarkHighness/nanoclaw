use super::pattern::{fingerprint_call, tool_name_from_pattern};
use super::types::{
    LoopDetectionConfig, LoopSignal, LoopSignalSeverity, ToolLoopEntry, ToolLoopHistory,
};
use types::{ToolCall, ToolResult};

#[derive(Clone, Debug)]
pub struct ToolLoopDetector {
    config: LoopDetectionConfig,
    history: ToolLoopHistory,
    last_warning_pattern: Option<String>,
}

impl ToolLoopDetector {
    #[must_use]
    pub fn new(config: LoopDetectionConfig) -> Self {
        Self {
            config,
            history: ToolLoopHistory::new(),
            last_warning_pattern: None,
        }
    }

    pub fn inspect(&mut self, call: &ToolCall) -> Option<LoopSignal> {
        if !self.config.enabled {
            return None;
        }
        let pattern = fingerprint_call(call);

        if let Some(signal) = self.same_pattern_no_progress_signal(&pattern) {
            return Some(signal);
        }
        if let Some(signal) = self.ping_pong_signal(&pattern) {
            return Some(signal);
        }
        if let Some(signal) = self.global_circuit_breaker_signal() {
            return Some(signal);
        }
        None
    }

    pub fn record_result(&mut self, call: &ToolCall, result: &ToolResult) {
        let outcome = if result.is_error {
            format!("error:{}", result.text_content())
        } else {
            format!("ok:{}", result.text_content())
        };
        self.record_entry(ToolLoopEntry {
            pattern: fingerprint_call(call),
            outcome,
        });
    }

    pub fn record_error(&mut self, call: &ToolCall, error: &str) {
        self.record_entry(ToolLoopEntry {
            pattern: fingerprint_call(call),
            outcome: format!("error:{error}"),
        });
    }

    fn record_entry(&mut self, entry: ToolLoopEntry) {
        // Warnings are pattern-scoped; once the tool pattern changes, a future repeat
        // is allowed to emit a fresh warning instead of being permanently suppressed.
        if self
            .last_warning_pattern
            .as_deref()
            .is_some_and(|pattern| pattern != entry.pattern)
        {
            self.last_warning_pattern = None;
        }
        self.history.push_back(entry);
        while self.history.len() > self.config.history_size.max(1) {
            self.history.pop_front();
        }
    }

    fn same_pattern_no_progress_signal(&mut self, pattern: &str) -> Option<LoopSignal> {
        let streak = self
            .history
            .iter()
            .rev()
            .take_while(|entry| entry.pattern == pattern)
            .collect::<Vec<_>>();
        if streak.is_empty() {
            return None;
        }
        let attempts = streak.len() + 1;
        let unchanged = streak
            .windows(2)
            .all(|pair| pair[0].outcome == pair[1].outcome);
        let reason = if unchanged {
            format!(
                "repeating `{}` with unchanged outcomes for {attempts} consecutive attempts",
                tool_name_from_pattern(pattern)
            )
        } else {
            format!(
                "repeating `{}` with the same arguments for {attempts} consecutive attempts",
                tool_name_from_pattern(pattern)
            )
        };

        if attempts >= self.config.critical_threshold {
            self.last_warning_pattern = Some(pattern.to_string());
            return Some(LoopSignal {
                severity: LoopSignalSeverity::Critical,
                reason,
            });
        }
        if attempts >= self.config.warning_threshold
            && self.last_warning_pattern.as_deref() != Some(pattern)
        {
            self.last_warning_pattern = Some(pattern.to_string());
            return Some(LoopSignal {
                severity: LoopSignalSeverity::Warning,
                reason,
            });
        }
        None
    }

    fn ping_pong_signal(&mut self, next_pattern: &str) -> Option<LoopSignal> {
        let Some(last) = self.history.back() else {
            return None;
        };
        if last.pattern == next_pattern {
            return None;
        }
        let mut expected = last.pattern.as_str();
        let mut alternate_len = 0usize;
        for entry in self.history.iter().rev() {
            if entry.pattern != expected {
                break;
            }
            alternate_len += 1;
            expected = if expected == next_pattern {
                last.pattern.as_str()
            } else {
                next_pattern
            };
        }
        let attempts = alternate_len + 1;
        if attempts < self.config.warning_threshold {
            return None;
        }

        let reason = format!(
            "alternating between repeated tool call patterns involving `{}` for {attempts} attempts",
            tool_name_from_pattern(next_pattern)
        );
        let severity = if attempts >= self.config.critical_threshold {
            LoopSignalSeverity::Critical
        } else {
            LoopSignalSeverity::Warning
        };
        Some(LoopSignal { severity, reason })
    }

    fn global_circuit_breaker_signal(&self) -> Option<LoopSignal> {
        let threshold = self.config.global_circuit_breaker_threshold.max(1);
        if self.history.len() + 1 < threshold {
            return None;
        }
        let recent = self
            .history
            .iter()
            .rev()
            .take(threshold - 1)
            .collect::<Vec<_>>();
        if recent.is_empty() {
            return None;
        }
        let first_outcome = &recent[0].outcome;
        let all_same_outcome = recent.iter().all(|entry| entry.outcome == *first_outcome);
        if !all_same_outcome {
            return None;
        }
        Some(LoopSignal {
            severity: LoopSignalSeverity::Critical,
            reason: format!(
                "recent tool activity produced the same outcome {} times in a row",
                threshold
            ),
        })
    }
}
