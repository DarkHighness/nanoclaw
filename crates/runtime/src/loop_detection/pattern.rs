use types::ToolCall;

pub(super) fn fingerprint_call(call: &ToolCall) -> String {
    format!("{}\n{}", call.tool_name, call.arguments)
}

pub(super) fn tool_name_from_pattern(pattern: &str) -> &str {
    pattern.split_once('\n').map_or(pattern, |(name, _)| name)
}
