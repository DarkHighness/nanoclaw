use super::*;

impl CodeAgentTui {
    pub(crate) fn start_wait_task(&mut self, task_or_agent_ref: String) {
        let wait_ref = task_or_agent_ref.clone();
        self.ui_state.mutate(|state| {
            state.clear_composer_context_hint();
            state.clear_toast();
            state.status = format!("Waiting for live task {}", preview_id(&wait_ref));
            state.push_activity(format!("waiting for live task {}", preview_id(&wait_ref)));
        });
        let session = self.session.clone();
        self.operator_task = Some(spawn_local(async move {
            let outcome = session
                .run::<LiveTaskWaitOutcome>(UIAsyncCommand::WaitLiveTask { task_or_agent_ref })
                .await?;
            Ok(OperatorTaskOutcome::WaitLiveTask(outcome))
        }));
    }

    pub(super) fn start_side_question(&mut self, question: String) {
        let preview = state::preview_text(&question, 56);
        self.ui_state.mutate(|state| {
            state.clear_toast();
            state.status = format!("Answering /btw {}", preview);
            state.push_activity(format!("running /btw {}", preview));
        });
        let session = self.session.clone();
        self.operator_task = Some(spawn_local(async move {
            let outcome = session
                .run::<SideQuestionOutcome>(UIAsyncCommand::AnswerSideQuestion { question })
                .await?;
            Ok(OperatorTaskOutcome::SideQuestion(outcome))
        }));
    }
}
