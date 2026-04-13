use super::*;
use crate::frontend::tui::state::ComposerInputProvenance;

#[cfg(test)]
pub(crate) fn command_palette_lines() -> Vec<InspectorEntry> {
    command_palette_lines_for(None)
}

#[cfg(test)]
pub(crate) fn command_palette_lines_for(query: Option<&str>) -> Vec<InspectorEntry> {
    command_palette_lines_for_skills(query, &[])
}

pub(crate) fn command_palette_lines_for_skills(
    query: Option<&str>,
    skills: &[SkillSummary],
) -> Vec<InspectorEntry> {
    let trimmed = query
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(|query| query.trim_start_matches('/').to_ascii_lowercase());
    let specs = trimmed
        .as_deref()
        .map(|query| palette_matching_specs(query, skills))
        .unwrap_or_else(|| all_slash_invocations(skills));
    if specs.is_empty() {
        return vec![
            InspectorEntry::section("Command Palette"),
            InspectorEntry::Muted("No commands match this query.".to_string()),
        ];
    }
    let mut lines = Vec::new();
    let mut current_section = None;
    for spec in specs {
        if current_section != Some(spec.section()) {
            current_section = Some(spec.section());
            lines.push(InspectorEntry::section(spec.section()));
        }
        let aliases = spec.aliases();
        let alias_suffix = if aliases.is_empty() {
            String::new()
        } else {
            format!(
                " · aliases: {}",
                aliases
                    .iter()
                    .map(|alias| format!("/{alias}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        lines.push(InspectorEntry::actionable_collection(
            spec.usage(),
            Some(format!("{}{}", spec.summary(), alias_suffix)),
            inspector_action_for_slash_spec(&spec),
        ));
    }
    lines
}

pub(crate) fn inspector_action_for_slash_name(name: &str) -> Option<InspectorAction> {
    SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .find(|spec| spec.name == name)
        .map(|spec| inspector_action_for_slash_spec(&SlashInvocationSpec::Builtin(spec)))
}

pub(crate) fn inspector_action_for_slash_spec(spec: &SlashInvocationSpec) -> InspectorAction {
    match spec {
        SlashInvocationSpec::Builtin(spec) if !spec.requires_arguments() => {
            InspectorAction::RunCommand(format!("/{}", spec.name))
        }
        _ => InspectorAction::FillInput(spec.completion_input()),
    }
}

pub(crate) fn composer_completion_hint(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    skills: &[SkillSummary],
) -> Option<ComposerCompletionHint> {
    if let Some(hint) = slash_command_hint(input, provenance, selected_index, skills) {
        return Some(ComposerCompletionHint::Slash(hint));
    }
    skill_invocation_hint(input, provenance, selected_index, skills)
        .map(ComposerCompletionHint::Skill)
}

pub(crate) fn cycle_composer_completion(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    backwards: bool,
    skills: &[SkillSummary],
) -> Option<(String, usize)> {
    cycle_slash_command(input, provenance, selected_index, backwards, skills)
        .or_else(|| cycle_skill_invocation(input, provenance, selected_index, backwards, skills))
}

pub(crate) fn move_composer_completion_selection(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    backwards: bool,
    skills: &[SkillSummary],
) -> Option<usize> {
    move_slash_command_selection(input, provenance, selected_index, backwards, skills).or_else(
        || move_skill_invocation_selection(input, provenance, selected_index, backwards, skills),
    )
}

pub(crate) fn resolve_composer_enter_action(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    skills: &[SkillSummary],
) -> Option<ComposerCompletionEnterAction> {
    if let Some(action) = resolve_slash_enter_action(input, provenance, selected_index, skills) {
        return Some(match action {
            SlashCommandEnterAction::Complete { input, index } => {
                ComposerCompletionEnterAction::Complete { input, index }
            }
            SlashCommandEnterAction::Execute(command) => {
                ComposerCompletionEnterAction::ExecuteSlash(command)
            }
        });
    }
    resolve_skill_enter_action(input, provenance, selected_index, skills)
}

fn slash_command_hint(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    skills: &[SkillSummary],
) -> Option<SlashCommandHint> {
    if !provenance.arms_slash_completion(input) {
        return None;
    }
    let (command_token, tail) = split_slash_input(input)?;
    let matches = matching_specs(command_token, skills);
    if let Some(selected) = selected_spec(command_token, tail, selected_index, &matches, skills) {
        return Some(SlashCommandHint {
            exact: selected.matches_token(command_token),
            arguments: selected
                .matches_token(command_token)
                .then(|| build_argument_hint(selected.clone(), tail))
                .flatten(),
            selected_match_index: matches
                .iter()
                .position(|spec| spec == &selected)
                .unwrap_or(0),
            selected,
            matches,
        });
    }
    None
}

fn cycle_slash_command(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    backwards: bool,
    skills: &[SkillSummary],
) -> Option<(String, usize)> {
    if !provenance.arms_slash_completion(input) {
        return None;
    }
    let (command_token, tail) = split_slash_input(input)?;
    if tail.is_some() {
        return None;
    }
    let matches = matching_specs(command_token, skills);
    if matches.is_empty() {
        return None;
    }
    let current = selected_index.min(matches.len().saturating_sub(1));
    let exact_at_current = matches
        .get(current)
        .is_some_and(|spec| spec.matches_token(command_token));
    let next = if backwards {
        if exact_at_current {
            current.checked_sub(1).unwrap_or(matches.len() - 1)
        } else {
            matches.len() - 1
        }
    } else if exact_at_current {
        (current + 1) % matches.len()
    } else {
        current
    };
    Some((matches[next].completion_input(), next))
}

fn move_slash_command_selection(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    backwards: bool,
    skills: &[SkillSummary],
) -> Option<usize> {
    if !provenance.arms_slash_completion(input) {
        return None;
    }
    let (command_token, tail) = split_slash_input(input)?;
    if tail.is_some() {
        return None;
    }
    let matches = matching_specs(command_token, skills);
    if matches.is_empty() {
        return None;
    }
    let current = selected_index.min(matches.len().saturating_sub(1));
    Some(if backwards {
        current.checked_sub(1).unwrap_or(matches.len() - 1)
    } else {
        (current + 1) % matches.len()
    })
}

fn resolve_slash_enter_action(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    skills: &[SkillSummary],
) -> Option<SlashCommandEnterAction> {
    if !provenance.arms_slash_completion(input) {
        return None;
    }
    let (_, tail) = split_slash_input(input)?;
    let tail_has_content = tail.is_some_and(|value| !value.trim().is_empty());
    let hint = slash_command_hint(input, provenance, selected_index, skills)?;
    if hint.exact {
        if hint
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.next)
            .is_some_and(|argument| argument.required)
        {
            return Some(SlashCommandEnterAction::Complete {
                input: hint.selected.completion_input(),
                index: hint.selected_match_index,
            });
        }
        if matches!(hint.selected, SlashInvocationSpec::Skill(_)) && !tail_has_content {
            return Some(SlashCommandEnterAction::Complete {
                input: hint.selected.completion_input(),
                index: hint.selected_match_index,
            });
        }
        if tail_has_content {
            return None;
        }
        return hint
            .selected
            .executable_input()
            .map(SlashCommandEnterAction::Execute);
    }
    if let Some(command) = (hint.matches.len() == 1)
        .then(|| hint.selected.executable_input())
        .flatten()
    {
        return Some(SlashCommandEnterAction::Execute(command));
    }
    Some(SlashCommandEnterAction::Complete {
        input: hint.selected.completion_input(),
        index: hint.selected_match_index,
    })
}

fn skill_invocation_hint(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    skills: &[SkillSummary],
) -> Option<SkillInvocationHint> {
    if !provenance.arms_skill_completion(input) {
        return None;
    }
    let (skill_token, tail) = split_skill_input(input)?;
    if tail.is_some_and(|value| !value.trim().is_empty()) {
        return None;
    }
    let matches = matching_skill_specs(skill_token, skills);
    let selected = selected_skill_spec(skill_token, tail, selected_index, &matches)?;
    Some(SkillInvocationHint {
        exact: selected.matches_token(skill_token),
        selected_match_index: matches
            .iter()
            .position(|spec| spec.name == selected.name)
            .unwrap_or(0),
        selected,
        matches,
    })
}

fn cycle_skill_invocation(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    backwards: bool,
    skills: &[SkillSummary],
) -> Option<(String, usize)> {
    if !provenance.arms_skill_completion(input) {
        return None;
    }
    let (skill_token, tail) = split_skill_input(input)?;
    if tail.is_some() {
        return None;
    }
    let matches = matching_skill_specs(skill_token, skills);
    if matches.is_empty() {
        return None;
    }
    let current = selected_index.min(matches.len().saturating_sub(1));
    let exact_at_current = matches
        .get(current)
        .is_some_and(|spec| spec.matches_token(skill_token));
    let next = if backwards {
        if exact_at_current {
            current.checked_sub(1).unwrap_or(matches.len() - 1)
        } else {
            matches.len() - 1
        }
    } else if exact_at_current {
        (current + 1) % matches.len()
    } else {
        current
    };
    Some((format!("${} ", matches[next].name), next))
}

fn move_skill_invocation_selection(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    backwards: bool,
    skills: &[SkillSummary],
) -> Option<usize> {
    if !provenance.arms_skill_completion(input) {
        return None;
    }
    let (skill_token, tail) = split_skill_input(input)?;
    if tail.is_some() {
        return None;
    }
    let matches = matching_skill_specs(skill_token, skills);
    if matches.is_empty() {
        return None;
    }
    let current = selected_index.min(matches.len().saturating_sub(1));
    Some(if backwards {
        current.checked_sub(1).unwrap_or(matches.len() - 1)
    } else {
        (current + 1) % matches.len()
    })
}

fn resolve_skill_enter_action(
    input: &str,
    provenance: ComposerInputProvenance,
    selected_index: usize,
    skills: &[SkillSummary],
) -> Option<ComposerCompletionEnterAction> {
    let hint = skill_invocation_hint(input, provenance, selected_index, skills)?;
    Some(ComposerCompletionEnterAction::Complete {
        input: format!("${} ", hint.selected.name),
        index: hint.selected_match_index,
    })
}

fn split_slash_input(input: &str) -> Option<(&str, Option<&str>)> {
    let body = input.strip_prefix('/')?;
    Some(
        body.split_once(' ')
            .map_or((body, None), |(command_token, tail)| {
                (command_token, Some(tail))
            }),
    )
}

fn split_skill_input(input: &str) -> Option<(&str, Option<&str>)> {
    let body = input.strip_prefix('$')?;
    Some(
        body.split_once(' ')
            .map_or((body, None), |(skill_token, tail)| {
                (skill_token, Some(tail))
            }),
    )
}

fn all_slash_invocations(skills: &[SkillSummary]) -> Vec<SlashInvocationSpec> {
    SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .map(SlashInvocationSpec::Builtin)
        .chain(
            skills
                .iter()
                .map(SkillInvocationSpec::from_summary)
                .map(SlashInvocationSpec::Skill),
        )
        .collect()
}

fn matching_specs(prefix: &str, skills: &[SkillSummary]) -> Vec<SlashInvocationSpec> {
    let prefix = prefix.trim().to_ascii_lowercase();
    let mut matches = all_slash_invocations(skills)
        .into_iter()
        .filter(|spec| spec.matches_prefix(&prefix))
        .collect::<Vec<_>>();
    sort_matching_specs(&mut matches, &prefix);
    matches
}

fn palette_matching_specs(prefix: &str, skills: &[SkillSummary]) -> Vec<SlashInvocationSpec> {
    let prefix = prefix.trim().to_ascii_lowercase();
    all_slash_invocations(skills)
        .into_iter()
        .filter(|spec| {
            spec.matches_prefix(&prefix) || spec.section().to_ascii_lowercase().starts_with(&prefix)
        })
        .collect()
}

fn matching_skill_specs(prefix: &str, skills: &[SkillSummary]) -> Vec<SkillInvocationSpec> {
    let prefix = prefix.trim().to_ascii_lowercase();
    let mut matches = skills
        .iter()
        .map(SkillInvocationSpec::from_summary)
        .filter(|spec| spec.matches_prefix(&prefix))
        .collect::<Vec<_>>();
    if let Some(exact_index) = matches.iter().position(|spec| spec.matches_token(&prefix)) {
        matches.swap(0, exact_index);
    }
    matches
}

fn sort_matching_specs(matches: &mut [SlashInvocationSpec], prefix: &str) {
    if prefix.is_empty() {
        return;
    }

    // Slash parsing still gives builtin commands precedence for exact tokens.
    // Completion uses an explicit rank instead of declaration order so skill
    // slash aliases remain discoverable alongside builtin commands.
    matches.sort_by_cached_key(|spec| spec_rank(spec, prefix));
}

fn spec_rank(spec: &SlashInvocationSpec, prefix: &str) -> (u8, u8) {
    let exact = spec.matches_token(prefix);
    let exact_rank = if exact { 0 } else { 1 };
    let kind_rank = if exact {
        match spec {
            SlashInvocationSpec::Builtin(_) => 0,
            SlashInvocationSpec::Skill(_) => 1,
        }
    } else {
        match spec {
            SlashInvocationSpec::Skill(_) => 0,
            SlashInvocationSpec::Builtin(_) => 1,
        }
    };
    (exact_rank, kind_rank)
}

fn selected_spec(
    command_token: &str,
    tail: Option<&str>,
    selected_index: usize,
    matches: &[SlashInvocationSpec],
    skills: &[SkillSummary],
) -> Option<SlashInvocationSpec> {
    if tail.is_some() {
        return all_slash_invocations(skills)
            .into_iter()
            .find(|spec| spec.matches_token(command_token));
    }
    matches
        .get(selected_index.min(matches.len().saturating_sub(1)))
        .cloned()
}

fn selected_skill_spec(
    skill_token: &str,
    tail: Option<&str>,
    selected_index: usize,
    matches: &[SkillInvocationSpec],
) -> Option<SkillInvocationSpec> {
    if tail.is_some() {
        return matches
            .iter()
            .find(|spec| spec.matches_token(skill_token))
            .cloned();
    }
    matches
        .get(selected_index.min(matches.len().saturating_sub(1)))
        .cloned()
}

fn build_argument_hint(
    spec: SlashInvocationSpec,
    tail: Option<&str>,
) -> Option<SlashCommandArgumentHint> {
    let placeholders = spec.argument_specs();
    if placeholders.is_empty() {
        return None;
    }

    let tail = tail.unwrap_or("").trim();
    let raw_values = if tail.is_empty() {
        Vec::new()
    } else {
        tail.split_whitespace().collect::<Vec<_>>()
    };
    let provided_count = raw_values.len().min(placeholders.len());
    let mut provided = Vec::new();
    for (index, placeholder) in placeholders.iter().take(provided_count).enumerate() {
        // The last positional is treated as a greedy tail because several host
        // commands intentionally accept spaces after the final placeholder
        // (export paths, free-form notes, arbitrary queries).
        let value = if index + 1 == placeholders.len() {
            raw_values[index..].join(" ")
        } else {
            raw_values[index].to_string()
        };
        provided.push(SlashCommandArgumentValue {
            placeholder: placeholder.placeholder,
            value,
        });
        if index + 1 == placeholders.len() {
            break;
        }
    }

    Some(SlashCommandArgumentHint {
        provided,
        next: placeholders.get(provided_count).copied(),
    })
}
