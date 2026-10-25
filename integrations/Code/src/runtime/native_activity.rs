//! Codex exec 0.162.1 omits SubAgentActivity items. Supplement its public wire
//! with read-only, incremental reads of the active native rollout. This observer
//! runs only on the active process worker and never acquires a native writer.
use super::{RuntimeEvent, StreamParser, history::HistoryCursor};
use crate::{
    chat_interactions::{ChatInteraction, InteractionStatus},
    discovery::{self, AgentKind},
};
use serde_json::Value;
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

const RECORD_BYTES: usize = 64 * 1024;
const READ_BUDGET: usize = 4 * 1024 * 1024;

pub(super) struct Observer {
    codex_root: PathBuf,
    initial: Option<(String, PathBuf, u64)>,
    session: String,
    tail: Option<Tail>,
    next_read: Instant,
    next_lookup: Instant,
    started_ms: i64,
}
struct Tail {
    path: PathBuf,
    offset: u64,
    partial: Vec<u8>,
    oversized: bool,
    after_ms: Option<i64>,
}
impl Observer {
    pub(super) fn new(codex_root: PathBuf, resume: Option<&str>) -> Self {
        let initial = resume.and_then(|id| {
            let path = discovery::codex_existing_transcript_in(&codex_root, id)?;
            if path.extension().is_some_and(|ext| ext == "zst") {
                return None;
            }
            let length = std::fs::metadata(&path).ok()?.len();
            Some((id.into(), path, length))
        });
        Self {
            codex_root,
            initial,
            session: String::new(),
            tail: None,
            next_read: Instant::now(),
            next_lookup: Instant::now(),
            started_ms: chrono::Utc::now().timestamp_millis().saturating_sub(1),
        }
    }
    pub(super) fn poll(&mut self, parser: &mut StreamParser, force: bool) -> Vec<RuntimeEvent> {
        let Some(id) = parser.codex_native_id.as_deref() else {
            return Vec::new();
        };
        // Fake/malformed IDs cannot trigger history discovery or filesystem scans.
        if uuid::Uuid::parse_str(id).is_err() {
            return Vec::new();
        }
        let now = Instant::now();
        if !force && now < self.next_read {
            return Vec::new();
        }
        self.next_read = now + Duration::from_millis(200);
        if self.session != id {
            self.session = id.into();
            self.tail = None;
            self.next_lookup = now;
        }
        if self.tail.is_none() && now >= self.next_lookup {
            self.next_lookup = now + Duration::from_millis(500);
            let initial = self
                .initial
                .as_ref()
                .filter(|(initial, _, _)| initial == id);
            let source = initial
                .map(|(_, path, length)| (path.clone(), *length))
                .or_else(|| {
                    discovery::codex_existing_transcript_in(&self.codex_root, id)
                        .map(|path| (path, 0))
                });
            if let Some((path, offset)) =
                source.filter(|(path, _)| path.extension().is_none_or(|ext| ext != "zst"))
            {
                self.tail = Some(Tail {
                    path,
                    offset,
                    partial: Vec::with_capacity(8192),
                    oversized: false,
                    after_ms: Some(self.started_ms),
                });
            }
        }
        let Some(tail) = &mut self.tail else {
            return Vec::new();
        };
        let Ok(items) = tail.read() else {
            return Vec::new();
        };
        let mut events = Vec::new();
        for item in items {
            let Some(id) = item["agent_thread_id"].as_str() else {
                continue;
            };
            let status = match item["kind"].as_str() {
                Some("started" | "running") => InteractionStatus::Working,
                Some("completed") => InteractionStatus::Completed,
                Some("errored" | "failed") => InteractionStatus::Failed,
                Some("interrupted" | "shutdown") => InteractionStatus::Cancelled,
                _ => continue,
            };
            let path = item["agent_path"].as_str().unwrap_or("");
            let mut details = if path.is_empty() {
                String::new()
            } else {
                format!("{path}\n")
            };
            if status == InteractionStatus::Completed {
                details.push_str(&child_result(&self.codex_root, id));
            }
            let title = match status {
                InteractionStatus::Completed => "Subagent completed",
                InteractionStatus::Failed => "Subagent needs attention",
                InteractionStatus::Cancelled => "Subagent stopped",
                _ => "Subagent working",
            };
            events.extend(parser.interaction(
                ChatInteraction::Delegation {
                    title: title.into(),
                    details,
                    status,
                    agent: Some(id.chars().take(256).collect()),
                },
                &format!("codex-native-child:{id}"),
            ));
        }
        events
    }
}

impl Tail {
    fn read(&mut self) -> std::io::Result<Vec<Value>> {
        let mut file = File::open(&self.path)?;
        if file.metadata()?.len() < self.offset {
            self.offset = 0;
            self.partial.clear();
            self.oversized = false;
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut buffer = [0_u8; 16 * 1024];
        let mut remaining = READ_BUDGET;
        let mut items = Vec::new();
        while remaining > 0 && items.len() < 32 {
            let read = file.read(&mut buffer[..remaining.min(16 * 1024)])?;
            if read == 0 {
                break;
            }
            self.offset += read as u64;
            remaining -= read;
            for bytes in buffer[..read].split_inclusive(|byte| *byte == b'\n') {
                let complete = bytes.last() == Some(&b'\n');
                if !self.oversized {
                    if self.partial.len() + bytes.len() > RECORD_BYTES {
                        self.oversized = true;
                        self.partial.clear();
                    } else {
                        self.partial.extend_from_slice(bytes);
                    }
                }
                if complete {
                    if !self.oversized
                        && self
                            .partial
                            .windows(b"SubAgentActivity".len())
                            .any(|part| part == b"SubAgentActivity")
                        && let Ok(value) = serde_json::from_slice::<Value>(&self.partial)
                        && value["type"] == "event_msg"
                        && value["payload"]["type"] == "item_completed"
                        && value["payload"]["item"]["type"] == "SubAgentActivity"
                        && value["timestamp"]
                            .as_str()
                            .and_then(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).ok())
                            .is_none_or(|stamp| {
                                self.after_ms
                                    .is_none_or(|after| stamp.timestamp_millis() >= after)
                            })
                    {
                        items.push(value["payload"]["item"].clone());
                    }
                    self.partial.clear();
                    self.oversized = false;
                }
            }
        }
        Ok(items)
    }
}

fn child_result(codex_root: &Path, id: &str) -> String {
    if uuid::Uuid::parse_str(id).is_err() {
        return String::new();
    }
    let Some(path) = discovery::codex_existing_transcript_in(codex_root, id) else {
        return String::new();
    };
    // Native lazy paging handles large message records without materializing
    // them. Only the bounded newest child response preview is requested.
    let Ok(mut cursor) = HistoryCursor::open(&path, AgentKind::Codex) else {
        return String::new();
    };
    let Ok(messages) = cursor.page_budget(4, 8 * 1024) else {
        return String::new();
    };
    let mut text = String::new();
    for message in messages
        .into_iter()
        .filter(|message| message.role == "assistant")
    {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        let mut end = message
            .text
            .len()
            .min((8 * 1024_usize).saturating_sub(text.len()));
        while !message.text.is_char_boundary(end) {
            end -= 1;
        }
        text.push_str(&message.text[..end]);
        if text.len() >= 8 * 1024 {
            break;
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    #[test]
    fn custom_native_root_does_not_replay_inherited_child_activity() {
        let root =
            std::env::temp_dir().join(format!("code-custom-codex-root-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let parent = uuid::Uuid::new_v4().to_string();
        let child = uuid::Uuid::new_v4().to_string();
        let source = root.join("parent.jsonl");
        let row = |stamp: &str, kind: &str| {
            format!(
                "{}\n",
                json!({"timestamp":stamp,"type":"event_msg","payload":{"type":"item_completed","item":{"type":"SubAgentActivity","agent_thread_id":child,"kind":kind}}})
            )
        };
        std::fs::write(&source, row("2000-01-01T00:00:00Z", "completed")).unwrap();
        let db = rusqlite::Connection::open(root.join("state_5.sqlite")).unwrap();
        db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,rollout_path TEXT)")
            .unwrap();
        db.execute(
            "INSERT INTO threads VALUES(?1,?2)",
            rusqlite::params![parent, source.to_string_lossy()],
        )
        .unwrap();
        let mut observer = Observer::new(root.clone(), None);
        let mut parser = StreamParser {
            codex_native_id: Some(parent),
            ..Default::default()
        };
        assert!(observer.poll(&mut parser, true).is_empty());
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&source)
            .unwrap();
        file.write_all(row(&chrono::Utc::now().to_rfc3339(), "started").as_bytes())
            .unwrap();
        let events = observer.poll(&mut parser, true);
        assert!(
            matches!(events.as_slice(), [RuntimeEvent::Interaction(card)] if matches!(card.as_ref(),ChatInteraction::Delegation{status:InteractionStatus::Working,agent:Some(id),..} if id == &child))
        );
        assert!(observer.poll(&mut parser, true).is_empty());
        drop(db);
        drop(file);
        drop(observer);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn incremental_native_activity_ignores_old_partial_and_huge_unrelated_records() {
        let path = std::env::temp_dir().join(format!(
            "code-native-activity-test-{}",
            uuid::Uuid::new_v4()
        ));
        let row = |kind: &str| {
            serde_json::to_vec(&json!({"type":"event_msg","payload":{"type":"item_completed","item":{"type":"SubAgentActivity","agent_thread_id":"child","kind":kind,"agent_path":"/root/child"}}})).unwrap()
        };
        let old = row("started");
        std::fs::write(&path, &old).unwrap();
        let mut tail = Tail {
            path: path.clone(),
            offset: old.len() as u64,
            partial: vec![],
            oversized: false,
            after_ms: None,
        };
        let completed = row("completed");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"\n").unwrap();
        file.write_all(&vec![b'x'; RECORD_BYTES + 1]).unwrap();
        file.write_all(b"\n").unwrap();
        file.write_all(&completed[..20]).unwrap();
        assert!(tail.read().unwrap().is_empty());
        assert!(tail.partial.len() <= RECORD_BYTES);
        file.write_all(&completed[20..]).unwrap();
        file.write_all(b"\n").unwrap();
        let items = tail.read().unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["kind"], "completed");
        assert!(tail.read().unwrap().is_empty());
        std::fs::remove_file(path).unwrap();
    }
}
