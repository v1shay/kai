//! Headless protocols verified against installed CLI help and official references:
//! geminicli.com/docs/cli/headless, opencode.ai/docs/cli,
//! antigravity.google/docs/cli/headless, docs.cursor.com/en/cli/reference/output-format,
//! github.com/xai-org/grok-build (Messages-compatible streaming protocol).
//! Unknown fields/events are ignored; diagnostics never become assistant prose.

use super::{RuntimeEvent, StreamParser, content_text_view};
use serde_json::Value;

/// Tool diagnostics are status, never agent prose. Keep the headline useful
/// without copying an entire stack trace or tool output into the status bar.
fn tool_failure_status(detail: Option<&str>) -> String {
    let detail = detail.unwrap_or("").lines().next().unwrap_or("").trim();
    if detail.is_empty() {
        "Tool needs attention".into()
    } else {
        let mut characters = detail.chars();
        let mut headline: String = characters.by_ref().take(180).collect();
        if characters.next().is_some() {
            headline.push('…');
        }
        format!("Tool needs attention · {headline}")
    }
}

impl StreamParser {
    fn protocol_failure(&mut self, message: &str) -> Vec<RuntimeEvent> {
        self.failed = true;
        vec![RuntimeEvent::Failed(message.to_owned())]
    }

    pub(super) fn parse_gemini(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        match value["type"].as_str().unwrap_or("") {
            "init" => {
                self.begin("gemini-response");
                let mut events = vec![RuntimeEvent::Status("Thinking".into())];
                if let Some(id) = value["session_id"].as_str() {
                    events.push(RuntimeEvent::SessionId(id.into()));
                }
                events
            }
            "message" if value["role"] == "assistant" => {
                let text = value["content"].as_str().unwrap_or("");
                if value["delta"].as_bool().unwrap_or(false) {
                    self.delta(text)
                } else {
                    self.snapshot(text)
                }
            }
            "tool_use" => vec![RuntimeEvent::Status(format!(
                "Using {}",
                value["tool_name"].as_str().unwrap_or("tool")
            ))],
            "tool_result" => vec![RuntimeEvent::Status(
                if value["status"] == "error" || !value["error"].is_null() {
                    tool_failure_status(value["error"]["message"].as_str())
                } else {
                    "Reading results".into()
                },
            )],
            "error" if value["severity"] == "warning" => vec![RuntimeEvent::Status(
                value["message"]
                    .as_str()
                    .unwrap_or("Agent needs attention")
                    .into(),
            )],
            "error" => self.protocol_failure(
                value["message"]
                    .as_str()
                    .unwrap_or("Gemini could not complete this turn"),
            ),
            "result" if value["status"] == "error" => self.protocol_failure(
                value["error"]["message"]
                    .as_str()
                    .unwrap_or("Gemini could not complete this turn"),
            ),
            _ => Vec::new(),
        }
    }

    pub(super) fn parse_opencode(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        let mut events = Vec::new();
        if let Some(id) = value["sessionID"]
            .as_str()
            .or_else(|| value["session_id"].as_str())
        {
            // Session IDs are cheap metadata, but emit only once per process.
            if !self.session_announced {
                self.session_announced = true;
                events.push(RuntimeEvent::SessionId(id.into()));
            }
        }
        match value["type"].as_str().unwrap_or("") {
            "text" => {
                let part = &value["part"];
                self.begin(part["id"].as_str().unwrap_or("opencode-response"));
                if let Some(text) = part["text"].as_str() {
                    events.extend(self.snapshot(text));
                }
            }
            "text_delta" => {
                self.begin(value["partID"].as_str().unwrap_or("opencode-response"));
                if let Some(text) = value["delta"].as_str() {
                    events.extend(self.delta(text));
                }
            }
            "step_start" | "reasoning" => events.push(RuntimeEvent::Status("Thinking".into())),
            "tool_use" => {
                let part = &value["part"];
                let name = part["tool"].as_str().unwrap_or("tool");
                let state = part["state"]["status"].as_str().unwrap_or("");
                if name == "task" {
                    events.extend(self.opencode_delegation(part));
                }
                events.push(RuntimeEvent::Status(if state == "completed" {
                    "Reading results".into()
                } else if state == "error" {
                    tool_failure_status(part["state"]["error"].as_str())
                } else {
                    format!("Using {name}")
                }));
            }
            "error" => events.extend(
                self.protocol_failure(
                    value["error"]["data"]["message"]
                        .as_str()
                        .or_else(|| value["error"]["message"].as_str())
                        .or_else(|| value["message"].as_str())
                        .unwrap_or("OpenCode could not complete this turn"),
                ),
            ),
            _ => {}
        }
        events
    }

    pub(super) fn parse_antigravity(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        match value["event"].as_str().unwrap_or("") {
            "init" => {
                let mut events = vec![RuntimeEvent::Status("Thinking".into())];
                if let Some(id) = value["conversation_id"].as_str() {
                    events.push(RuntimeEvent::SessionId(id.into()));
                }
                events
            }
            "step_update" => {
                let step = &value["step_update"];
                match step["step_type"].as_str().unwrap_or("") {
                    "agent_response" => {
                        self.begin(&format!("agy-step-{}", step["step_index"]));
                        step["text_delta"]
                            .as_str()
                            .map(|text| self.delta(text))
                            .unwrap_or_default()
                    }
                    "tool" => vec![RuntimeEvent::Status(if step["state"] == "DONE" {
                        "Reading results".into()
                    } else {
                        format!("Using {}", step["tool_name"].as_str().unwrap_or("tool"))
                    })],
                    "user_input" | "checkpoint" => vec![RuntimeEvent::Status("Thinking".into())],
                    _ => Vec::new(),
                }
            }
            "result" => {
                let result = &value["result"];
                if result["status"] != "SUCCESS" {
                    let fallback = match result["status"].as_str() {
                        Some("CANCELED") => "Antigravity canceled this turn",
                        Some("INTERRUPTED") => "Antigravity was interrupted",
                        Some("WAITING") => "Antigravity needs input; open its CLI and retry",
                        Some("RUNNING") => "Antigravity ended before this turn completed",
                        Some("INVALID") => "Antigravity reached an invalid state",
                        _ => "Antigravity could not complete this turn",
                    };
                    return self.protocol_failure(result["error"].as_str().unwrap_or(fallback));
                }
                self.complete_response(result["response"].as_str().unwrap_or(""))
            }
            _ => Vec::new(),
        }
    }

    fn complete_response(&mut self, text: &str) -> Vec<RuntimeEvent> {
        if text.is_empty()
            || (self.current_validated
                && self.output.len() == text.len()
                && self.matches_at(0, text))
        {
            return Vec::new();
        }
        // The terminal result is an aggregate of all assistant text in this turn.
        // Replace rather than append when a CLI rewrites its partial response.
        self.current_start = 0;
        self.current_text_len = self.output.len();
        self.snapshot(text)
    }

    pub(super) fn parse_cursor(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        match value["type"].as_str().unwrap_or("") {
            "system" if value["subtype"] == "init" => self.parse_claude(value),
            "assistant" => {
                // With --stream-partial-output only timestamped events without
                // model_call_id carry new text. Pre-tool buffered flushes and
                // the final untimestamped flush repeat text already streamed.
                // The terminal result is authoritative even if an older CLI
                // emits only buffered events, so it remains a safe fallback.
                if value["timestamp_ms"].is_null() || !value["model_call_id"].is_null() {
                    return Vec::new();
                }
                self.begin("cursor-response");
                let text = content_text_view(&value["message"]["content"]);
                if text.is_empty() {
                    Vec::new()
                } else {
                    self.delta(&text)
                }
            }
            "tool_call" => {
                let name = value["tool_call"]
                    .as_object()
                    .and_then(|tools| tools.keys().next())
                    .map(String::as_str)
                    .unwrap_or("tool")
                    .trim_end_matches("ToolCall");
                let result = value["tool_call"]
                    .as_object()
                    .and_then(|tools| tools.values().next())
                    .map(|tool| &tool["result"]);
                vec![RuntimeEvent::Status(
                    if let Some(result) = result
                        && !result["error"].is_null()
                    {
                        tool_failure_status(
                            result["error"]["message"]
                                .as_str()
                                .or_else(|| result["error"].as_str()),
                        )
                    } else if value["subtype"] == "completed" {
                        "Reading results".into()
                    } else {
                        format!("Using {name}")
                    },
                )]
            }
            "result" if value["is_error"].as_bool().unwrap_or(false) => self.protocol_failure(
                value["result"]
                    .as_str()
                    .unwrap_or("Cursor could not complete this turn"),
            ),
            "result" => self.complete_response(value["result"].as_str().unwrap_or("")),
            "error" => self.protocol_failure(
                value["message"]
                    .as_str()
                    .unwrap_or("Cursor could not complete this turn"),
            ),
            _ => Vec::new(),
        }
    }

    pub(super) fn parse_grok(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        if !value["parent_tool_use_id"].is_null() {
            return Vec::new();
        }
        if value["type"] == "error" {
            return self.protocol_failure(
                value["message"]
                    .as_str()
                    .or_else(|| value["error"]["message"].as_str())
                    .unwrap_or("Grok Build could not complete this turn"),
            );
        }
        let mut events = self.parse_claude(value);
        for event in &mut events {
            if let RuntimeEvent::Failed(message) = event
                && message == "Claude could not complete this turn"
            {
                *message = "Grok Build could not complete this turn".into();
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn commands_use_native_resume_and_permissions_without_bypass_flags() {
        use super::super::{RunRequest, command_for, prompt_input};
        use crate::discovery::AgentKind;
        for (kind, read_mode, edit_mode, resume_flag) in [
            (AgentKind::Gemini, "plan", "auto_edit", "--resume"),
            (AgentKind::OpenCode, "plan", "build", "--session"),
            (
                AgentKind::Antigravity,
                "plan",
                "accept-edits",
                "--conversation",
            ),
            (AgentKind::Cursor, "plan", "", "--resume"),
            (AgentKind::Grok, "plan", "acceptEdits", "--resume"),
        ] {
            let mut request = RunRequest { reasoning_effort:None,
                agent: kind,
                executable: kind.command().into(),
                project: "/tmp".into(),
                prompt: "A prompt with \"quotes\" and Unicode 🌊".into(),
                resume_id: Some("native-session-id".into()),
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                codex_access: Default::default(),
                trust_workspace: false,
                fork_session: false,
            };
            for (edits, mode) in [(false, read_mode), (true, edit_mode)] {
                request.allow_edits = edits;
                let command = command_for(&request).unwrap();
                let args: Vec<_> = command
                    .get_args()
                    .map(|arg| arg.to_str().unwrap())
                    .collect();
                if !mode.is_empty() {
                    assert!(args.contains(&mode), "{kind:?}: {args:?}");
                }
                assert!(
                    args.windows(2)
                        .any(|args| args == [resume_flag, "native-session-id"])
                );
                assert!(!args.iter().any(|arg| {
                    [
                        "--yolo",
                        "--force",
                        "--auto",
                        "--trust",
                        "--skip-trust",
                        "--always-approve",
                        "--dangerously-skip-permissions",
                        "bypassPermissions",
                    ]
                    .contains(arg)
                }));
                assert!(
                    !args.contains(&request.prompt.as_str()),
                    "Prompt should stay in stdin"
                );
            }
            request.resume_id = Some("--malicious-flag".into());
            assert!(command_for(&request).is_err());
        }
        let prompt = "first\nsecond\n🌊".to_owned();
        let input = prompt_input(AgentKind::Antigravity, prompt.clone());
        let value: Value = serde_json::from_str(input.trim()).unwrap();
        assert_eq!(value["event"], "user");
        assert_eq!(value["message"]["content"], prompt);
    }

    #[test]
    fn all_native_model_overrides_are_single_values_and_preserve_resume() {
        use super::super::{RunRequest, command_for};
        use crate::discovery::AgentKind;
        for kind in [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::Gemini,
            AgentKind::OpenCode,
            AgentKind::Antigravity,
            AgentKind::Cursor,
            AgentKind::Grok,
        ] {
            let mut request = RunRequest { reasoning_effort:None,
                agent: kind,
                executable: kind.command().into(),
                project: "/tmp".into(),
                prompt: "fixture".into(),
                resume_id: Some("native-id".into()),
                model: Some("provider/model[effort=high,fast=false]".into()),
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                codex_access: Default::default(),
                trust_workspace: false,
                fork_session: false,
            };
            let command = command_for(&request).unwrap();
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect();
            assert!(
                args.windows(2)
                    .any(|pair| pair == ["--model", request.model.as_deref().unwrap()])
            );
            assert!(args.contains(&"native-id"));
            assert!(args.contains(&"plan") || args.contains(&"read-only"));
            if kind == AgentKind::Codex {
                assert!(
                    args.iter().position(|arg| *arg == "--model")
                        < args.iter().position(|arg| *arg == "resume")
                );
            }
            for bad in [
                "",
                "--yolo",
                " model",
                "model\n--force",
                "model\0",
                "$(whoami)",
                "model;cmd",
                "a b",
            ] {
                request.model = Some(bad.into());
                assert!(command_for(&request).is_err(), "{kind:?} accepted {bad:?}");
            }
            request.model = Some("m".repeat(257));
            assert!(command_for(&request).is_err());
        }
    }

    #[test]
    fn native_images_preserve_paths_resume_and_permissions_or_fail_before_spawn() {
        use super::super::{RunRequest, command_for};
        use crate::discovery::AgentKind;
        let path = std::env::temp_dir().join(format!("orbit image {}.png", uuid::Uuid::new_v4()));
        std::fs::write(&path, b"\x89PNG\r\n\x1a\nfixture").unwrap();
        for (kind, flag) in [
            (AgentKind::Codex, "--image"),
            (AgentKind::OpenCode, "--file"),
            (AgentKind::Claude, "--input-format"),
        ] {
            let mut request = RunRequest { reasoning_effort:None,
                agent: kind,
                executable: kind.command().into(),
                project: std::env::temp_dir(),
                prompt: "fixture".into(),
                resume_id: Some("native-id".into()),
                model: None,
                images: vec![path.clone()],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                codex_access: Default::default(),
                trust_workspace: false,
                fork_session: false,
            };
            let command = command_for(&request).unwrap();
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect();
            assert!(args.contains(&"native-id"));
            assert!(args.contains(&"plan") || args.contains(&"read-only"));
            assert!(args.contains(&flag));
            if kind != AgentKind::Claude {
                assert!(
                    args.windows(2)
                        .any(|pair| pair == [flag, path.to_str().unwrap()])
                );
            } else {
                assert!(args.contains(&"stream-json"));
            }
            request.images = vec![path.clone(); 5];
            assert!(command_for(&request).is_err());
        }
        for kind in [AgentKind::Antigravity, AgentKind::Cursor, AgentKind::Grok] {
            let request = RunRequest { reasoning_effort:None,
                agent: kind,
                executable: "must-not-be-started".into(),
                project: std::env::temp_dir(),
                prompt: "fixture".into(),
                resume_id: None,
                model: None,
                images: vec![path.clone()],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                codex_access: Default::default(),
                trust_workspace: false,
                fork_session: false,
            };
            let error = super::super::launch(request).err().unwrap().to_string();
            assert!(error.contains("supported native image input"));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn workspace_trust_requires_explicit_confirmation_and_keeps_plan_mode() {
        use super::super::{RunRequest, command_for};
        use crate::discovery::AgentKind;
        for (agent, trust_flag) in [
            (AgentKind::Gemini, "--skip-trust"),
            (AgentKind::Cursor, "--trust"),
        ] {
            let mut request = RunRequest { reasoning_effort:None,
                agent,
                executable: agent.command().into(),
                project: "/tmp".into(),
                prompt: "fixture".into(),
                resume_id: None,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                codex_access: Default::default(),
                trust_workspace: false,
                fork_session: false,
            };
            let before = command_for(&request).unwrap();
            assert!(!before.get_args().any(|arg| arg == trust_flag));
            request.trust_workspace = true;
            let after = command_for(&request).unwrap();
            let args: Vec<_> = after.get_args().map(|arg| arg.to_str().unwrap()).collect();
            assert!(args.contains(&trust_flag));
            assert!(args.contains(&"plan"));
            assert!(
                !args
                    .iter()
                    .any(|arg| ["--force", "--yolo", "--always-approve"].contains(arg))
            );
        }
    }

    #[test]
    fn claude_image_input_is_real_json_with_chunked_base64_and_escaped_text() {
        use super::super::{base64_chunk, write_agent_input};
        use crate::discovery::AgentKind;
        for (input, expected) in [
            (b"".as_slice(), ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
        ] {
            let mut output = [0_u8; 16];
            let count = base64_chunk(input, &mut output);
            assert_eq!(&output[..count], expected.as_bytes());
        }
        let path =
            std::env::temp_dir().join(format!("orbit-image-wire-{}.png", uuid::Uuid::new_v4()));
        // Cross several complete input chunks and exercise final padding.
        let bytes = vec![255_u8; 3 * 8192 * 3 + 1];
        std::fs::write(&path, &bytes).unwrap();
        let prompt = "A \"quoted\" line\n🌊";
        let mut output = Vec::new();
        write_agent_input(
            AgentKind::Claude,
            prompt,
            std::slice::from_ref(&path),
            &mut output,
        )
        .unwrap();
        let value: Value = serde_json::from_slice(&output).unwrap();
        assert_eq!(value["type"], "user");
        assert_eq!(value["message"]["role"], "user");
        assert_eq!(value["message"]["content"][0]["text"], prompt);
        let source = &value["message"]["content"][1]["source"];
        assert_eq!(source["media_type"], "image/png");
        assert_eq!(source["type"], "base64");
        let encoded = source["data"].as_str().unwrap();
        assert_eq!(encoded.len(), bytes.len().div_ceil(3) * 4);
        assert!(encoded.ends_with("/w=="));
        assert!(encoded[..encoded.len() - 4].bytes().all(|b| b == b'/'));
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn typed_native_cards_deduplicate_and_survive_corrected_snapshots() {
        use crate::chat_interactions::ChatInteraction;
        let mut parser = StreamParser::default();
        parser.parse_codex(&serde_json::json!({"type":"item.updated","item":{"id":"reply","type":"agent_message","text":"Partial 🌊"}}));
        let delegation = serde_json::json!({"type":"item.started","item":{"id":"spawn1","type":"collab_tool_call","tool":"spawn_agent","prompt":"Review parser","receiver_thread_ids":["agent1"],"status":"in_progress"}});
        assert!(matches!(
            parser.parse_codex(&delegation).as_slice(),
            [RuntimeEvent::Interaction(card)] if matches!(card.as_ref(), ChatInteraction::Delegation { .. })
        ));
        assert!(parser.parse_codex(&delegation).is_empty());
        let events = parser.parse_codex(&serde_json::json!({"type":"item.completed","item":{"id":"reply","type":"agent_message","text":"Corrected 😀"}}));
        let RuntimeEvent::ReplaceText(text) = &events[0] else {
            panic!("Expected corrected snapshot");
        };
        assert!(text.contains("Corrected 😀"));
        assert!(text.contains("<orbit_interaction version=\"1\">"));
        assert!(text.contains("Review parser"));
        assert!(!text.contains("Partial"));
        assert!(parser.interaction_bytes > 0);
        assert_eq!(parser.current_text(), "Corrected 😀");
        let completion =
            json!({"type":"turn.completed","usage":{"input_tokens":8,"output_tokens":3}});
        assert!(matches!(
            parser.parse_codex(&completion).as_slice(),
            [RuntimeEvent::Interaction(card)] if matches!(card.as_ref(), ChatInteraction::Status {
                status: crate::chat_interactions::InteractionStatus::Completed,
                ..
            })
        ));
        assert!(parser.parse_codex(&completion).is_empty());
    }

    #[test]
    fn historical_native_questions_import_as_multiple_readonly_cards() {
        use super::super::transcript_record;
        use crate::{
            chat_interactions::{self, ChatInteraction},
            discovery::AgentKind,
        };
        let input = serde_json::json!({"questions":[{"id":"one","question":"First?"},{"id":"two","question":"Second?"}]});
        let value = serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"request_user_input_async","arguments":input.to_string()}});
        let messages = transcript_record(&value, AgentKind::Codex);
        assert_eq!(messages.len(), 2);
        for message in messages {
            assert_eq!(message.role, "tool");
            let ChatInteraction::Question(card) = chat_interactions::decode(&message.text).unwrap()
            else {
                panic!("Expected question card");
            };
            assert!(card.historical);
        }
        let status = serde_json::json!({"type":"event_msg","payload":{"type":"task_complete"}});
        assert!(matches!(
            chat_interactions::decode(&transcript_record(&status, AgentKind::Codex)[0].text),
            Some(ChatInteraction::Status { .. })
        ));
    }

    #[test]
    fn codex_agent_message_authors_are_strict_and_encrypted_parts_are_private() {
        use super::super::transcript_record;
        use crate::discovery::AgentKind;
        let mut value = json!({"type":"response_item","payload":{
            "type":"agent_message","author":"assistant",
            "content":[{"type":"input_text","text":"Readable reply"},
                {"type":"encrypted_content","text":"Private opaque data"}]
        }});
        let messages = transcript_record(&value, AgentKind::Codex);
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(messages[0].text, "Readable reply");
        value["payload"]["author"] = json!("system");
        assert!(transcript_record(&value, AgentKind::Codex).is_empty());
        value["payload"]["author"] = json!({"role":"assistant"});
        assert!(transcript_record(&value, AgentKind::Codex).is_empty());
    }

    #[test]
    fn grok_messages_wire_protocol_uses_same_delta_reconciliation() {
        let mut parser = StreamParser::default();
        assert!(
            parser
                .parse_claude(&json!({"type":"system","subtype":"init","session_id":"grok1"}))
                .contains(&RuntimeEvent::SessionId("grok1".into()))
        );
        parser.parse_claude(
            &json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"g1"}}}),
        );
        parser.parse_claude(&json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Answer"}}}));
        assert!(parser.parse_claude(&json!({"type":"assistant","message":{"id":"g1","content":[{"type":"text","text":"Answer"}]}})).is_empty());
        assert!(
            parser
                .parse_claude(
                    &json!({"type":"result","subtype":"success","is_error":false,"result":"Answer"})
                )
                .is_empty()
        );
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Answer");
    }

    #[test]
    fn gemini_native_records_support_untyped_text_parts_and_tools() {
        use super::super::transcript_record;
        use crate::discovery::AgentKind;
        let messages = transcript_record(
            &json!({"type":"gemini","content":[{"text":"Native answer"}],"toolCalls":[{"name":"read_file","args":{"path":"README.md"}}]}),
            AgentKind::Gemini,
        );
        assert_eq!(messages[0].text, "Native answer");
        assert_eq!(messages[0].role, "assistant");
        assert_eq!(messages[1].role, "tool");
        assert!(
            transcript_record(
                &json!({"type":"info","content":"system metadata"}),
                AgentKind::Gemini
            )
            .is_empty()
        );
    }

    #[test]
    fn opencode_plain_permission_notice_is_status_never_chat_text() {
        use super::super::dispatch_line;
        use crate::discovery::AgentKind;
        let (tx, rx) = std::sync::mpsc::sync_channel(4);
        let mut parser = StreamParser::default();
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        dispatch_line(
            &mut parser,
            AgentKind::OpenCode,
            b"! permission requested: edit (/tmp/file); auto-rejecting",
            &tx,
            &cancelled,
        )
        .unwrap();
        assert_eq!(
            rx.recv().unwrap(),
            RuntimeEvent::Status("Permission denied · configure OpenCode and retry".into())
        );
        assert!(parser.output.is_empty());
        assert!(!parser.failed);
        assert!(
            dispatch_line(
                &mut parser,
                AgentKind::OpenCode,
                b"unrecognized garbage",
                &tx,
                &cancelled
            )
            .is_err()
        );
    }

    #[test]
    fn gemini_stream_filters_user_and_warning_and_reports_failure() {
        let mut parser = StreamParser::default();
        assert!(
            parser
                .parse_gemini(&json!({"type":"init","session_id":"g1"}))
                .contains(&RuntimeEvent::SessionId("g1".into()))
        );
        assert!(
            parser
                .parse_gemini(&json!({"type":"message","role":"user","content":"secret prompt"}))
                .is_empty()
        );
        for text in ["Hello", " world"] {
            parser.parse_gemini(
                &json!({"type":"message","role":"assistant","content":text,"delta":true}),
            );
        }
        parser.parse_gemini(&json!({"type":"error","severity":"warning","message":"Retrying"}));
        assert!(!parser.failed);
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Hello world");
        assert_eq!(
            parser.parse_gemini(&json!({"type":"tool_use","tool_name":"read_file"})),
            vec![RuntimeEvent::Status("Using read_file".into())]
        );
        parser.parse_gemini(
            &json!({"type":"result","status":"error","error":{"message":"Not authenticated"}}),
        );
        assert!(parser.failed);
    }

    #[test]
    fn opencode_snapshots_and_session_id_do_not_duplicate() {
        let mut parser = StreamParser::default();
        for text in ["Hello", "Hello world", "Hello world"] {
            parser.parse_opencode(
                &json!({"type":"text","sessionID":"oc1","part":{"id":"p1","text":text}}),
            );
        }
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Hello world");
        assert!(
            parser
                .parse_opencode(&json!({"type":"step_finish","sessionID":"oc1"}))
                .is_empty()
        );
        assert_eq!(
            parser.parse_opencode(
                &json!({"type":"tool_use","part":{"tool":"edit","state":{"status":"running"}}})
            ),
            vec![RuntimeEvent::Status("Using edit".into())]
        );
    }

    #[test]
    fn antigravity_deltas_and_final_result_reconcile_without_duplicates() {
        let mut parser = StreamParser::default();
        assert!(
            parser
                .parse_antigravity(&json!({"event":"init","conversation_id":"agy1"}))
                .contains(&RuntimeEvent::SessionId("agy1".into()))
        );
        for (state, text) in [("ACTIVE", "Hello"), ("DONE", " world")] {
            parser.parse_antigravity(&json!({"event":"step_update","step_update":{"step_type":"agent_response","step_index":2,"state":state,"text_delta":text}}));
        }
        assert!(parser.parse_antigravity(&json!({"event":"result","result":{"status":"SUCCESS","response":"Hello world"}})).is_empty());
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Hello world");
        assert_eq!(
            parser.parse_antigravity(
                &json!({"event":"result","result":{"status":"ERROR","error":"Permission denied"}})
            ),
            vec![RuntimeEvent::Failed("Permission denied".into())]
        );
    }

    #[test]
    fn cursor_buffered_flushes_never_duplicate_partial_output_before_tools() {
        let mut parser = StreamParser::default();
        let assistant = |text: &str, timestamp, model_call_id: Value| {
            json!({"type":"assistant","timestamp_ms":timestamp,"model_call_id":model_call_id,
                "message":{"role":"assistant","content":[{"type":"text","text":text}]}})
        };
        for text in ["Before", " tool. 🌊"] {
            parser.parse_cursor(&assistant(text, json!(12), Value::Null));
        }
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Before tool. 🌊");
        assert!(
            parser
                .parse_cursor(&assistant("Before tool. 🌊", json!(12), json!("model-1")))
                .is_empty()
        );
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Before tool. 🌊");
        parser.parse_cursor(&json!({"type":"tool_call","subtype":"completed",
            "tool_call":{"readToolCall":{"result":{"success":{"content":"file"}}}}}));
        parser.parse_cursor(&assistant("After tool.", json!(13), Value::Null));
        assert!(
            parser
                .parse_cursor(&assistant(
                    "Before tool. 🌊After tool.",
                    Value::Null,
                    Value::Null
                ))
                .is_empty()
        );
        assert_eq!(
            parser.output.tail(1024 * 1024).unwrap(),
            "Before tool. 🌊After tool."
        );
        assert!(
            parser
                .parse_cursor(&json!({"type":"result","subtype":"success",
                "result":"Before tool. 🌊After tool."}))
                .is_empty()
        );
    }

    #[test]
    fn cursor_terminal_result_recovers_buffered_only_streams_and_suffix_corrections() {
        let mut parser = StreamParser::default();
        assert!(
            parser
                .parse_cursor(&json!({"type":"assistant","message":{"content":
            [{"type":"text","text":"Final answer"}]}}))
                .is_empty()
        );
        parser.parse_cursor(&json!({"type":"result","result":"Prefix Final answer"}));
        assert_eq!(
            parser.output.tail(1024 * 1024).unwrap(),
            "Prefix Final answer"
        );
        assert_eq!(
            parser.parse_cursor(&json!({"type":"result","result":"Final answer"})),
            vec![RuntimeEvent::ReplaceText("Final answer".into())]
        );
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Final answer");
    }

    #[test]
    fn tool_errors_surface_a_bounded_diagnostic_without_becoming_assistant_text() {
        let mut gemini = StreamParser::default();
        // Gemini can emit a success status plus an error payload for some
        // scheduler stop states. The explicit error remains authoritative.
        assert_eq!(
            gemini.parse_gemini(&json!({"type":"tool_result","status":"success",
            "error":{"type":"permission_denied","message":"Permission denied\nstack trace"}})),
            vec![RuntimeEvent::Status(
                "Tool needs attention · Permission denied".into()
            )]
        );
        assert!(!gemini.failed);
        assert!(gemini.output.is_empty());
        let mut cursor = StreamParser::default();
        assert_eq!(
            cursor.parse_cursor(&json!({"type":"tool_call","subtype":"completed",
            "tool_call":{"writeToolCall":{"result":{"error":{"message":"Access denied"}}}}})),
            vec![RuntimeEvent::Status(
                "Tool needs attention · Access denied".into()
            )]
        );
        let mut opencode = StreamParser::default();
        assert_eq!(
            opencode.parse_opencode(&json!({"type":"tool_use",
            "part":{"tool":"edit","state":{"status":"error","error":"Patch failed"}}})),
            vec![RuntimeEvent::Status(
                "Tool needs attention · Patch failed".into()
            )]
        );
        assert!(opencode.output.is_empty());
        assert!(
            tool_failure_status(Some(&"🌊".repeat(10_000)))
                .chars()
                .count()
                < 220
        );
    }

    #[test]
    fn grok_standalone_errors_and_antigravity_terminal_states_are_truthful() {
        let mut grok = StreamParser::default();
        assert!(
            grok.parse_grok(&json!({"type":"error","parent_tool_use_id":"subagent-tool",
            "message":"Child needs a retry"}))
                .is_empty()
        );
        assert!(!grok.failed);
        assert_eq!(
            grok.parse_grok(&json!({"type":"error","message":"Couldn't start session"})),
            vec![RuntimeEvent::Failed("Couldn't start session".into())]
        );
        assert!(grok.failed);
        assert!(grok.output.is_empty());
        let mut grok = StreamParser::default();
        assert_eq!(
            grok.parse_grok(&json!({"type":"result","is_error":true})),
            vec![RuntimeEvent::Failed(
                "Grok Build could not complete this turn".into()
            )]
        );
        for (status, expected) in [
            ("CANCELED", "Antigravity canceled this turn"),
            ("INTERRUPTED", "Antigravity was interrupted"),
            ("WAITING", "Antigravity needs input; open its CLI and retry"),
        ] {
            let mut parser = StreamParser::default();
            assert_eq!(
                parser.parse_antigravity(&json!({"event":"result","result":{"status":status}})),
                vec![RuntimeEvent::Failed(expected.into())]
            );
            assert!(parser.failed);
        }
    }

    #[test]
    fn cursor_delta_chunks_are_concatenated_and_result_reconciled() {
        let mut parser = StreamParser::default();
        for text in ["Hello", " world"] {
            parser.parse_cursor(
                &json!({"type":"assistant","timestamp_ms":12,"message":{"content":[{"type":"text","text":text}]}}),
            );
        }
        assert!(
            parser
                .parse_cursor(&json!({"type":"result","result":"Hello world"}))
                .is_empty()
        );
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Hello world");
        assert_eq!(parser.parse_cursor(&json!({"type":"tool_call","subtype":"started","tool_call":{"readToolCall":{"args":{"path":"README.md"}}}})),vec![RuntimeEvent::Status("Using read".into())]);
        assert_eq!(
            parser.parse_cursor(&json!({"type":"result","result":"Rewritten answer"})),
            vec![RuntimeEvent::ReplaceText("Rewritten answer".into())]
        );
    }
}
