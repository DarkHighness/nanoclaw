use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CriticSeverity {
    Info,
    Warning,
    Blocking,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriticFinding {
    pub code: String,
    pub severity: CriticSeverity,
    pub summary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CriticReport {
    #[serde(default)]
    pub findings: Vec<CriticFinding>,
}

impl CriticReport {
    #[must_use]
    pub fn has_blockers(&self) -> bool {
        self.findings
            .iter()
            .any(|finding| finding.severity == CriticSeverity::Blocking)
    }

    #[must_use]
    pub fn rejection_reason(&self) -> Option<String> {
        let blocking = self
            .findings
            .iter()
            .filter(|finding| finding.severity == CriticSeverity::Blocking)
            .map(|finding| finding.summary.as_str())
            .collect::<Vec<_>>();
        if blocking.is_empty() {
            None
        } else {
            Some(format!("blocking critic findings: {}", blocking.join("; ")))
        }
    }
}
