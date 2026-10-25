//! Readonly, backwards paging of OpenCode's native SQLite parts. Only the
//! selected part is streamed to a private temporary envelope; whole sessions
//! are never materialized or copied into the application's database.
use crate::{
    discovery::AgentKind,
    presentation,
    runtime::{TranscriptMessage, history::HistoryCursor},
    store::LocalFingerprint,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use std::{
    collections::VecDeque,
    fs::OpenOptions as FileOptions,
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};
const BUFFER: usize = 64 * 1024;
const METADATA_BATCH: usize = 128;
type PartMetadata = (i64, i64, i64, i64, String);

pub struct OpenCodeCursor {
    connection: Connection,
    source: PathBuf,
    stamp: crate::history_artifact::FileStamp,
    wal_stamp: Option<crate::history_artifact::FileStamp>,
    session: String,
    before: Option<(i64, i64, i64, i64)>,
    upper: (i64, i64, i64, i64),
    exhausted: bool,
    seek_head: bool,
    part_seek: Option<f64>,
    pub(crate) head_end: Option<f64>,
    part: Option<(HistoryCursor, Arc<crate::history_artifact::HistoryArtifact>)>,
    pending: Vec<TranscriptMessage>,
    metadata: VecDeque<PartMetadata>,
    #[cfg(test)]
    metadata_queries: usize,
    #[cfg(test)]
    spooled_parts: usize,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct OpenCodeCheckpoint {
    source: PathBuf,
    stamp: crate::history_artifact::FileStamp,
    wal_stamp: Option<crate::history_artifact::FileStamp>,
    session: String,
    before: Option<(i64, i64, i64, i64)>,
    upper: (i64, i64, i64, i64),
    exhausted: bool,
    #[serde(default)]
    seek_head: bool,
    #[serde(default)]
    part_seek: Option<f64>,
    part: Option<(crate::runtime::history::HistoryCheckpoint, PathBuf)>,
    pending: Vec<TranscriptMessage>,
    metadata: VecDeque<PartMetadata>,
}
fn wal_path(source: &Path) -> PathBuf {
    let mut p = source.as_os_str().to_os_string();
    p.push("-wal");
    PathBuf::from(p)
}
impl OpenCodeCheckpoint {
    pub(crate) fn matches_session(&self, session: &crate::discovery::SessionSummary) -> bool {
        session.agent == AgentKind::OpenCode
            && session.transcript_path.as_ref() == Some(&self.source)
            && session.external_id.as_deref().unwrap_or(&session.id) == self.session
    }
    pub(crate) fn source_path(&self) -> &PathBuf {
        &self.source
    }
    pub(crate) fn rewrite_artifacts(&mut self, map: &std::collections::HashMap<PathBuf, PathBuf>) {
        if let Some((cursor, path)) = &mut self.part {
            cursor.rewrite_artifacts(map);
            if let Some(saved) = map.get(path) {
                *path = saved.clone();
                cursor.source = saved.clone();
            }
        }
    }
}

impl Drop for OpenCodeCursor {
    fn drop(&mut self) {
        self.clear_part();
    }
}
impl OpenCodeCursor {
    pub fn open(path: &Path, session: &str) -> Result<Self> {
        let connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .context("Could not open OpenCode history")?;
        connection.execute_batch(
            "PRAGMA query_only=ON; PRAGMA cache_size=-4096; PRAGMA temp_store=FILE;",
        )?;
        let upper: Option<(i64,i64,i64,i64)> = connection.query_row(
            "SELECT m.time_created,m.rowid,p.time_created,p.rowid FROM part p JOIN message m ON p.message_id=m.id WHERE p.session_id=?1 ORDER BY m.time_created DESC,m.rowid DESC,p.time_created DESC,p.rowid DESC LIMIT 1",
            [session], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))
        ).optional()?;
        Ok(Self {
            connection,
            source: path.to_path_buf(),
            stamp: crate::history_artifact::FileStamp::read(path)?,
            wal_stamp: crate::history_artifact::FileStamp::read(&wal_path(path)).ok(),
            session: session.into(),
            before: None,
            upper: upper.unwrap_or((0, 0, 0, 0)),
            exhausted: upper.is_none(),
            seek_head: false,
            part_seek: None,
            head_end: None,
            part: None,
            pending: Vec::new(),
            metadata: VecDeque::new(),
            #[cfg(test)]
            metadata_queries: 0,
            #[cfg(test)]
            spooled_parts: 0,
        })
    }
    fn first_key(&self) -> Result<Option<(i64, i64, i64, i64)>> {
        Ok(self.connection.query_row("SELECT m.time_created,m.rowid,p.time_created,p.rowid FROM part p JOIN message m ON p.message_id=m.id WHERE p.session_id=?1 ORDER BY m.time_created,m.rowid,p.time_created,p.rowid LIMIT 1",[&self.session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?)
    }
    pub(crate) fn seek_fraction(&mut self, fraction: f64) -> Result<()> {
        self.head_end = None;
        self.part_seek = None;
        self.pending.clear();
        self.metadata.clear();
        self.before = None;
        let f = if fraction.is_finite() {
            fraction.clamp(0., 1.)
        } else {
            1.
        };
        let Some(first) = self.first_key()? else {
            self.clear_part();
            self.exhausted = true;
            return Ok(());
        };
        self.exhausted = false;
        self.seek_head = f == 0.;
        // A single giant part already has a seekable private envelope and a
        // sparse native span index. Reposition those instead of copying and
        // indexing its entire SQLite cell on every scrollbar release.
        if first == self.upper
            && let Some((cursor, _)) = &mut self.part
        {
            cursor.seek_fraction(f, &|| false)?;
            self.before = Some(self.upper);
            self.part_seek = Some(f);
            return Ok(());
        }
        self.clear_part();
        if self.seek_head {
            self.before = Some((i64::MIN, i64::MIN, i64::MIN, i64::MIN));
        } else if f < 1. {
            if first == self.upper {
                self.part_seek = Some(f);
                return Ok(());
            }
            let key = if first.0 != self.upper.0 {
                (
                    (first.0 as f64 + (self.upper.0 as f64 - first.0 as f64) * f) as i64,
                    i64::MAX,
                    i64::MAX,
                    i64::MAX,
                )
            } else if first.1 != self.upper.1 {
                (
                    first.0,
                    (first.1 as f64 + (self.upper.1 as f64 - first.1 as f64) * f) as i64,
                    i64::MAX,
                    i64::MAX,
                )
            } else if first.2 != self.upper.2 {
                (
                    first.0,
                    first.1,
                    (first.2 as f64 + (self.upper.2 as f64 - first.2 as f64) * f) as i64,
                    i64::MAX,
                )
            } else {
                (
                    first.0,
                    first.1,
                    first.2,
                    ((first.3 as f64 + (self.upper.3 as f64 - first.3 as f64) * f) as i64)
                        .saturating_add(1),
                )
            };
            self.before = Some(key);
        }
        Ok(())
    }
    pub(crate) fn source_fraction(&self) -> Result<f64> {
        if let Some(fraction) = self.part_seek {
            return Ok(fraction);
        }
        if self.seek_head || self.exhausted && self.pending.is_empty() && self.part.is_none() {
            return Ok(0.);
        }
        let Some(first) = self.first_key()? else {
            return Ok(0.);
        };
        if first == self.upper
            && let Some((cursor, _)) = &self.part
        {
            return Ok(cursor.source_fraction());
        }
        let Some(key) = self.before else {
            return Ok(1.);
        };
        let (start, end, at) = if first.0 != self.upper.0 {
            (first.0, self.upper.0, key.0)
        } else if first.1 != self.upper.1 {
            (first.1, self.upper.1, key.1)
        } else if first.2 != self.upper.2 {
            (first.2, self.upper.2, key.2)
        } else {
            (first.3, self.upper.3, key.3)
        };
        Ok(if start == end {
            0.5
        } else {
            ((at as f64 - start as f64) / (end as f64 - start as f64)).clamp(0., 1.)
        })
    }
    #[cfg(test)]
    pub(crate) fn restore_io_counts(&self) -> (usize, u64) {
        (
            self.spooled_parts,
            self.part
                .as_ref()
                .map_or(0, |(cursor, _)| cursor.bytes_read),
        )
    }
    pub(crate) fn source_matches(&self) -> bool {
        self.stamp.matches(&self.source)
            && crate::history_artifact::FileStamp::read(&wal_path(&self.source)).ok()
                == self.wal_stamp
    }
    pub(crate) fn checkpoint(&self) -> OpenCodeCheckpoint {
        OpenCodeCheckpoint {
            source: self.source.clone(),
            stamp: self.stamp.clone(),
            wal_stamp: self.wal_stamp.clone(),
            session: self.session.clone(),
            before: self.before,
            upper: self.upper,
            exhausted: self.exhausted,
            seek_head: self.seek_head,
            part_seek: self.part_seek,
            part: self
                .part
                .as_ref()
                .map(|(cursor, path)| (cursor.checkpoint(), path.path.clone())),
            pending: self.pending.clone(),
            metadata: self.metadata.clone(),
        }
    }
    pub(crate) fn artifacts(&self) -> Vec<Arc<crate::history_artifact::HistoryArtifact>> {
        self.part.as_ref().map_or_else(Vec::new, |(cursor, path)| {
            let mut v = cursor.artifacts();
            v.push(path.clone());
            v
        })
    }
    pub(crate) fn restore(
        c: OpenCodeCheckpoint,
        resolve: &mut impl FnMut(&Path) -> Result<Arc<crate::history_artifact::HistoryArtifact>>,
    ) -> Result<Self> {
        anyhow::ensure!(
            c.stamp.matches(&c.source)
                && crate::history_artifact::FileStamp::read(&wal_path(&c.source)).ok()
                    == c.wal_stamp,
            "OpenCode history changed since the saved viewport"
        );
        anyhow::ensure!(
            c.session.len() <= 512
                && c.pending.len() <= 1024
                && c.pending.iter().map(|m| m.text.len()).sum::<usize>() <= 2 * 1024 * 1024
                && c.metadata.len() <= METADATA_BATCH,
            "Invalid OpenCode checkpoint"
        );
        let mut cursor = Self::open(&c.source, &c.session)?;
        anyhow::ensure!(
            cursor.upper == c.upper,
            "OpenCode history changed since the saved viewport"
        );
        cursor.before = c.before;
        cursor.upper = c.upper;
        cursor.exhausted = c.exhausted;
        cursor.seek_head = c.seek_head;
        cursor.part_seek = c.part_seek;
        cursor.pending = c.pending;
        cursor.metadata = c.metadata;
        if let Some((state, path)) = c.part {
            let artifact = resolve(&path)?;
            let part = HistoryCursor::restore(state, Some(artifact.path.clone()), resolve)?;
            cursor.part = Some((part, artifact));
        }
        Ok(cursor)
    }
    fn clear_part(&mut self) {
        if let Some((cursor, path)) = self.part.take() {
            drop(cursor);
            drop(path);
        }
    }
    pub fn has_older(&self) -> bool {
        !self.exhausted
            || !self.pending.is_empty()
            || self
                .part
                .as_ref()
                .is_some_and(|(cursor, _)| cursor.has_older())
    }
    fn next_part(&mut self, cancelled: &impl Fn() -> bool) -> Result<bool> {
        self.clear_part();
        let key = self
            .before
            .unwrap_or((i64::MAX, i64::MAX, i64::MAX, i64::MAX));
        if self.metadata.is_empty() {
            #[cfg(test)]
            {
                self.metadata_queries += 1;
            }
            let query = if self.seek_head {
                "SELECT m.time_created,m.rowid,p.time_created,p.rowid,json_extract(m.data,'$.role') FROM part p JOIN message m ON p.message_id=m.id WHERE p.session_id=?1 AND (m.time_created,m.rowid,p.time_created,p.rowid)>(?2,?3,?4,?5) AND (m.time_created,m.rowid,p.time_created,p.rowid)<=(?6,?7,?8,?9) ORDER BY m.time_created,m.rowid,p.time_created,p.rowid LIMIT ?10"
            } else {
                "SELECT m.time_created,m.rowid,p.time_created,p.rowid,json_extract(m.data,'$.role') FROM part p JOIN message m ON p.message_id=m.id WHERE p.session_id=?1 AND (m.time_created,m.rowid,p.time_created,p.rowid)<(?2,?3,?4,?5) AND (m.time_created,m.rowid,p.time_created,p.rowid)<=(?6,?7,?8,?9) ORDER BY m.time_created DESC,m.rowid DESC,p.time_created DESC,p.rowid DESC LIMIT ?10"
            };
            let mut statement = self.connection.prepare_cached(query)?;
            let rows = statement.query_map(
                params![
                    self.session,
                    key.0,
                    key.1,
                    key.2,
                    key.3,
                    self.upper.0,
                    self.upper.1,
                    self.upper.2,
                    self.upper.3,
                    METADATA_BATCH
                ],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
            self.metadata = rows.collect::<rusqlite::Result<VecDeque<_>>>()?;
        }
        let row = self.metadata.pop_front();
        let Some((created, message, part_created, rowid, role)) = row else {
            self.exhausted = true;
            return Ok(false);
        };
        self.before = Some((created, message, part_created, rowid));
        if !matches!(role.as_str(), "user" | "assistant") {
            return Ok(true);
        }
        let mut blob = self
            .connection
            .blob_open("main", "part", "data", rowid, true)?;
        if blob.size() as usize <= BUFFER * 2 {
            self.part_seek = None;
            let mut raw = Vec::with_capacity(blob.size() as usize);
            blob.read_to_end(&mut raw)?;
            let value: serde_json::Value =
                serde_json::from_slice(&raw).context("Malformed OpenCode history part")?;
            if value["type"] == "tool" {
                let tool = value["tool"].as_str().unwrap_or("tool");
                let status = value["state"]["status"].as_str().unwrap_or("used");
                let detail = value["state"]["output"]
                    .as_str()
                    .or_else(|| value["state"]["error"].as_str())
                    .unwrap_or("");
                self.pending.extend(crate::runtime::history::split_message(TranscriptMessage { role:"tool".into(), code_language:None,
                    text:serde_json::json!({"orbit_tool":true,"label":format!("{tool} · {status}"),"details":detail}).to_string() }));
                if self.seek_head {
                    self.pending.reverse();
                }
                return Ok(true);
            }
            if value["type"] != "text" {
                return Ok(true);
            }
            if let Some(text) = value["text"]
                .as_str()
                .and_then(|text| presentation::sanitize_transcript_text(&role, text))
            {
                self.pending
                    .extend(crate::runtime::history::split_message(TranscriptMessage {
                        role,
                        code_language: None,
                        text,
                    }));
            }
            if self.seek_head {
                self.pending.reverse();
            }
        } else {
            // Large parts retain the bounded disk span index; short parts never
            // need temporary files, a second JSON parse, or filesystem syscalls.
            let path = Self::spool(&role, &mut blob, cancelled)?;
            match HistoryCursor::open(&path, AgentKind::Codex) {
                Ok(mut cursor) => {
                    if self.seek_head {
                        cursor.seek_fraction(0., cancelled)?;
                    } else if let Some(fraction) = self.part_seek.take() {
                        cursor.seek_fraction(fraction, cancelled)?;
                    }
                    self.part = Some((cursor, crate::history_artifact::HistoryArtifact::own(path)))
                }
                Err(error) => {
                    let _ = std::fs::remove_file(path);
                    return Err(error);
                }
            }
            #[cfg(test)]
            {
                self.spooled_parts += 1;
            }
        }
        Ok(true)
    }
    fn spool(role: &str, mut source: impl Read, cancelled: &impl Fn() -> bool) -> Result<PathBuf> {
        let path = std::env::temp_dir().join(format!(
            "orbit-opencode-part-{}.jsonl",
            uuid::Uuid::new_v4()
        ));
        let mut file = FileOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let result = (|| -> Result<()> {
            write!(
                file,
                "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"{role}\",\"content\":["
            )?;
            let mut buffer = [0; BUFFER];
            loop {
                anyhow::ensure!(!cancelled(), "History seek was superseded");
                let size = source.read(&mut buffer)?;
                if size == 0 {
                    break;
                }
                file.write_all(&buffer[..size])?;
            }
            file.write_all(b"]}}\n")?;
            Ok(())
        })();
        if let Err(error) = result {
            let _ = std::fs::remove_file(&path);
            return Err(error);
        }
        drop(file);
        Ok(path)
    }
    #[cfg(test)]
    pub fn page_budget(&mut self, limit: usize, budget: usize) -> Result<Vec<TranscriptMessage>> {
        self.page_budget_cancellable(limit, budget, &|| false)
    }
    pub(crate) fn page_budget_cancellable(
        &mut self,
        limit: usize,
        budget: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<TranscriptMessage>> {
        if self.exhausted && !self.seek_head && self.pending.is_empty() {
            return Ok(Vec::new());
        }
        if self.part.is_some() {
            // A retained part was already positioned by seek_fraction; from
            // here source_fraction reports the actual decoded window boundary.
            self.part_seek = None;
        }
        let forward = self.seek_head;
        let mut output = Vec::new();
        let mut used = 0;
        let mut scanned = 0;
        while output.len() < limit.clamp(1, 400) && scanned < 400 {
            anyhow::ensure!(!cancelled(), "History seek was superseded");
            if let Some(message) = self.pending.pop() {
                if used + message.text.len() > budget {
                    self.pending.push(message);
                    break;
                }
                used += message.text.len();
                output.push(message);
            } else if let Some((cursor, _)) = &mut self.part {
                if !cursor.has_older() {
                    if self.first_key()? == Some(self.upper) {
                        self.exhausted = true;
                        break;
                    }
                    self.clear_part();
                    continue;
                }
                let mut page = cursor.page_budget_cancellable(
                    limit - output.len(),
                    budget - used,
                    cancelled,
                )?;
                if page.is_empty() && cursor.has_older() {
                    break;
                }
                used += page.iter().map(|m| m.text.len()).sum::<usize>();
                if !forward {
                    page.reverse();
                }
                output.extend(page);
            } else if self.exhausted || !self.next_part(cancelled)? {
                break;
            } else {
                scanned += 1;
            }
            if used >= budget {
                break;
            }
        }
        if !forward {
            output.reverse();
        } else {
            self.seek_head = false;
            let single_part = self.first_key()? == Some(self.upper);
            let partial = self.part.as_ref().and_then(|(cursor, _)| cursor.head_end);
            self.head_end = Some(if self.exhausted {
                1.
            } else if single_part {
                partial.unwrap_or(if self.pending.is_empty() { 1. } else { 0.5 })
            } else {
                self.source_fraction()?
            });
            self.exhausted = true;
            self.metadata.clear();
            self.pending.clear();
            if !single_part {
                self.clear_part();
            }
        }
        Ok(output)
    }
    #[cfg(test)]
    pub fn skip_local_copy(
        &mut self,
        fingerprint: &LocalFingerprint,
        expected: &rusqlite::blob::Blob<'_>,
    ) -> Result<bool> {
        self.skip_local_copy_cancellable(fingerprint, expected, &|| false)
    }
    pub(crate) fn skip_local_copy_cancellable(
        &mut self,
        fingerprint: &LocalFingerprint,
        expected: &rusqlite::blob::Blob<'_>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool> {
        // Keep the newest source part available for the same seam check as JSONL.
        if self
            .part
            .as_ref()
            .is_some_and(|(cursor, _)| !cursor.has_older())
        {
            self.clear_part();
        }
        while self.part.is_none() && self.pending.is_empty() && !self.exhausted {
            if !self.next_part(cancelled)? {
                break;
            }
        }
        if let Some((cursor, _)) = &mut self.part {
            cursor.skip_local_copy_cancellable(fingerprint, expected, cancelled)
        } else if self
            .pending
            .iter()
            .all(|message| message.role == fingerprint.role)
        {
            let mut comparison =
                crate::runtime::history::BodyComparison::new(expected, fingerprint, false);
            for message in &self.pending {
                if !comparison.feed(message.text.as_bytes())? {
                    return Ok(false);
                }
            }
            if comparison.complete() {
                self.pending.clear();
                Ok(true)
            } else {
                Ok(false)
            }
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn giant_single_opencode_part_seeks_distinct_ten_fifty_ninety_percent_bodies() {
        let path = std::env::temp_dir().join(format!(
            "orbit-opencode-fraction-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch(r#"CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT);INSERT INTO message VALUES('m','selected',42,'{"role":"assistant"}');"#).unwrap();
        let text = format!(
            "{}{}{}",
            "A".repeat(2 * 1024 * 1024),
            "B".repeat(2 * 1024 * 1024),
            "C".repeat(2 * 1024 * 1024)
        );
        writer
            .execute(
                "INSERT INTO part VALUES('p','m','selected',42,?1)",
                [serde_json::json!({"type":"text","text":text}).to_string()],
            )
            .unwrap();
        drop(writer);
        let mut cursor = OpenCodeCursor::open(&path, "selected").unwrap();
        let mut prepared = None;
        let mut spans = None;
        for (fraction, expected) in [(0.1, 'A'), (0.5, 'B'), (0.9, 'C'), (0., 'A'), (0.5, 'B')] {
            let read_before = cursor.restore_io_counts().1;
            let already_prepared = prepared.is_some();
            cursor.seek_fraction(fraction).unwrap();
            assert_eq!(cursor.source_fraction().unwrap(), fraction);
            let page = cursor
                .page_budget_cancellable(200, 256 * 1024, &|| false)
                .unwrap();
            let body = page.iter().map(|m| m.text.as_str()).collect::<String>();
            assert!(!body.is_empty());
            assert!(body.len() <= 256 * 1024);
            assert_eq!(
                cursor.spooled_parts, 1,
                "repeated seeks must reuse the prepared SQLite part"
            );
            let (inner, artifact) = cursor
                .part
                .as_ref()
                .expect("single-part seek retains its prepared source");
            let index = inner.artifacts().last().unwrap().path.clone();
            if let Some(expected) = &prepared {
                assert_eq!(&artifact.path, expected);
            } else {
                prepared = Some(artifact.path.clone());
            }
            if let Some(expected) = &spans {
                assert_eq!(&index, expected);
            } else {
                spans = Some(index);
            }
            if already_prepared {
                assert!(
                    cursor.restore_io_counts().1 - read_before < 128 * 1024,
                    "reused part must not rescan the giant envelope"
                );
            }
            assert!(
                body.chars().all(|c| c == expected),
                "wrong body at {fraction}"
            );
            let position = cursor.source_fraction().unwrap();
            assert!(
                position <= fraction + 0.01 && position > fraction - 0.1,
                "{fraction}: {position}"
            );
            if fraction == 0. {
                assert!(!cursor.has_older());
                assert!(cursor.page_budget(200, 256 * 1024).unwrap().is_empty());
                assert!(
                    cursor.part.is_some(),
                    "an exhausted read retains the index for the next seek"
                );
            }
        }
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn same_message_and_time_opencode_parts_seek_by_part_key() {
        let path = std::env::temp_dir().join(format!(
            "orbit-opencode-parts-{}.sqlite",
            uuid::Uuid::new_v4()
        ));
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch(r#"CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT);INSERT INTO message VALUES('m','selected',42,'{"role":"user"}');"#).unwrap();
        for n in 0..20 {
            writer
                .execute(
                    "INSERT INTO part VALUES(?1,'m','selected',42,?2)",
                    params![
                        n.to_string(),
                        serde_json::json!({"type":"text","text":format!("part {n:02}")})
                            .to_string()
                    ],
                )
                .unwrap();
        }
        drop(writer);
        let mut cursor = OpenCodeCursor::open(&path, "selected").unwrap();
        cursor.seek_fraction(0.5).unwrap();
        let page = cursor
            .page_budget_cancellable(1, 256 * 1024, &|| false)
            .unwrap();
        assert_eq!(page[0].text, "part 09");
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn fractional_opencode_seek_uses_first_last_and_middle_source_keys() {
        let path =
            std::env::temp_dir().join(format!("orbit-native-seek-{}.sqlite", uuid::Uuid::new_v4()));
        let mut writer = Connection::open(&path).unwrap();
        writer.execute_batch("CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT);CREATE INDEX parts_session ON part(session_id,message_id);").unwrap();
        let tx = writer.transaction().unwrap();
        for n in 0..3000 {
            let id = format!("m{n}");
            tx.execute(
                r#"INSERT INTO message VALUES(?1,'selected',?2,'{"role":"user"}')"#,
                params![id, n],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO part VALUES(?1,?1,'selected',?2,?3)",
                params![
                    id,
                    n,
                    serde_json::json!({"type":"text","text":format!("message {n:04}")}).to_string()
                ],
            )
            .unwrap();
        }
        tx.commit().unwrap();
        drop(writer);
        for (f, label) in [
            (0., "message 0000"),
            (0.5, "message 1499"),
            (1., "message 2999"),
        ] {
            let mut cursor = OpenCodeCursor::open(&path, "selected").unwrap();
            cursor.seek_fraction(f).unwrap();
            let page = cursor
                .page_budget_cancellable(10, 256 * 1024, &|| false)
                .unwrap();
            assert!(
                page.iter().any(|m| m.text == label),
                "fraction {f}: {:?}",
                page.iter().map(|m| m.text.as_str()).collect::<Vec<_>>()
            );
            if f == 0. {
                assert!(!cursor.has_older());
            }
            assert!(cursor.spooled_parts == 0);
            assert!(cursor.metadata_queries <= 1);
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn checkpoint_marked_local_prompt_matches_small_and_spooled_native_parts_exactly() {
        for (large, associated) in [(false, false), (true, false), (false, true), (true, true)] {
            let root =
                std::env::temp_dir().join(format!("code-opencode-seam-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&root).unwrap();
            let path = root.join("native.sqlite3");
            let writer = Connection::open(&path).unwrap();
            writer.execute_batch("CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT);").unwrap();
            let body = if large {
                "Native 🦀 request\n".repeat(20_000)
            } else {
                "Native request".into()
            };
            for index in 1..=2 {
                writer
                    .execute(
                        "INSERT INTO message VALUES(?1,'selected',?2,'{\"role\":\"user\"}')",
                        params![index.to_string(), index],
                    )
                    .unwrap();
                writer
                    .execute(
                        "INSERT INTO part VALUES(?1,?1,'selected',?2,?3)",
                        params![
                            index.to_string(),
                            index,
                            serde_json::json!({"type":"text","text":body}).to_string()
                        ],
                    )
                    .unwrap();
            }
            let store = crate::store::Store::open(&root.join("local.sqlite3")).unwrap();
            let session = crate::discovery::SessionSummary {
                id: "local".into(),
                title: "Local".into(),
                agent: crate::discovery::AgentKind::OpenCode,
                external_id: Some("selected".into()),
                transcript_path: Some(path.clone()),
                project_path: root.clone(),
                execution_path: None,
                updated_at: 0,
            };
            store.upsert_session(&session).unwrap();
            let local = format!(
                "{}\n\n<!--code-checkpoint:{}-->",
                if associated { "Readable task" } else { &body },
                uuid::Uuid::new_v4()
            );
            if associated {
                store
                    .save_prompt(
                        &session.id,
                        &local,
                        crate::store::NativeInput::from_prompt(&body).as_ref(),
                    )
                    .unwrap();
            } else {
                store.save_message(&session.id, "user", &local).unwrap();
            }
            let (id, fingerprint) = store
                .local_fingerprint(&session.id, i64::MAX)
                .unwrap()
                .unwrap();
            let mut cursor = OpenCodeCursor::open(&path, "selected").unwrap();
            assert!(
                cursor
                    .skip_local_copy(&fingerprint, &store.message_blob(id).unwrap())
                    .unwrap()
            );
            let mut pages = Vec::new();
            while cursor.has_older() {
                pages.push(cursor.page_budget(200, 256 * 1024).unwrap());
            }
            pages.reverse();
            let older: String = pages
                .into_iter()
                .flatten()
                .map(|message| message.text)
                .collect();
            assert_eq!(older, body, "the earlier identical prompt must survive");
            drop(cursor);
            drop(store);
            drop(writer);
            std::fs::remove_dir_all(root).unwrap();
        }
    }
    #[test]
    fn many_small_native_parts_use_bounded_metadata_batches_and_no_spools() {
        let path = std::env::temp_dir().join(format!(
            "orbit-opencode-small-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,data TEXT);CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,data TEXT);CREATE INDEX part_session ON part(session_id);BEGIN;").unwrap();
        for i in 0..1000 {
            let id = i.to_string();
            writer
                .execute(
                    "INSERT INTO message VALUES(?1,'selected',?2,'{\"role\":\"user\"}')",
                    params![id, i],
                )
                .unwrap();
            writer
                .execute(
                    "INSERT INTO part VALUES(?1,?1,'selected',?2,?3)",
                    params![
                        id,
                        i,
                        serde_json::json!({"type":"text","text":format!("Question {i}")})
                            .to_string()
                    ],
                )
                .unwrap();
        }
        writer.execute_batch("COMMIT").unwrap();
        let mut cursor = OpenCodeCursor::open(&path, "selected").unwrap();
        let mut pages = Vec::new();
        while cursor.has_older() {
            let page = cursor.page_budget(200, 256 * 1024).unwrap();
            assert!(cursor.metadata.len() <= METADATA_BATCH);
            pages.push(page);
        }
        pages.reverse();
        let messages: Vec<_> = pages.into_iter().flatten().collect();
        assert_eq!(messages.len(), 1000);
        for (i, message) in messages.iter().enumerate() {
            assert_eq!(message.text, format!("Question {i}"));
        }
        assert!(
            cursor.metadata_queries <= 9,
            "Should query 128-part batches instead of one sort per part"
        );
        assert_eq!(cursor.spooled_parts, 0);
        drop(cursor);
        drop(writer);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_sqlite_history_is_selected_paged_and_readonly_even_for_giant_parts() {
        let path = std::env::temp_dir().join(format!(
            "orbit-opencode-fixture-{}.sqlite3",
            uuid::Uuid::new_v4()
        ));
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE session(id TEXT PRIMARY KEY); CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT); CREATE TABLE part(id TEXT PRIMARY KEY,message_id TEXT,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT); INSERT INTO session VALUES('selected'); INSERT INTO session VALUES('other');").unwrap();
        let text = "Unicode α☃ answer.\n".repeat(1_000_000);
        assert!(text.len() > 16 * 1024 * 1024);
        for (id, session, role, created, body) in [
            (
                "question",
                "selected",
                "user",
                1,
                serde_json::json!({"type":"text","text":"Original question"}),
            ),
            (
                "answer",
                "selected",
                "assistant",
                2,
                serde_json::json!({"type":"text","text":text}),
            ),
            (
                "tool",
                "selected",
                "assistant",
                3,
                serde_json::json!({"type":"tool","tool":"bash","state":{"status":"completed","output":"command output"}}),
            ),
            (
                "gianttool",
                "selected",
                "assistant",
                4,
                serde_json::json!({"type":"tool","tool":"bash","state":{"status":"completed","output":"log line\n".repeat(50_000)}}),
            ),
            (
                "other",
                "other",
                "user",
                5,
                serde_json::json!({"type":"text","text":"Other session must never appear"}),
            ),
        ] {
            writer
                .execute(
                    "INSERT INTO message VALUES(?1,?2,?3,?3,?4)",
                    params![
                        id,
                        session,
                        created,
                        serde_json::json!({"role":role}).to_string()
                    ],
                )
                .unwrap();
            writer
                .execute(
                    "INSERT INTO part VALUES(?1,?1,?2,?3,?3,?4)",
                    params![id, session, created, body.to_string()],
                )
                .unwrap();
        }
        let mut cursor = OpenCodeCursor::open(&path, "selected").unwrap();
        // The captured maximum key excludes subsequently appended turns.
        writer
            .execute(
                "INSERT INTO message VALUES('later','selected',6,6,'{\"role\":\"user\"}')",
                [],
            )
            .unwrap();
        writer.execute("INSERT INTO part VALUES('later','later','selected',6,6,'{\"type\":\"text\",\"text\":\"Later append\"}')",[]).unwrap();
        let mut pages = Vec::new();
        let mut page_count = 0;
        while cursor.has_older() {
            let page = cursor.page_budget(200, 256 * 1024).unwrap();
            assert!(page.iter().map(|m| m.text.len()).sum::<usize>() <= 256 * 1024);
            pages.push(page);
            page_count += 1;
            if page_count == 1 {
                let busy: i64 = writer
                    .query_row("PRAGMA wal_checkpoint(FULL)", [], |r| r.get(0))
                    .unwrap();
                assert_eq!(
                    busy, 0,
                    "Browsing history must not pin the native writer WAL"
                );
            }
            assert!(page_count < 1000);
        }
        pages.reverse();
        let messages: Vec<_> = pages.into_iter().flatten().collect();
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "Original question");
        let answer: String = messages
            .iter()
            .filter(|m| m.role == "assistant")
            .map(|m| m.text.as_str())
            .collect();
        assert!(answer == text);
        assert!(messages.iter().any(|m| m.role == "tool"
            && m.text.contains("bash · completed")
            && m.text.contains("command output")));
        let tool_output: String = messages
            .iter()
            .filter(|m| m.role == "tool")
            .filter_map(|m| serde_json::from_str::<serde_json::Value>(&m.text).ok())
            .filter_map(|v| v["details"].as_str().map(str::to_owned))
            .collect();
        assert!(tool_output.len() > 400_000);
        assert!(
            messages
                .iter()
                .all(|m| !m.text.contains("Other session") && !m.text.contains("Later append"))
        );
        let count: i64 = writer
            .query_row("SELECT count(*) FROM part", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 6);
        drop(cursor);
        drop(writer);
        let _ = std::fs::remove_file(path);
    }
}
