use crate::interaction::{
    PendingControlKind, PendingControlReason, PendingControlSummary, PermissionProfile,
    PermissionRequestPrompt, SkillSummary, UserInputOption, UserInputPrompt, UserInputQuestion,
};
use agent::runtime::PermissionGrantSnapshot;
use agent::tools::{
    GrantedNetworkPermissions, GrantedPermissionProfile, PermissionRequest, UserInputRequest,
};
use agent::types::message_operator_text;
use agent::{RuntimeCommand, Skill};

/// These helpers are the single translation seam from runtime-owned types into
/// frontend-facing contracts. Keeping the mapping here prevents the backend
/// session and individual coordinators from growing ad hoc UI knowledge.
pub(crate) fn permission_profile_from_granted(
    granted: &GrantedPermissionProfile,
) -> PermissionProfile {
    let (network_full, network_domains) = match granted.network.as_ref() {
        None => (false, Vec::new()),
        Some(GrantedNetworkPermissions::Full) => (true, Vec::new()),
        Some(GrantedNetworkPermissions::AllowDomains(domains)) => (false, domains.clone()),
    };

    PermissionProfile {
        read_roots: granted
            .file_system
            .read_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        write_roots: granted
            .file_system
            .write_roots
            .iter()
            .map(|path| path.display().to_string())
            .collect(),
        network_full,
        network_domains,
    }
}

pub(crate) fn permission_request_prompt_from_request(
    prompt_id: String,
    request: &PermissionRequest,
    snapshot: &PermissionGrantSnapshot,
) -> PermissionRequestPrompt {
    PermissionRequestPrompt {
        prompt_id,
        reason: request.reason.clone(),
        requested: permission_profile_from_granted(&request.permissions),
        current_turn: permission_profile_from_granted(&snapshot.turn),
        current_session: permission_profile_from_granted(&snapshot.session),
    }
}

pub(crate) fn user_input_prompt_from_request(
    prompt_id: String,
    request: UserInputRequest,
) -> UserInputPrompt {
    UserInputPrompt {
        prompt_id,
        questions: request
            .questions
            .into_iter()
            .map(|question| UserInputQuestion {
                id: question.id,
                header: question.header,
                question: question.question,
                options: question
                    .options
                    .into_iter()
                    .map(|option| UserInputOption {
                        label: option.label,
                        description: option.description,
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub(crate) fn pending_control_summary(
    id: impl Into<String>,
    command: RuntimeCommand,
) -> PendingControlSummary {
    match command {
        RuntimeCommand::Prompt { message, .. } => PendingControlSummary {
            id: id.into(),
            kind: PendingControlKind::Prompt,
            preview: message_operator_text(&message),
            reason: None,
        },
        RuntimeCommand::Steer { message, reason } => PendingControlSummary {
            id: id.into(),
            kind: PendingControlKind::Steer,
            preview: message,
            reason: reason.map(PendingControlReason::from_runtime_label),
        },
    }
}

pub(crate) fn skill_summary_from_skill(skill: &Skill) -> SkillSummary {
    SkillSummary {
        name: skill.name.clone(),
        description: skill.description.clone(),
        aliases: skill.aliases.clone(),
        tags: skill.tags.clone(),
    }
}
