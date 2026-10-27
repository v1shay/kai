//! Typed, bounded display models for native Codex interactions. These models do
//! not carry a responder handle: imported questions cannot resume a live tool.
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Speaker {
    User,
    Assistant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InteractionStatus {
    Started,
    Working,
    Completed,
    Cancelled,
    Failed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestionCard {
    pub id: String,
    pub header: String,
    pub prompt: String,
    pub options: Vec<QuestionOption>,
    pub answer: Option<String>,
    pub historical: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatInteraction {
    NativeRequest(crate::runtime::NativeRequest),
    Speaker {
        speaker: Speaker,
        text: String,
    },
    Delegation {
        title: String,
        details: String,
        status: InteractionStatus,
        agent: Option<String>,
    },
    Status {
        title: String,
        details: String,
        status: InteractionStatus,
    },
    Question(QuestionCard),
    Answer {
        question: String,
        answer: String,
    },
}

/// Provider question IDs are only scoped to one request (and can be absent).
/// Include the visible row so choices from a later question cannot select an
/// earlier question with the same provider ID.
pub fn question_choice_key(question: &QuestionCard, row: usize) -> String {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    question.id.hash(&mut hash);
    question.prompt.hash(&mut hash);
    for option in &question.options {
        option.label.hash(&mut hash);
        option.description.hash(&mut hash);
    }
    format!("{row}:{:016x}", hash.finish())
}

pub fn encode(card: &ChatInteraction) -> String {
    encode_with_context(card, None)
}

/// Fence context is presentation metadata, not part of the speaker's source
/// text. Older decoders continue to accept the same versioned card envelope.
pub fn encode_with_context(card: &ChatInteraction, code_context: Option<&str>) -> String {
    if let Some(context) = code_context {
        serde_json::json!({"orbit_interaction":1,"card":card,"code_context":context}).to_string()
    } else {
        serde_json::json!({"orbit_interaction":1,"card":card}).to_string()
    }
}

/// A private, versioned marker for interleaving live cards with the final text
/// persisted by the existing SQLite engine. Escape '<' so content cannot close it.
pub fn serialize_interaction(card: &ChatInteraction) -> String {
    format!(
        "<orbit_interaction version=\"1\">{}</orbit_interaction>",
        encode(card).replace('<', "\\u003c")
    )
}

#[cfg(test)]
pub fn decode(text: &str) -> Option<ChatInteraction> {
    decode_with_context(text).map(|(card, _)| card)
}

pub fn decode_with_context(text: &str) -> Option<(ChatInteraction, Option<String>)> {
    if text.len() > 2 * 1024 * 1024 {
        return None;
    }
    decode_value_with_context(serde_json::from_str(text).ok()?)
}

pub(crate) fn decode_value_with_context(mut value:Value)->Option<(ChatInteraction,Option<String>)>{
    if value["orbit_interaction"] != 1 {
        return None;
    }
    let context = value
        .get("code_context")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= 64 && !value.contains(['\n', '\r']))
        .map(str::to_owned);
    let card = value.get_mut("card")?.take();
    let mut card:ChatInteraction=serde_json::from_value(card).ok()?;
    if let ChatInteraction::Status{title,details,..}=&mut card
        && title=="File changes"
        && let Ok(item)=serde_json::from_str::<Value>(details)
        && item["type"]=="fileChange"
        && let Some(changes)=item["changes"].as_array()
        && changes.iter().all(|c|c["path"].is_string() && c["diff"].is_string()){
        *details=changes.iter().map(|c|format!("{}\n{}",c["path"].as_str().unwrap(),c["diff"].as_str().unwrap())).collect::<Vec<_>>().join("\n\n");
    }
    Some((card, context))
}

/// Preserve long interaction bodies without turning a continuation into raw
/// JSON. The serialized envelope remains independently readable on each page.
pub fn continuations(card: ChatInteraction, budget: usize) -> Vec<ChatInteraction> {
    let parts = |text: &str| {
        let mut pieces = vec![];
        let mut start = 0;
        while start < text.len() {
            let mut end = (start + budget.max(4)).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            if end < text.len()
                && let Some(newline) = text[start..end].rfind('\n')
                && newline > budget / 2
            {
                end = start + newline + 1;
            }
            pieces.push(text[start..end].to_owned());
            start = end;
        }
        pieces
    };
    match card {
        ChatInteraction::Speaker { speaker, text } if text.len() > budget => parts(&text)
            .into_iter()
            .map(|text| ChatInteraction::Speaker { speaker, text })
            .collect(),
        ChatInteraction::Delegation {
            title,
            details,
            status,
            agent,
        } if details.len() > budget => parts(&details)
            .into_iter()
            .map(|details| ChatInteraction::Delegation {
                title: title.clone(),
                details,
                status,
                agent: agent.clone(),
            })
            .collect(),
        card => vec![card],
    }
}

fn field(value: &Value, key: &str, limit: usize) -> String {
    value[key]
        .as_str()
        .map(|text| bounded(text, limit))
        .unwrap_or_default()
}

fn bounded(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_owned();
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &text[..end])
}

fn status(value: &Value, fallback: InteractionStatus) -> InteractionStatus {
    let state = value.as_str().or_else(|| {
        value
            .as_object()
            .and_then(|object| object.keys().next().map(String::as_str))
    });
    match state {
        Some("completed" | "done" | "ended" | "shutdown") => InteractionStatus::Completed,
        Some("errored" | "failed" | "not_found") => InteractionStatus::Failed,
        Some("interrupted" | "cancelled" | "canceled" | "aborted" | "stopped") => {
            InteractionStatus::Cancelled
        }
        Some("running" | "working" | "in_progress" | "interacted") => InteractionStatus::Working,
        Some("pending_init" | "started") => InteractionStatus::Started,
        _ => fallback,
    }
}

/// Legacy Codex agent states are externally tagged (for example,
/// `{ "completed": "review result" }`), unlike exec's state/message pair.
fn agent_status_message(value: &Value) -> Option<&str> {
    value
        .as_object()?
        .values()
        .find_map(Value::as_str)
        .filter(|message| !message.is_empty())
}

fn agent_status_summary(name: &str, value: &Value) -> String {
    let label = match status(value, InteractionStatus::Working) {
        InteractionStatus::Started => "Starting",
        InteractionStatus::Working => "Working",
        InteractionStatus::Completed => "Complete",
        InteractionStatus::Cancelled => "Stopped",
        InteractionStatus::Failed => "Needs attention",
    };
    let mut summary = format!("{} · {label}", bounded(name, 256));
    if let Some(message) = agent_status_message(value) {
        summary.push('\n');
        summary.push_str(&bounded(message, 1024));
    }
    summary
}

fn aggregate_status(states: impl IntoIterator<Item = InteractionStatus>) -> InteractionStatus {
    let mut seen = false;
    let mut active = false;
    let mut cancelled = false;
    for state in states {
        seen = true;
        match state {
            InteractionStatus::Failed => return InteractionStatus::Failed,
            InteractionStatus::Started | InteractionStatus::Working => active = true,
            InteractionStatus::Cancelled => cancelled = true,
            InteractionStatus::Completed => {}
        }
    }
    if !seen || active {
        InteractionStatus::Working
    } else if cancelled {
        InteractionStatus::Cancelled
    } else {
        InteractionStatus::Completed
    }
}

/// Accept only native event payloads selected by the runtime, never arbitrary
/// JSON inside a user's message. Audio frames and encrypted media are omitted.
pub fn native_event(value: &Value) -> Option<ChatInteraction> {
    let kind = value["type"].as_str()?;
    if kind == "plan_update" {
        return task_plan(value);
    }
    let (title, state) = match kind {
        "realtime_session_started" | "realtime_conversation_started" => {
            ("Voice conversation", InteractionStatus::Started)
        }
        "realtime_session_closed" | "realtime_conversation_closed" => (
            "Voice conversation ended",
            status(&value["outcome"], InteractionStatus::Completed),
        ),
        "task_complete" | "turn_complete" if !value["error"].is_null() => {
            ("Agent failed", InteractionStatus::Failed)
        }
        "task_complete" | "turn_complete" => ("Completed", InteractionStatus::Completed),
        "turn_aborted" => ("Stopped", InteractionStatus::Cancelled),
        "transcript_segment" => {
            let speaker = match value["role"].as_str()? {
                "user" => Speaker::User,
                "assistant" => Speaker::Assistant,
                _ => return None,
            };
            return Some(ChatInteraction::Speaker {
                speaker,
                text: field(value, "text", 128 * 1024),
            });
        }
        "collab_agent_spawn_begin"
        | "collab_agent_spawn_end"
        | "collab_agent_interaction_begin"
        | "collab_agent_interaction_end"
        | "collab_waiting_begin"
        | "collab_waiting_end"
        | "collab_close_begin"
        | "collab_close_end"
        | "collab_resume_begin"
        | "collab_resume_end"
        | "sub_agent_activity" => {
            // A spawn/send/wait call ending does not mean the child finished.
            // Codex's protocol reports the child state separately. Empty wait
            // results mean the wait elapsed while agents can still be running.
            let mut state = status(
                &value["status"],
                status(&value["kind"], InteractionStatus::Working),
            );
            let mut details = field(value, "prompt", 8 * 1024);
            if kind == "collab_waiting_end" {
                let states = value["statuses"].as_object();
                let entries = value["agent_statuses"].as_array();
                let mut summaries = Vec::new();
                if let Some(entries) = entries.filter(|entries| !entries.is_empty()) {
                    state = aggregate_status(
                        entries
                            .iter()
                            .map(|entry| status(&entry["status"], InteractionStatus::Working)),
                    );
                    // Keep failure details visible even when there are more
                    // agents than fit in the bounded summary.
                    let failed = |entry: &&Value| {
                        status(&entry["status"], InteractionStatus::Working)
                            == InteractionStatus::Failed
                    };
                    for entry in entries
                        .iter()
                        .filter(failed)
                        .chain(entries.iter().filter(|entry| !failed(entry)))
                        .take(16)
                    {
                        let name = entry["agent_nickname"]
                            .as_str()
                            .or_else(|| entry["thread_id"].as_str())
                            .unwrap_or("Agent");
                        summaries.push(agent_status_summary(name, &entry["status"]));
                    }
                } else if let Some(states) = states {
                    state = aggregate_status(
                        states
                            .values()
                            .map(|outcome| status(outcome, InteractionStatus::Working)),
                    );
                    let failed = |(_, outcome): &(&String, &Value)| {
                        status(outcome, InteractionStatus::Working) == InteractionStatus::Failed
                    };
                    for (id, outcome) in states
                        .iter()
                        .filter(failed)
                        .chain(states.iter().filter(|entry| !failed(entry)))
                        .take(16)
                    {
                        summaries.push(agent_status_summary(id, outcome));
                    }
                }
                details = bounded(&summaries.join("\n\n"), 8 * 1024);
            } else if let Some(message) = agent_status_message(&value["status"]) {
                if !details.is_empty() {
                    details.push_str("\n\n");
                }
                details.push_str(&bounded(message, 8 * 1024 - details.len().min(8 * 1024)));
            }
            if kind == "collab_close_end" && state != InteractionStatus::Failed {
                state = InteractionStatus::Cancelled;
            }
            let title = if kind.contains("spawn") {
                "Agent delegated"
            } else if kind.contains("waiting") {
                "Waiting for agents"
            } else if kind.contains("close") {
                "Agent closed"
            } else if kind.contains("resume") {
                "Agent resumed"
            } else {
                "Agent activity"
            };
            let agent = [
                "new_agent_nickname",
                "receiver_agent_nickname",
                "agent_path",
                "new_thread_id",
                "receiver_thread_id",
                "agent_thread_id",
            ]
            .iter()
            .find_map(|key| value[*key].as_str())
            .map(|name| bounded(name, 256));
            return Some(ChatInteraction::Delegation {
                title: title.into(),
                details,
                status: state,
                agent,
            });
        }
        _ => return None,
    };
    let details = if state == InteractionStatus::Failed {
        value["error"]["message"]
            .as_str()
            .or_else(|| value["error"].as_str())
            .or_else(|| value["reason"].as_str())
            .map(|message| bounded(message, 8 * 1024))
            .unwrap_or_default()
    } else if kind == "task_complete" || kind == "turn_complete" {
        value["duration_ms"]
            .as_u64()
            .map(|duration| format!("{}.{:01} s", duration / 1000, duration % 1000 / 100))
            .unwrap_or_default()
    } else {
        field(value, "reason", 1024)
    };
    Some(ChatInteraction::Status {
        title: title.into(),
        details,
        status: state,
    })
}

pub fn tool_interactions(name: &str, input: &Value, historical: bool) -> Vec<ChatInteraction> {
    let name = name
        .rsplit('.')
        .next()
        .unwrap_or(name)
        .rsplit("__")
        .next()
        .unwrap_or(name);
    if matches!(
        name,
        "update_plan" | "TodoWrite" | "todowrite" | "todo_write"
    ) {
        return task_plan(input).into_iter().collect();
    }
    if matches!(name, "request_user_input" | "request_user_input_async") {
        return input["questions"]
            .as_array()
            .into_iter()
            .flatten()
            .take(12)
            .filter_map(|question| {
                let prompt = question["question"].as_str()?;
                let options = question["options"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .take(16)
                    .filter_map(|option| {
                        Some(QuestionOption {
                            label: bounded(option["label"].as_str()?, 256),
                            description: field(option, "description", 1024),
                        })
                    })
                    .collect();
                Some(ChatInteraction::Question(QuestionCard {
                    id: field(question, "id", 256),
                    header: field(question, "header", 256),
                    prompt: bounded(prompt, 8 * 1024),
                    options,
                    answer: None,
                    historical,
                }))
            })
            .collect();
    }
    if matches!(
        name,
        "spawn_agent"
            | "Task"
            | "Agent"
            | "task"
            | "followup_task"
            | "send_message"
            | "send_input"
            | "wait"
            | "wait_agent"
            | "interrupt_agent"
            | "close_agent"
            | "resume_agent"
    ) {
        let title = match name {
            "spawn_agent" | "Task" | "Agent" | "task" => "Agent delegated",
            "followup_task" | "send_message" | "send_input" => "Sent to agent",
            "wait" | "wait_agent" => "Waiting for agents",
            "interrupt_agent" | "close_agent" => "Stopped agent",
            "resume_agent" => "Resumed agent",
            _ => "Agent activity",
        };
        return vec![ChatInteraction::Delegation {
            title: title.into(),
            details: input["message"]
                .as_str()
                .or_else(|| input["prompt"].as_str())
                .map(|text| bounded(text, 8 * 1024))
                .unwrap_or_default(),
            status: InteractionStatus::Working,
            agent: input["task_name"]
                .as_str()
                .or_else(|| input["description"].as_str())
                .or_else(|| input["subagent_type"].as_str())
                .or_else(|| input["target"].as_str())
                .or_else(|| input["id"].as_str())
                .map(|text| bounded(text, 256)),
        }];
    }
    Vec::new()
}

/// Codex plans and Claude/OpenCode task lists share display semantics, while
/// retaining each provider's authoritative task states. This is display only;
/// rendering a proposed plan never launches tools or changes agent permissions.
fn task_plan(input: &Value) -> Option<ChatInteraction> {
    let tasks = input["plan"]
        .as_array()
        .or_else(|| input["todos"].as_array())?;
    if tasks.is_empty() {
        return None;
    }
    let complete = tasks
        .iter()
        .filter(|task| task["status"] == "completed")
        .count();
    let mut details = field(input, "explanation", 2048);
    let mut shown = 0;
    for task in tasks.iter().take(64) {
        let Some(text) = task["step"].as_str().or_else(|| task["content"].as_str()) else {
            continue;
        };
        let marker = match task["status"].as_str() {
            Some("completed") => "☑",
            Some("in_progress") => "◉",
            Some("cancelled" | "canceled") => "⊘",
            _ => "☐",
        };
        let line = format!("{marker} {}", bounded(text, 1024));
        if details.len() + line.len() + 2 > 8 * 1024 {
            break;
        }
        if !details.is_empty() {
            details.push('\n');
        }
        details.push_str(&line);
        shown += 1;
    }
    if shown < tasks.len() {
        details.push_str(&format!(
            "\n… {} more tasks in the tool transcript",
            tasks.len() - shown
        ));
    }
    Some(ChatInteraction::Status {
        title: format!("Plan · {complete}/{} complete", tasks.len()),
        details,
        status: if complete == tasks.len() {
            InteractionStatus::Completed
        } else {
            InteractionStatus::Working
        },
    })
}

/// Reply wrappers use the desktop's actual historical answer schema.
pub fn question_answers(text: &str) -> Vec<ChatInteraction> {
    if text.len() > 128 * 1024 {
        return Vec::new();
    }
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return Vec::new();
    };
    value
        .as_array()
        .into_iter()
        .flatten()
        .take(12)
        .filter_map(|item| {
            let question = item["question"].as_str()?;
            let answer = item["answer"].as_str()?;
            Some(ChatInteraction::Answer {
                question: bounded(question, 8 * 1024),
                answer: bounded(answer, 8 * 1024),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn provider_task_lists_keep_real_task_states_and_never_fake_completion() {
        for (name, payload) in [
            (
                "functions.update_plan",
                json!({"explanation":"Verify first", "plan":[
                {"step":"Inspect", "status":"completed"},
                {"step":"Repair", "status":"in_progress"},
                {"step":"Ship", "status":"pending"}]}),
            ),
            (
                "TodoWrite",
                json!({"todos":[
                {"content":"Inspect", "status":"completed"},
                {"content":"Repair", "status":"in_progress"},
                {"content":"Ship", "status":"pending"}]}),
            ),
            (
                "todowrite",
                json!({"todos":[
                {"content":"Inspect", "status":"completed"},
                {"content":"Repair", "status":"in_progress"},
                {"content":"Ship", "status":"pending"}]}),
            ),
        ] {
            let cards = tool_interactions(name, &payload, true);
            assert!(
                matches!(&cards[..], [ChatInteraction::Status { title, details, status }]
                if title == "Plan · 1/3 complete" && details.contains("☑ Inspect")
                && details.contains("◉ Repair") && details.contains("☐ Ship")
                && *status == InteractionStatus::Working)
            );
        }
        let complete = json!({"type":"plan_update","plan":[{"step":"Done","status":"completed"}]});
        assert!(matches!(
            native_event(&complete),
            Some(ChatInteraction::Status {
                status: InteractionStatus::Completed,
                ..
            })
        ));
        let unknown = json!({"plan":[{"step":"Unknown","status":"unexpected"}]});
        assert!(matches!(
            &tool_interactions("update_plan", &unknown, false)[..],
            [ChatInteraction::Status {
                status: InteractionStatus::Working,
                ..
            }]
        ));
    }

    #[test]
    fn huge_task_lists_have_bounded_preview_and_explicit_remainder() {
        let input = json!({"todos":(0..200).map(|index| json!({"content":format!("{index} {}","😀".repeat(2000)),"status":"pending"})).collect::<Vec<_>>()});
        let cards = tool_interactions("todowrite", &input, true);
        assert!(matches!(&cards[..], [ChatInteraction::Status {details,..}]
            if details.len() < 9*1024 && details.contains("more tasks in the tool transcript")));
    }

    #[test]
    fn historical_questions_preserve_choices_without_a_responder() {
        let cards = tool_interactions(
            "functions.request_user_input",
            &json!({"questions":[{"id":"theme","header":"Appearance","question":"Which theme?","options":[{"label":"Glass","description":"Desktop translucency"},{"label":"Solid","description":"Opaque panels"}]}]}),
            true,
        );
        assert!(
            matches!(&cards[0], ChatInteraction::Question(question) if question.historical && question.options.len()==2 && question.answer.is_none())
        );
        assert_eq!(decode(&encode(&cards[0])), Some(cards[0].clone()));
    }
    #[test]
    fn native_voice_and_completion_events_have_semantic_states() {
        assert!(
            matches!(native_event(&json!({"type":"transcript_segment","role":"assistant","text":"Ready"})), Some(ChatInteraction::Speaker { speaker: Speaker::Assistant, text }) if text=="Ready")
        );
        assert!(matches!(
            native_event(&json!({"type":"realtime_session_closed","outcome":"failed"})),
            Some(ChatInteraction::Status {
                status: InteractionStatus::Failed,
                ..
            })
        ));
        assert!(
            matches!(native_event(&json!({"type":"task_complete","last_agent_message":"Do not repeat the full answer","duration_ms":2400})), Some(ChatInteraction::Status { details, status:InteractionStatus::Completed, .. }) if details=="2.4 s")
        );
        assert!(native_event(&json!({"type":"audio_out","data":"base64"})).is_none());
    }
    #[test]
    fn delegation_uses_native_status_and_keeps_metadata_small() {
        let card = native_event(&json!({"type":"collab_agent_spawn_end","status":{"errored":"unavailable"},"new_agent_nickname":"Reviewer","prompt":"Inspect the patch"})).unwrap();
        assert!(
            matches!(card,ChatInteraction::Delegation { agent:Some(agent),status:InteractionStatus::Failed,.. } if agent=="Reviewer")
        );
        let oversized = tool_interactions(
            "spawn_agent",
            &json!({"message":"😀".repeat(100_000),"task_name":"review"}),
            true,
        );
        assert!(encode(&oversized[0]).len() < 10 * 1024);
    }
    #[test]
    fn legacy_subagent_state_controls_completion_and_keeps_child_results() {
        let spawn = native_event(&json!({"type":"collab_agent_spawn_end", "status":"running", "new_thread_id":"child", "prompt":"Review"})).unwrap();
        assert!(matches!(
            spawn,
            ChatInteraction::Delegation {
                status: InteractionStatus::Working,
                ..
            }
        ));
        let wait = native_event(&json!({"type":"collab_waiting_end", "statuses":{"child":{"completed":"Reviewed the parser"}, "other":"running"}})).unwrap();
        assert!(
            matches!(wait, ChatInteraction::Delegation { status:InteractionStatus::Working, details, .. } if details.contains("Reviewed the parser") && details.contains("other · Working"))
        );
        let timeout = native_event(&json!({"type":"collab_waiting_end", "statuses":{}})).unwrap();
        assert!(matches!(
            timeout,
            ChatInteraction::Delegation {
                status: InteractionStatus::Working,
                ..
            }
        ));
        let failed = native_event(
            &json!({"type":"collab_waiting_end", "statuses":{"child":{"errored":"No permission"}}}),
        )
        .unwrap();
        assert!(
            matches!(failed, ChatInteraction::Delegation { status:InteractionStatus::Failed, details, .. } if details.contains("No permission"))
        );
        let named = native_event(&json!({"type":"collab_waiting_end", "agent_statuses":[{"agent_nickname":"Reviewer", "thread_id":"child", "status":{"completed":"Looks good"}}], "statuses":{"child":{"completed":"Looks good"}}})).unwrap();
        assert!(
            matches!(named, ChatInteraction::Delegation { status:InteractionStatus::Completed, details, .. } if details.contains("Reviewer · Complete") && details.matches("Looks good").count() == 1)
        );
        let interrupted = native_event(
            &json!({"type":"sub_agent_activity", "agent_path":"/review", "kind":"interrupted"}),
        )
        .unwrap();
        assert!(matches!(
            interrupted,
            ChatInteraction::Delegation {
                status: InteractionStatus::Cancelled,
                ..
            }
        ));
        let closed = native_event(&json!({"type":"collab_close_end", "status":"running"})).unwrap();
        assert!(matches!(
            closed,
            ChatInteraction::Delegation {
                status: InteractionStatus::Cancelled,
                ..
            }
        ));
    }

    #[test]
    fn large_agent_groups_keep_failure_details_inside_the_bounded_summary() {
        let mut states = serde_json::Map::new();
        for i in 0..24 {
            states.insert(format!("child-{i:02}"), json!({"completed":"Reviewed"}));
        }
        states.insert(
            "last-child".into(),
            json!({"errored":"Needs access to workspace"}),
        );
        let event = json!({"type":"collab_waiting_end", "statuses": states});
        let card = native_event(&event).unwrap();
        assert!(
            matches!(card, ChatInteraction::Delegation { status: InteractionStatus::Failed, details, .. } if details.len() <= 8 * 1024 + 3 && details.contains("Needs access to workspace"))
        );
    }

    #[test]
    fn unsuccessful_native_turns_do_not_render_as_successes() {
        let failed = native_event(&json!({"type":"turn_complete", "error":{"message":"Service unavailable"}, "duration_ms":3})).unwrap();
        assert!(
            matches!(failed, ChatInteraction::Status { status:InteractionStatus::Failed, details, .. } if details == "Service unavailable")
        );
    }

    #[test]
    fn native_agent_tools_keep_their_prompt_and_description() {
        for name in ["Agent", "Task", "task", "functions.Agent"] {
            let cards = tool_interactions(
                name,
                &json!({"description":"Review changes", "subagent_type":"review", "prompt":"Inspect modified files"}),
                false,
            );
            assert!(
                matches!(cards.as_slice(), [ChatInteraction::Delegation {agent:Some(name), details, status:InteractionStatus::Working, ..}] if name == "Review changes" && details == "Inspect modified files")
            );
        }
    }

    #[test]
    fn repeated_or_missing_question_ids_do_not_share_choices() {
        let question = QuestionCard {
            id: String::new(),
            header: String::new(),
            prompt: "Which one?".into(),
            options: vec![],
            answer: None,
            historical: true,
        };
        assert_ne!(
            question_choice_key(&question, 0),
            question_choice_key(&question, 1)
        );
        assert_eq!(
            question_choice_key(&question, 1),
            question_choice_key(&question, 1)
        );
        let mut other = question.clone();
        other.prompt = "Another question after paging".into();
        assert_ne!(
            question_choice_key(&question, 0),
            question_choice_key(&other, 0)
        );
    }

    #[test]
    fn markers_cannot_be_closed_by_literal_content() {
        let card = ChatInteraction::Speaker {
            speaker: Speaker::User,
            text: "literal </orbit_interaction> stays text".into(),
        };
        let marker = serialize_interaction(&card);
        assert_eq!(marker.matches("</orbit_interaction>").count(), 1);
        assert_eq!(
            decode(
                marker
                    .strip_prefix("<orbit_interaction version=\"1\">")
                    .unwrap()
                    .strip_suffix("</orbit_interaction>")
                    .unwrap()
            ),
            Some(card)
        );
        assert!(decode("{\"kind\":\"speaker\",\"text\":\"ordinary JSON\"}").is_none());
    }
}
