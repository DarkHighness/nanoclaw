use crate::{Result, ToolError};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextBuffer {
    lines: Vec<String>,
    trailing_newline: bool,
}

impl TextBuffer {
    #[must_use]
    pub fn parse(text: &str) -> Self {
        if text.is_empty() {
            return Self {
                lines: Vec::new(),
                trailing_newline: false,
            };
        }

        Self {
            lines: text.split_terminator('\n').map(ToOwned::to_owned).collect(),
            trailing_newline: text.ends_with('\n'),
        }
    }

    #[must_use]
    pub fn line_count(&self) -> usize {
        self.lines.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    #[must_use]
    pub fn to_text(&self) -> String {
        let mut text = self.lines.join("\n");
        if self.trailing_newline && !self.lines.is_empty() {
            text.push('\n');
        }
        text
    }

    pub fn line_slice_text(&self, start_line: usize, end_line: usize) -> Result<String> {
        let (start_index, end_index) =
            normalize_line_range(self.line_count(), start_line, end_line)?;
        Ok(self.lines[start_index..end_index].join("\n"))
    }

    pub fn replace_lines(&mut self, start_line: usize, end_line: usize, text: &str) -> Result<()> {
        let (start_index, end_index) =
            normalize_line_range(self.line_count(), start_line, end_line)?;
        self.lines
            .splice(start_index..end_index, parse_replacement_lines(text));
        Ok(())
    }

    pub fn insert_after(&mut self, after_line: usize, text: &str) -> Result<()> {
        if after_line > self.line_count() {
            return Err(ToolError::invalid(format!(
                "insert_line {} is beyond end of file ({} lines total)",
                after_line,
                self.line_count()
            )));
        }
        self.lines
            .splice(after_line..after_line, parse_replacement_lines(text));
        Ok(())
    }

    #[must_use]
    pub fn lines(&self) -> &[String] {
        &self.lines
    }
}

fn normalize_line_range(
    line_count: usize,
    start_line: usize,
    end_line: usize,
) -> Result<(usize, usize)> {
    if start_line == 0 {
        return Err(ToolError::invalid("line numbers are 1-indexed"));
    }
    if end_line < start_line {
        return Err(ToolError::invalid(format!(
            "end_line {end_line} is before start_line {start_line}"
        )));
    }
    if end_line > line_count {
        return Err(ToolError::invalid(format!(
            "end_line {end_line} is beyond end of file ({line_count} lines total)"
        )));
    }
    Ok((start_line - 1, end_line))
}

fn parse_replacement_lines(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }

    let mut parts = text.split('\n').map(ToOwned::to_owned).collect::<Vec<_>>();
    if text.ends_with('\n') {
        // Line-oriented edit operations treat terminal newlines as line separators,
        // not as an instruction to append an implicit extra blank line. Keeping a
        // short slice hash in read output gives us stability without the visual noise
        // of prefixing every line with a per-line checksum.
        parts.pop();
    }
    parts
}

#[must_use]
pub fn stable_text_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest[..6]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[must_use]
pub fn format_numbered_lines(lines: &[String], start_line: usize) -> String {
    let width = (start_line + lines.len().saturating_sub(1))
        .to_string()
        .len()
        .max(2);
    lines
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{:>width$} | {}", start_line + index, line, width = width))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{TextBuffer, format_numbered_lines, stable_text_hash};

    #[test]
    fn text_buffer_preserves_trailing_newline() {
        let buffer = TextBuffer::parse("a\nb\n");
        assert_eq!(buffer.line_count(), 2);
        assert_eq!(buffer.to_text(), "a\nb\n");
    }

    #[test]
    fn replace_lines_uses_logical_line_ranges() {
        let mut buffer = TextBuffer::parse("a\nb\nc\n");
        buffer.replace_lines(2, 2, "x\ny").unwrap();
        assert_eq!(buffer.to_text(), "a\nx\ny\nc\n");
    }

    #[test]
    fn insert_after_appends_after_selected_line() {
        let mut buffer = TextBuffer::parse("a\nb");
        buffer.insert_after(2, "x").unwrap();
        assert_eq!(buffer.to_text(), "a\nb\nx");
    }

    #[test]
    fn numbered_lines_are_aligned() {
        let lines = vec!["alpha".to_string(), "beta".to_string()];
        assert_eq!(format_numbered_lines(&lines, 9), " 9 | alpha\n10 | beta");
    }

    #[test]
    fn stable_text_hash_is_short_and_deterministic() {
        assert_eq!(stable_text_hash("hello"), stable_text_hash("hello"));
        assert_ne!(stable_text_hash("hello"), stable_text_hash("world"));
        assert_eq!(stable_text_hash("hello").len(), 12);
    }
}
