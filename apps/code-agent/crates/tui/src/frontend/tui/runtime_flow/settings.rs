use super::*;

impl CodeAgentTui {
    pub(crate) fn apply_tui_motion(&mut self, field: TuiMotionField, enabled: bool, persist: bool) {
        self.ui_state.mutate(|state| {
            state.session.motion.set_enabled(field, enabled);
            state.status = if enabled {
                "Transcript intro motion enabled".to_string()
            } else {
                "Transcript intro motion disabled".to_string()
            };
            state.push_activity(format!(
                "transcript motion {}",
                if enabled { "enabled" } else { "disabled" }
            ));
            if !enabled {
                state.advance_transcript_motion(Instant::now());
            }
        });

        if !persist {
            return;
        }

        let workspace_root = self.workspace_root_buf();
        if let Err(error) = persist_tui_motion_selection(&workspace_root, field, enabled) {
            let message = summarize_nonfatal_error("persist tui motion", &error);
            self.ui_state.mutate(|state| {
                state.status = format!("Transcript motion changed, but failed to save: {message}");
                state.push_activity(format!(
                    "motion persistence failed: {}",
                    state::preview_text(&message, 56)
                ));
            });
        }
    }

    pub(crate) fn toggle_tui_motion(&mut self, field: TuiMotionField) {
        let enabled = !self.ui_state.snapshot().session.motion.enabled(field);
        self.apply_tui_motion(field, enabled, true);
    }

    pub(crate) fn cycle_model_reasoning_effort(&mut self) {
        match self.cycle_model_reasoning_effort_result() {
            Ok(outcome) => self.apply_model_reasoning_effort_outcome(outcome, "cycled"),
            Err(error) => self.record_model_reasoning_effort_error(summarize_nonfatal_error(
                "cycle model reasoning effort",
                &error,
            )),
        }
    }

    pub(crate) fn set_model_reasoning_effort(&mut self, effort: &str) {
        match self.set_model_reasoning_effort_result(effort) {
            Ok(outcome) => self.apply_model_reasoning_effort_outcome(outcome, "set"),
            Err(error) => self.record_model_reasoning_effort_error(summarize_nonfatal_error(
                "set model reasoning effort",
                &error,
            )),
        }
    }

    pub(crate) fn apply_model_reasoning_effort_outcome(
        &mut self,
        outcome: ModelReasoningEffortOutcome,
        verb: &str,
    ) {
        let current = outcome
            .current
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let previous = outcome
            .previous
            .clone()
            .unwrap_or_else(|| "default".to_string());
        self.ui_state.mutate(|state| {
            state.session.model_reasoning_effort = outcome.current;
            state.status =
                format!("Thinking effort {verb} to {current}; next model request will use it");
            state.push_activity(format!("thinking effort {previous} -> {current}"));
        });
    }

    pub(crate) fn record_model_reasoning_effort_error(&mut self, message: String) {
        self.ui_state.mutate(|state| {
            state.status = format!("Thinking effort unavailable: {message}");
            state.push_activity(format!(
                "thinking effort rejected: {}",
                state::preview_text(&message, 56)
            ));
        });
    }

    pub(crate) fn preview_selected_theme(&mut self) {
        let snapshot = self.ui_state.snapshot();
        if let Some(theme_id) = snapshot.selected_theme() {
            self.apply_tui_theme(&theme_id, false, None);
        }
    }

    pub(crate) fn apply_tui_theme(
        &mut self,
        theme_id: &str,
        persist: bool,
        previous_override: Option<String>,
    ) {
        match set_active_theme(theme_id) {
            Ok(()) => {
                let current = theme_id.to_string();
                let mut previous = None;
                self.ui_state.mutate(|state| {
                    previous = Some(state.theme.clone());
                    state.theme = current.clone();
                    state.themes = crate::theme::theme_summaries();
                    if !persist {
                        state.status = format!("Previewing theme {current}");
                    }
                });
                if !persist {
                    return;
                }

                let workspace_root = self.workspace_root_buf();
                match persist_tui_theme_selection(&workspace_root, &current) {
                    Ok(()) => {
                        let previous = previous_override
                            .or(previous)
                            .unwrap_or_else(|| current.clone());
                        self.ui_state.mutate(|state| {
                            state.status = format!("Theme saved as {current}");
                            if previous == current {
                                state.push_activity(format!("theme persisted: {current}"));
                            } else {
                                state.push_activity(format!("theme {previous} -> {current}"));
                            }
                        });
                    }
                    Err(error) => {
                        let message = summarize_nonfatal_error("persist tui theme", &error);
                        self.ui_state.mutate(|state| {
                            state.status =
                                format!("Theme {current} active, but failed to save: {message}");
                            state.push_activity(format!(
                                "theme persistence failed: {}",
                                state::preview_text(&message, 56)
                            ));
                        });
                    }
                }
            }
            Err(error) => {
                let message = summarize_nonfatal_error("set tui theme", &error);
                self.ui_state.mutate(|state| {
                    state.status = format!("Theme unavailable: {message}");
                    state.push_activity(format!(
                        "theme rejected: {}",
                        state::preview_text(&message, 56)
                    ));
                });
            }
        }
    }
}
