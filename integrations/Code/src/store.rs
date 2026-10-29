//! Indexed, paged local history. SQLite work belongs on the state worker, not the UI thread.

#[path = "store_history.rs"]
mod local_history;

use crate::discovery::{AgentKind, SessionSummary};
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, Row, params};
use std::{
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

pub struct Store {
    connection: Connection,
}
pub const SESSION_ID_PAGE_SIZE: usize = 128;

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredMessage {
    pub role: String,
    pub text: String,
}

pub struct LazyMessage {
    pub role: String,
    pub provider: Option<String>,
    pub text: String,
    pub code_language: Option<String>,
}

pub struct LocalFingerprint {
    pub role: String,
    pub prefix: String,
    pub tail: String,
    pub bytes: u64,
    pub digest: Option<[u8; 32]>,
    pub raw_digest: Option<(u64, [u8; 32])>,
}

/// Bounded identity of the actual provider-visible input, independently of its
/// readable local representation. Never a substring/heuristic prompt match.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct NativeInput {
    bytes: u64,
    prefix: String,
    tail: String,
    digest: [u8; 32],
    #[serde(default)]
    raw_digest: Option<(u64, [u8; 32])>,
}
impl NativeInput {
    pub fn from_prompt(prompt: &str) -> Option<Self> {
        use sha2::{Digest, Sha256};
        let text = crate::presentation::sanitize_transcript_text("user", prompt)?;
        let mut end = text.len().min(256);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        let mut start = text.len().saturating_sub(256);
        while !text.is_char_boundary(start) {
            start += 1;
        }
        Some(Self {
            bytes: text.len() as u64,
            prefix: text[..end].into(),
            tail: text[start..].into(),
            digest: Sha256::digest(text.as_bytes()).into(),
            raw_digest: (text != prompt).then(|| {
                (
                    prompt.len() as u64,
                    Sha256::digest(prompt.as_bytes()).into(),
                )
            }),
        })
    }
    fn valid(&self) -> bool {
        self.prefix.len() <= 256
            && self.tail.len() <= 256
            && self.prefix.len() as u64 <= self.bytes
            && self.tail.len() as u64 <= self.bytes
    }
}

/// A row-id cursor plus a UTF-8 byte cursor inside a potentially huge message.
/// SQLite incremental BLOB reads never materialize the entire text cell.
#[derive(Default)]
pub struct MessageCursor {
    pub local_only: bool,
    before: Option<i64>,
    continuation: Option<(i64, String, usize)>,
    exhausted: bool,
    code_index: Option<(i64, local_history::PlainIndex)>,
    seek_head: bool,
    pub(crate) head_end: Option<f64>,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct MessageCheckpoint {
    local_only: bool,
    before: Option<i64>,
    continuation: Option<(i64, String, usize)>,
    exhausted: bool,
    code_index: Option<(i64, PathBuf)>,
    #[serde(default)]
    seek_head: bool,
}
impl MessageCheckpoint {
    pub(crate) fn rewrite_artifacts(&mut self, map: &std::collections::HashMap<PathBuf, PathBuf>) {
        if let Some((_, path)) = &mut self.code_index
            && let Some(saved) = map.get(path)
        {
            *path = saved.clone();
        }
    }
}

impl MessageCursor {
    pub(crate) fn checkpoint(&self) -> MessageCheckpoint {
        MessageCheckpoint {
            seek_head: self.seek_head,
            local_only: self.local_only,
            before: self.before,
            continuation: self.continuation.clone(),
            exhausted: self.exhausted,
            code_index: self
                .code_index
                .as_ref()
                .map(|(id, index)| (*id, index.artifact().path.clone())),
        }
    }
    pub(crate) fn artifacts(
        &self,
    ) -> Vec<std::sync::Arc<crate::history_artifact::HistoryArtifact>> {
        self.code_index
            .as_ref()
            .map_or_else(Vec::new, |(_, index)| vec![index.artifact()])
    }
    pub(crate) fn restore(
        c: MessageCheckpoint,
        store: &Store,
        session: &str,
        resolve: &mut impl FnMut(
            &Path,
        )
            -> Result<std::sync::Arc<crate::history_artifact::HistoryArtifact>>,
    ) -> Result<Self> {
        anyhow::ensure!(
            c.continuation
                .as_ref()
                .is_none_or(|(_, role, end)| role.len() <= 64 && *end <= i32::MAX as usize),
            "Invalid local history checkpoint"
        );
        if let Some((id, role, end)) = &c.continuation {
            let binding: Option<(String, String)> = store
                .connection
                .query_row(
                    "SELECT session_id,role FROM messages WHERE id=?1",
                    [id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            anyhow::ensure!(
                binding == Some((session.to_owned(), role.clone()))
                    && *end <= store.message_blob(*id)?.size() as usize,
                "Saved local message changed"
            );
        }
        let code_index = if let Some((id, path)) = c.code_index {
            Some((id, local_history::PlainIndex::restore(resolve(&path)?)?))
        } else {
            None
        };
        Ok(Self {
            seek_head: c.seek_head,
            head_end: None,
            local_only: c.local_only,
            before: c.before,
            continuation: c.continuation,
            exhausted: c.exhausted,
            code_index,
        })
    }
    #[cfg(test)]
    pub fn new(local_only: bool) -> Self {
        Self {
            local_only,
            ..Default::default()
        }
    }
    /// Freeze the upper row boundary before the first page is read. Appended
    /// turns must not leak into rehydrated checkpoints for older pages.
    pub(crate) fn snapshot(local_only: bool, head: Option<i64>) -> Self {
        Self {
            local_only,
            before: Some(head.map_or(i64::MIN, |head| head.saturating_add(1))),
            exhausted: head.is_none(),
            ..Default::default()
        }
    }
    pub fn has_older(&self) -> bool {
        !self.exhausted || self.continuation.is_some()
    }
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if path != Path::new(":memory:")
            && let Some(parent) = path
                .parent()
                .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent).context("Create Orbit history directory")?;
        }
        let connection = Connection::open(path).context("Open Orbit history")?;
        connection.busy_timeout(Duration::from_secs(3))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=NORMAL;
             PRAGMA foreign_keys=ON;
             PRAGMA cache_size=-4096;
             PRAGMA temp_store=FILE;
             CREATE TABLE IF NOT EXISTS sessions (
               id TEXT PRIMARY KEY,
               title TEXT NOT NULL,
               agent TEXT NOT NULL,
               project_path TEXT NOT NULL,
               updated_at INTEGER NOT NULL,
               external_id TEXT,
               transcript_path TEXT,
               execution_path TEXT
             );
             CREATE INDEX IF NOT EXISTS sessions_updated ON sessions(updated_at DESC);
             CREATE INDEX IF NOT EXISTS sessions_project ON sessions(project_path, updated_at DESC);
             CREATE INDEX IF NOT EXISTS sessions_external ON sessions(agent, external_id);
             CREATE TABLE IF NOT EXISTS session_options (
               session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
               model TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS session_usage (
               session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
               report TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS session_handoffs (
               session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
               packet TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS conversation_baselines (
               session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
               source TEXT NOT NULL,
               overlap_before INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS conversation_native_ids (
               agent TEXT NOT NULL,
               external_id TEXT NOT NULL,
               session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
               PRIMARY KEY(agent, external_id)
             );
             CREATE INDEX IF NOT EXISTS conversation_native_owner ON conversation_native_ids(session_id);
             CREATE TABLE IF NOT EXISTS session_imports (
               session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
               version INTEGER NOT NULL DEFAULT 1
             );
             CREATE TABLE IF NOT EXISTS messages (
               id INTEGER PRIMARY KEY,
               session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
               role TEXT NOT NULL,
               text TEXT NOT NULL
             );
             CREATE INDEX IF NOT EXISTS messages_session ON messages(session_id, id DESC);
             CREATE VIRTUAL TABLE IF NOT EXISTS message_search USING fts5(
               text, content='messages', content_rowid='id'
             );
             CREATE TRIGGER IF NOT EXISTS messages_insert AFTER INSERT ON messages BEGIN
               INSERT INTO message_search(rowid, text) VALUES (new.id, new.text);
             END;
             CREATE TRIGGER IF NOT EXISTS messages_delete AFTER DELETE ON messages BEGIN
               INSERT INTO message_search(message_search, rowid, text) VALUES ('delete', old.id, old.text);
             END;
             CREATE TRIGGER IF NOT EXISTS messages_update AFTER UPDATE OF text,id ON messages BEGIN
               INSERT INTO message_search(message_search, rowid, text) VALUES ('delete', old.id, old.text);
               INSERT INTO message_search(rowid, text) VALUES (new.id, new.text);
             END;"
        ).context("Initialize Orbit history")?;
        // Session ownership moves do not change indexed text. Older databases
        // used an all-column UPDATE trigger that re-tokenized entire transcripts
        // merely to merge their sidebar metadata.
        let update_trigger: String = connection.query_row(
            "SELECT sql FROM sqlite_master WHERE type='trigger' AND name='messages_update'",
            [],
            |row| row.get(0),
        )?;
        if update_trigger
            .to_ascii_uppercase()
            .contains("AFTER UPDATE ON")
        {
            connection.execute_batch("DROP TRIGGER messages_update;
                CREATE TRIGGER messages_update AFTER UPDATE OF text,id ON messages BEGIN
                    INSERT INTO message_search(message_search,rowid,text) VALUES('delete',old.id,old.text);
                    INSERT INTO message_search(rowid,text) VALUES(new.id,new.text);
                END;")?;
        }
        // Large streamed responses are incremental BLOBs. Tokenizing a giant
        // zeroblob (or its completed contents) would materialize the response.
        // Keep normal TEXT messages indexed, while disk-paged responses remain
        // title-searchable and readable through the same BLOB cursor.
        if !update_trigger.contains("typeof(new.text)") {
            connection.execute_batch("BEGIN;
                DROP TRIGGER IF EXISTS messages_insert;
                DROP TRIGGER IF EXISTS messages_delete;
                DROP TRIGGER IF EXISTS messages_update;
                CREATE TRIGGER messages_insert AFTER INSERT ON messages WHEN typeof(new.text)='text' BEGIN
                    INSERT INTO message_search(rowid,text) VALUES(new.id,new.text);
                END;
                CREATE TRIGGER messages_delete AFTER DELETE ON messages WHEN typeof(old.text)='text' BEGIN
                    INSERT INTO message_search(message_search,rowid,text) VALUES('delete',old.id,old.text);
                END;
                CREATE TRIGGER messages_update AFTER UPDATE OF text,id ON messages BEGIN
                    INSERT INTO message_search(message_search,rowid,text) SELECT 'delete',old.id,old.text WHERE typeof(old.text)='text';
                    INSERT INTO message_search(rowid,text) SELECT new.id,new.text WHERE typeof(new.text)='text';
                END;
                COMMIT;")?;
        }
        let columns = connection
            .prepare("PRAGMA table_info(sessions)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !columns.iter().any(|column| column == "execution_path") {
            connection.execute("ALTER TABLE sessions ADD COLUMN execution_path TEXT", [])?;
        }
        let message_columns = connection
            .prepare("PRAGMA table_info(messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !message_columns.iter().any(|column| column == "provider") {
            connection.execute("ALTER TABLE messages ADD COLUMN provider TEXT", [])?;
        }
        if !message_columns
            .iter()
            .any(|column| column == "native_input")
        {
            connection.execute("ALTER TABLE messages ADD COLUMN native_input TEXT", [])?;
        }
        let imports = connection
            .prepare("PRAGMA table_info(session_imports)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if !imports.iter().any(|column| column == "version") {
            connection.execute(
                "ALTER TABLE session_imports ADD COLUMN version INTEGER NOT NULL DEFAULT 1",
                [],
            )?;
        }
        Ok(Self { connection })
    }

    /// Commit a discovery refresh once instead of one implicit WAL transaction
    /// per session. Duplicate native IDs are merged inside the same transaction.
    #[allow(dead_code)] // Shared batch API used by the standalone state benchmark.
    pub fn upsert_sessions(&self, sessions: &[SessionSummary]) -> Result<()> {
        self.upsert_batch(sessions, false)
    }
    pub fn upsert_discovered_sessions(&self, sessions: &[SessionSummary]) -> Result<()> {
        self.upsert_batch(sessions, true)
    }
    fn upsert_batch(&self, sessions: &[SessionSummary], discovered: bool) -> Result<()> {
        if sessions.is_empty() {
            return Ok(());
        }
        let transaction = self.connection.unchecked_transaction()?;
        for session in sessions {
            if discovered {
                self.upsert_discovered_session(session)?;
            } else {
                self.upsert_session(session)?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_session(&self, session: &SessionSummary) -> Result<()> {
        // Only a runtime SessionId event may replace an established binding.
        // Queued UI snapshots and original-native rediscovery can arrive later.
        if let Some(existing) = self.session(&session.id)?
            && existing.external_id.is_some()
            && existing.external_id != session.external_id
        {
            return Ok(());
        }
        let fallback = format!("{} session", session.agent.name());
        let title =
            crate::discovery::clean_title(&session.title).unwrap_or_else(|| fallback.clone());
        let previous_title: Option<String> = self
            .connection
            .prepare_cached("SELECT title FROM sessions WHERE id=?1")?
            .query_row([&session.id], |row| row.get(0))
            .optional()?;
        let repair_title = previous_title.as_ref().is_some_and(|previous| {
            crate::discovery::clean_title(previous).is_none()
                || (previous == &fallback && title != fallback)
        });
        self.connection.prepare_cached(
            "INSERT INTO sessions (id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(id) DO UPDATE SET title=CASE WHEN ?9 OR excluded.updated_at>=sessions.updated_at
                 THEN excluded.title ELSE sessions.title END,agent=excluded.agent,
               project_path=excluded.project_path,updated_at=MAX(sessions.updated_at,excluded.updated_at),
               external_id=COALESCE(sessions.external_id,excluded.external_id),
               transcript_path=COALESCE(excluded.transcript_path,sessions.transcript_path),
               execution_path=COALESCE(excluded.execution_path,sessions.execution_path)
             WHERE sessions.updated_at<excluded.updated_at
                OR sessions.title IS NOT CASE WHEN ?9 OR excluded.updated_at>=sessions.updated_at THEN excluded.title ELSE sessions.title END
                OR sessions.agent IS NOT excluded.agent OR sessions.project_path IS NOT excluded.project_path
                OR (sessions.external_id IS NULL AND excluded.external_id IS NOT NULL)
                OR (excluded.transcript_path IS NOT NULL AND sessions.transcript_path IS NOT excluded.transcript_path)
                OR (excluded.execution_path IS NOT NULL AND sessions.execution_path IS NOT excluded.execution_path)"
        )?.execute(
            params![session.id, title, session.agent.command(),
                session.project_path.to_string_lossy(), session.updated_at, session.external_id,
                session.transcript_path.as_ref().map(|path| path.to_string_lossy().into_owned()),
                session.execution_path.as_ref().map(|path| path.to_string_lossy().into_owned()), repair_title],
        )?;
        // The CLI learns its ID after launch. A discovery refresh may already
        // have inserted that native session; retain the active Orbit ID.
        if let Some(external_id) = &session.external_id {
            let mut statement = self.connection.prepare_cached(
                "SELECT id FROM sessions WHERE agent=?1 AND external_id=?2 AND id<>?3",
            )?;
            let duplicates = statement
                .query_map(
                    params![session.agent.command(), external_id, session.id],
                    |row| row.get::<_, String>(0),
                )?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            for duplicate in duplicates {
                self.merge_session(&session.id, &duplicate)?;
            }
        }
        Ok(())
    }

    pub fn session(&self, id: &str) -> Result<Option<SessionSummary>> {
        Ok(self.connection.prepare_cached(
            "SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path FROM sessions WHERE id=?1"
        )?.query_row([id], session_from_row).optional()?)
    }

    /// Authoritative runtime ID binding, including imported-native forks. Keep
    /// the UI ID and its local turns while adopting the child's native source.
    pub fn bind_runtime_session(&self, session: &SessionSummary) -> Result<()> {
        anyhow::ensure!(
            session
                .external_id
                .as_ref()
                .is_some_and(|id| !id.is_empty()),
            "Missing runtime session ID"
        );
        let transaction = self.connection.unchecked_transaction()?;
        let previous = self.session(&session.id)?;
        let mut bound = session.clone();
        if let Some(previous) = &previous
            && previous.external_id != session.external_id
        {
            // The UI SessionId snapshot can still carry the parent's path.
            if bound.transcript_path == previous.transcript_path {
                bound.transcript_path = None;
            }
            self.connection.execute(
                "UPDATE sessions SET external_id=?2,transcript_path=?3 WHERE id=?1",
                params![
                    session.id,
                    session.external_id,
                    bound
                        .transcript_path
                        .as_ref()
                        .map(|p| p.to_string_lossy().into_owned())
                ],
            )?;
        }
        if bound.transcript_path.is_none() {
            bound.transcript_path = self.connection.query_row(
                "SELECT transcript_path FROM sessions WHERE agent=?1 AND external_id=?2 AND id<>?3 AND transcript_path IS NOT NULL LIMIT 1",
                params![session.agent.command(), session.external_id, session.id],
                |row| row.get::<_, String>(0),
            ).optional()?.map(std::path::PathBuf::from);
        }
        self.upsert_session(&bound)?;
        if self.conversation_baseline(&session.id)?.is_some() {
            self.remember_native_identity(&bound)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Discovery attaches native metadata to the existing Orbit conversation.
    /// Imported IDs must not create a second sidebar item for an Orbit run.
    pub fn upsert_discovered_session(&self, session: &SessionSummary) -> Result<()> {
        if let Some(external) = &session.external_id {
            let owner: Option<String> = self.connection.query_row(
                "SELECT session_id FROM conversation_native_ids WHERE agent=?1 AND external_id=?2",
                params![session.agent.command(), external], |row| row.get(0),
            ).optional()?;
            if let Some(owner) = owner {
                // Historical provider sessions still exist on disk, but belong
                // to one visible Code conversation after a handoff.
                if let Some(current) = self.session(&owner)? {
                    if current.agent == session.agent && current.external_id == session.external_id
                    {
                        let mut discovered = session.clone();
                        discovered.id = owner;
                        discovered.title = current.title;
                        return self.upsert_session(&discovered);
                    }
                    if let Some((mut baseline, _)) = self.conversation_baseline(&owner)?
                        && baseline.agent == session.agent
                        && baseline.external_id == session.external_id
                        && session
                            .transcript_path
                            .as_ref()
                            .is_some_and(|path| path.is_file())
                    {
                        baseline.transcript_path = session.transcript_path.clone();
                        self.update_conversation_baseline(&owner, &baseline)?;
                    }
                    return Ok(());
                }
            }
        }
        if let Some(existing) = self.session(&session.id)?
            && existing.external_id.is_some()
            && existing.external_id != session.external_id
        {
            return Ok(());
        }
        let existing = if let Some(external_id) = &session.external_id {
            self.connection.prepare_cached(
                "SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path
                 FROM sessions WHERE agent=?1 AND external_id=?2
                 ORDER BY (id <> agent || ':' || external_id) DESC,
                   EXISTS(SELECT 1 FROM session_imports WHERE session_id=sessions.id) DESC LIMIT 1"
            )?.query_row(
                params![session.agent.command(), external_id], session_from_row,
            ).optional()?
        } else {
            None
        };
        let mut discovered = session.clone();
        if let Some(existing) = existing {
            discovered.id = existing.id;
            // Preserve the first prompt/custom title of locally owned sessions.
            if discovered.id
                != format!(
                    "{}:{}",
                    discovered.agent.command(),
                    discovered.external_id.as_deref().unwrap_or_default()
                )
                && !existing.title.is_empty()
                && existing.title != format!("{} session", discovered.agent.name())
            {
                discovered.title = existing.title;
            }
            discovered.updated_at = discovered.updated_at.max(existing.updated_at);
        }
        self.upsert_session(&discovered)
    }

    pub fn session_model(&self, id: &str) -> Result<Option<String>> {
        Ok(self
            .connection
            .query_row(
                "SELECT model FROM session_options WHERE session_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?)
    }
    pub fn session_usage(&self, id: &str) -> Result<Option<crate::usage::Usage>> {
        let report: Option<String> = self
            .connection
            .query_row(
                "SELECT substr(report,1,4097) FROM session_usage WHERE session_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(report
            .filter(|s| s.len() <= 4096)
            .and_then(|s| serde_json::from_str(&s).ok()))
    }
    /// The first imported source remains lazily readable across all agent
    /// switches. New turns live once in Code's indexed local history.
    pub fn conversation_baseline(&self, id: &str) -> Result<Option<(SessionSummary, i64)>> {
        let row: Option<(String, i64)> = self
            .connection
            .query_row(
                "SELECT source,overlap_before FROM conversation_baselines WHERE session_id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        row.map(|(source, before)| Ok((serde_json::from_str(&source)?, before)))
            .transpose()
    }
    fn remember_native_identity(&self, session: &SessionSummary) -> Result<()> {
        if let Some(external) = &session.external_id {
            self.connection.execute("INSERT INTO conversation_native_ids(agent,external_id,session_id) VALUES(?1,?2,?3) ON CONFLICT(agent,external_id) DO UPDATE SET session_id=excluded.session_id", params![session.agent.command(),external,session.id])?;
        }
        Ok(())
    }
    pub fn update_conversation_baseline(&self, id: &str, source: &SessionSummary) -> Result<()> {
        self.connection.execute(
            "UPDATE conversation_baselines SET source=?2 WHERE session_id=?1",
            params![id, serde_json::to_string(source)?],
        )?;
        Ok(())
    }
    pub fn switch_agent(
        &self,
        destination: &SessionSummary,
        packet: &crate::handoff::Packet,
        model: Option<&str>,
    ) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let source = self
            .session(&destination.id)?
            .context("Conversation is no longer available")?;
        anyhow::ensure!(
            source.agent != destination.agent
                && destination.external_id.is_none()
                && destination.transcript_path.is_none(),
            "Invalid agent handoff binding"
        );
        anyhow::ensure!(
            packet.source_id == source.id
                && packet.sources.last().is_some_and(
                    |last| last.agent == source.agent && last.native_id == source.external_id
                ),
            "Handoff source changed while preparing context"
        );
        anyhow::ensure!(
            source.project_path == destination.project_path
                && source.execution_path == destination.execution_path,
            "Agent handoff changed the working directory"
        );
        if self.conversation_baseline(&destination.id)?.is_none() {
            let before = self
                .viewport_head(&source.id)?
                .unwrap_or(0)
                .saturating_add(1);
            self.connection.execute("INSERT INTO conversation_baselines(session_id,source,overlap_before) VALUES(?1,?2,?3)", params![source.id,serde_json::to_string(&source)?,before])?;
        }
        self.remember_native_identity(&source)?;
        self.connection.execute("UPDATE sessions SET agent=?2,external_id=NULL,transcript_path=NULL,updated_at=MAX(updated_at,?3) WHERE id=?1", params![destination.id,destination.agent.command(),destination.updated_at])?;
        self.set_session_model(&destination.id, model)?;
        self.connection.execute(
            "DELETE FROM session_usage WHERE session_id=?1",
            [&destination.id],
        )?;
        self.set_session_handoff(&destination.id, packet)?;
        transaction.commit()?;
        Ok(())
    }
    pub fn session_handoff(&self, id: &str) -> Result<Option<crate::handoff::Packet>> {
        let encoded: Option<String> = self
            .connection
            .query_row(
                "SELECT substr(packet,1,49153) FROM session_handoffs WHERE session_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        let packet = encoded
            .filter(|s| s.len() <= crate::handoff::MAX_PACKET_BYTES)
            .and_then(|s| serde_json::from_str::<crate::handoff::Packet>(&s).ok())
            .filter(crate::handoff::Packet::valid);
        Ok(packet)
    }
    pub fn set_session_handoff(&self, id: &str, packet: &crate::handoff::Packet) -> Result<()> {
        let encoded = serde_json::to_string(packet)?;
        anyhow::ensure!(
            packet.valid() && encoded.len() <= crate::handoff::MAX_PACKET_BYTES,
            "Handoff context exceeded its metadata budget"
        );
        self.connection.execute("INSERT INTO session_handoffs(session_id,packet) VALUES(?1,?2) ON CONFLICT(session_id) DO UPDATE SET packet=excluded.packet", params![id, encoded])?;
        Ok(())
    }
    pub fn first_saved_request(&self, id: &str) -> Result<Option<String>> {
        let mut rows = self.connection.prepare(
            "SELECT id FROM messages WHERE session_id=?1 AND role='user' ORDER BY id ASC LIMIT 32",
        )?;
        let ids = rows
            .query_map([id], |row| row.get::<_, i64>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for id in ids {
            let mut bytes = Vec::with_capacity(4096);
            self.message_blob(id)?.take(4096).read_to_end(&mut bytes)?;
            if let Err(error) = std::str::from_utf8(&bytes) {
                if error.error_len().is_some() {
                    continue;
                }
                bytes.truncate(error.valid_up_to());
            }
            let text = String::from_utf8(bytes)?;
            if text.trim().is_empty() || text.split_whitespace().next() == Some("/compact") {
                continue;
            }
            return Ok(Some(text));
        }
        Ok(None)
    }
    pub fn set_session_usage(&self, id: &str, usage: &crate::usage::Usage) -> Result<()> {
        let report = serde_json::to_string(usage)?;
        anyhow::ensure!(
            report.len() <= 4096,
            "Usage report exceeded its metadata budget"
        );
        self.connection.execute("INSERT INTO session_usage(session_id,report) VALUES(?1,?2) ON CONFLICT(session_id) DO UPDATE SET report=excluded.report WHERE report<>excluded.report", params![id, report])?;
        Ok(())
    }
    pub fn set_session_model(&self, id: &str, model: Option<&str>) -> Result<()> {
        if let Some(model) = model {
            self.connection.execute("INSERT INTO session_options(session_id,model) VALUES(?1,?2) ON CONFLICT(session_id) DO UPDATE SET model=excluded.model", params![id,model])?;
        } else {
            self.connection
                .execute("DELETE FROM session_options WHERE session_id=?1", [id])?;
        }
        Ok(())
    }

    fn merge_session(&self, canonical: &str, duplicate: &str) -> Result<()> {
        let transaction = self
            .connection
            .is_autocommit()
            .then(|| self.connection.unchecked_transaction())
            .transpose()?;
        // Preserve the native history preceding the canonical snapshot. Match
        // its first saved turn at the latest occurrence, rather than deleting
        // every equal string (people can intentionally repeat a prompt).
        let first_saved: Option<(i64, String)> = self
            .connection
            .query_row(
                "SELECT id,role FROM messages WHERE session_id=?1 ORDER BY id LIMIT 1",
                [canonical],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((first, role)) = first_saved {
            let mut before = i64::MAX;
            let overlap = loop {
                let candidate: Option<i64> = self.connection.prepare_cached(
                    "SELECT id FROM messages WHERE session_id=?1 AND role=?2 AND id<?3 ORDER BY id DESC LIMIT 1"
                )?.query_row(params![duplicate,role,before],|row|row.get(0)).optional()?;
                let Some(candidate) = candidate else {
                    break None;
                };
                if self.message_bytes_equal(first, candidate)? {
                    break Some(candidate);
                }
                before = candidate;
            };
            if let Some(overlap) = overlap {
                self.connection.execute(
                    "DELETE FROM messages WHERE session_id=?1 AND id>=?2",
                    params![duplicate, overlap],
                )?;
            }
        }
        self.connection.execute(
            "UPDATE messages SET session_id=?1 WHERE session_id=?2",
            params![canonical, duplicate],
        )?;
        self.connection.execute(
            "INSERT INTO session_imports(session_id,version)
                SELECT ?1,version FROM session_imports WHERE session_id=?2
                ON CONFLICT(session_id) DO UPDATE SET version=MAX(session_imports.version,excluded.version)",
            params![canonical, duplicate],
        )?;
        self.connection.execute(
            "UPDATE sessions SET transcript_path=COALESCE(transcript_path,
            (SELECT transcript_path FROM sessions WHERE id=?2)),
            execution_path=COALESCE(execution_path,(SELECT execution_path FROM sessions WHERE id=?2)) WHERE id=?1",
            params![canonical, duplicate],
        )?;
        self.connection.execute("INSERT OR IGNORE INTO session_options(session_id,model) SELECT ?1,model FROM session_options WHERE session_id=?2", params![canonical,duplicate])?;
        self.connection.execute("INSERT OR IGNORE INTO session_usage(session_id,report) SELECT ?1,report FROM session_usage WHERE session_id=?2", params![canonical,duplicate])?;
        self.connection.execute("INSERT OR IGNORE INTO session_handoffs(session_id,packet) SELECT ?1,packet FROM session_handoffs WHERE session_id=?2", params![canonical,duplicate])?;
        self.connection.execute("INSERT OR IGNORE INTO conversation_baselines(session_id,source,overlap_before) SELECT ?1,source,overlap_before FROM conversation_baselines WHERE session_id=?2", params![canonical,duplicate])?;
        self.connection.execute(
            "UPDATE conversation_native_ids SET session_id=?1 WHERE session_id=?2",
            params![canonical, duplicate],
        )?;
        self.connection
            .execute("DELETE FROM sessions WHERE id=?1", [duplicate])?;
        if let Some(transaction) = transaction {
            transaction.commit()?;
        }
        Ok(())
    }

    /// Compare legacy duplicate rows without materializing either full message.
    fn message_bytes_equal(&self, left: i64, right: i64) -> Result<bool> {
        let left = self
            .connection
            .blob_open("main", "messages", "text", left, true)?;
        let right = self
            .connection
            .blob_open("main", "messages", "text", right, true)?;
        if left.size() != right.size() {
            return Ok(false);
        }
        let chunk = (left.size() as usize).min(64 * 1024);
        if chunk == 0 {
            return Ok(true);
        }
        let mut a = vec![0; chunk];
        let mut b = vec![0; chunk];
        for offset in (0..left.size() as usize).step_by(a.len()) {
            let size = (left.size() as usize - offset).min(a.len());
            left.read_at_exact(&mut a[..size], offset)?;
            right.read_at_exact(&mut b[..size], offset)?;
            if a[..size] != b[..size] {
                return Ok(false);
            }
        }
        Ok(true)
    }

    #[cfg(test)]
    pub fn transcript_imported(&self, session_id: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_imports WHERE session_id=?1)",
            [session_id],
            |row| row.get(0),
        )?)
    }

    #[cfg(test)]
    pub fn transcript_fully_imported(&self, session_id: &str) -> Result<bool> {
        Ok(self.connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_imports WHERE session_id=?1 AND version>=2)",
            [session_id],
            |row| row.get(0),
        )?)
    }

    /// Stream complete native history through a disk-backed staging table.
    /// A malformed record rolls back the import; no partial snapshot is marked.
    #[cfg(test)]
    pub fn import_messages_stream(
        &self,
        session_id: &str,
        records: impl Iterator<Item = anyhow::Result<StoredMessage>>,
    ) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let imported: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_imports WHERE session_id=?1 AND version>=2)",
            [session_id],
            |row| row.get(0),
        )?;
        if imported {
            return Ok(());
        }
        transaction.execute_batch(
            "CREATE TEMP TABLE IF NOT EXISTS history_import_stage(
            sequence INTEGER PRIMARY KEY,role TEXT NOT NULL,text TEXT NOT NULL);
            DELETE FROM history_import_stage;",
        )?;
        let mut sequence = 0i64;
        {
            let mut insert = transaction
                .prepare("INSERT INTO history_import_stage(sequence,role,text) VALUES(?1,?2,?3)")?;
            for record in records {
                let message = record?;
                let Some(text) =
                    crate::presentation::sanitize_transcript_text(&message.role, &message.text)
                else {
                    continue;
                };
                insert.execute(params![sequence, message.role, text])?;
                sequence = sequence
                    .checked_add(1)
                    .context("Native history exceeds SQLite capacity")?;
            }
        }
        let mut prefix = sequence;
        {
            let mut statement = transaction.prepare(
                "SELECT role,text FROM messages WHERE session_id=?1 ORDER BY id LIMIT 256",
            )?;
            let existing = statement.query_map([session_id], |row| {
                Ok(StoredMessage {
                    role: row.get(0)?,
                    text: row.get(1)?,
                })
            })?;
            for message in existing {
                let message = message?;
                let Some(text) =
                    crate::presentation::sanitize_transcript_text(&message.role, &message.text)
                else {
                    continue;
                };
                if let Some(overlap)=transaction.query_row(
                    "SELECT sequence FROM history_import_stage WHERE role=?1 AND text=?2 ORDER BY sequence DESC LIMIT 1",
                    params![message.role,text],|row|row.get::<_,i64>(0),
                ).optional()? { prefix=overlap; break; }
            }
        }
        let minimum: i64 =
            transaction.query_row("SELECT COALESCE(MIN(id),0) FROM messages", [], |row| {
                row.get(0)
            })?;
        let start = minimum
            .checked_sub(prefix)
            .and_then(|value| value.checked_sub(1))
            .context("Native history exceeds SQLite row capacity")?;
        transaction.execute("INSERT INTO messages(id,session_id,role,text)
            SELECT ?1+sequence,?2,role,text FROM history_import_stage WHERE sequence<?3 ORDER BY sequence",
            params![start,session_id,prefix])?;
        transaction.execute(
            "INSERT INTO session_imports(session_id,version) VALUES(?1,2)
            ON CONFLICT(session_id) DO UPDATE SET version=2",
            [session_id],
        )?;
        transaction.execute("DELETE FROM history_import_stage", [])?;
        transaction.commit()?;
        Ok(())
    }

    /// Atomically snapshot a bounded native transcript before saving Orbit turns.
    /// Repeated loads never reimport newer CLI records or duplicate saved turns.
    #[cfg(test)]
    pub fn import_messages_once(&self, session_id: &str, messages: &[StoredMessage]) -> Result<()> {
        let transaction = self.connection.unchecked_transaction()?;
        let imported: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM session_imports WHERE session_id=?1)",
            [session_id],
            |row| row.get(0),
        )?;
        if imported {
            return Ok(());
        }
        let first_saved: Option<StoredMessage> = transaction
            .query_row(
                "SELECT role,text FROM messages WHERE session_id=?1 ORDER BY id LIMIT 1",
                [session_id],
                |row| {
                    Ok(StoredMessage {
                        role: row.get(0)?,
                        text: row.get(1)?,
                    })
                },
            )
            .optional()?;
        // Migration/recovery: if previous Orbit turns are already in the native
        // log, import only the history preceding those saved turns.
        let prefix = first_saved
            .as_ref()
            .and_then(|first| messages.iter().rposition(|message| message == first))
            .map_or(messages.len(), |position| position);
        let minimum: i64 =
            transaction.query_row("SELECT COALESCE(MIN(id),0) FROM messages", [], |row| {
                row.get(0)
            })?;
        for (offset, message) in messages[..prefix].iter().enumerate() {
            transaction.execute(
                "INSERT INTO messages(id,session_id,role,text) VALUES(?1,?2,?3,?4)",
                params![
                    minimum - prefix as i64 - 1 + offset as i64,
                    session_id,
                    message.role,
                    message.text
                ],
            )?;
        }
        transaction.execute(
            "INSERT INTO session_imports(session_id) VALUES(?1)",
            [session_id],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Load lightweight summaries, never transcript bodies. This also bounds sidebar memory.
    pub fn workspace_sessions(&self,project:Option<&Path>,query:&str,offset:usize)->Result<Vec<SessionSummary>>{
        let pattern=format!("%{}%",query.replace('\\',"\\\\").replace('%',"\\%").replace('_',"\\_"));
        let mut statement=self.connection.prepare("SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path FROM sessions WHERE (?1 IS NULL OR project_path=?1) AND title LIKE ?2 ESCAPE '\\' ORDER BY updated_at DESC,id LIMIT 100 OFFSET ?3")?;
        Ok(statement.query_map(params![project.map(|p|p.to_string_lossy().into_owned()),pattern,offset as i64],session_from_row)?.collect::<rusqlite::Result<Vec<_>>>()?)
    }
    pub fn sessions(&self) -> Result<Vec<SessionSummary>> {
        let mut statement = self.connection.prepare(
            "SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path
             FROM sessions ORDER BY updated_at DESC, id LIMIT 6000",
        )?;
        let rows = statement.query_map([], session_from_row)?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|session| !session.title.is_empty())
            .collect())
    }

    /// Fetch only the requested metadata IDs using the primary-key index.
    /// Archived chats can be older than the recent/project sidebar windows;
    /// never scan those windows or load message/transcript bodies to find them.
    pub fn sessions_by_id(&self, ids: &[String]) -> Result<Vec<SessionSummary>> {
        anyhow::ensure!(
            ids.len() <= SESSION_ID_PAGE_SIZE,
            "Request archived chats in pages of at most 128 IDs"
        );
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        anyhow::ensure!(
            ids.iter()
                .all(|id| !id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)),
            "Invalid archived chat ID"
        );
        let placeholders = (0..ids.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let mut statement = self.connection.prepare(&format!(
            "SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path
             FROM sessions WHERE id IN ({placeholders}) ORDER BY updated_at DESC,id"
        ))?;
        let rows = statement.query_map(rusqlite::params_from_iter(ids), session_from_row)?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|session| !session.title.is_empty())
            .collect())
    }

    /// Project navigation queries its complete local index independently of
    /// the global recent sidebar. Include sessions launched in child folders.
    pub fn sessions_for_project(&self, project: &Path) -> Result<Vec<SessionSummary>> {
        let project = project
            .canonicalize()
            .unwrap_or_else(|_| project.to_owned());
        if project.parent().is_none() {
            return Ok(Vec::new());
        }
        let prefix = format!(
            "{}/%",
            project
                .to_string_lossy()
                .trim_end_matches('/')
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut statement = self.connection.prepare(
            "SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path
             FROM sessions WHERE project_path=?1 OR project_path LIKE ?2 ESCAPE '\\'
             ORDER BY updated_at DESC,id LIMIT 1000",
        )?;
        let rows =
            statement.query_map(params![project.to_string_lossy(), prefix], session_from_row)?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|session| !session.title.is_empty())
            .collect())
    }

    pub fn save_message(&self, session_id: &str, role: &str, text: &str) -> Result<()> {
        self.save_message_with_input(session_id, role, text, None)
    }
    pub fn save_prompt(
        &self,
        session_id: &str,
        text: &str,
        input: Option<&NativeInput>,
    ) -> Result<()> {
        self.save_message_with_input(session_id, "user", text, input)
    }
    fn save_message_with_input(
        &self,
        session_id: &str,
        role: &str,
        text: &str,
        input: Option<&NativeInput>,
    ) -> Result<()> {
        anyhow::ensure!(
            input.is_none_or(NativeInput::valid),
            "Invalid native prompt association"
        );
        let input = input.map(serde_json::to_string).transpose()?;
        anyhow::ensure!(
            input.as_ref().is_none_or(|s| s.len() <= 4096),
            "Native prompt association exceeded its budget"
        );
        // The caller persists a completed message once; streaming deltas remain in memory.
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO messages (id,session_id,role,text,provider,native_input) SELECT MAX(COALESCE(MAX(id),0),0)+1,?1,?2,?3,(SELECT agent FROM sessions WHERE id=?1),?4 FROM messages",
            params![session_id, role, text, input],
        )?;
        transaction.execute(
            "UPDATE sessions SET updated_at=MAX(updated_at,?1) WHERE id=?2",
            params![chrono::Utc::now().timestamp(), session_id],
        )?;
        transaction.commit()?;
        Ok(())
    }
    pub fn save_response_source(
        &self,
        session_id: &str,
        source: &crate::response_buffer::ResponseSource,
    ) -> Result<()> {
        use crate::response_buffer::ResponseSource;
        use std::io::Write;
        match source {
            ResponseSource::Memory(text) => self.save_message(session_id, "assistant", text),
            ResponseSource::File { artifact, len } => {
                let mut input = std::fs::File::open(&artifact.path)?;
                anyhow::ensure!(
                    input.metadata()?.len() >= *len as u64,
                    "Saved response changed before it could be committed"
                );
                let bytes =
                    i32::try_from(*len).context("Response exceeds SQLite's message capacity")?;
                let transaction = self.connection.unchecked_transaction()?;
                transaction.execute("INSERT INTO messages(id,session_id,role,text,provider)
                    SELECT MAX(COALESCE(MAX(id),0),0)+1,?1,'assistant',?2,(SELECT agent FROM sessions WHERE id=?1) FROM messages",
                    params![session_id, rusqlite::blob::ZeroBlob(bytes)])?;
                let rowid = transaction.last_insert_rowid();
                {
                    let mut blob =
                        transaction.blob_open("main", "messages", "text", rowid, false)?;
                    let mut buffer = [0_u8; 64 * 1024];
                    let mut remaining = *len;
                    while remaining > 0 {
                        let size = remaining.min(buffer.len());
                        input.read_exact(&mut buffer[..size])?;
                        blob.write_all(&buffer[..size])?;
                        remaining -= size;
                    }
                }
                transaction.execute(
                    "UPDATE sessions SET updated_at=MAX(updated_at,?1) WHERE id=?2",
                    params![chrono::Utc::now().timestamp(), session_id],
                )?;
                transaction.commit()?;
                Ok(())
            }
        }
    }

    pub fn message_blob(&self, id: i64) -> Result<rusqlite::blob::Blob<'_>> {
        Ok(self
            .connection
            .blob_open("main", "messages", "text", id, true)?)
    }

    /// Read one completed local turn's short bookends at the native/local seam.
    pub fn local_fingerprint(
        &self,
        session_id: &str,
        before: i64,
    ) -> Result<Option<(i64, LocalFingerprint)>> {
        let row:Option<(i64,String,Option<String>)>=self.connection.query_row("SELECT id,role,substr(native_input,1,4097) FROM messages WHERE session_id=?1 AND id>=0 AND id<?2 ORDER BY id DESC LIMIT 1",params![session_id,before],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        let Some((id, role, input)) = row else {
            return Ok(None);
        };
        if role == "user"
            && let Some(input) = input
        {
            anyhow::ensure!(
                input.len() <= 4096,
                "Native prompt association exceeded its budget"
            );
            let input: NativeInput = serde_json::from_str(&input)?;
            anyhow::ensure!(input.valid(), "Invalid native prompt association");
            return Ok(Some((
                id,
                LocalFingerprint {
                    role,
                    prefix: input.prefix,
                    tail: input.tail,
                    bytes: input.bytes,
                    digest: Some(input.digest),
                    raw_digest: input.raw_digest,
                },
            )));
        }
        let blob = self
            .connection
            .blob_open("main", "messages", "text", id, true)?;
        let mut bytes = blob.size() as usize;
        // Checkpoint associations belong to the local presentation, never to
        // the prompt sent to the provider. Inspect only the fixed-size suffix;
        // the BLOB and its display/Undo metadata remain untouched.
        const MARKER: &[u8] = b"\n\n<!--code-checkpoint:";
        const SUFFIX_BYTES: usize = MARKER.len() + 36 + 3;
        if role == "user" && bytes >= SUFFIX_BYTES {
            let mut suffix = [0; SUFFIX_BYTES];
            blob.read_at_exact(&mut suffix, bytes - SUFFIX_BYTES)?;
            let id = &suffix[MARKER.len()..MARKER.len() + 36];
            if suffix.starts_with(MARKER)
                && suffix.ends_with(b"-->")
                && id.iter().enumerate().all(|(index, byte)| {
                    if matches!(index, 8 | 13 | 18 | 23) {
                        *byte == b'-'
                    } else {
                        byte.is_ascii_hexdigit()
                    }
                })
                && std::str::from_utf8(id)
                    .ok()
                    .and_then(|id| uuid::Uuid::parse_str(id).ok())
                    .is_some()
            {
                bytes -= SUFFIX_BYTES;
            }
        }
        let length = bytes.min(256);
        let mut prefix = vec![0; length];
        blob.read_at_exact(&mut prefix, 0)?;
        while std::str::from_utf8(&prefix).is_err() {
            prefix.pop();
        }
        let mut tail = vec![0; length];
        blob.read_at_exact(&mut tail, bytes - length)?;
        let boundary = tail.iter().take_while(|b| **b & 0xc0 == 0x80).count();
        Ok(Some((
            id,
            LocalFingerprint {
                role,
                prefix: String::from_utf8(prefix)?,
                tail: std::str::from_utf8(&tail[boundary..])?.to_owned(),
                bytes: bytes as u64,
                digest: None,
                raw_digest: None,
            },
        )))
    }

    pub(crate) fn viewport_head(&self, id: &str) -> Result<Option<i64>> {
        Ok(self
            .connection
            .query_row(
                "SELECT id FROM messages WHERE session_id=?1 ORDER BY id DESC LIMIT 1",
                [id],
                |r| r.get(0),
            )
            .optional()?)
    }
    pub(crate) fn history_extent(
        &self,
        session: &str,
        local_only: bool,
    ) -> Result<Option<(i64, i64)>> {
        let lower = if local_only { 0 } else { i64::MIN };
        let first: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM messages WHERE session_id=?1 AND id>=?2 ORDER BY id LIMIT 1",
                params![session, lower],
                |r| r.get(0),
            )
            .optional()?;
        let last: Option<i64> = self
            .connection
            .query_row(
                "SELECT id FROM messages WHERE session_id=?1 AND id>=?2 ORDER BY id DESC LIMIT 1",
                params![session, lower],
                |r| r.get(0),
            )
            .optional()?;
        Ok(first.zip(last))
    }
    pub(crate) fn seek_messages(
        &self,
        session: &str,
        local_only: bool,
        fraction: f64,
    ) -> Result<MessageCursor> {
        let Some((first, last)) = self.history_extent(session, local_only)? else {
            return Ok(MessageCursor::snapshot(local_only, None));
        };
        let f = if fraction.is_finite() {
            fraction.clamp(0., 1.)
        } else {
            1.
        };
        let mut cursor = MessageCursor::snapshot(local_only, Some(last));
        if f == 0. {
            cursor.seek_head = true;
            return Ok(cursor);
        }
        if f == 1. {
            return Ok(cursor);
        }
        let key = (first as f64 + (last as f64 - first as f64) * f) as i64;
        let row: Option<(i64,String)> = self.connection.query_row("SELECT id,role FROM messages WHERE session_id=?1 AND id>=?2 AND id<=?3 ORDER BY id DESC LIMIT 1",params![session,first,key],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
        if let Some((id, role)) = row {
            let blob = self.message_blob(id)?;
            let mut end = if first == last {
                (blob.size() as f64 * f) as usize
            } else {
                blob.size() as usize
            };
            end = end.max(1).min(blob.size() as usize);
            while end < blob.size() as usize && end > 0 {
                let mut b = [0];
                blob.read_at_exact(&mut b, end)?;
                if b[0] & 0xc0 != 0x80 {
                    break;
                }
                end -= 1;
            }
            // An interior byte position can fall inside the very first UTF-8
            // character. Include that character rather than returning an empty chat.
            if end == 0 && blob.size() > 0 {
                end = 1;
                while end < blob.size() as usize {
                    let mut byte = [0];
                    blob.read_at_exact(&mut byte, end)?;
                    if byte[0] & 0xc0 != 0x80 {
                        break;
                    }
                    end += 1;
                }
            }
            cursor.continuation = Some((id, role, end));
            cursor.before = Some(id);
        }
        Ok(cursor)
    }
    pub(crate) fn history_fraction(&self, session: &str, cursor: &MessageCursor) -> Result<f64> {
        if cursor.seek_head || cursor.exhausted && cursor.continuation.is_none() {
            return Ok(0.);
        }
        let Some((first, last)) = self.history_extent(session, cursor.local_only)? else {
            return Ok(0.);
        };
        let key = cursor
            .continuation
            .as_ref()
            .map(|(id, _, _)| *id)
            .or(cursor.before)
            .unwrap_or(last);
        if first == last {
            if let Some((id, _, end)) = &cursor.continuation {
                return Ok(*end as f64 / self.message_blob(*id)?.size().max(1) as f64);
            }
            return Ok(1.);
        }
        Ok(((key as f64 - first as f64) / (last as f64 - first as f64)).clamp(0., 1.))
    }
    fn head_messages(
        &self,
        session: &str,
        cursor: &mut MessageCursor,
        limit: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<LazyMessage>> {
        let mut output = Vec::new();
        let mut used = 0;
        let extent = self.history_extent(session, cursor.local_only)?;
        let mut head_end = 0.;
        let mut after = if cursor.local_only { 0 } else { i64::MIN };
        'rows: while output.len() < limit.clamp(1, 400) && used < 128 * 1024 {
            anyhow::ensure!(!cancelled(), "History seek was superseded");
            let row:Option<(i64,String)>=self.connection.query_row("SELECT id,role FROM messages WHERE session_id=?1 AND id>=?2 ORDER BY id LIMIT 1",params![session,after],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
            let Some((id, role)) = row else {
                break;
            };
            let blob = self.message_blob(id)?;
            let size = blob.size() as usize;
            let mut start = 0;
            let mut index = if size > 64 * 1024 {
                Some(local_history::PlainIndex::new_cancellable(
                    &blob,
                    role == "assistant",
                    cancelled,
                )?)
            } else {
                None
            };
            while start < size {
                anyhow::ensure!(!cancelled(), "History seek was superseded");
                let mut end = (start + 64 * 1024).min(size);
                while end < size {
                    let mut b = [0];
                    blob.read_at_exact(&mut b, end)?;
                    if b[0] & 0xc0 != 0x80 {
                        break;
                    }
                    end -= 1;
                }
                let mut raw = vec![0; end - start];
                blob.read_at_exact(&mut raw, start)?;
                let text = std::str::from_utf8(&raw)?;
                let (language,visible,_)=if let Some(index)=index.as_mut(){
                    index.text(&blob,start,end,role=="assistant",text,cancelled)?
                }else{(None,text.to_owned(),None)};
                if let Some(text) = (if language.is_some() {
                    Some(visible)
                } else {
                    crate::presentation::sanitize_transcript_text(&role, &visible)
                })
                .filter(|s| !s.is_empty())
                {
                    if used + text.len() > 128 * 1024 {
                        break 'rows;
                    }
                    used += text.len();
                    if let Some((first, last)) = extent {
                        head_end = ((id as f64 - first as f64 + end as f64 / size.max(1) as f64)
                            / (last as f64 - first as f64 + 1.))
                            .clamp(0., 1.);
                    }
                    let provider = self.connection.query_row(
                        "SELECT provider FROM messages WHERE id=?1",
                        [id],
                        |r| r.get(0),
                    )?;
                    output.push(LazyMessage {
                        role: role.clone(),
                        provider,
                        text,
                        code_language: language,
                    });
                    if output.len() >= limit.clamp(1, 400) {
                        break 'rows;
                    }
                }
                start = end;
            }
            if id == i64::MAX {
                break;
            }
            after = id + 1;
        }
        cursor.head_end = Some(head_end);
        cursor.seek_head = false;
        cursor.exhausted = true;
        cursor.before = Some(i64::MIN);
        cursor.continuation = None;
        cursor.code_index = None;
        Ok(output)
    }
    #[cfg(test)]
    pub fn lazy_messages(
        &self,
        session_id: &str,
        cursor: &mut MessageCursor,
        limit: usize,
    ) -> Result<Vec<LazyMessage>> {
        self.lazy_messages_cancellable(session_id, cursor, limit, &|| false)
    }
    pub(crate) fn lazy_messages_cancellable(
        &self,
        session_id: &str,
        cursor: &mut MessageCursor,
        limit: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<LazyMessage>> {
        if cursor.seek_head {
            return self.head_messages(session_id, cursor, limit, cancelled);
        }
        let mut output = Vec::new();
        let mut bytes = 0;
        let mut scanned = 0;
        while output.len() < limit.clamp(1, 400) && bytes <= 128 * 1024 && scanned <= 128 * 1024 {
            anyhow::ensure!(!cancelled(), "History seek was superseded");
            if cursor.continuation.is_none() {
                if cursor.exhausted {
                    break;
                }
                let row:Option<(i64,String)>=self.connection.query_row(
                    "SELECT id,role FROM messages WHERE session_id=?1 AND id<?2 AND id>=?3 ORDER BY id DESC LIMIT 1",
                    params![session_id,cursor.before.unwrap_or(i64::MAX),if cursor.local_only {0}else{i64::MIN}],
                    |r|Ok((r.get(0)?,r.get(1)?)),
                ).optional()?;
                let Some((id, role)) = row else {
                    cursor.exhausted = true;
                    break;
                };
                let blob = self
                    .connection
                    .blob_open("main", "messages", "text", id, true)?;
                cursor.continuation = Some((id, role, blob.size() as usize));
            }
            let (id, role, end) = cursor.continuation.clone().unwrap();
            let mut start = end.saturating_sub(64 * 1024);
            let blob = self
                .connection
                .blob_open("main", "messages", "text", id, true)?;
            let mut raw = vec![0; end - start];
            blob.read_at_exact(&mut raw, start)?;
            let boundary = raw.iter().take_while(|b| **b & 0xc0 == 0x80).count();
            start += boundary;
            let text = std::str::from_utf8(&raw[boundary..])
                .context("Cached history is not valid UTF-8")?;
            if end > 64 * 1024 && cursor.code_index.as_ref().is_none_or(|(row, _)| *row != id) {
                cursor.code_index = Some((
                    id,
                    local_history::PlainIndex::new_cancellable(
                        &blob,
                        role == "assistant",
                        cancelled,
                    )?,
                ));
            }
            scanned += raw.len();
            let (code_language,visible,skip_before)=if let Some((row,index))=&mut cursor.code_index {
                if *row==id{index.text(&blob,start,end,role=="assistant",text,cancelled)?}else{(None,text.to_owned(),None)}
            }else{(None,text.to_owned(),None)};
            let text=visible.as_str();
            // A disk snapshot knows the opening boundary of a hidden envelope.
            // Jump over its entire body instead of producing hundreds of empty
            // pages for a giant injected context string.
            if text.is_empty()
                && let Some(before) = skip_before
            {
                start = start.min(before);
            }
            if start == 0 {
                cursor.before = Some(id);
                cursor.continuation = None;
                cursor.code_index = None;
            } else {
                cursor.continuation = Some((id, role.clone(), start));
            }
            let cleaned = if text.is_empty() {
                None
            } else if code_language.is_some() || text.trim().is_empty() {
                Some(text.to_owned())
            } else {
                crate::presentation::sanitize_transcript_text(&role, text).map(|clean| {
                    if clean == text {
                        clean
                    } else if clean == text.trim() {
                        text.to_owned()
                    } else {
                        clean
                    }
                })
            };
            if let Some(text) = cleaned {
                bytes += text.len();
                let provider = self.connection.query_row(
                    "SELECT provider FROM messages WHERE id=?1",
                    [id],
                    |row| row.get(0),
                )?;
                output.push(LazyMessage {
                    provider,
                    role,
                    text,
                    code_language,
                });
            }
        }
        if cursor.continuation.is_none() && !cursor.exhausted {
            cursor.exhausted = !self.connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE session_id=?1 AND id<?2 AND id>=?3)",
                params![
                    session_id,
                    cursor.before.unwrap_or(i64::MAX),
                    if cursor.local_only { 0 } else { i64::MIN }
                ],
                |r| r.get::<_, bool>(0),
            )?;
        }
        output.reverse();
        Ok(output)
    }

    pub fn message_count(&self, session_id: &str) -> Result<usize> {
        let count: i64 = self.connection.query_row(
            "SELECT COUNT(*) FROM messages WHERE session_id=?1",
            [session_id],
            |row| row.get(0),
        )?;
        usize::try_from(count).context("History count exceeds platform capacity")
    }

    /// Fetch newest messages in chronological order without loading older history.
    #[cfg(test)]
    pub fn messages(&self, session_id: &str, limit: usize) -> Result<Vec<StoredMessage>> {
        self.messages_window(session_id, limit, 0)
    }

    /// Offset counts source rows, keeping pagination stable while legacy harness
    /// records are hidden. Only the selected window's bodies enter Rust memory.
    #[cfg(test)]
    pub fn messages_window(
        &self,
        session_id: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<StoredMessage>> {
        let mut statement=self.connection.prepare(
            "SELECT role,text FROM (
                SELECT id,role,text FROM messages WHERE session_id=?1 ORDER BY id DESC LIMIT ?2 OFFSET ?3
             ) ORDER BY id ASC",
        )?;
        let rows = statement.query_map(
            params![
                session_id,
                limit.min(10_000) as i64,
                i64::try_from(offset).unwrap_or(i64::MAX)
            ],
            |row| {
                Ok(StoredMessage {
                    role: row.get(0)?,
                    text: row.get(1)?,
                })
            },
        )?;
        let mut visible = Vec::new();
        for message in rows {
            let message = message?;
            if let Some(text) =
                crate::presentation::sanitize_transcript_text(&message.role, &message.text)
            {
                visible.push(StoredMessage {
                    role: message.role,
                    text,
                });
            }
        }
        Ok(visible)
    }

    pub fn search(&self, query: &str) -> Result<Vec<SessionSummary>> {
        let query = query.trim();
        if query.is_empty() {
            return self.sessions();
        }
        // User input is literal text, not executable FTS syntax. Bound expression size.
        let bounded: String = query.chars().take(256).collect();
        let phrase = format!("\"{}\"", bounded.replace('"', "\"\""));
        let like = format!(
            "%{}%",
            bounded
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let mut statement = self.connection.prepare(
            "SELECT id,title,agent,project_path,updated_at,external_id,transcript_path,execution_path
             FROM sessions WHERE title LIKE ?1 ESCAPE '\\' OR project_path LIKE ?1 ESCAPE '\\'
               OR id IN (SELECT m.session_id FROM message_search f
                         JOIN messages m ON m.id=f.rowid WHERE message_search MATCH ?2)
             ORDER BY updated_at DESC, id LIMIT 100",
        )?;
        let rows = statement.query_map(params![like, phrase], session_from_row)?;
        Ok(rows
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|session| !session.title.is_empty())
            .collect())
    }
}

fn session_from_row(row: &Row<'_>) -> rusqlite::Result<SessionSummary> {
    let agent: String = row.get(2)?;
    let agent = match agent.as_str() {
        "codex" => AgentKind::Codex,
        "claude" => AgentKind::Claude,
        "gemini" => AgentKind::Gemini,
        "opencode" => AgentKind::OpenCode,
        "agy" => AgentKind::Antigravity,
        "cursor-agent" => AgentKind::Cursor,
        "grok" => AgentKind::Grok,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                2,
                rusqlite::types::Type::Text,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Unknown agent in session history",
                )),
            ));
        }
    };
    Ok(SessionSummary {
        id: row.get(0)?,
        title: crate::discovery::clean_title(&row.get::<_, String>(1)?).unwrap_or_default(),
        agent,
        project_path: PathBuf::from(row.get::<_, String>(3)?),
        updated_at: row.get(4)?,
        external_id: row.get(5)?,
        transcript_path: row.get::<_, Option<String>>(6)?.map(PathBuf::from),
        execution_path: row.get::<_, Option<String>>(7)?.map(PathBuf::from),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(id: &str, title: &str) -> SessionSummary {
        SessionSummary {
            id: id.to_owned(),
            title: title.to_owned(),
            agent: AgentKind::Codex,
            project_path: PathBuf::from("/projects/orbit"),
            execution_path: None,
            updated_at: 100,
            external_id: None,
            transcript_path: None,
        }
    }

    #[test]
    fn fractional_sqlite_seek_reads_real_head_tail_and_unicode_giant_interiors() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store.upsert_session(&session("seek", "Seek")).unwrap();
        let body = format!("REAL START\n{}REAL END", "🦀 middle ".repeat(500_000));
        store.save_message("seek", "assistant", &body).unwrap();
        for f in [0., 0.5, 1.] {
            let mut cursor = store.seek_messages("seek", true, f).unwrap();
            let page = store.lazy_messages("seek", &mut cursor, 200).unwrap();
            let text = page.iter().map(|m| m.text.as_str()).collect::<String>();
            assert!(!text.is_empty());
            assert!(text.len() <= 192 * 1024);
            if f == 0. {
                assert!(text.starts_with("REAL START"));
                assert!(!cursor.has_older());
            }
            if f == 1. {
                assert!(text.ends_with("REAL END"));
            }
            if f == 0.5 {
                assert!(!text.contains("REAL START") && !text.contains("REAL END"));
            }
        }
        store
            .upsert_session(&session("tiny-seek", "Tiny seek"))
            .unwrap();
        store.save_message("tiny-seek", "user", "🦀").unwrap();
        let mut tiny = store.seek_messages("tiny-seek", true, 0.5).unwrap();
        let page = store.lazy_messages("tiny-seek", &mut tiny, 200).unwrap();
        assert_eq!(page[0].text, "🦀");
        let checks = std::cell::Cell::new(0);
        let mut cursor = store.seek_messages("seek", true, 0.).unwrap();
        assert!(
            store
                .lazy_messages_cancellable("seek", &mut cursor, 200, &|| {
                    checks.set(checks.get() + 1);
                    checks.get() > 4
                })
                .is_err()
        );
    }
    #[test]
    fn archived_id_pages_reach_beyond_sidebar_caps_without_truncating_ids() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let long_id = format!("archive:'?_%{}", "界".repeat(150));
        let mut old = session(&long_id, "Old archived chat");
        old.updated_at = 0;
        old.external_id = Some("native-identity-must-survive".into());
        old.transcript_path =
            Some("/missing/native-history-is-not-needed-for-summary.jsonl".into());
        old.execution_path = Some("/projects/orbit/worktree".into());
        let mut summaries = vec![old.clone()];
        summaries.extend((1..6002).map(|index| {
            let mut chat = session(&format!("recent-{index}"), "Recent chat");
            chat.updated_at = index;
            chat
        }));
        store.upsert_sessions(&summaries).unwrap();
        assert_eq!(store.sessions().unwrap().len(), 6000);
        assert!(
            store
                .sessions()
                .unwrap()
                .iter()
                .all(|chat| chat.id != long_id)
        );
        assert_eq!(
            store.sessions_for_project(&old.project_path).unwrap().len(),
            1000
        );
        assert!(
            store
                .sessions_for_project(&old.project_path)
                .unwrap()
                .iter()
                .all(|chat| chat.id != long_id)
        );
        let found = store
            .sessions_by_id(&[long_id.clone(), "not-saved".into(), long_id.clone()])
            .unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, long_id);
        assert_eq!(found[0].external_id, old.external_id);
        assert_eq!(found[0].transcript_path, old.transcript_path);
        assert_eq!(found[0].execution_path, old.execution_path);
        assert!(store.sessions_by_id(&[]).unwrap().is_empty());
        assert!(
            store
                .sessions_by_id(&vec![long_id; SESSION_ID_PAGE_SIZE + 1])
                .is_err()
        );
    }

    #[test]
    fn fork_binding_survives_restart_and_parent_child_rediscovery() {
        let path = std::env::temp_dir().join(format!("orbit-fork-{}.sqlite", uuid::Uuid::new_v4()));
        let store = Store::open(&path).unwrap();
        let mut original = session("codex:parent", "Original conversation");
        original.external_id = Some("parent".into());
        original.transcript_path = Some("/native/parent.jsonl".into());
        store.upsert_discovered_session(&original).unwrap();
        store
            .save_message(&original.id, "user", "Orbit fork turn")
            .unwrap();
        store
            .set_session_model(&original.id, Some("model-choice"))
            .unwrap();
        let mut child = original.clone();
        child.id = "codex:child".into();
        child.external_id = Some("child".into());
        child.title = "Native child title".into();
        child.transcript_path = Some("/native/child.jsonl".into());
        store.upsert_discovered_session(&child).unwrap();
        let mut bound = original.clone();
        bound.external_id = child.external_id.clone();
        store.bind_runtime_session(&bound).unwrap();
        drop(store);
        let store = Store::open(&path).unwrap();
        original.updated_at = 1000;
        original.title = "Parent rediscovery must not win".into();
        store.upsert_discovered_session(&original).unwrap();
        child.updated_at = 1001;
        store.upsert_discovered_session(&child).unwrap();
        // Both a queued UI write and a fresh discovery snapshot of the parent
        // must leave the authoritative child binding alone.
        store.upsert_session(&original).unwrap();
        let sessions = store.sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "codex:parent");
        assert_eq!(sessions[0].external_id.as_deref(), Some("child"));
        assert_eq!(sessions[0].title, "Original conversation");
        assert_eq!(sessions[0].transcript_path, child.transcript_path);
        assert_eq!(
            store.messages("codex:parent", 20).unwrap()[0].text,
            "Orbit fork turn"
        );
        assert_eq!(
            store.session_model("codex:parent").unwrap().as_deref(),
            Some("model-choice")
        );
        assert_eq!(store.message_count("codex:parent").unwrap(), 1);
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn streamed_response_over_twenty_mb_saves_atomically_and_paginates_without_full_text_indexing()
    {
        use crate::response_buffer::ResponseBuffer;
        let store = Store::open(Path::new(":memory:")).unwrap();
        let chat = session("streamed-response", "Streamed response fixture");
        store.upsert_session(&chat).unwrap();
        store
            .save_message(&chat.id, "user", "Ordinary searchablemarker")
            .unwrap();
        let mut output = ResponseBuffer::default();
        output.push_str("BEGIN-MARKER\n");
        let line = format!("{}\n", "🦀ab".repeat(500));
        for _ in 0..7500 {
            output.push_str(&line);
        }
        output.push_str("END-MARKER");
        let source = output.into_source().unwrap();
        assert!(source.len() > 20 * 1024 * 1024);
        store.save_response_source(&chat.id, &source).unwrap();
        let id: i64 = store
            .connection
            .query_row("SELECT MAX(id) FROM messages", [], |row| row.get(0))
            .unwrap();
        let blob = store.message_blob(id).unwrap();
        assert_eq!(blob.size() as usize, source.len());
        let mut start = [0_u8; 12];
        blob.read_at_exact(&mut start, 0).unwrap();
        assert_eq!(&start, b"BEGIN-MARKER");
        drop(blob);
        let mut cursor = MessageCursor::new(true);
        let mut total = 0;
        let mut pages = 0;
        while cursor.has_older() || pages == 0 {
            let page = store.lazy_messages(&chat.id, &mut cursor, 400).unwrap();
            let bytes = page.iter().map(|message| message.text.len()).sum::<usize>();
            assert!(bytes <= 256 * 1024);
            total += page
                .iter()
                .filter(|message| message.role == "assistant")
                .map(|message| message.text.len())
                .sum::<usize>();
            pages += 1;
        }
        assert_eq!(total, source.len());
        assert!(pages > 80);
        assert_eq!(store.search("searchablemarker").unwrap().len(), 1);
        store
            .connection
            .execute("DELETE FROM sessions WHERE id=?1", [&chat.id])
            .unwrap();
        assert!(store.search("searchablemarker").unwrap().is_empty());
        store
            .connection
            .execute(
                "INSERT INTO message_search(message_search,rank) VALUES('integrity-check',0)",
                [],
            )
            .unwrap();
    }

    #[test]
    fn fork_binding_never_retains_parent_rollout_when_child_is_not_indexed_yet() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let mut original = session("codex:parent", "Original");
        original.external_id = Some("parent".into());
        original.transcript_path = Some("/parent.jsonl".into());
        store.upsert_session(&original).unwrap();
        let mut bound = original.clone();
        bound.external_id = Some("child".into());
        store.bind_runtime_session(&bound).unwrap();
        assert!(
            store
                .session(&original.id)
                .unwrap()
                .unwrap()
                .transcript_path
                .is_none()
        );
        let mut child = bound;
        child.id = "codex:child".into();
        child.transcript_path = Some("/child.jsonl".into());
        store.upsert_discovered_session(&child).unwrap();
        assert_eq!(store.sessions().unwrap().len(), 1);
        assert_eq!(
            store
                .session(&original.id)
                .unwrap()
                .unwrap()
                .transcript_path,
            child.transcript_path
        );
    }

    #[test]
    fn handoff_goal_reads_one_small_blob_slice_and_skips_compaction_controls() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let source = session("source-goal", "Goal");
        store.upsert_session(&source).unwrap();
        store.save_message(&source.id, "user", "/compact").unwrap();
        store
            .save_message(&source.id, "user", &"日本語 🦀".repeat(1_000_000))
            .unwrap();
        let goal = store.first_saved_request(&source.id).unwrap().unwrap();
        assert!(
            goal.starts_with("日本語 🦀")
                && goal.len() <= 4096
                && goal.is_char_boundary(goal.len())
        );
    }

    #[test]
    fn model_options_survive_reopen_native_merge_and_reset_to_cli_default() {
        let path =
            std::env::temp_dir().join(format!("orbit-model-{}.sqlite", uuid::Uuid::new_v4()));
        let store = Store::open(&path).unwrap();
        let mut native = session("native", "Native");
        native.external_id = Some("native-id".into());
        store.upsert_session(&native).unwrap();
        store
            .set_session_model(&native.id, Some("configured-model"))
            .unwrap();
        let mut local = native.clone();
        local.id = "local".into();
        store.upsert_session(&local).unwrap();
        assert_eq!(
            store.session_model(&local.id).unwrap().as_deref(),
            Some("configured-model")
        );
        assert!(store.session_model(&native.id).unwrap().is_none());
        drop(store);
        let store = Store::open(&path).unwrap();
        assert_eq!(
            store.session_model(&local.id).unwrap().as_deref(),
            Some("configured-model")
        );
        store.set_session_model(&local.id, None).unwrap();
        assert!(store.session_model(&local.id).unwrap().is_none());
        drop(store);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn messages_are_paged_in_chronological_order_and_upserts_keep_history() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store.upsert_session(&session("a", "Original")).unwrap();
        for text in ["first", "second", "third"] {
            store.save_message("a", "assistant", text).unwrap();
        }
        let mut renamed = session("a", "Renamed");
        renamed.updated_at = chrono::Utc::now().timestamp() + 1;
        store.upsert_session(&renamed).unwrap();
        let messages = store.messages("a", 2).unwrap();
        assert_eq!(
            messages
                .iter()
                .map(|message| message.text.as_str())
                .collect::<Vec<_>>(),
            ["second", "third"]
        );
        assert!(store.messages("a", 0).unwrap().is_empty());
        assert_eq!(store.sessions().unwrap()[0].title, "Renamed");
        assert!(store.sessions().unwrap()[0].updated_at >= 100);
    }

    #[test]
    fn search_matches_titles_and_bodies_and_treats_user_input_literally() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store
            .upsert_session(&session("a", "Metal renderer"))
            .unwrap();
        store
            .upsert_session(&session("b", "A 100% improvement"))
            .unwrap();
        store
            .save_message(
                "a",
                "assistant",
                "SQLite persistence supports long conversations",
            )
            .unwrap();
        assert_eq!(store.search("SQLite persistence").unwrap()[0].id, "a");
        assert_eq!(store.search("Metal").unwrap()[0].id, "a");
        assert_eq!(store.search("100%").unwrap()[0].id, "b");
        assert!(store.search("\" OR *").unwrap().is_empty());
        assert!(
            store
                .search("%")
                .unwrap()
                .iter()
                .all(|session| session.id == "b")
        );
    }

    #[test]
    fn history_windows_reach_beyond_ten_thousand_turns_with_bounded_memory() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store
            .upsert_session(&session("huge", "Complete history"))
            .unwrap();
        store
            .import_messages_stream(
                "huge",
                (0..12_000).map(|index| {
                    Ok(StoredMessage {
                        role: "user".into(),
                        text: format!("User turn {index}"),
                    })
                }),
            )
            .unwrap();
        let latest = store.messages_window("huge", 400, 0).unwrap();
        assert_eq!(latest.len(), 400);
        assert_eq!(latest[0].text, "User turn 11600");
        let oldest = store.messages_window("huge", 400, 11_600).unwrap();
        assert_eq!(oldest.len(), 400);
        assert_eq!(oldest[0].text, "User turn 0");
        assert_eq!(oldest[399].text, "User turn 399");
        assert!(
            store
                .messages_window("huge", 400, usize::MAX)
                .unwrap()
                .is_empty()
        );
        assert!(store.messages_window("huge", 0, 0).unwrap().is_empty());
        assert_eq!(
            store.messages_window("huge", usize::MAX, 0).unwrap().len(),
            10_000
        );
        store.save_message("huge","assistant",r#"{"risk_level":"low","user_authorization":"high","outcome":"allow","rationale":"Internal"}"#).unwrap();
        let window = store.messages_window("huge", 3, 0).unwrap();
        assert_eq!(window.len(), 2);
        assert_eq!(window[0].text, "User turn 11998");
        let previous = store.messages_window("huge", 3, 3).unwrap();
        assert_eq!(previous[2].text, "User turn 11997");
    }

    #[test]
    fn complete_stream_upgrades_tail_cache_and_preserves_new_saved_turns() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store
            .upsert_session(&session("stream", "Long session"))
            .unwrap();
        let record = |index| StoredMessage {
            role: if index % 2 == 0 { "user" } else { "assistant" }.into(),
            text: format!("Historical turn {index}"),
        };
        store
            .import_messages_once("stream", &[record(1998), record(1999)])
            .unwrap();
        store
            .save_message("stream", "user", "New locally saved question")
            .unwrap();
        assert!(!store.transcript_fully_imported("stream").unwrap());
        store
            .import_messages_stream("stream", (0..2000).map(|index| Ok(record(index))))
            .unwrap();
        assert!(store.transcript_fully_imported("stream").unwrap());
        assert_eq!(store.message_count("stream").unwrap(), 2001);
        let messages = store.messages("stream", 2100).unwrap();
        assert_eq!(messages.len(), 2001);
        assert_eq!(messages[0].text, "Historical turn 0");
        assert_eq!(messages[2000].text, "New locally saved question");
        assert_eq!(
            store.messages("stream", 200).unwrap()[0].text,
            "Historical turn 1801"
        );
        store
            .import_messages_stream(
                "stream",
                std::iter::once(Err(anyhow::anyhow!("must not consume again"))),
            )
            .unwrap();
        assert_eq!(store.message_count("stream").unwrap(), 2001);
    }

    #[test]
    fn failed_stream_import_rolls_back_records_and_marker() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store
            .upsert_session(&session("stream", "Atomic import"))
            .unwrap();
        let good = StoredMessage {
            role: "user".into(),
            text: "A real question".into(),
        };
        assert!(
            store
                .import_messages_stream(
                    "stream",
                    [Ok(good.clone()), Err(anyhow::anyhow!("bad record"))].into_iter()
                )
                .is_err()
        );
        assert_eq!(store.message_count("stream").unwrap(), 0);
        assert!(!store.transcript_imported("stream").unwrap());
        store
            .import_messages_stream("stream", std::iter::once(Ok(good)))
            .unwrap();
        assert_eq!(store.message_count("stream").unwrap(), 1);
        assert!(store.transcript_fully_imported("stream").unwrap());
    }

    #[test]
    fn existing_database_migrates_execution_directory_and_import_version() {
        let root =
            std::env::temp_dir().join(format!("orbit-store-migrate-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("legacy.sqlite3");
        {
            let connection = Connection::open(&path).unwrap();
            connection.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY,title TEXT NOT NULL,agent TEXT NOT NULL,
                project_path TEXT NOT NULL,updated_at INTEGER NOT NULL,external_id TEXT,transcript_path TEXT);
                CREATE TABLE session_imports(session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE);
                INSERT INTO sessions VALUES('legacy','Legacy title','codex','/projects/orbit',1,NULL,NULL);
                INSERT INTO session_imports VALUES('legacy');").unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert!(store.transcript_imported("legacy").unwrap());
        assert!(!store.transcript_fully_imported("legacy").unwrap());
        let mut row = store.sessions().unwrap().remove(0);
        row.execution_path = Some(PathBuf::from("/actual/worktree"));
        store.upsert_session(&row).unwrap();
        assert_eq!(
            store.sessions().unwrap()[0].execution_path,
            row.execution_path
        );
        drop(store);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn polluted_cached_titles_and_messages_are_repaired_without_losing_real_json() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let mut local = session("local-id", "Initial prompt");
        local.external_id = Some("native-id".into());
        store.upsert_session(&local).unwrap();
        store
            .connection
            .execute(
                "UPDATE sessions SET title=?1,updated_at=9999999999 WHERE id=?2",
                params![
                    "The following is the Codex agent history added since the last turn",
                    local.id
                ],
            )
            .unwrap();
        assert!(
            store.sessions().unwrap().is_empty(),
            "Internal harness chats stay out of navigation until a real title is recovered"
        );
        let mut refreshed = local.clone();
        refreshed.id = "codex:native-id".into();
        refreshed.title = "Actual user question".into();
        store.upsert_discovered_session(&refreshed).unwrap();
        assert_eq!(store.sessions().unwrap()[0].title, "Actual user question");
        store.save_message(&local.id,"assistant",r#"{"risk_level":"low","user_authorization":"high","outcome":"allow","rationale":"Internal approval"}"#).unwrap();
        store
            .save_message(&local.id, "user", r#"{"outcome":"allow"}"#)
            .unwrap();
        store
            .save_message(
                &local.id,
                "tool",
                r#"{"orbit_tool":true,"label":"Read file","details":"src/main.rs"}"#,
            )
            .unwrap();
        let messages = store.messages(&local.id, 20).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, r#"{"outcome":"allow"}"#);
        assert_eq!(messages[1].role, "tool");
    }

    #[test]
    fn project_query_includes_child_folders_but_not_similarly_named_workspaces() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        for (id, path) in [
            ("root", "/projects/orbit"),
            ("child", "/projects/orbit/src"),
            ("other", "/projects/orbit-other"),
        ] {
            let mut row = session(id, id);
            row.project_path = PathBuf::from(path);
            store.upsert_session(&row).unwrap();
        }
        let rows = store
            .sessions_for_project(Path::new("/projects/orbit"))
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(!rows.iter().any(|row| row.id == "other"));
        assert!(
            store
                .sessions_for_project(Path::new("/"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn foreign_keys_prevent_orphans_and_file_store_uses_wal() {
        let root = std::env::temp_dir().join(format!("orbit-store-{}", uuid::Uuid::new_v4()));
        let path = root.join("history.sqlite3");
        {
            let store = Store::open(&path).unwrap();
            assert!(store.save_message("missing", "user", "hello").is_err());
            let mode: String = store
                .connection
                .query_row("PRAGMA journal_mode", [], |row| row.get(0))
                .unwrap();
            assert_eq!(mode, "wal");
            store.upsert_session(&session("a", "Persistent")).unwrap();
            store.save_message("a", "user", "hello").unwrap();
        }
        let reopened = Store::open(&path).unwrap();
        assert_eq!(reopened.messages("a", 10).unwrap()[0].text, "hello");
        drop(reopened);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn session_ownership_moves_preserve_fts_without_retokenizing_text() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store.upsert_session(&session("before", "Before")).unwrap();
        store.upsert_session(&session("after", "After")).unwrap();
        store
            .save_message("before", "user", "unique searchable content")
            .unwrap();
        let changed = store.connection.total_changes();
        store
            .connection
            .execute(
                "UPDATE messages SET session_id='after' WHERE session_id='before'",
                [],
            )
            .unwrap();
        assert_eq!(
            store.connection.total_changes() - changed,
            1,
            "Only the ownership row should change, not the FTS index"
        );
        assert_eq!(store.search("searchable").unwrap()[0].id, "after");
        store
            .connection
            .execute(
                "UPDATE messages SET text='replacement indexed text' WHERE session_id='after'",
                [],
            )
            .unwrap();
        assert!(store.search("searchable").unwrap().is_empty());
        assert_eq!(store.search("replacement").unwrap()[0].id, "after");
    }
    #[test]
    fn batch_upserts_are_atomic_skip_noops_and_allow_native_duplicate_merges() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        let values = vec![session("a", "First"), session("b", "Second")];
        store.upsert_sessions(&values).unwrap();
        let changed = store.connection.total_changes();
        store.upsert_sessions(&values).unwrap();
        assert_eq!(store.connection.total_changes(), changed);
        let mut native = session("codex:native", "Native");
        native.external_id = Some("native".into());
        store.upsert_session(&native).unwrap();
        store
            .save_message(&native.id, "user", "native history")
            .unwrap();
        let mut local = session("local", "Local");
        local.external_id = native.external_id.clone();
        store.upsert_sessions(std::slice::from_ref(&local)).unwrap();
        assert_eq!(
            store.messages("local", 10).unwrap()[0].text,
            "native history"
        );
        assert!(
            store
                .sessions()
                .unwrap()
                .iter()
                .all(|s| s.id != "codex:native")
        );
        store.connection.execute_batch("CREATE TRIGGER reject_fixture BEFORE INSERT ON sessions WHEN new.id='reject' BEGIN SELECT RAISE(ABORT,'fixture rejected'); END;").unwrap();
        let count = store.sessions().unwrap().len();
        assert!(
            store
                .upsert_sessions(&[
                    session("rollback", "Rollback"),
                    session("reject", "Rejected")
                ])
                .is_err()
        );
        assert_eq!(store.sessions().unwrap().len(), count);
        assert!(store.sessions().unwrap().iter().all(|s| s.id != "rollback"));
    }
    #[test]
    fn every_provider_survives_persisted_session_reopening() {
        let path = std::env::temp_dir().join(format!(
            "orbit-provider-store-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        {
            let store = Store::open(&path).unwrap();
            for agent in [
                AgentKind::Codex,
                AgentKind::Claude,
                AgentKind::Gemini,
                AgentKind::OpenCode,
                AgentKind::Antigravity,
                AgentKind::Cursor,
                AgentKind::Grok,
            ] {
                let mut value = session(agent.command(), agent.name());
                value.agent = agent;
                store.upsert_session(&value).unwrap();
            }
        }
        let store = Store::open(&path).unwrap();
        for value in store.sessions().unwrap() {
            assert_eq!(value.id, value.agent.command());
        }
        assert_eq!(store.sessions().unwrap().len(), 7);
        drop(store);
        let _ = std::fs::remove_file(path);
    }
    #[test]
    fn giant_cached_harness_context_never_leaks_from_middle_pages() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store
            .upsert_session(&session("plugins", "Cached plugin context"))
            .unwrap();
        // The opening tag crosses a 64KiB source block, and the context contains
        // Markdown fences that must not change the real message's code context.
        let padding = " ".repeat(64 * 1024 - 10);
        let context =
            "- Figma (figma@openai-curated-remote)\n```xml\nprivate context\n```\n".repeat(300_000);
        let text = format!(
            "{padding}<recommended_plugins>\n{context}</recommended_plugins>\nReal request.\n```xml\n<recommended_plugins>literal example</recommended_plugins>\n``` "
        );
        assert!(text.len() > 16 * 1024 * 1024);
        store.save_message("plugins", "user", &text).unwrap();
        let mut cursor = MessageCursor::new(false);
        let mut pages = Vec::new();
        while cursor.has_older() {
            let page = store.lazy_messages("plugins", &mut cursor, 200).unwrap();
            assert!(page.iter().map(|m| m.text.len()).sum::<usize>() <= 192 * 1024);
            assert!(
                page.iter()
                    .all(|m| !m.text.contains("figma@") && !m.text.contains("private context"))
            );
            pages.push(page.iter().map(|m| m.text.as_str()).collect::<String>());
        }
        assert!(
            pages.len() < 5,
            "Hidden context should not require empty-page navigation"
        );
        pages.reverse();
        let visible = pages.concat();
        assert!(visible.contains("Real request."));
        assert!(visible.contains("<recommended_plugins>literal example</recommended_plugins>"));
        let unchanged: bool = store
            .connection
            .query_row(
                "SELECT text=?1 FROM messages WHERE session_id='plugins'",
                [&text],
                |row| row.get(0),
            )
            .unwrap();
        assert!(unchanged);
    }
    #[test]
    fn saved_cards_remain_typed_across_blob_pages_without_changing_source(){
        use crate::chat_interactions::{ChatInteraction,InteractionStatus};
        let store=Store::open(Path::new(":memory:")).unwrap();store.upsert_session(&session("cards","Saved cards")).unwrap();
        let details="diff --git a/view.swift b/view.swift\n+let text = \"☃ literal \\n and <tag> `ticks`\"\n".repeat(8000);
        let card=ChatInteraction::Status{title:"Changes".into(),details:details.clone(),status:InteractionStatus::Completed};
        let source=format!("Before\n{}\nAfter",crate::chat_interactions::serialize_interaction(&card));
        store.save_message("cards","assistant",&source).unwrap();
        let mut cursor=MessageCursor::new(false);let mut pages=vec![];
        while cursor.has_older(){let mut page=store.lazy_messages("cards",&mut cursor,200).unwrap();assert!(page.iter().map(|m|m.text.len()).sum::<usize>()<220*1024);page.append(&mut pages);pages=page;}
        let mut actual=String::new();let mut prose=String::new();
        for page in pages{for block in crate::presentation::message_blocks("assistant",&page.text){match block{
            crate::presentation::MessageBlock::Interaction(ChatInteraction::Status{title,details,..})=>{assert_eq!(title,"Changes");actual.push_str(&details)},
            crate::presentation::MessageBlock::Paragraph(text)=>prose.push_str(&text),
            other=>panic!("Saved card leaked into {other:?}"),
        }}}
        assert_eq!(actual,details);assert_eq!(prose,"BeforeAfter");
        let stored:String=store.connection.query_row("select text from messages where session_id='cards'",[],|r|r.get(0)).unwrap();assert_eq!(stored,source);
        // A literal example in a fenced code block remains source code.
        let fenced=format!("```xml\n{}\n```",source);store.upsert_session(&session("literal","Literal")).unwrap();store.save_message("literal","assistant",&fenced).unwrap();
        let mut cursor=MessageCursor::new(false);let mut raw=0;while cursor.has_older(){for page in store.lazy_messages("literal",&mut cursor,200).unwrap(){raw+=page.text.len();}}assert_eq!(raw,fenced.len());
    }
    #[test]
    #[ignore="Read-only replay of an explicitly selected saved reply copied into an isolated fixture"]
    fn existing_saved_reply_keeps_cards_typed_on_every_page(){
        let source=std::fs::read_to_string(std::env::var("KAI_SAVED_HISTORY_FIXTURE").expect("Select a saved reply fixture")).unwrap();
        let store=Store::open(Path::new(":memory:")).unwrap();store.upsert_session(&session("existing","Existing reply")).unwrap();store.save_message("existing","assistant",&source).unwrap();
        let mut cursor=MessageCursor::new(false);let mut cards=0;
        while cursor.has_older(){for page in store.lazy_messages("existing",&mut cursor,200).unwrap(){for block in crate::presentation::message_blocks("assistant",&page.text){match block{
            crate::presentation::MessageBlock::Interaction(_)=>cards+=1,
            crate::presentation::MessageBlock::Paragraph(text)|crate::presentation::MessageBlock::Code{text,..}|crate::presentation::MessageBlock::RichParagraph{text,..}=>assert!(!text.contains("\"orbit_interaction\"") && !text.contains("\\u003c") && !text.contains("\"status\":\"working\""),"Encoded saved card leaked into visible prose"),
            _=>{},
        }}}}
        assert!(cards>100,"Saved fixture did not exercise the older card history");eprintln!("Actual saved SQLite reply: {cards} typed cards across every page; original data unchanged");
    }
    #[test]
    fn incremental_blob_pages_preserve_a_twenty_mb_unicode_message_and_code_context() {
        let store = Store::open(Path::new(":memory:")).unwrap();
        store
            .upsert_session(&session("large-local", "Large local response"))
            .unwrap();
        let text = format!("```text\n{}\n```", "☃α x \n".repeat(2_400_000));
        assert!(text.len() > 16 * 1024 * 1024);
        store
            .save_message("large-local", "assistant", &text)
            .unwrap();
        let start = std::time::Instant::now();
        let mut cursor = MessageCursor::new(false);
        let mut bytes = 0;
        let mut chunks = 0;
        while cursor.has_older() {
            let page = store
                .lazy_messages("large-local", &mut cursor, 200)
                .unwrap();
            let page_bytes = page.iter().map(|m| m.text.len()).sum::<usize>();
            assert!(page_bytes <= 192 * 1024);
            for message in page {
                if !message.text.starts_with("```text\n") {
                    assert_eq!(message.code_language.as_deref(), Some("text"));
                }
                bytes += message.text.len();
                chunks += 1;
            }
        }
        assert_eq!(bytes, text.len());
        assert!(chunks > 100);
        eprintln!(
            "{} MiB SQLite BLOB history paged in {:?}",
            text.len() / 1024 / 1024,
            start.elapsed()
        );
    }
}
