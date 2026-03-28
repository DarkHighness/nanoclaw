mod detector;
mod pattern;
mod types;

pub use detector::ToolLoopDetector;
pub use types::{LoopDetectionConfig, LoopSignal, LoopSignalSeverity};

#[cfg(test)]
mod tests {
    use super::{LoopDetectionConfig, LoopSignalSeverity, ToolLoopDetector};
    use types::{ToolCall, ToolCallId, ToolOrigin, ToolResult};

    fn call(tool_name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: ToolCallId::new(),
            call_id: "call-1".into(),
            tool_name: tool_name.into(),
            arguments,
            origin: ToolOrigin::Local,
        }
    }

    #[test]
    fn detector_warns_then_blocks_same_pattern_repetition() {
        let mut detector = ToolLoopDetector::new(LoopDetectionConfig {
            enabled: true,
            warning_threshold: 3,
            critical_threshold: 4,
            ..LoopDetectionConfig::default()
        });
        let read = call("read", serde_json::json!({"path":"a.txt"}));

        detector.record_result(&read, &ToolResult::text(read.id.clone(), "read", "same"));
        detector.record_result(&read, &ToolResult::text(read.id.clone(), "read", "same"));

        let warning = detector.inspect(&read).unwrap();
        assert_eq!(warning.severity, LoopSignalSeverity::Warning);

        detector.record_result(&read, &ToolResult::text(read.id.clone(), "read", "same"));
        let critical = detector.inspect(&read).unwrap();
        assert_eq!(critical.severity, LoopSignalSeverity::Critical);
    }

    #[test]
    fn detector_flags_ping_pong_patterns() {
        let mut detector = ToolLoopDetector::new(LoopDetectionConfig {
            enabled: true,
            warning_threshold: 4,
            critical_threshold: 6,
            ..LoopDetectionConfig::default()
        });
        let a = call("read", serde_json::json!({"path":"a.txt"}));
        let b = call("glob", serde_json::json!({"pattern":"*.rs"}));

        detector.record_result(&a, &ToolResult::text(a.id.clone(), "read", "A"));
        detector.record_result(&b, &ToolResult::text(b.id.clone(), "glob", "B"));
        detector.record_result(&a, &ToolResult::text(a.id.clone(), "read", "A"));

        let signal = detector.inspect(&b).unwrap();
        assert_eq!(signal.severity, LoopSignalSeverity::Warning);
    }
}
