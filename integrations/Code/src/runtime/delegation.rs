//! Provider-owned child runs. No additional process, approval policy, or model
//! backend is invented here: the running parent invokes its native tool.
//! Wire schemas checked against codex rust-v0.162.1 exec_events.rs, OpenCode
//! v1.18.3 task.ts/run.ts, and Claude Code's documented parent_tool_use_id.
use super::{RuntimeEvent, StreamParser, content_text_view};
use crate::{
    chat_interactions::{ChatInteraction, InteractionStatus},
    discovery::AgentKind,
};
use anyhow::{Result, bail};
use serde_json::Value;

const CHILDREN_PER_EVENT: usize = 32;
const CHILD_PREVIEW_BYTES: usize = 8 * 1024;
const EVENT_PREVIEW_BYTES: usize = 64 * 1024;
const TRACKED_CLAUDE_CHILDREN: usize = 64;

pub fn supports_native_delegation(agent: AgentKind) -> bool {
    matches!(
        agent,
        AgentKind::Codex | AgentKind::Claude | AgentKind::OpenCode | AgentKind::Cursor
    )
}

/// Explicitly request a provider-native child inside the current parent chat.
/// A request is not a launch confirmation; only actual wire events confirm it.
pub fn delegation_prompt(agent: AgentKind, task: &str) -> Result<String> {
    let task = task.trim();
    if task.is_empty() {
        bail!("Describe the task to delegate first.");
    }
    if task.len() > 32 * 1024 || task.contains('\0') {
        bail!("The delegated task must be at most 32 KB and contain no null characters.");
    }
    let instruction = match agent {
        AgentKind::Codex => {
            "Use your native spawn_agent tool to delegate the task below to one subagent. Wait for that child with your native collaboration tools, then report its result in this conversation."
        }
        AgentKind::Claude => {
            "Use your native Agent tool (Task on older versions) to delegate the task below to one suitable subagent. Run it in the foreground, wait for its result, then report the result in this conversation."
        }
        AgentKind::OpenCode => {
            "Use your native task tool to delegate the task below to one suitable subagent. Run it in the foreground, wait for its result, then report the result in this conversation."
        }
        AgentKind::Cursor => {
            "Use your native Task tool to delegate the task below to one suitable subagent. Run it in the foreground, wait for its result, then report the result in this conversation."
        }
        _ => bail!(
            "{} does not expose supported native subagent delegation in Code.",
            agent.name()
        ),
    };
    Ok(format!(
        "{instruction}\nKeep the current model defaults and permission restrictions. If the native tool is unavailable or denied, explain that and do not pretend a child was launched or launch a replacement CLI process.\n\nDelegated task:\n{task}"
    ))
}

fn preview(text: &str, budget: usize) -> String {
    let mut end = text.len().min(budget);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    if end == text.len() {
        text.to_owned()
    } else {
        format!(
            "{}\n… Preview shortened; full output remains in the native session.",
            &text[..end]
        )
    }
}

fn state_status(state: &str) -> Option<InteractionStatus> {
    match state {
        "pending_init" | "running" => Some(InteractionStatus::Working),
        "completed" => Some(InteractionStatus::Completed),
        "interrupted" | "shutdown" => Some(InteractionStatus::Cancelled),
        "errored" | "not_found" => Some(InteractionStatus::Failed),
        _ => None,
    }
}

fn title(status: InteractionStatus) -> &'static str {
    match status {
        InteractionStatus::Started | InteractionStatus::Working => "Subagent working",
        InteractionStatus::Completed => "Subagent completed",
        InteractionStatus::Cancelled => "Subagent stopped",
        InteractionStatus::Failed => "Subagent needs attention",
    }
}

impl StreamParser {
    pub(super) fn codex_delegation(&mut self, item: &Value) -> Vec<RuntimeEvent> {
        let tool = item["tool"].as_str().unwrap_or("");
        let states = item["agents_states"].as_object();
        // A tool can finish while its child is still running. The authoritative
        // child status is agents_states, never the tool's completed status.
        let mut ids: Vec<&str> = item["receiver_thread_ids"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|id| !id.is_empty())
            .take(CHILDREN_PER_EVENT)
            .collect();
        if let Some(states) = states {
            for id in states.keys() {
                if ids.len() == CHILDREN_PER_EVENT {
                    break;
                }
                if !id.is_empty() && !ids.contains(&id.as_str()) {
                    ids.push(id);
                }
            }
        }
        if ids.is_empty() {
            return crate::chat_interactions::tool_interactions(tool, item, false)
                .into_iter()
                .flat_map(|mut card| {
                    if item["status"] == "failed"
                        && let ChatInteraction::Delegation { status, .. } = &mut card
                    {
                        *status = InteractionStatus::Failed;
                    }
                    self.interaction(card, item["id"].as_str().unwrap_or("delegation"))
                })
                .collect();
        }
        let budget = CHILD_PREVIEW_BYTES.min(EVENT_PREVIEW_BYTES / ids.len());
        let mut events = Vec::new();
        for id in ids {
            let state = states.and_then(|states| states.get(id));
            let status = state
                .and_then(|state| state["status"].as_str())
                .and_then(state_status)
                .unwrap_or_else(|| {
                    if item["status"] == "failed" {
                        InteractionStatus::Failed
                    } else if tool == "close_agent" && item["status"] == "completed" {
                        InteractionStatus::Cancelled
                    } else {
                        InteractionStatus::Working
                    }
                });
            let message = state
                .and_then(|state| state["message"].as_str())
                .unwrap_or("");
            let details = if message.is_empty() {
                preview(item["prompt"].as_str().unwrap_or(""), budget)
            } else {
                preview(message, budget)
            };
            events.extend(self.interaction(
                ChatInteraction::Delegation {
                    title: title(status).into(),
                    details,
                    status,
                    agent: Some(preview(id, 256)),
                },
                &format!("codex-child:{id}"),
            ));
        }
        events
    }

    pub(super) fn opencode_delegation(&mut self, part: &Value) -> Vec<RuntimeEvent> {
        let state = &part["state"];
        let metadata = &state["metadata"];
        // OpenCode task.ts returns a completed *tool* with background=true and
        // <task state="running"> when a child is still running.
        let background = metadata["background"].as_bool().unwrap_or(false);
        let status = match state["status"].as_str() {
            Some("error") => InteractionStatus::Failed,
            Some("completed") if !background => InteractionStatus::Completed,
            _ => InteractionStatus::Working,
        };
        let id = metadata["sessionId"]
            .as_str()
            .or_else(|| state["input"]["task_id"].as_str());
        let output = state["output"]
            .as_str()
            .or_else(|| state["error"].as_str())
            .unwrap_or("");
        let details = if output.is_empty() {
            state["input"]["prompt"].as_str().unwrap_or("")
        } else {
            // Provider's task wrapper is protocol metadata, not chat prose.
            task_output(output)
        };
        self.interaction(
            ChatInteraction::Delegation {
                title: title(status).into(),
                details: preview(details, CHILD_PREVIEW_BYTES),
                status,
                agent: id.map(|id| preview(id, 256)).or_else(|| {
                    state["input"]["subagent_type"]
                        .as_str()
                        .map(|s| preview(s, 256))
                }),
            },
            part["id"].as_str().unwrap_or("opencode-task"),
        )
    }

    pub(super) fn remember_claude_child(&mut self, block: &Value) {
        if !matches!(block["name"].as_str(), Some("Agent" | "Task")) {
            return;
        }
        let Some(id) = block["id"]
            .as_str()
            .filter(|id| !id.is_empty() && id.len() <= 256)
        else {
            return;
        };
        if self.child_tasks.len() >= TRACKED_CLAUDE_CHILDREN && !self.child_tasks.contains_key(id) {
            return;
        }
        // Preserve full tool identity; name/description can be shown as details
        // without losing the correlation needed for the terminal tool result.
        self.child_tasks.insert(
            id.into(),
            preview(block["input"]["description"].as_str().unwrap_or(""), 512),
        );
    }

    pub(super) fn claude_child_event(&mut self, value: &Value, parent: &str) -> Vec<RuntimeEvent> {
        let status = if value["is_error"] == true {
            InteractionStatus::Failed
        } else {
            InteractionStatus::Working
        };
        let mut details = String::new();
        if matches!(value["type"].as_str(), Some("assistant" | "user")) {
            let content = &value["message"]["content"];
            if let Some(blocks) = content.as_array() {
                for block in blocks.iter().take(32) {
                    if block["type"] == "tool_use" {
                        self.remember_claude_child(block);
                    }
                    let next = match block["type"].as_str() {
                        Some("text") if value["type"] == "assistant" => {
                            block["text"].as_str().unwrap_or("")
                        }
                        Some("tool_use") => block["name"].as_str().unwrap_or("Tool"),
                        _ => "",
                    };
                    if !next.is_empty() && details.len() < CHILD_PREVIEW_BYTES {
                        if !details.is_empty() {
                            details.push('\n');
                        }
                        details.push_str(&preview(next, CHILD_PREVIEW_BYTES - details.len()));
                    }
                }
            }
        } else if value["type"] == "tool_progress" {
            details = format!(
                "Using {}",
                preview(value["tool_name"].as_str().unwrap_or("tool"), 128)
            );
        }
        // A nested child can itself delegate: its result is still keyed to
        // the nested Agent tool, while parent_tool_use_id identifies its owner.
        let mut events = if value["type"] == "user" {
            self.claude_child_results(value)
        } else {
            Vec::new()
        };
        if details.is_empty() {
            return events;
        }
        events.extend(self.interaction(
            ChatInteraction::Delegation {
                title: title(status).into(),
                details,
                status,
                agent: Some(preview(parent, 256)),
            },
            &format!("claude-child:{parent}"),
        ));
        events
    }

    pub(super) fn claude_task_event(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        let Some(id) = value["tool_use_id"]
            .as_str()
            .filter(|id| self.child_tasks.contains_key(*id))
        else {
            // These system events also describe shell jobs. They must not be
            // misrepresented as subagents just because they have a task ID.
            return Vec::new();
        };
        let status = if value["subtype"] == "task_notification" {
            match value["status"].as_str() {
                Some("completed") => InteractionStatus::Completed,
                Some("failed") => InteractionStatus::Failed,
                Some("killed") => InteractionStatus::Cancelled,
                _ => InteractionStatus::Working,
            }
        } else {
            InteractionStatus::Working
        };
        let details = value["summary"]
            .as_str()
            .or_else(|| value["description"].as_str())
            .unwrap_or("");
        let card = ChatInteraction::Delegation {
            title: title(status).into(),
            details: preview(details, CHILD_PREVIEW_BYTES),
            status,
            agent: Some(preview(id, 256)),
        };
        if matches!(
            status,
            InteractionStatus::Completed | InteractionStatus::Cancelled | InteractionStatus::Failed
        ) {
            self.child_tasks.remove(id);
        }
        self.interaction(card, &format!("claude-child:{id}"))
    }

    pub(super) fn claude_child_results(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();
        if let Some(blocks) = value["message"]["content"].as_array() {
            let budget = CHILD_PREVIEW_BYTES
                .min(EVENT_PREVIEW_BYTES / blocks.len().clamp(1, CHILDREN_PER_EVENT));
            for block in blocks.iter().take(CHILDREN_PER_EVENT) {
                if block["type"] != "tool_result" {
                    continue;
                }
                let Some(id) = block["tool_use_id"].as_str() else {
                    continue;
                };
                let Some(description) = self.child_tasks.get(id) else {
                    continue;
                };
                let native_result = &value["tool_use_result"];
                let background = native_result["isAsync"] == true
                    || native_result["data"]["isAsync"] == true
                    || native_result["status"] == "async_launched"
                    || native_result["data"]["status"] == "async_launched";
                let status = if block["is_error"] == true {
                    InteractionStatus::Failed
                } else if background {
                    InteractionStatus::Working
                } else {
                    InteractionStatus::Completed
                };
                let text = content_text_view(&block["content"]);
                events.extend(self.interaction(
                    ChatInteraction::Delegation {
                        title: title(status).into(),
                        details: preview(if text.is_empty() { description } else { &text }, budget),
                        status,
                        agent: Some(id.into()),
                    },
                    &format!("claude-child:{id}"),
                ));
                if !background || status == InteractionStatus::Failed {
                    self.child_tasks.remove(id);
                }
            }
        }
        events
    }
}

/// Peel only the provider's documented outer task envelope. Arbitrary user
/// angle-bracket text inside the result remains untouched.
fn task_output(output: &str) -> &str {
    if !output.starts_with("<task ") {
        return output;
    }
    for tag in ["task_result", "task_error"] {
        let begin = format!("<{tag}>\n");
        let end = format!("\n</{tag}>\n</task>");
        if let Some((_, body)) = output.split_once(&begin)
            && let Some(body) = body.strip_suffix(&end)
        {
            return body;
        }
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    #[ignore = "uses an installed authenticated Codex and real parent/child models; set CODE_DELEGATION_SMOKE_EXECUTABLE"]
    fn installed_codex_native_child_round_trip() {
        use super::super::{RunRequest, launch};
        use std::{
            path::PathBuf,
            time::{Duration, Instant},
        };
        let executable = PathBuf::from(
            std::env::var_os("CODE_DELEGATION_SMOKE_EXECUTABLE")
                .expect("set CODE_DELEGATION_SMOKE_EXECUTABLE"),
        );
        let project =
            std::env::temp_dir().join(format!("code-delegation-smoke-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&project).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(project.clone());
        let prompt = delegation_prompt(AgentKind::Codex,"The child must reply exactly CODE_CHILD_SMOKE_OK. Neither parent nor child may read files, run commands, edit files, or use tools other than the parent's native spawn_agent and wait tools. After the child completes, the parent must reply exactly CODE_PARENT_SMOKE_OK. This is a minimal integration test, so spawn exactly one child.").unwrap();
        let handle = launch(RunRequest { orchestrator:false, service_tier:None, reasoning_effort:None,
            agent: AgentKind::Codex,
            executable,
            project,
            prompt,
            resume_id: None,
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: false,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            codex_access: Default::default(),
            trust_workspace: false,
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(150);
        let mut text = String::new();
        let mut child_id = None;
        let mut running = false;
        let mut child_complete = false;
        let mut result_seen = false;
        let mut finished = false;
        loop {
            if Instant::now() >= deadline {
                handle.cancel();
                panic!(
                    "Native delegation timed out; running={running}, child_complete={child_complete}, result_seen={result_seen}"
                );
            }
            match handle.events.recv_timeout(Duration::from_secs(1)) {
                Ok(RuntimeEvent::Text(delta)) => text.push_str(&delta),
                Ok(RuntimeEvent::ReplaceText(snapshot)) => text = snapshot,
                Ok(RuntimeEvent::Interaction(card)) => {
                    if let ChatInteraction::Delegation {
                        agent: Some(id),
                        status,
                        details,
                        ..
                    } = *card
                    {
                        child_id = Some(id);
                        running |= matches!(
                            status,
                            InteractionStatus::Working | InteractionStatus::Started
                        );
                        child_complete |= status == InteractionStatus::Completed;
                        result_seen |= details.contains("CODE_CHILD_SMOKE_OK");
                        assert_ne!(
                            status,
                            InteractionStatus::Failed,
                            "Native child reported failure"
                        );
                    }
                }
                Ok(RuntimeEvent::Finished) => {
                    finished = true;
                    break;
                }
                Ok(RuntimeEvent::Failed(error)) => panic!(
                    "Native delegation failed: {}",
                    super::super::diagnostic_summary(&error)
                ),
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(
            finished && text.contains("CODE_PARENT_SMOKE_OK"),
            "Parent turn did not finish successfully"
        );
        assert!(
            running && child_complete && result_seen,
            "Missing native child lifecycle metadata; running={running}, child_complete={child_complete}, result_seen={result_seen}"
        );
        eprintln!(
            "NATIVE_DELEGATION_SMOKE completed: child_id={child_id:?}, working={running}, child_complete={child_complete}, child_result={result_seen}, parent_complete={finished}"
        );
    }

    fn cards(events: Vec<RuntimeEvent>) -> Vec<ChatInteraction> {
        events
            .into_iter()
            .filter_map(|event| match event {
                RuntimeEvent::Interaction(card) => Some(*card),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn delegation_requests_stay_with_the_parent_and_do_not_change_permissions() {
        for agent in [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::OpenCode,
            AgentKind::Cursor,
        ] {
            let prompt = delegation_prompt(agent, "Review the parser 🌊").unwrap();
            assert!(prompt.contains("Review the parser 🌊"));
            assert!(prompt.contains("current model defaults and permission restrictions"));
            assert!(prompt.contains("do not pretend"));
            assert!(prompt.contains("in this conversation"));
        }
        for agent in [AgentKind::Gemini, AgentKind::Grok, AgentKind::Antigravity] {
            assert!(!supports_native_delegation(agent));
            assert!(delegation_prompt(agent, "task").is_err());
        }
        assert!(delegation_prompt(AgentKind::Codex, " \n").is_err());
        assert!(delegation_prompt(AgentKind::Codex, "bad\0task").is_err());
        assert!(delegation_prompt(AgentKind::Codex, &"x".repeat(32 * 1024 + 1)).is_err());
    }

    #[test]
    fn completed_codex_spawn_reports_running_child_and_full_identity() {
        let mut parser = StreamParser::default();
        let event = json!({"type":"item.completed","item":{
            "id":"call1","type":"collab_tool_call","tool":"spawn_agent", "status":"completed",
            "receiver_thread_ids":["native-child-1234567890"],
            "prompt":"Inspect parser", "agents_states":{"native-child-1234567890":{"status":"running","message":null}}
        }});
        assert!(
            matches!(&cards(parser.parse_codex(&event))[0], ChatInteraction::Delegation {
            status:InteractionStatus::Working, agent:Some(id),details,..
        } if id=="native-child-1234567890" && details=="Inspect parser")
        );
        assert!(parser.parse_codex(&event).is_empty());
        assert!(parser.output.is_empty());
    }

    #[test]
    fn codex_wait_keeps_separate_child_results_and_failure_states() {
        let mut parser = StreamParser::default();
        let result = cards(parser.parse_codex(&json!({"type":"item.completed","item":{
            "id":"wait","type":"collab_tool_call","tool":"wait", "status":"completed",
            "receiver_thread_ids":["finished","running","failed","interrupted"],
            "agents_states":{
                "finished":{"status":"completed","message":"The parser is correct 🌊"},
                "running":{"status":"running","message":null},
                "failed":{"status":"errored","message":"Quota reached"},
                "interrupted":{"status":"interrupted","message":null}
            }
        }})));
        assert_eq!(result.len(), 4);
        for (index, expected) in [
            InteractionStatus::Completed,
            InteractionStatus::Working,
            InteractionStatus::Failed,
            InteractionStatus::Cancelled,
        ]
        .into_iter()
        .enumerate()
        {
            assert!(
                matches!(&result[index],ChatInteraction::Delegation {status,..} if *status==expected)
            );
        }
        assert!(
            matches!(&result[0],ChatInteraction::Delegation {details,..} if details=="The parser is correct 🌊")
        );
        assert!(
            !parser.failed,
            "A child failure is not the parent turn's failure"
        );
        assert!(parser.rendered_output().contains("Quota reached"));
    }

    #[test]
    fn unknown_child_state_does_not_fabricate_completion_and_children_are_bounded() {
        let mut parser = StreamParser::default();
        let ids: Vec<_> = (0..128).map(|i| format!("child{i}")).collect();
        let result = cards(parser.parse_codex(&json!({"type":"item.completed","item":{
            "id":"wait","type":"collab_tool_call","tool":"wait", "status":"completed",
            "receiver_thread_ids":ids,"prompt":"🌊".repeat(8192)
        }})));
        assert_eq!(result.len(), CHILDREN_PER_EVENT);
        assert!(result.iter().all(|card| matches!(card,ChatInteraction::Delegation {status:InteractionStatus::Working, details,..} if details.len()<2200)));
        assert!(parser.interaction_bytes < 80 * 1024);
    }

    #[test]
    fn failed_codex_spawn_marker_matches_emitted_card() {
        let mut parser = StreamParser::default();
        let result = cards(parser.parse_codex(&json!({"type":"item.completed","item":{
            "id":"spawn","type":"collab_tool_call","tool":"spawn_agent", "status":"failed","prompt":"Task"
        }})));
        assert!(matches!(
            &result[0],
            ChatInteraction::Delegation {
                status: InteractionStatus::Failed,
                ..
            }
        ));
        assert!(parser.rendered_output().contains("\"status\":\"failed\""));
    }

    #[test]
    fn opencode_native_task_result_is_typed_and_background_is_not_finished() {
        let mut parser = StreamParser::default();
        let event = json!({"type":"tool_use","sessionID":"parent","part":{
            "id":"task1","tool":"task","state":{"status":"completed","input":{"subagent_type":"explore","prompt":"Find parser"},
                "metadata":{"sessionId":"ses_child","background":false},
                "output":"<task id=\"ses_child\" state=\"completed\">\n<task_result>\nFound src/parser.rs\n</task_result>\n</task>"}
        }});
        assert!(
            matches!(&cards(parser.parse_opencode(&event))[0], ChatInteraction::Delegation {
            agent:Some(id),details,status:InteractionStatus::Completed,..
        } if id=="ses_child" && details=="Found src/parser.rs")
        );
        let mut background = event;
        background["part"]["id"] = json!("task2");
        background["part"]["state"]["metadata"]["background"] = json!(true);
        assert!(matches!(
            &cards(parser.parse_opencode(&background))[0],
            ChatInteraction::Delegation {
                status: InteractionStatus::Working,
                ..
            }
        ));
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "");
    }

    #[test]
    fn claude_child_progress_and_result_do_not_pollute_main_text_or_fail_parent() {
        let mut parser = StreamParser::default();
        parser.parse_claude(&json!({"type":"assistant","message":{"id":"parent","content":[{"type":"tool_use","id":"agent-call","name":"Agent","input":{"description":"Inspect parser","prompt":"Task"}}]}}));
        let result = cards(parser.parse_claude(&json!({"type":"assistant","parent_tool_use_id":"agent-call","message":{"id":"child-message","content":[{"type":"text","text":"Found the bug 🌊"},{"type":"tool_use","name":"Read","input":{"file_path":"src/main.rs"}}]}})));
        assert!(
            matches!(&result[0],ChatInteraction::Delegation {details,agent:Some(id),status:InteractionStatus::Working,..} if details.contains("Found the bug") && details.contains("Read") && id=="agent-call")
        );
        let result = cards(parser.parse_claude(&json!({"type":"user","parent_tool_use_id":null,"message":{"content":[{"type":"tool_result","tool_use_id":"agent-call","is_error":true,"content":"Child permission denied"}]}})));
        assert!(
            matches!(&result[0],ChatInteraction::Delegation {status:InteractionStatus::Failed,details,..} if details=="Child permission denied")
        );
        assert!(!parser.failed);
        assert!(parser.output.is_empty());
        assert!(parser.child_tasks.is_empty());
    }

    #[test]
    fn claude_tracking_and_child_previews_have_fixed_budgets() {
        let mut parser = StreamParser::default();
        for i in 0..256 {
            parser.remember_claude_child(
                &json!({"type":"tool_use","id":format!("call-{i}"),"name":"Task","input":{}}),
            );
        }
        assert_eq!(parser.child_tasks.len(), TRACKED_CLAUDE_CHILDREN);
        let result = cards(parser.claude_child_event(&json!({"type":"assistant","message":{"content":[{"type":"text","text":"🌊".repeat(100_000)}]}}),"child"));
        assert!(
            matches!(&result[0],ChatInteraction::Delegation {details,..} if details.len()<CHILD_PREVIEW_BYTES+100 && details.contains("Preview shortened"))
        );
        assert!(parser.output.is_empty());
    }

    #[test]
    fn claude_background_launch_waits_for_native_task_notification() {
        let mut parser = StreamParser::default();
        parser.remember_claude_child(
            &json!({"id":"call-bg","name":"Agent","input":{"description":"Review parser"}}),
        );
        let result = cards(parser.parse_claude(&json!({"type":"user","parent_tool_use_id":null,
            "tool_use_result":{"isAsync":true,"status":"async_launched","agentId":"child-native-id"},
            "message":{"content":[{"type":"tool_result","tool_use_id":"call-bg","content":"Agent launched"}]}
        })));
        assert!(matches!(
            &result[0],
            ChatInteraction::Delegation {
                status: InteractionStatus::Working,
                ..
            }
        ));
        assert!(parser.child_tasks.contains_key("call-bg"));
        let progress = json!({"type":"system","subtype":"task_progress","tool_use_id":"call-bg","task_id":"child-native-id","description":"Reading parser"});
        assert!(
            matches!(&cards(parser.parse_claude(&progress))[0],ChatInteraction::Delegation {status:InteractionStatus::Working,details,..} if details=="Reading parser")
        );
        assert!(parser.parse_claude(&progress).is_empty());
        let result = cards(parser.parse_claude(&json!({"type":"system","subtype":"task_notification","tool_use_id":"call-bg","task_id":"child-native-id","status":"completed","summary":"Reviewed parser"})));
        assert!(
            matches!(&result[0],ChatInteraction::Delegation {status:InteractionStatus::Completed,details,..} if details=="Reviewed parser")
        );
        assert!(parser.child_tasks.is_empty());
        assert!(parser.parse_claude(&json!({"type":"system","subtype":"task_started","tool_use_id":"shell-call","task_type":"local_bash","description":"Shell job"})).is_empty());
    }

    #[test]
    fn claude_nested_child_result_tracks_its_actual_tool_identity() {
        let mut parser = StreamParser::default();
        parser.parse_claude(&json!({"type":"assistant","parent_tool_use_id":"first-child","message":{"content":[{"type":"tool_use","id":"nested-call","name":"Agent","input":{"description":"Nested task"}}]}}));
        assert!(parser.child_tasks.contains_key("nested-call"));
        let result = cards(parser.parse_claude(&json!({"type":"user","parent_tool_use_id":"first-child","message":{"content":[{"type":"tool_result","tool_use_id":"nested-call","content":"Nested result"}]}})));
        assert!(
            matches!(&result[0],ChatInteraction::Delegation {status:InteractionStatus::Completed,agent:Some(id),details,..} if id=="nested-call" && details=="Nested result")
        );
        assert!(parser.child_tasks.is_empty());
    }
}
