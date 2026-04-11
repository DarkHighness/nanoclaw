use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusLineField {
    Status,
    Model,
    Cwd,
    Repo,
    Branch,
    ContextWindow,
    InputTokens,
    OutputTokens,
    Queue,
    Clock,
    Session,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StatusLineFieldSpec {
    pub field: StatusLineField,
    pub label: &'static str,
    pub summary: &'static str,
}

const STATUS_LINE_FIELDS: &[StatusLineFieldSpec] = &[
    StatusLineFieldSpec {
        field: StatusLineField::Status,
        label: "status",
        summary: "runtime status marker and summary",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Model,
        label: "model",
        summary: "full model name and reasoning effort",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Cwd,
        label: "cwd",
        summary: "current workspace directory name",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Repo,
        label: "repo",
        summary: "git repository name when available",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Branch,
        label: "branch",
        summary: "git branch name when available",
    },
    StatusLineFieldSpec {
        field: StatusLineField::ContextWindow,
        label: "context",
        summary: "context-window usage",
    },
    StatusLineFieldSpec {
        field: StatusLineField::InputTokens,
        label: "input",
        summary: "cumulative input tokens",
    },
    StatusLineFieldSpec {
        field: StatusLineField::OutputTokens,
        label: "output",
        summary: "cumulative output tokens",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Queue,
        label: "queue",
        summary: "queued command depth",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Clock,
        label: "clock",
        summary: "local wall clock",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Session,
        label: "session",
        summary: "active session reference",
    },
];

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct StatusLineConfig {
    pub status: bool,
    pub model: bool,
    pub cwd: bool,
    pub repo: bool,
    pub branch: bool,
    pub context_window: bool,
    pub input_tokens: bool,
    pub output_tokens: bool,
    pub queue: bool,
    pub clock: bool,
    pub session: bool,
}

impl StatusLineConfig {
    pub fn enabled(&self, field: StatusLineField) -> bool {
        match field {
            StatusLineField::Status => self.status,
            StatusLineField::Model => self.model,
            StatusLineField::Cwd => self.cwd,
            StatusLineField::Repo => self.repo,
            StatusLineField::Branch => self.branch,
            StatusLineField::ContextWindow => self.context_window,
            StatusLineField::InputTokens => self.input_tokens,
            StatusLineField::OutputTokens => self.output_tokens,
            StatusLineField::Queue => self.queue,
            StatusLineField::Clock => self.clock,
            StatusLineField::Session => self.session,
        }
    }

    pub fn set_enabled(&mut self, field: StatusLineField, enabled: bool) {
        match field {
            StatusLineField::Status => self.status = enabled,
            StatusLineField::Model => self.model = enabled,
            StatusLineField::Cwd => self.cwd = enabled,
            StatusLineField::Repo => self.repo = enabled,
            StatusLineField::Branch => self.branch = enabled,
            StatusLineField::ContextWindow => self.context_window = enabled,
            StatusLineField::InputTokens => self.input_tokens = enabled,
            StatusLineField::OutputTokens => self.output_tokens = enabled,
            StatusLineField::Queue => self.queue = enabled,
            StatusLineField::Clock => self.clock = enabled,
            StatusLineField::Session => self.session = enabled,
        }
    }

    pub fn toggle(&mut self, field: StatusLineField) -> bool {
        let next = !self.enabled(field);
        self.set_enabled(field, next);
        next
    }
}

impl Default for StatusLineConfig {
    fn default() -> Self {
        Self {
            status: true,
            model: true,
            cwd: true,
            repo: true,
            branch: true,
            context_window: true,
            input_tokens: true,
            output_tokens: true,
            queue: true,
            clock: true,
            session: false,
        }
    }
}

pub fn status_line_fields() -> &'static [StatusLineFieldSpec] {
    STATUS_LINE_FIELDS
}

#[cfg(test)]
mod tests {
    use super::{StatusLineConfig, StatusLineField, status_line_fields};

    #[test]
    fn toggles_individual_statusline_fields() {
        let mut config = StatusLineConfig::default();
        assert!(!config.enabled(StatusLineField::Session));
        assert!(config.toggle(StatusLineField::Session));
        assert!(config.enabled(StatusLineField::Session));
        assert!(!config.toggle(StatusLineField::Model));
        assert!(!config.enabled(StatusLineField::Model));
    }

    #[test]
    fn statusline_field_catalog_stays_operator_facing() {
        let labels = status_line_fields()
            .iter()
            .map(|spec| spec.label)
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            vec![
                "status", "model", "cwd", "repo", "branch", "context", "input", "output", "queue",
                "clock", "session",
            ]
        );
    }
}
