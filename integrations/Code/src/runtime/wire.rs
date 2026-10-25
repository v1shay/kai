//! Large JSONL records spill privately to disk; only a bounded protocol view is
//! decoded. Ordinary records retain the existing in-memory serde fast path.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    sync::atomic::{AtomicBool, Ordering},
};

pub(super) const MEMORY_BYTES: usize = 1024 * 1024;
const METADATA_BYTES: usize = 256 * 1024;
const NODES: usize = 8192;

pub(super) struct Record {
    memory: Vec<u8>,
    spill: Option<File>,
}
impl Default for Record {
    fn default() -> Self {
        Self {
            memory: Vec::with_capacity(8192),
            spill: None,
        }
    }
}
impl Record {
    pub(super) fn is_empty(&self) -> bool {
        self.spill.is_none() && self.memory.is_empty()
    }
    pub(super) fn small(&self) -> Option<&[u8]> {
        self.spill.is_none().then_some(&self.memory)
    }
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<()> {
        if self.spill.is_none() && self.memory.len() + bytes.len() > MEMORY_BYTES {
            let path =
                std::env::temp_dir().join(format!("code-agent-event-{}", uuid::Uuid::new_v4()));
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&path)
                .context("Could not buffer a large agent event")?;
            // No named artifact survives cancellation, normal exit, or a crash.
            std::fs::remove_file(&path).context("Could not make the agent buffer private")?;
            file.write_all(&self.memory)?;
            self.memory = Vec::with_capacity(8192);
            self.spill = Some(file);
        }
        if let Some(file) = &mut self.spill {
            file.write_all(bytes)
                .context("Could not buffer a large agent event")?;
        } else {
            self.memory.extend_from_slice(bytes);
        }
        Ok(())
    }
    pub(super) fn projected(&mut self, cancelled: &AtomicBool) -> Result<(Value, bool)> {
        let file = self
            .spill
            .as_mut()
            .context("No large agent event to decode")?;
        file.seek(SeekFrom::Start(0))?;
        project(BufReader::with_capacity(64 * 1024, file), cancelled)
    }
    pub(super) fn clear(&mut self) {
        self.memory.clear();
        self.spill = None;
    }
}

fn project(reader: impl BufRead, cancelled: &AtomicBool) -> Result<(Value, bool)> {
    let mut scan = Projection {
        reader,
        cancelled,
        text_left: super::LIVE_OUTPUT_LIMIT,
        metadata_left: METADATA_BYTES,
        nodes_left: NODES,
        shallow_nodes_left: 256,
        shortened: false,
    };
    let value = scan.value(true, false, false, 0)?;
    scan.whitespace()?;
    ensure!(
        scan.peek()?.is_none(),
        "Unexpected data after agent JSON event"
    );
    Ok((value, scan.shortened))
}

struct Projection<'a, R> {
    reader: R,
    cancelled: &'a AtomicBool,
    text_left: usize,
    metadata_left: usize,
    nodes_left: usize,
    // Bulk arrays cannot consume the final shallow identity/status fields.
    shallow_nodes_left: usize,
    shortened: bool,
}
impl<R: BufRead> Projection<'_, R> {
    fn has_nodes(&self, depth: usize) -> bool {
        self.nodes_left > 0 || (depth <= 4 && self.shallow_nodes_left > 0)
    }
    fn peek(&mut self) -> Result<Option<u8>> {
        ensure!(!self.cancelled.load(Ordering::Acquire), "Stopped");
        Ok(self.reader.fill_buf()?.first().copied())
    }
    fn byte(&mut self) -> Result<u8> {
        let byte = self.peek()?.context("Unfinished agent JSON event")?;
        self.reader.consume(1);
        Ok(byte)
    }
    fn expect(&mut self, expected: u8) -> Result<()> {
        ensure!(self.byte()? == expected, "Malformed agent JSON event");
        Ok(())
    }
    fn whitespace(&mut self) -> Result<()> {
        while self
            .peek()?
            .is_some_and(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        {
            self.reader.consume(1);
        }
        Ok(())
    }
    /// Retain a bounded encoded prefix, consuming the entire string in buffered
    /// spans. Escape tokens (including surrogate pairs) are never split.
    fn string(&mut self, cap: usize) -> Result<(String, bool)> {
        self.expect(b'"')?;
        let mut raw = Vec::with_capacity(cap.min(8192) + 2);
        raw.push(b'"');
        let mut shortened = false;
        loop {
            match self.peek()?.context("Unfinished agent JSON string")? {
                b'"' => {
                    self.reader.consume(1);
                    break;
                }
                b'\\' => {
                    let mut token = [0; 12];
                    token[0] = self.byte()?;
                    token[1] = self.byte()?;
                    let mut len = 2;
                    if token[1] == b'u' {
                        for byte in &mut token[2..6] {
                            *byte = self.byte()?;
                        }
                        len = 6;
                        let high = hex(&token[2..6])?;
                        if (0xd800..=0xdbff).contains(&high) {
                            self.expect(b'\\')?;
                            self.expect(b'u')?;
                            token[6..8].copy_from_slice(b"\\u");
                            for byte in &mut token[8..12] {
                                *byte = self.byte()?;
                            }
                            ensure!(
                                (0xdc00..=0xdfff).contains(&hex(&token[8..12])?),
                                "Invalid JSON surrogate pair"
                            );
                            len = 12;
                        } else {
                            ensure!(
                                !(0xdc00..=0xdfff).contains(&high),
                                "Invalid JSON surrogate pair"
                            );
                        }
                    } else {
                        ensure!(
                            b"\"\\/bfnrt".contains(&token[1]),
                            "Invalid agent JSON escape"
                        );
                    }
                    if !shortened && raw.len() - 1 + len <= cap {
                        raw.extend_from_slice(&token[..len]);
                    } else {
                        shortened = true;
                    }
                }
                0..=31 => bail!("Invalid agent JSON string control character"),
                _ => {
                    let bytes = self.reader.fill_buf()?;
                    let len = bytes
                        .iter()
                        .position(|b| *b == b'"' || *b == b'\\' || *b < 32)
                        .unwrap_or(bytes.len());
                    let take = if shortened {
                        0
                    } else {
                        len.min(cap.saturating_sub(raw.len() - 1))
                    };
                    raw.extend_from_slice(&bytes[..take]);
                    shortened |= take < len;
                    self.reader.consume(len);
                }
            }
        }
        // A byte budget may end inside the final raw UTF-8 character.
        if shortened && let Err(error) = std::str::from_utf8(&raw[1..]) {
            ensure!(error.error_len().is_none(), "Invalid agent JSON UTF-8");
            raw.truncate(1 + error.valid_up_to());
        }
        raw.push(b'"');
        Ok((serde_json::from_slice(&raw)?, shortened))
    }
    fn value(&mut self, keep: bool, text: bool, dynamic: bool, depth: usize) -> Result<Value> {
        ensure!(depth <= 128, "Agent JSON nesting exceeds 128 levels");
        self.whitespace()?;
        let keep = keep && self.has_nodes(depth);
        if keep {
            if self.nodes_left > 0 {
                self.nodes_left -= 1;
            } else {
                self.shallow_nodes_left -= 1;
            }
        }
        match self.peek()?.context("Unfinished agent JSON event")? {
            b'{' => {
                self.reader.consume(1);
                let mut map = Map::new();
                self.whitespace()?;
                if self.peek()? == Some(b'}') {
                    self.reader.consume(1);
                    return Ok(Value::Object(map));
                }
                loop {
                    let (key, cut) = self.string(if keep { 256 } else { 0 })?;
                    self.whitespace()?;
                    self.expect(b':')?;
                    // Unknown/binary/tool-output fields are consumed, never
                    // deserialized into large Values. Dynamic tool/model keys
                    // are needed by Cursor and provider usage reports.
                    let selected = keep
                        && !cut
                        && map.len() < 256
                        && self.has_nodes(depth + 1)
                        && (dynamic || relevant(&key));
                    self.shortened |= keep && !selected && (dynamic || relevant(&key));
                    let value = self.value(
                        selected,
                        is_text(&key),
                        matches!(
                            key.as_str(),
                            "tool_call"
                                | "modelUsage"
                                | "agents_states"
                                | "agentsStates"
                                | "rate_limits_by_limit_id"
                        ),
                        depth + 1,
                    )?;
                    if selected {
                        map.insert(key, value);
                    }
                    self.whitespace()?;
                    match self.byte()? {
                        b'}' => break,
                        b',' => self.whitespace()?,
                        _ => bail!("Malformed agent JSON object"),
                    }
                }
                Ok(Value::Object(map))
            }
            b'[' => {
                self.reader.consume(1);
                let mut values = Vec::new();
                self.whitespace()?;
                if self.peek()? == Some(b']') {
                    self.reader.consume(1);
                    return Ok(Value::Array(values));
                }
                loop {
                    let selected = keep && values.len() < 128 && self.nodes_left > 0;
                    self.shortened |= keep && !selected;
                    let value = self.value(selected, text, false, depth + 1)?;
                    if selected {
                        values.push(value);
                    }
                    self.whitespace()?;
                    match self.byte()? {
                        b']' => break,
                        b',' => self.whitespace()?,
                        _ => bail!("Malformed agent JSON array"),
                    }
                }
                Ok(Value::Array(values))
            }
            b'"' => {
                let cap = if !keep {
                    0
                } else if text {
                    self.text_left
                } else {
                    self.metadata_left.min(8192)
                };
                let (value, cut) = self.string(cap)?;
                if keep {
                    if text {
                        self.text_left = self.text_left.saturating_sub(value.len());
                    } else {
                        self.metadata_left = self.metadata_left.saturating_sub(value.len());
                    }
                    self.shortened |= cut;
                    Ok(Value::String(value))
                } else {
                    Ok(Value::Null)
                }
            }
            _ => {
                let mut raw = Vec::with_capacity(32);
                while self.peek()?.is_some_and(|b| {
                    !matches!(b, b' ' | b'\r' | b'\n' | b'\t' | b',' | b']' | b'}')
                }) {
                    ensure!(raw.len() < 128, "Invalid agent JSON scalar");
                    raw.push(self.byte()?);
                }
                let scalar = serde_json::from_slice(&raw)?;
                Ok(if keep { scalar } else { Value::Null })
            }
        }
    }
}
fn hex(bytes: &[u8]) -> Result<u16> {
    let text = std::str::from_utf8(bytes)?;
    u16::from_str_radix(text, 16).context("Invalid agent JSON Unicode escape")
}
fn is_text(key: &str) -> bool {
    matches!(
        key,
        "text" | "content" | "result" | "response" | "delta" | "message" | "text_delta" | "review"
    )
}
fn relevant(key: &str) -> bool {
    matches!(
        key,
        "method"
            | "review"
            | "goal"
            | "objective"
            | "tokenBudget"
            | "tokensUsed"
            | "timeUsedSeconds"
            | "run"
            | "entries"
            | "eventName"
            | "statusMessage"
            | "fromModel"
            | "toModel"
            | "properties"
            | "update"
            | "sessionUpdate"
            | "toolCall"
            | "toolCallId"
            | "rawInput"
            | "rawOutput"
            | "locations"
            | "oldText"
            | "newText"
            | "title"
            | "optionId"
            | "availableCommands"
            | "availableModes"
            | "currentModeId"
            | "modeId"
            | "availableModels"
            | "currentModelId"
            | "modelId"
            | "protocolVersion"
            | "agentCapabilities"
            | "loadSession"
            | "promptCapabilities"
            | "modes"
            | "models"
            | "permission"
            | "patterns"
            | "resources"
            | "always"
            | "multiple"
            | "custom"
            | "auto"
            | "synthetic"
            | "compaction_continue"
            | "parentID"
            | "messageID"
            | "callID"
            | "field"
            | "params"
            | "thread"
            | "turn"
            | "threadId"
            | "turnId"
            | "itemId"
            | "requestId"
            | "willRetry"
            | "agentsStates"
            | "receiverThreadIds"
            | "senderThreadId"
            | "agentThreadId"
            | "agentPath"
            | "reasoningEffort"
            | "tokenUsage"
            | "last"
            | "total"
            | "modelContextWindow"
            | "inputTokens"
            | "outputTokens"
            | "cachedInputTokens"
            | "totalTokens"
            | "aggregatedOutput"
            | "command"
            | "cwd"
            | "changes"
            | "diff"
            | "path"
            | "savedPath"
            | "phase"
            | "delivery"
            | "type"
            | "subtype"
            | "event"
            | "payload"
            | "item"
            | "message"
            | "content"
            | "id"
            | "text"
            | "role"
            | "author"
            | "thread_id"
            | "session_id"
            | "sessionID"
            | "conversation_id"
            | "parent_tool_use_id"
            | "timestamp_ms"
            | "model_call_id"
            | "status"
            | "state"
            | "error"
            | "errors"
            | "severity"
            | "is_error"
            | "result"
            | "response"
            | "delta"
            | "text_delta"
            | "part"
            | "partID"
            | "data"
            | "tool"
            | "tool_name"
            | "name"
            | "server"
            | "arguments"
            | "input"
            | "tool_call"
            | "call_id"
            | "content_block"
            | "step_update"
            | "step_type"
            | "step_index"
            | "agents_states"
            | "metadata"
            | "sessionId"
            | "parentSessionId"
            | "background"
            | "subagent_type"
            | "tool_use_id"
            | "tool_use_result"
            | "isAsync"
            | "summary"
            | "task_type"
            | "task_id"
            | "receiver_thread_ids"
            | "prompt"
            | "target"
            | "task_name"
            | "questions"
            | "header"
            | "question"
            | "options"
            | "label"
            | "description"
            | "slash_commands"
            | "outcome"
            | "kind"
            | "reason"
            | "duration_ms"
            | "new_agent_nickname"
            | "receiver_agent_nickname"
            | "agent_path"
            | "new_thread_id"
            | "receiver_thread_id"
            | "agent_thread_id"
            | "usage"
            | "model"
            | "modelUsage"
            | "contextWindow"
            | "input_tokens"
            | "output_tokens"
            | "cached_input_tokens"
            | "cache_read_input_tokens"
            | "cache_creation_input_tokens"
            | "total_cost_usd"
            | "cost_usd"
            | "stats"
            | "cached"
            | "tokens"
            | "output"
            | "cache"
            | "read"
            | "write"
            | "cost"
            | "info"
            | "last_token_usage"
            | "total_tokens"
            | "model_context_window"
            | "rate_limits"
            | "primary"
            | "secondary"
            | "rate_limits_by_limit_id"
            | "limit_id"
            | "window_minutes"
            | "used_percent"
            | "resets_at"
    ) || key.ends_with("ToolCall")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::{io::Cursor, os::unix::fs::MetadataExt};

    fn decode(bytes: &[u8]) -> Result<(Value, bool)> {
        // Exercise escapes/UTF-8 at refill boundaries, not just slice parsing.
        project(
            BufReader::with_capacity(7, Cursor::new(bytes)),
            &AtomicBool::new(false),
        )
    }

    #[test]
    fn small_records_stay_in_memory_and_large_records_release_their_storage() {
        let mut record = Record::default();
        record.push(&vec![b' '; MEMORY_BYTES]).unwrap();
        assert_eq!(record.small().unwrap().len(), MEMORY_BYTES);
        record.push(b" ").unwrap();
        assert!(record.small().is_none());
        assert!(record.memory.capacity() <= 8192);
        let metadata = record.spill.as_ref().unwrap().metadata().unwrap();
        assert_eq!(metadata.nlink(), 0);
        assert_eq!(metadata.mode() & 0o777, 0o600);
        assert_eq!(
            record.spill.as_ref().unwrap().metadata().unwrap().len(),
            (MEMORY_BYTES + 1) as u64
        );
        record.clear();
        assert!(record.is_empty());
        assert!(record.spill.is_none());
        record.push(b"{}").unwrap();
        assert_eq!(record.small(), Some(b"{}".as_slice()));
    }

    #[test]
    fn escapes_unicode_scalars_and_dynamic_tool_and_model_keys_are_preserved() {
        let bytes = br#"{"message":{"content":[{"type":"text","text":"\"\\\/\b\f\n\r\t \u263a \ud83c\udf0a"}]},"tool_call":{"readToolCall":{"result":{"error":{"message":"Denied"}}}},"modelUsage":{"arbitrary-model/1":{"contextWindow":200000}},"usage":{"input_tokens":123,"output_tokens":0},"is_error":false,"timestamp_ms":123,"parent_tool_use_id":null}"#;
        assert_eq!(
            decode(bytes).unwrap(),
            (serde_json::from_slice(bytes).unwrap(), false)
        );
        let literal = json!({"text":"🌊 café 漢字", "delta":true, "cost_usd":0.125});
        assert_eq!(
            decode(&serde_json::to_vec(&literal).unwrap()).unwrap(),
            (literal, false)
        );
    }

    #[test]
    fn large_event_projection_retains_dynamic_child_states_and_native_task_metadata() {
        let value = json!({"type":"item.completed","item":{"type":"collab_tool_call","id":"call","tool":"wait","receiver_thread_ids":["child:🌊"],"agents_states":{"child:🌊":{"status":"completed","message":"Child report"}}},"part":{"state":{"metadata":{"sessionId":"ses_child","background":true},"input":{"subagent_type":"explore","task_id":"ses_child"}}},"tool_use_result":{"isAsync":true,"status":"async_launched"}});
        assert_eq!(
            decode(&serde_json::to_vec(&value).unwrap()).unwrap(),
            (value, false)
        );
    }

    #[test]
    fn huge_ignored_outputs_do_not_displace_late_identity_status_or_usage() {
        let mut record = Record::default();
        record.push(br#"{"item":{"aggregated_output":""#).unwrap();
        for _ in 0..320 {
            record.push(&[b'x'; 65536]).unwrap();
        }
        record.push(br#"","type":"command_execution","status":"completed"},"type":"item.completed","thread_id":"keep-id","usage":{"input_tokens":5}}"#).unwrap();
        let (value, shortened) = record.projected(&AtomicBool::new(false)).unwrap();
        assert_eq!(
            value,
            json!({"item":{"type":"command_execution","status":"completed"},"type":"item.completed","thread_id":"keep-id","usage":{"input_tokens":5}})
        );
        assert!(!shortened);
        assert!(record.memory.capacity() <= 8192);
        assert!(serde_json::to_vec(&value).unwrap().len() < 256);
    }

    #[test]
    fn meaningful_strings_arrays_and_node_counts_have_bounded_previews() {
        let cancelled = AtomicBool::new(false);
        let mut scan = Projection {
            reader: Cursor::new(
                r#"{"text":"a🌊\ud83c\udf0ab","type":"done","options":[1,2,3]}"#.as_bytes(),
            ),
            cancelled: &cancelled,
            text_left: 4,
            metadata_left: 10,
            nodes_left: 8,
            shallow_nodes_left: 256,
            shortened: false,
        };
        let value = scan.value(true, false, false, 0).unwrap();
        assert_eq!(value["text"], "a");
        assert_eq!(value["type"], "done");
        assert!(scan.shortened);
        let mut scan = Projection {
            reader: Cursor::new(br#"{"text":"ab\ud83c\udf0aend","type":"done"}"#),
            cancelled: &cancelled,
            text_left: 6,
            metadata_left: 10,
            nodes_left: 10,
            shallow_nodes_left: 256,
            shortened: false,
        };
        let value = scan.value(true, false, false, 0).unwrap();
        assert_eq!(value["text"], "ab");
        assert_eq!(value["type"], "done");
        let value = json!({"options":vec![1; 10000], "type":"after-array"});
        let (view, shortened) = decode(&serde_json::to_vec(&value).unwrap()).unwrap();
        assert_eq!(view["options"].as_array().unwrap().len(), 128);
        assert_eq!(view["type"], "after-array");
        assert!(shortened);
        let (view, shortened) = decode(&serde_json::to_vec(&json!({"options":vec![json!({"options":vec![1;128]});128],"type":"after-many-nodes"})).unwrap()).unwrap();
        assert!(serde_json::to_vec(&view).unwrap().len() < 100000);
        assert_eq!(view["type"], "after-many-nodes");
        assert!(shortened);
    }

    #[test]
    fn discarded_fields_still_validate_json_framing_and_cancellation() {
        for bytes in [
            br#"{"unknown":"bad\q"}"#.as_slice(),
            br#"{"unknown":"\ud800oops"}"#,
            br#"{"unknown":[1,2,]}"#,
            br#"{"text":"unfinished"#,
            br#"{"text":"ok"} trailing"#,
            br#"{"text":true nope}"#,
        ] {
            assert!(decode(bytes).is_err(), "accepted malformed JSON: {bytes:?}");
        }
        assert!(
            project(Cursor::new(b"{}"), &AtomicBool::new(true))
                .unwrap_err()
                .to_string()
                .contains("Stopped")
        );
        let deep = format!("{}0{}", "[".repeat(130), "]".repeat(130));
        assert!(decode(deep.as_bytes()).is_err());
    }
}
