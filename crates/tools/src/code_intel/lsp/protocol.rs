use crate::code_intel::{CodeLocation, CodeReference, CodeSymbol, CodeSymbolKind};
use serde_json::{Value, json};
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt};

pub(crate) async fn read_lsp_message<R>(reader: &mut R) -> io::Result<Option<Value>>
where
    R: AsyncBufRead + Unpin,
{
    let mut content_length = None;
    loop {
        let mut line = String::new();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            return Ok(None);
        }
        let line = line.trim_end_matches(['\r', '\n']);
        if line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().ok();
        }
    }

    let Some(length) = content_length else {
        return Ok(None);
    };
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(io::Error::other)
}

pub(crate) fn file_uri_from_path(path: &Path) -> String {
    let path = path.to_string_lossy();
    let mut encoded = String::with_capacity(path.len() + 8);
    encoded.push_str("file://");
    for byte in path.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || matches!(ch, '/' | '-' | '_' | '.' | '~') {
            encoded.push(ch);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{byte:02X}"));
        }
    }
    encoded
}

pub(crate) fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let raw = uri.strip_prefix("file://")?;
    let mut decoded = Vec::with_capacity(raw.len());
    let bytes = raw.as_bytes();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok()?;
            decoded.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(decoded).ok()?))
}

pub(crate) fn zero_based_position(line: usize, column: usize) -> Value {
    json!({
        "line": line.saturating_sub(1),
        "character": column.saturating_sub(1),
    })
}

pub(crate) fn parse_workspace_symbols(value: &Value, workspace_root: &Path) -> Vec<CodeSymbol> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let name = entry.get("name")?.as_str()?.to_string();
            let location = parse_location_like(entry.get("location")?, workspace_root)?;
            Some(CodeSymbol {
                name,
                kind: parse_symbol_kind(entry.get("kind")),
                location,
                signature: entry
                    .get("detail")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
            })
        })
        .collect()
}

pub(crate) fn parse_document_symbols(
    value: &Value,
    workspace_root: &Path,
    document_path: &Path,
) -> Vec<CodeSymbol> {
    let mut symbols = Vec::new();
    if let Some(entries) = value.as_array() {
        for entry in entries {
            collect_document_symbols(entry, workspace_root, document_path, &mut symbols);
        }
    }
    symbols.sort_by(|left, right| {
        (
            left.location.path.as_str(),
            left.location.line,
            left.location.column,
            left.name.as_str(),
        )
            .cmp(&(
                right.location.path.as_str(),
                right.location.line,
                right.location.column,
                right.name.as_str(),
            ))
    });
    symbols
}

fn collect_document_symbols(
    entry: &Value,
    workspace_root: &Path,
    document_path: &Path,
    output: &mut Vec<CodeSymbol>,
) {
    if let Some(symbol) = parse_document_symbol(entry, workspace_root, document_path) {
        output.push(symbol);
    }
    if let Some(children) = entry.get("children").and_then(Value::as_array) {
        for child in children {
            collect_document_symbols(child, workspace_root, document_path, output);
        }
    }
}

fn parse_document_symbol(
    entry: &Value,
    workspace_root: &Path,
    document_path: &Path,
) -> Option<CodeSymbol> {
    let name = entry.get("name")?.as_str()?.to_string();
    let uri = entry
        .get("uri")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| file_uri_from_path(document_path));
    let selection_range = entry
        .get("selectionRange")
        .or_else(|| entry.get("range"))
        .or_else(|| entry.pointer("/location/range"))?;
    let location = parse_uri_and_range(uri.as_str(), selection_range, workspace_root)?;
    Some(CodeSymbol {
        name,
        kind: parse_symbol_kind(entry.get("kind")),
        location,
        signature: entry
            .get("detail")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

pub(crate) fn parse_locations_as_symbols(
    value: &Value,
    workspace_root: &Path,
    symbol_name: &str,
    kind: CodeSymbolKind,
) -> Vec<CodeSymbol> {
    collect_locations(value, workspace_root)
        .into_iter()
        .map(|location| CodeSymbol {
            name: symbol_name.to_string(),
            kind,
            location,
            signature: None,
        })
        .collect()
}

pub(crate) async fn parse_locations_as_references(
    value: &Value,
    workspace_root: &Path,
    symbol_name: &str,
) -> Vec<CodeReference> {
    let mut references = Vec::new();
    for location in collect_locations(value, workspace_root) {
        let absolute_path = workspace_root.join(&location.path);
        let line_text = fs::read_to_string(&absolute_path)
            .await
            .ok()
            .and_then(|source| {
                source
                    .lines()
                    .nth(location.line.saturating_sub(1))
                    .map(compact_line)
            })
            .unwrap_or_default();
        references.push(CodeReference {
            symbol: symbol_name.to_string(),
            location,
            line_text,
            is_definition: false,
        });
    }
    references
}

fn collect_locations(value: &Value, workspace_root: &Path) -> Vec<CodeLocation> {
    match value {
        Value::Array(entries) => entries
            .iter()
            .filter_map(|entry| parse_location_like(entry, workspace_root))
            .collect(),
        Value::Object(_) => parse_location_like(value, workspace_root)
            .into_iter()
            .collect(),
        _ => Vec::new(),
    }
}

pub(crate) fn parse_location_like(value: &Value, workspace_root: &Path) -> Option<CodeLocation> {
    if let Some(uri) = value.get("uri").and_then(Value::as_str) {
        let range = value.get("range")?;
        return parse_uri_and_range(uri, range, workspace_root);
    }
    if let Some(uri) = value.get("targetUri").and_then(Value::as_str) {
        let range = value
            .get("targetSelectionRange")
            .or_else(|| value.get("targetRange"))?;
        return parse_uri_and_range(uri, range, workspace_root);
    }
    None
}

pub(crate) fn parse_uri_and_range(
    uri: &str,
    range: &Value,
    workspace_root: &Path,
) -> Option<CodeLocation> {
    let path = file_uri_to_path(uri)?;
    Some(CodeLocation {
        path: display_path(workspace_root, &path),
        line: range.pointer("/start/line")?.as_u64()? as usize + 1,
        column: range.pointer("/start/character")?.as_u64()? as usize + 1,
    })
}

pub(crate) fn parse_symbol_kind(value: Option<&Value>) -> CodeSymbolKind {
    match value.and_then(Value::as_u64).unwrap_or_default() {
        2 | 3 | 4 => CodeSymbolKind::Module,
        5 => CodeSymbolKind::Class,
        6 | 9 | 12 | 24 => CodeSymbolKind::Function,
        10 | 22 => CodeSymbolKind::Enum,
        11 => CodeSymbolKind::Interface,
        13 | 15 | 16 | 17 | 18 | 20 | 21 | 25 => CodeSymbolKind::Variable,
        14 => CodeSymbolKind::Constant,
        19 | 23 => CodeSymbolKind::Struct,
        26 => CodeSymbolKind::TypeAlias,
        _ => CodeSymbolKind::Unknown,
    }
}

pub(crate) fn compact_line(line: &str) -> String {
    let compact = line.trim();
    let mut shortened = compact.chars().take(240).collect::<String>();
    if compact.chars().count() > 240 {
        shortened.push_str("...");
    }
    shortened
}

pub(crate) fn identifier_at_position(path: &Path, line: usize, column: usize) -> Option<String> {
    let source = std::fs::read_to_string(path).ok()?;
    let line_text = source.lines().nth(line.saturating_sub(1))?;
    let cursor = column.saturating_sub(1).min(line_text.len());
    let bytes = line_text.as_bytes();
    if bytes.is_empty() {
        return None;
    }

    let mut start = cursor.min(bytes.len().saturating_sub(1));
    while start > 0 && is_identifier_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = cursor.min(bytes.len());
    while end < bytes.len() && is_identifier_byte(bytes[end]) {
        end += 1;
    }
    (start < end).then(|| line_text[start..end].to_string())
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$'
}

pub(crate) fn configuration_response(params: &Value) -> Vec<Value> {
    let count = params
        .get("items")
        .and_then(Value::as_array)
        .map_or(1, std::vec::Vec::len);
    (0..count).map(|_| json!({})).collect()
}

// Diagnostics are cached immediately on publish so later tool surfaces can expose them
// without re-parsing raw server payloads.
#[allow(dead_code)]
pub(crate) struct DiagnosticEntry {
    pub(crate) location: CodeLocation,
    pub(crate) severity: Option<u64>,
    pub(crate) message: String,
    pub(crate) source: Option<String>,
}

pub(crate) fn parse_diagnostic_entry(
    value: &Value,
    document_path: &Path,
    workspace_root: &Path,
) -> Option<DiagnosticEntry> {
    Some(DiagnosticEntry {
        location: CodeLocation {
            path: display_path(workspace_root, document_path),
            line: value.pointer("/range/start/line")?.as_u64()? as usize + 1,
            column: value.pointer("/range/start/character")?.as_u64()? as usize + 1,
        },
        severity: value.get("severity").and_then(Value::as_u64),
        message: value.get("message")?.as_str()?.to_string(),
        source: value
            .get("source")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
    })
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Cursor;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn read_lsp_message_handles_content_length_header() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"result":null}"#;
        let header = format!("Content-Length: {}\r\n\r\n", payload.len());
        let mut reader = BufReader::new(Cursor::new(format!("{header}{payload}")));
        let message = read_lsp_message(&mut reader).await.unwrap().unwrap();
        assert_eq!(message.get("id").and_then(Value::as_i64), Some(1));
    }

    #[test]
    fn uri_round_trip() {
        let path = Path::new("/tmp/hello world/src/lib.rs");
        let uri = file_uri_from_path(path);
        assert_eq!(file_uri_to_path(&uri).unwrap(), path);
    }

    #[test]
    fn zero_based_position_is_lsp_friendly() {
        assert_eq!(
            zero_based_position(3, 5),
            json!({ "line": 2, "character": 4 })
        );
    }

    #[test]
    fn identifier_helper_extracts_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("src.rs");
        std::fs::write(&path, "fn compute_total() { compute_total(); }\n").unwrap();
        assert_eq!(
            identifier_at_position(&path, 1, 5).unwrap(),
            "compute_total"
        );
    }

    #[test]
    fn configuration_response_matches_requested_item_count() {
        let params = json!({ "items": [{}, {}, {}] });
        assert_eq!(configuration_response(&params).len(), 3);
    }

    #[test]
    fn parse_location_like_handles_target_uri() {
        let workspace = Path::new("/tmp/work");
        let entry = json!({
            "targetUri": "file:///tmp/work/src/lib.rs",
            "targetSelectionRange": {
                "start": { "line": 9, "character": 4 }
            }
        });
        let location = parse_location_like(&entry, workspace).unwrap();
        assert_eq!(location.path, "src/lib.rs");
        assert_eq!(location.line, 10);
    }

    #[test]
    fn parse_diagnostic_entry_uses_document_path_context() {
        let entry = json!({
            "range": {
                "start": { "line": 1, "character": 2 }
            },
            "severity": 1,
            "message": "oops",
            "source": "test"
        });
        let diag = parse_diagnostic_entry(
            &entry,
            Path::new("/tmp/work/src.rs"),
            Path::new("/tmp/work"),
        )
        .unwrap();
        assert_eq!(diag.location.path, "src.rs");
        assert_eq!(diag.message, "oops");
    }
}
