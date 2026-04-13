use super::*;
use crate::tool_render::{ToolReview, ToolReviewItem};
use agent::types::MessageId;

#[derive(Clone, Debug, Default)]
pub(crate) struct StatusLinePickerState {
    pub(crate) selected: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ThinkingEffortPickerState {
    pub(crate) selected: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ThemePickerState {
    pub(crate) selected: usize,
    pub(crate) original_theme: String,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PendingControlPickerState {
    pub(crate) selected: usize,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CollectionPickerState {
    pub(crate) selected: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PendingControlEditorState {
    pub(crate) id: String,
    pub(crate) kind: PendingControlKind,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoryRollbackCandidate {
    pub(crate) message_id: MessageId,
    pub(crate) prompt: String,
    pub(crate) draft: ComposerDraftState,
    pub(crate) turn_preview_lines: Vec<TranscriptEntry>,
    pub(crate) removed_turn_count: usize,
    pub(crate) removed_message_count: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct HistoryRollbackOverlayState {
    pub(crate) selected: usize,
    pub(crate) candidates: Vec<HistoryRollbackCandidate>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum HistoryRollbackState {
    Primed,
    Selecting(HistoryRollbackOverlayState),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ToolReviewOverlayState {
    pub(crate) selection: ToolSelectionTarget,
    pub(crate) tool_name: String,
    pub(crate) review: ToolReview,
    pub(crate) selected: usize,
}

impl TuiState {
    pub(crate) fn show_main_view<I>(&mut self, title: impl Into<String>, lines: I)
    where
        I: IntoIterator<Item = InspectorEntry>,
    {
        self.main_pane = MainPaneMode::View;
        self.inspector_title = title.into();
        self.inspector = lines.into_iter().collect();
        self.inspector_scroll = 0;
        self.collection_picker = first_actionable_collection_index(&self.inspector)
            .map(|selected| CollectionPickerState { selected });
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.theme_picker = None;
        self.history_rollback = None;
        self.tool_review = None;
    }

    pub(crate) fn show_transcript_pane(&mut self) {
        self.main_pane = MainPaneMode::Transcript;
        self.collection_picker = None;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.theme_picker = None;
        self.history_rollback = None;
        self.tool_review = None;
    }

    pub(crate) fn open_statusline_picker(&mut self) {
        self.main_pane = MainPaneMode::View;
        self.inspector_title = "Status Line".to_string();
        self.inspector.clear();
        self.inspector_scroll = 0;
        self.collection_picker = None;
        self.pending_control_picker = None;
        self.thinking_effort_picker = None;
        self.theme_picker = None;
        self.history_rollback = None;
        self.tool_review = None;
        self.statusline_picker
            .get_or_insert_with(StatusLinePickerState::default)
            .selected = 0;
    }

    pub(crate) fn open_thinking_effort_picker(&mut self) {
        self.main_pane = MainPaneMode::View;
        self.inspector_title = "Thinking Effort".to_string();
        self.inspector.clear();
        self.inspector_scroll = 0;
        self.collection_picker = None;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.theme_picker = None;
        self.history_rollback = None;
        self.tool_review = None;
        let selected = self
            .session
            .supported_model_reasoning_efforts
            .iter()
            .position(|level| {
                Some(level.as_str()) == self.session.model_reasoning_effort.as_deref()
            })
            .unwrap_or(0);
        self.thinking_effort_picker = Some(ThinkingEffortPickerState { selected });
    }

    pub(crate) fn open_theme_picker(&mut self) {
        self.main_pane = MainPaneMode::View;
        self.inspector_title = "Theme".to_string();
        self.inspector.clear();
        self.inspector_scroll = 0;
        self.collection_picker = None;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.theme_picker = None;
        self.history_rollback = None;
        self.tool_review = None;
        let selected = self
            .themes
            .iter()
            .position(|candidate| candidate.id == self.theme)
            .unwrap_or(0);
        self.theme_picker = Some(ThemePickerState {
            selected,
            original_theme: self.theme.clone(),
        });
    }

    pub(crate) fn close_statusline_picker(&mut self) {
        self.statusline_picker = None;
        self.show_transcript_pane();
    }

    pub(crate) fn close_thinking_effort_picker(&mut self) {
        self.thinking_effort_picker = None;
        self.show_transcript_pane();
    }

    pub(crate) fn close_theme_picker(&mut self) {
        self.theme_picker = None;
        self.show_transcript_pane();
    }

    pub(crate) fn open_pending_control_picker(&mut self, select_latest: bool) -> bool {
        if self.pending_controls.is_empty() {
            return false;
        }
        self.main_pane = MainPaneMode::Transcript;
        self.collection_picker = None;
        let selected = if select_latest {
            self.pending_controls.len().saturating_sub(1)
        } else {
            self.pending_control_picker
                .as_ref()
                .map(|picker| picker.selected)
                .unwrap_or_else(|| self.pending_controls.len().saturating_sub(1))
                .min(self.pending_controls.len().saturating_sub(1))
        };
        self.pending_control_picker = Some(PendingControlPickerState { selected });
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.theme_picker = None;
        self.history_rollback = None;
        self.tool_review = None;
        true
    }

    pub(crate) fn close_pending_control_picker(&mut self) {
        self.pending_control_picker = None;
    }

    pub(crate) fn move_pending_control_picker(&mut self, backwards: bool) -> bool {
        let Some(picker) = self.pending_control_picker.as_mut() else {
            return false;
        };
        let total = self.pending_controls.len();
        if total == 0 {
            return false;
        }
        picker.selected = if backwards {
            picker.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (picker.selected + 1) % total
        };
        true
    }

    pub(crate) fn selected_pending_control(&self) -> Option<PendingControlSummary> {
        let picker = self.pending_control_picker.as_ref()?;
        self.pending_controls.get(picker.selected).cloned()
    }

    pub(crate) fn begin_pending_control_edit(&mut self) -> Option<PendingControlSummary> {
        let selected = self.selected_pending_control()?;
        self.replace_input(selected.preview.clone());
        self.editing_pending_control = Some(PendingControlEditorState {
            id: selected.id.clone(),
            kind: selected.kind,
        });
        self.pending_control_picker = None;
        Some(selected)
    }

    pub(crate) fn begin_latest_pending_control_edit(&mut self) -> Option<PendingControlSummary> {
        if self.pending_controls.is_empty() {
            return None;
        }
        self.pending_control_picker = Some(PendingControlPickerState {
            selected: self.pending_controls.len().saturating_sub(1),
        });
        self.begin_pending_control_edit()
    }

    pub(crate) fn clear_pending_control_edit(&mut self) {
        self.editing_pending_control = None;
    }

    pub(crate) fn prime_history_rollback(&mut self) {
        self.main_pane = MainPaneMode::Transcript;
        self.collection_picker = None;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.theme_picker = None;
        self.history_rollback = Some(HistoryRollbackState::Primed);
        self.tool_review = None;
    }

    pub(crate) fn open_history_rollback_overlay(
        &mut self,
        candidates: Vec<HistoryRollbackCandidate>,
    ) -> bool {
        if candidates.is_empty() {
            return false;
        }
        self.main_pane = MainPaneMode::Transcript;
        self.collection_picker = None;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.theme_picker = None;
        self.history_rollback = Some(HistoryRollbackState::Selecting(
            HistoryRollbackOverlayState {
                selected: candidates.len().saturating_sub(1),
                candidates,
            },
        ));
        self.tool_review = None;
        true
    }

    pub(crate) fn clear_history_rollback(&mut self) {
        self.history_rollback = None;
    }

    pub(crate) fn open_selected_tool_review_overlay(&mut self) -> bool {
        let Some(selection) = self.tool_selection.clone() else {
            return false;
        };
        let Some(tool) = self.selected_tool_entry() else {
            return false;
        };
        let tool_name = tool.tool_name.clone();
        let Some(review) = tool.review.clone() else {
            return false;
        };

        self.main_pane = MainPaneMode::Transcript;
        self.collection_picker = None;
        self.pending_control_picker = None;
        self.statusline_picker = None;
        self.thinking_effort_picker = None;
        self.theme_picker = None;
        self.history_rollback = None;
        self.tool_review = Some(ToolReviewOverlayState {
            selection,
            tool_name,
            review,
            selected: 0,
        });
        true
    }

    pub(crate) fn clear_tool_review(&mut self) {
        self.tool_review = None;
    }

    pub(crate) fn tool_review_overlay(&self) -> Option<&ToolReviewOverlayState> {
        self.tool_review.as_ref()
    }

    pub(crate) fn move_tool_review_selection(&mut self, backwards: bool) -> bool {
        let Some(overlay) = self.tool_review.as_mut() else {
            return false;
        };
        let total = overlay.review.items.len();
        if total == 0 {
            return false;
        }
        overlay.selected = if backwards {
            overlay.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (overlay.selected + 1) % total
        };
        true
    }

    pub(crate) fn jump_tool_review_selection(&mut self, oldest: bool) -> bool {
        let Some(overlay) = self.tool_review.as_mut() else {
            return false;
        };
        if overlay.review.items.is_empty() {
            return false;
        }
        overlay.selected = if oldest {
            0
        } else {
            overlay.review.items.len().saturating_sub(1)
        };
        true
    }

    pub(crate) fn selected_tool_review_item(&self) -> Option<&ToolReviewItem> {
        let overlay = self.tool_review_overlay()?;
        overlay.review.items.get(overlay.selected)
    }

    pub(crate) fn move_collection_picker(&mut self, backwards: bool) -> bool {
        let Some(picker) = self.collection_picker.as_mut() else {
            return false;
        };
        let total = actionable_collection_count(&self.inspector);
        if total == 0 {
            return false;
        }
        picker.selected = if backwards {
            picker.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (picker.selected + 1) % total
        };
        true
    }

    pub(crate) fn selected_collection_entry(&self) -> Option<InspectorEntry> {
        let selected = self.collection_picker.as_ref()?.selected;
        self.inspector
            .iter()
            .filter(|entry| is_actionable_collection_entry(entry))
            .nth(selected)
            .cloned()
    }

    pub(crate) fn history_rollback_is_primed(&self) -> bool {
        matches!(self.history_rollback, Some(HistoryRollbackState::Primed))
    }

    pub(crate) fn history_rollback_overlay(&self) -> Option<&HistoryRollbackOverlayState> {
        match self.history_rollback.as_ref() {
            Some(HistoryRollbackState::Selecting(overlay)) => Some(overlay),
            _ => None,
        }
    }

    pub(crate) fn move_history_rollback_selection(&mut self, backwards: bool) -> bool {
        let Some(HistoryRollbackState::Selecting(overlay)) = self.history_rollback.as_mut() else {
            return false;
        };
        let total = overlay.candidates.len();
        if total == 0 {
            return false;
        }
        overlay.selected = if backwards {
            overlay.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (overlay.selected + 1) % total
        };
        true
    }

    pub(crate) fn jump_history_rollback_selection(&mut self, oldest: bool) -> bool {
        let Some(HistoryRollbackState::Selecting(overlay)) = self.history_rollback.as_mut() else {
            return false;
        };
        if overlay.candidates.is_empty() {
            return false;
        }
        overlay.selected = if oldest {
            0
        } else {
            overlay.candidates.len().saturating_sub(1)
        };
        true
    }

    pub(crate) fn selected_history_rollback_candidate(&self) -> Option<&HistoryRollbackCandidate> {
        let overlay = self.history_rollback_overlay()?;
        overlay.candidates.get(overlay.selected)
    }

    pub(crate) fn sync_pending_controls(&mut self, controls: Vec<PendingControlSummary>) {
        self.pending_controls = controls;
        if let Some(picker) = self.pending_control_picker.as_mut() {
            picker.selected = picker
                .selected
                .min(self.pending_controls.len().saturating_sub(1));
            if self.pending_controls.is_empty() {
                self.pending_control_picker = None;
            }
        }
        if let Some(editor) = self.editing_pending_control.as_ref()
            && !self
                .pending_controls
                .iter()
                .any(|control| control.id == editor.id)
        {
            self.editing_pending_control = None;
        }
    }

    pub(crate) fn move_statusline_picker(&mut self, backwards: bool) -> bool {
        let Some(picker) = self.statusline_picker.as_mut() else {
            return false;
        };
        let total = status_line_fields().len();
        if total == 0 {
            return false;
        }
        picker.selected = if backwards {
            picker.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (picker.selected + 1) % total
        };
        true
    }

    pub(crate) fn selected_statusline_field(&self) -> Option<StatusLineField> {
        let picker = self.statusline_picker.as_ref()?;
        status_line_fields()
            .get(picker.selected)
            .map(|spec| spec.field)
    }

    pub(crate) fn toggle_selected_statusline_field(&mut self) -> Option<(StatusLineField, bool)> {
        let field = self.selected_statusline_field()?;
        let enabled = self.session.statusline.toggle(field);
        Some((field, enabled))
    }

    pub(crate) fn move_thinking_effort_picker(&mut self, backwards: bool) -> bool {
        let Some(picker) = self.thinking_effort_picker.as_mut() else {
            return false;
        };
        let total = self.session.supported_model_reasoning_efforts.len();
        if total == 0 {
            return false;
        }
        picker.selected = if backwards {
            picker.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (picker.selected + 1) % total
        };
        true
    }

    pub(crate) fn selected_thinking_effort(&self) -> Option<String> {
        let picker = self.thinking_effort_picker.as_ref()?;
        self.session
            .supported_model_reasoning_efforts
            .get(picker.selected)
            .cloned()
    }

    pub(crate) fn move_theme_picker(&mut self, backwards: bool) -> bool {
        let Some(picker) = self.theme_picker.as_mut() else {
            return false;
        };
        let total = self.themes.len();
        if total == 0 {
            return false;
        }
        picker.selected = if backwards {
            picker.selected.checked_sub(1).unwrap_or(total - 1)
        } else {
            (picker.selected + 1) % total
        };
        true
    }

    pub(crate) fn selected_theme(&self) -> Option<String> {
        let picker = self.theme_picker.as_ref()?;
        self.themes
            .get(picker.selected)
            .map(|theme| theme.id.clone())
    }

    pub(crate) fn original_theme(&self) -> Option<String> {
        self.theme_picker
            .as_ref()
            .map(|picker| picker.original_theme.clone())
    }
}

fn first_actionable_collection_index(lines: &[InspectorEntry]) -> Option<usize> {
    lines
        .iter()
        .any(is_actionable_collection_entry)
        .then_some(0)
}

fn actionable_collection_count(lines: &[InspectorEntry]) -> usize {
    lines
        .iter()
        .filter(|entry| is_actionable_collection_entry(entry))
        .count()
}

fn is_actionable_collection_entry(entry: &InspectorEntry) -> bool {
    matches!(
        entry,
        InspectorEntry::CollectionItem {
            action,
            alternate_action,
            ..
        } if action.is_some() || alternate_action.is_some()
    )
}
