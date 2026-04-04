//! Subagent linking functions - task call matching, team metadata propagation, and color enrichment.
//!
//! These are free functions that link subagents to their parent Task calls and
//! propagate team metadata. Extracted from `subagent_resolver.rs`.

use crate::types::chunks::TeamInfo;
use crate::types::domain::MessageType;
use crate::types::messages::{ParsedMessage, ToolCall};
use std::collections::{HashMap, HashSet};

/// Maximum depth for parentUuid chain traversal when propagating team metadata.
pub(super) const MAX_PARENT_DEPTH: usize = 10;

/// Enrich a subagent Process with metadata from its parent Task call.
pub(super) fn enrich_subagent_from_task(subagent: &mut super::Process, task_call: &ToolCall) {
    subagent.task_id = Some(task_call.id.clone());
    subagent.description = task_call.task_description.clone();
    subagent.subagent_type = task_call.task_subagent_type.clone();

    let team_name = task_call.input.get("team_name").and_then(|v| v.as_str());
    let member_name = task_call.input.get("name").and_then(|v| v.as_str());
    if let (Some(tn), Some(mn)) = (team_name, member_name) {
        subagent.team = Some(TeamInfo {
            team_name: tn.to_string(),
            member_name: mn.to_string(),
            member_color: String::new(),
        });
    }
}

/// Extract the summary attribute from a teammate-message tag in the first user message.
pub(super) fn extract_team_message_summary(messages: &[ParsedMessage]) -> Option<String> {
    let first_user = messages.iter().find(|m| m.message_type == MessageType::User)?;
    let content_str = first_user.content.as_str().unwrap_or("");
    let re = regex::Regex::new(r#"<teammate-message[^>]*\bsummary="([^"]+)""#).ok()?;
    re.captures(content_str)
        .map(|cap| cap[1].to_string())
}

/// Link subagents to their parent Task calls using a 3-phase matching algorithm.
pub(super) fn link_to_task_calls(
    subagents: &mut [super::Process],
    task_calls: &[ToolCall],
    messages: &[ParsedMessage],
) {
    // Phase 0: Preprocessing
    let task_calls_only: Vec<&ToolCall> = task_calls.iter().filter(|tc| tc.is_task).collect();
    if task_calls_only.is_empty() || subagents.is_empty() {
        return;
    }

    // Build agentId -> taskCallId mapping from tool results
    let mut agent_id_to_task_id: HashMap<String, String> = HashMap::new();
    for msg in messages {
        if let Some(result) = &msg.tool_use_result {
            let agent_id = result
                .get("agentId")
                .or_else(|| result.get("agent_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let task_call_id = msg.source_tool_use_id.clone()
                .or_else(|| msg.tool_results.first().map(|tr| tr.tool_use_id.clone()));
            if let (Some(aid), Some(tcid)) = (agent_id, task_call_id) {
                agent_id_to_task_id.insert(aid, tcid);
            }
        }
    }

    let task_call_by_id: HashMap<&str, &ToolCall> = task_calls_only
        .iter()
        .map(|tc| (tc.id.as_str(), *tc))
        .collect();

    let mut matched_subagent_ids: HashSet<String> = HashSet::new();
    let mut matched_task_ids: HashSet<String> = HashSet::new();

    // Phase 1: Result matching (agentId exact match)
    for subagent in subagents.iter_mut() {
        if let Some(task_call_id) = agent_id_to_task_id.get(&subagent.id) {
            if let Some(&task_call) = task_call_by_id.get(task_call_id.as_str()) {
                enrich_subagent_from_task(subagent, task_call);
                matched_subagent_ids.insert(subagent.id.clone());
                matched_task_ids.insert(task_call_id.clone());
            }
        }
    }

    // Phase 2: Description matching (team members)
    let team_task_calls: Vec<&&ToolCall> = task_calls_only
        .iter()
        .filter(|tc| {
            !matched_task_ids.contains(&tc.id)
                && tc.input.get("team_name").is_some()
                && tc.input.get("name").is_some()
        })
        .collect();

    if !team_task_calls.is_empty() {
        let mut subagent_summaries: HashMap<String, String> = HashMap::new();
        for subagent in subagents.iter() {
            if matched_subagent_ids.contains(&subagent.id) {
                continue;
            }
            if let Some(summary) = extract_team_message_summary(&subagent.messages) {
                subagent_summaries.insert(subagent.id.clone(), summary);
            }
        }

        for team_tc in &team_task_calls {
            let desc = match &team_tc.task_description {
                Some(d) if !d.is_empty() => d.clone(),
                _ => continue,
            };
            let mut best_match_idx: Option<usize> = None;
            let mut best_match_time: u64 = u64::MAX;
            for (i, subagent) in subagents.iter().enumerate() {
                if matched_subagent_ids.contains(&subagent.id) {
                    continue;
                }
                if subagent_summaries.get(&subagent.id).map(|s| s == &desc).unwrap_or(false) {
                    if subagent.start_time_ms < best_match_time {
                        best_match_time = subagent.start_time_ms;
                        best_match_idx = Some(i);
                    }
                }
            }
            if let Some(idx) = best_match_idx {
                enrich_subagent_from_task(&mut subagents[idx], team_tc);
                matched_subagent_ids.insert(subagents[idx].id.clone());
                matched_task_ids.insert(team_tc.id.clone());
            }
        }
    }

    // Phase 3: Positional fallback (no wrap-around)
    let mut unmatched_indices: Vec<usize> = subagents
        .iter()
        .enumerate()
        .filter(|(_, s)| !matched_subagent_ids.contains(&s.id))
        .map(|(i, _)| i)
        .collect();
    unmatched_indices.sort_by_key(|&i| subagents[i].start_time_ms);

    let unmatched_tasks: Vec<&&ToolCall> = task_calls_only
        .iter()
        .filter(|tc| !matched_task_ids.contains(&tc.id) && tc.input.get("team_name").is_none())
        .collect();

    let pair_count = unmatched_indices.len().min(unmatched_tasks.len());
    for i in 0..pair_count {
        enrich_subagent_from_task(&mut subagents[unmatched_indices[i]], unmatched_tasks[i]);
    }
}

/// Propagate team metadata to continuation files via parentUuid chain.
pub(super) fn propagate_team_metadata(subagents: &mut [super::Process]) {
    // Build last message uuid -> subagent index mapping
    let mut last_uuid_to_idx: HashMap<String, usize> = HashMap::new();
    for (i, subagent) in subagents.iter().enumerate() {
        if let Some(last) = subagent.messages.last() {
            if !last.uuid.is_empty() {
                last_uuid_to_idx.insert(last.uuid.clone(), i);
            }
        }
    }

    // Phase 1: Collect which subagent each continuation should inherit from
    let mut inherit_from: Vec<Option<usize>> = vec![None; subagents.len()];
    for (i, subagent) in subagents.iter().enumerate() {
        if subagent.team.is_some() {
            continue;
        }
        if subagent.messages.is_empty() {
            continue;
        }

        let first_parent_uuid = match subagent.messages.first().and_then(|m| m.parent_uuid.as_ref()) {
            Some(uuid) if !uuid.is_empty() => uuid.clone(),
            _ => continue,
        };

        // Walk parentUuid chain
        let mut current_uuid = first_parent_uuid;
        let mut depth = 0;
        let mut ancestor_idx: Option<usize> = None;

        while depth < MAX_PARENT_DEPTH {
            if let Some(&idx) = last_uuid_to_idx.get(&current_uuid) {
                if subagents[idx].team.is_some() {
                    ancestor_idx = Some(idx);
                    break;
                }
                if let Some(prev_last) = subagents[idx].messages.last() {
                    if let Some(prev_parent) = &prev_last.parent_uuid {
                        current_uuid = prev_parent.clone();
                        depth += 1;
                        continue;
                    }
                }
            }
            break;
        }

        inherit_from[i] = ancestor_idx;
    }

    // Phase 2: Apply inheritance
    // Collect cloned data to avoid simultaneous borrow of different indices in the slice
    let inherited: Vec<(usize, Option<TeamInfo>, Option<String>, Option<String>, Option<String>)> =
        inherit_from
            .iter()
            .enumerate()
            .filter_map(|(i, anc)| {
                let anc = (*anc)?;
                let ancestor = &subagents[anc];
                Some((
                    i,
                    ancestor.team.clone(),
                    ancestor.task_id.clone(),
                    ancestor.description.clone(),
                    ancestor.subagent_type.clone(),
                ))
            })
            .collect();

    for (i, team, task_id, description, subagent_type) in inherited {
        subagents[i].team = team;
        subagents[i].task_id = subagents[i].task_id.take().or(task_id);
        subagents[i].description = subagents[i].description.take().or(description);
        subagents[i].subagent_type = subagents[i].subagent_type.take().or(subagent_type);
    }
}

/// Inject team member colors from teammate_spawned tool results.
pub(super) fn enrich_team_colors(subagents: &mut [super::Process], messages: &[ParsedMessage]) {
    for msg in messages {
        let source_id = match &msg.source_tool_use_id {
            Some(id) if !id.is_empty() => id.as_str(),
            _ => continue,
        };
        let result = match &msg.tool_use_result {
            Some(r) => r,
            None => continue,
        };
        let status = match result.get("status").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => continue,
        };
        let color = match result.get("color").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c.to_string(),
            _ => continue,
        };
        if status != "teammate_spawned" {
            continue;
        }
        for subagent in subagents.iter_mut() {
            if subagent.task_id.as_deref() == Some(source_id) {
                if let Some(team) = &mut subagent.team {
                    team.member_color = color.clone();
                }
            }
        }
    }
}
