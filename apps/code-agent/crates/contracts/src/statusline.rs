use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusLineContextWindowStyle {
    Summary,
    #[default]
    Meter,
}

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
        label: "Status",
        summary: "Runtime status marker and summary",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Model,
        label: "Model",
        summary: "Full model name and reasoning effort",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Cwd,
        label: "Workspace",
        summary: "Current workspace directory name",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Repo,
        label: "Repo",
        summary: "Git repository name when available",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Branch,
        label: "Branch",
        summary: "Git branch name when available",
    },
    StatusLineFieldSpec {
        field: StatusLineField::ContextWindow,
        label: "Context",
        summary: "Context-window usage",
    },
    StatusLineFieldSpec {
        field: StatusLineField::InputTokens,
        label: "Input",
        summary: "Cumulative input tokens",
    },
    StatusLineFieldSpec {
        field: StatusLineField::OutputTokens,
        label: "Output",
        summary: "Cumulative output tokens",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Queue,
        label: "Queue",
        summary: "Queued command depth",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Clock,
        label: "Clock",
        summary: "Local wall clock",
    },
    StatusLineFieldSpec {
        field: StatusLineField::Session,
        label: "Session",
        summary: "Active substrate session id",
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
    pub context_window_style: StatusLineContextWindowStyle,
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
            context_window_style: StatusLineContextWindowStyle::Meter,
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
    use super::{
        StatusLineConfig, StatusLineContextWindowStyle, StatusLineField, status_line_fields,
    };

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
                "Status",
                "Model",
                "Workspace",
                "Repo",
                "Branch",
                "Context",
                "Input",
                "Output",
                "Queue",
                "Clock",
                "Session",
            ]
        );
    }

    #[test]
    fn statusline_defaults_to_context_meter_style() {
        let config = StatusLineConfig::default();
        assert_eq!(
            config.context_window_style,
            StatusLineContextWindowStyle::Meter
        );
    }
}
