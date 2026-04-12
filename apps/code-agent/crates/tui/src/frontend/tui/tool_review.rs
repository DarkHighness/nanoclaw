use super::*;

impl CodeAgentTui {
    pub(super) fn handle_tool_review_key(&mut self, key: KeyEvent) -> Result<bool> {
        let snapshot = self.ui_state.snapshot();
        if snapshot.tool_review_overlay().is_none() {
            return Ok(false);
        }
        if !snapshot.input.is_empty() {
            self.ui_state.mutate(|state| state.clear_tool_review());
            return Ok(false);
        }

        match key.code {
            KeyCode::Up | KeyCode::Left => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_tool_review_selection(true);
                });
                self.refresh_tool_review_status();
            }
            KeyCode::Down | KeyCode::Right => {
                self.ui_state.mutate(|state| {
                    let _ = state.move_tool_review_selection(false);
                });
                self.refresh_tool_review_status();
            }
            KeyCode::Home => {
                self.ui_state.mutate(|state| {
                    let _ = state.jump_tool_review_selection(true);
                });
                self.refresh_tool_review_status();
            }
            KeyCode::End => {
                self.ui_state.mutate(|state| {
                    let _ = state.jump_tool_review_selection(false);
                });
                self.refresh_tool_review_status();
            }
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Backspace | KeyCode::Delete => {
                self.ui_state.mutate(|state| {
                    state.clear_tool_review();
                    state.status = "Closed tool review".to_string();
                    state.push_activity("closed tool review overlay");
                });
            }
            _ => {}
        }

        Ok(true)
    }

    pub(super) fn open_selected_tool_review(&mut self) -> bool {
        let opened = {
            let snapshot = self.ui_state.snapshot();
            snapshot
                .selected_tool_entry()
                .is_some_and(|tool| tool.review.is_some())
        };

        if !opened {
            self.ui_state.mutate(|state| {
                state.status = "Selected tool does not expose a review surface".to_string();
                state.push_activity("tool review unavailable for selection");
            });
            return false;
        }

        self.ui_state.mutate(|state| {
            let opened = state.open_selected_tool_review_overlay();
            if opened {
                state.status = "Opened tool review overlay".to_string();
                state.push_activity("opened tool review overlay");
            }
        });
        self.refresh_tool_review_status();
        true
    }

    pub(super) fn refresh_tool_review_status(&self) {
        let snapshot = self.ui_state.snapshot();
        let Some(overlay) = snapshot.tool_review_overlay() else {
            return;
        };
        let Some(item) = snapshot.selected_tool_review_item() else {
            return;
        };
        self.ui_state.mutate(|state| {
            let noun = overlay.review.kind.singular_label();
            state.status = format!(
                "Reviewing {} {} {} of {}",
                item.title,
                noun,
                overlay.selected + 1,
                overlay.review.items.len()
            );
        });
    }
}
