use super::*;

#[cfg(test)]
pub(crate) fn command_palette_lines() -> Vec<InspectorEntry> {
    command_palette_lines_for(None)
}

pub(crate) fn command_palette_lines_for(query: Option<&str>) -> Vec<InspectorEntry> {
    let trimmed = query
        .map(str::trim)
        .filter(|query| !query.is_empty())
        .map(|query| query.trim_start_matches('/').to_ascii_lowercase());
    let specs = trimmed
        .as_deref()
        .map(palette_matching_specs)
        .unwrap_or_else(|| SLASH_COMMAND_SPECS.to_vec());
    if specs.is_empty() {
        return vec![
            InspectorEntry::section("Command Palette"),
            InspectorEntry::Muted("No commands match this query.".to_string()),
        ];
    }
    let mut lines = Vec::new();
    let mut current_section = None;
    for spec in specs {
        if current_section != Some(spec.section) {
            current_section = Some(spec.section);
            lines.push(InspectorEntry::section(spec.section));
        }
        let alias_suffix = if spec.aliases().is_empty() {
            String::new()
        } else {
            format!(
                " · aliases: {}",
                spec.aliases()
                    .iter()
                    .map(|alias| format!("/{alias}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            )
        };
        lines.push(InspectorEntry::actionable_collection(
            format!("/{}", spec.usage),
            Some(format!("{}{}", spec.summary, alias_suffix)),
            inspector_action_for_slash_spec(spec),
        ));
    }
    lines
}

pub(crate) fn inspector_action_for_slash_name(name: &str) -> Option<InspectorAction> {
    SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .find(|spec| spec.name == name)
        .map(inspector_action_for_slash_spec)
}

pub(crate) fn inspector_action_for_slash_spec(spec: SlashCommandSpec) -> InspectorAction {
    if spec.requires_arguments() {
        InspectorAction::FillInput(format!("/{} ", spec.name))
    } else {
        InspectorAction::RunCommand(format!("/{}", spec.name))
    }
}

pub(crate) fn slash_command_hint(input: &str, selected_index: usize) -> Option<SlashCommandHint> {
    let (command_token, tail) = split_slash_input(input)?;
    let matches = matching_specs(command_token);
    if let Some(selected) = selected_spec(command_token, tail, selected_index, &matches) {
        return Some(SlashCommandHint {
            exact: selected.matches_token(command_token),
            arguments: selected
                .matches_token(command_token)
                .then(|| build_argument_hint(selected, tail))
                .flatten(),
            selected_match_index: matches
                .iter()
                .position(|spec| spec.name == selected.name)
                .unwrap_or(0),
            selected,
            matches,
        });
    }
    None
}

pub(crate) fn cycle_slash_command(
    input: &str,
    selected_index: usize,
    backwards: bool,
) -> Option<(String, usize)> {
    let (command_token, tail) = split_slash_input(input)?;
    if tail.is_some() {
        return None;
    }
    let matches = matching_specs(command_token);
    if matches.is_empty() {
        return None;
    }
    let current = selected_index.min(matches.len().saturating_sub(1));
    let exact_at_current = matches
        .get(current)
        .is_some_and(|spec| spec.name == command_token);
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
    Some((format!("/{} ", matches[next].name), next))
}

pub(crate) fn move_slash_command_selection(
    input: &str,
    selected_index: usize,
    backwards: bool,
) -> Option<usize> {
    let (command_token, tail) = split_slash_input(input)?;
    if tail.is_some() {
        return None;
    }
    let matches = matching_specs(command_token);
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

pub(crate) fn resolve_slash_enter_action(
    input: &str,
    selected_index: usize,
) -> Option<SlashCommandEnterAction> {
    let hint = slash_command_hint(input, selected_index)?;
    if hint.exact {
        if hint
            .arguments
            .as_ref()
            .and_then(|arguments| arguments.next)
            .is_some_and(|argument| argument.required)
        {
            return Some(SlashCommandEnterAction::Complete {
                input: format!("/{} ", hint.selected.name),
                index: hint.selected_match_index,
            });
        }
        return None;
    }
    if hint.matches.len() == 1 && !hint.selected.requires_arguments() {
        return Some(SlashCommandEnterAction::Execute(format!(
            "/{}",
            hint.selected.name
        )));
    }
    Some(SlashCommandEnterAction::Complete {
        input: format!("/{} ", hint.selected.name),
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

fn matching_specs(prefix: &str) -> Vec<SlashCommandSpec> {
    let prefix = prefix.trim().to_ascii_lowercase();
    let mut matches = SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .filter(|spec| spec.matches_prefix(&prefix))
        .collect::<Vec<_>>();
    if let Some(exact_index) = matches.iter().position(|spec| spec.matches_token(&prefix)) {
        matches.swap(0, exact_index);
    }
    matches
}

fn palette_matching_specs(prefix: &str) -> Vec<SlashCommandSpec> {
    let prefix = prefix.trim().to_ascii_lowercase();
    SLASH_COMMAND_SPECS
        .iter()
        .copied()
        .filter(|spec| {
            spec.matches_prefix(&prefix) || spec.section.to_ascii_lowercase().starts_with(&prefix)
        })
        .collect()
}

fn selected_spec(
    command_token: &str,
    tail: Option<&str>,
    selected_index: usize,
    matches: &[SlashCommandSpec],
) -> Option<SlashCommandSpec> {
    if tail.is_some() {
        return SLASH_COMMAND_SPECS
            .iter()
            .copied()
            .find(|spec| spec.matches_token(command_token));
    }
    matches
        .get(selected_index.min(matches.len().saturating_sub(1)))
        .copied()
}

fn build_argument_hint(
    spec: SlashCommandSpec,
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
        // (`spawn_task <prompt>`, export paths, free-form notes).
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
