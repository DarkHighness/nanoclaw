use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TuiMotionField {
    TranscriptCellIntro,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TuiMotionFieldSpec {
    pub field: TuiMotionField,
    pub label: &'static str,
    pub summary: &'static str,
}

const TUI_MOTION_FIELDS: &[TuiMotionFieldSpec] = &[TuiMotionFieldSpec {
    field: TuiMotionField::TranscriptCellIntro,
    label: "transcript_intro",
    summary: "typewriter and shimmer intro for newly appended transcript cells",
}];

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct TuiMotionConfig {
    pub transcript_cell_intro: bool,
}

impl TuiMotionConfig {
    pub fn enabled(&self, field: TuiMotionField) -> bool {
        match field {
            TuiMotionField::TranscriptCellIntro => self.transcript_cell_intro,
        }
    }

    pub fn set_enabled(&mut self, field: TuiMotionField, enabled: bool) {
        match field {
            TuiMotionField::TranscriptCellIntro => self.transcript_cell_intro = enabled,
        }
    }
}

impl Default for TuiMotionConfig {
    fn default() -> Self {
        Self {
            transcript_cell_intro: true,
        }
    }
}

pub fn tui_motion_fields() -> &'static [TuiMotionFieldSpec] {
    TUI_MOTION_FIELDS
}

#[cfg(test)]
mod tests {
    use super::{TuiMotionConfig, TuiMotionField, tui_motion_fields};

    #[test]
    fn toggles_individual_motion_fields() {
        let mut config = TuiMotionConfig::default();
        assert!(config.enabled(TuiMotionField::TranscriptCellIntro));
        config.set_enabled(TuiMotionField::TranscriptCellIntro, false);
        assert!(!config.enabled(TuiMotionField::TranscriptCellIntro));
    }

    #[test]
    fn motion_field_catalog_stays_operator_facing() {
        let labels = tui_motion_fields()
            .iter()
            .map(|spec| spec.label)
            .collect::<Vec<_>>();
        assert_eq!(labels, vec!["transcript_intro"]);
    }
}
