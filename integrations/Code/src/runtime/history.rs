//! Reverse native JSONL paging. Records and strings have no size cap: only the
//! decoded viewport page is bounded. Large text is indexed on disk, never copied
//! into one serde_json Value.
use super::{TranscriptMessage, transcript_record};
use crate::history_artifact::{FileStamp, HistoryArtifact};
use crate::{discovery::AgentKind, presentation};
use anyhow::{Context, Result, bail};
use std::{
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::Arc,
};

pub const PAGE_BYTES: usize = 256 * 1024;
const CHUNK_BYTES: usize = 64 * 1024;
const INDEX_BYTES: u64 = 88;
const SMALL_RECORD: u64 = 128 * 1024;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct HistoryCheckpoint {
    pub(crate) source: PathBuf,
    stamp: FileStamp,
    #[serde(default)]
    prepared_source: Option<PathBuf>,
    pub(crate) agent: AgentKind,
    before: u64,
    snapshot_len: u64,
    pending: Vec<TranscriptMessage>,
    large: Option<LargeCheckpoint>,
    exhausted: bool,
    #[serde(default)]
    seek_head: bool,
    #[serde(default)]
    seek_offset: Option<u64>,
}

#[cfg(test)]
mod completion_boundary_tests {
    use super::*;
    use crate::chat_interactions::{ChatInteraction,InteractionStatus};
    struct Transcript(PathBuf);
    impl Transcript{
        fn write(records:&[serde_json::Value])->Self{let path=std::env::temp_dir().join(format!("kai-native-boundaries-{}.jsonl",uuid::Uuid::new_v4()));let mut file=File::create(&path).unwrap();for record in records{writeln!(file,"{record}").unwrap();}Self(path)}
    }
    impl Drop for Transcript{fn drop(&mut self){let _=std::fs::remove_file(&self.0);}}
    fn message(role:&str,phase:&str,text:&str)->serde_json::Value{serde_json::json!({"type":"response_item","payload":{"type":"message","role":role,"phase":phase,"content":[{"type":"output_text","text":text}]}})}
    fn status(message:&TranscriptMessage)->Option<(String,InteractionStatus)>{match crate::chat_interactions::decode(&message.text){Some(ChatInteraction::Status{title,status,..})=>Some((title,status)),_=>None}}
    #[test]fn existing_native_turns_keep_final_boundaries_and_cancelled_completion(){
        let source=Transcript::write(&[message("user","","First prompt"),message("assistant","commentary","Working"),message("assistant","final_answer","First final"),serde_json::json!({"type":"event_msg","payload":{"type":"task_complete"}}),message("user","","Second prompt"),message("assistant","final_answer","Second final"),serde_json::json!({"type":"event_msg","payload":{"type":"turn_complete","status":"interrupted"}})]);
        let mut cursor=HistoryCursor::open(&source.0,AgentKind::Codex).unwrap();let messages=cursor.page(50).unwrap();
        assert_eq!(messages.iter().filter(|m|super::super::is_final_answer_boundary(m)).count(),2);
        let text:Vec<_>=messages.iter().filter(|m|m.role!="tool").map(|m|m.text.as_str()).collect();assert_eq!(text,["First prompt","Working","First final","Second prompt","Second final"]);
        assert_eq!(messages.iter().filter(|m|status(m)==Some(("Completed".into(),InteractionStatus::Completed))).count(),1);
        assert!(messages.iter().any(|m|status(m)==Some(("Stopped".into(),InteractionStatus::Cancelled))));
        let at=messages.iter().position(|m|m.text=="First final").unwrap();assert!(super::super::is_final_answer_boundary(&messages[at-1]));
    }
    #[test]fn giant_final_and_completion_metadata_survive_paging_checkpoint_and_head_seek(){
        let body="A complete final answer 中😀\n".repeat(14_000);
        let source=Transcript::write(&[message("user","","Prompt"),message("assistant","final_answer",&body),serde_json::json!({"type":"event_msg","payload":{"type":"task_complete","last_agent_message":body}})]);
        let mut cursor=HistoryCursor::open(&source.0,AgentKind::Codex).unwrap();let first=cursor.page(2).unwrap();
        let checkpoint:HistoryCheckpoint=serde_json::from_slice(&serde_json::to_vec(&cursor.checkpoint()).unwrap()).unwrap();
        let held:std::collections::HashMap<_,_>=cursor.artifacts().into_iter().map(|a|(a.path.clone(),a)).collect();
        let mut resolve=|name:&Path|held.get(name).cloned().ok_or_else(||anyhow::anyhow!("missing index"));
        let mut cursor=HistoryCursor::restore(checkpoint,None,&mut resolve).unwrap();let mut all=first;
        while cursor.has_older(){let mut page=cursor.page(2).unwrap();page.append(&mut all);all=page;}
        assert_eq!(all.iter().filter(|m|super::super::is_final_answer_boundary(m)).count(),1);
        assert_eq!(all.iter().filter(|m|status(m)==Some(("Completed".into(),InteractionStatus::Completed))).count(),1);
        assert_eq!(all.iter().filter(|m|m.role=="assistant").map(|m|m.text.as_str()).collect::<String>(),body);
        let mut head=HistoryCursor::open(&source.0,AgentKind::Codex).unwrap();head.seek_fraction(0.,&||false).unwrap();let page=head.page(4).unwrap();assert!(page.iter().any(super::super::is_final_answer_boundary));
    }
    #[test]fn giant_failed_terminal_record_never_becomes_success(){
        let source=Transcript::write(&[serde_json::json!({"type":"event_msg","payload":{"type":"turn_complete","error":{"message":"Failed"},"last_agent_message":"x".repeat(200_000)}})]);
        let messages=HistoryCursor::open(&source.0,AgentKind::Codex).unwrap().page(5).unwrap();assert_eq!(messages.len(),1);assert_eq!(status(&messages[0]),Some(("Agent failed".into(),InteractionStatus::Failed)));
    }
    #[test]fn head_limit_one_keeps_final_text_with_its_boundary(){
        let source=Transcript::write(&[message("assistant","final_answer",&"Final text\n".repeat(30_000))]);let mut cursor=HistoryCursor::open(&source.0,AgentKind::Codex).unwrap();cursor.seek_fraction(0.,&||false).unwrap();let page=cursor.page(1).unwrap();assert!(page.iter().any(super::super::is_final_answer_boundary));assert!(page.iter().any(|m|m.role=="assistant"&&m.text.contains("Final text")));assert!(cursor.head_end.is_some_and(|end|end>0.));
        let mut small=HistoryCursor::open(&source.0,AgentKind::Codex).unwrap();small.seek_fraction(0.,&||false).unwrap();let page=small.page_budget(1,512).unwrap();assert!(page.is_empty(),"A metadata-only head must not replace the real final text");assert!(small.large.as_ref().is_some_and(|record|record.final_answer));
    }
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct LargeCheckpoint {
    artifact: PathBuf,
    remaining: u64,
    accepted: u8,
    role: String,
    label: Option<String>,
    decoded_bytes: u64,
    voice: Option<crate::chat_interactions::Speaker>,
    #[serde(default)]
    final_answer:bool,
    #[serde(default)]
    terminal:Option<(String,crate::chat_interactions::InteractionStatus)>,
}
impl HistoryCheckpoint {
    pub(crate) fn rewrite_artifacts(&mut self, map: &std::collections::HashMap<PathBuf, PathBuf>) {
        if let Some(source) = &mut self.prepared_source
            && let Some(path) = map.get(source)
        {
            *source = path.clone();
        }
        if let Some(large) = &mut self.large
            && let Some(path) = map.get(&large.artifact)
        {
            large.artifact = path.clone();
        }
    }
}

/// Zstd archives are sequential. Prepare one private seekable source on the
/// state worker, then reuse ordinary bounded reverse paging and checkpoints.
/// The decoder has a fixed window ceiling and never buffers the full history.
fn prepare_archive(
    input: File,
    cancelled: &impl Fn() -> bool,
) -> Result<(File, Arc<HistoryArtifact>)> {
    anyhow::ensure!(!cancelled(), "Archived chat loading was superseded");
    let path = std::env::temp_dir().join(format!("code-history-archive-{}", uuid::Uuid::new_v4()));
    let mut output = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .context("Could not prepare this archived chat")?;
    let artifact = HistoryArtifact::own(path);
    let mut decoder =
        zstd::stream::read::Decoder::with_buffer(BufReader::with_capacity(CHUNK_BYTES, input))
            .context("Could not read the compressed chat archive")?;
    decoder
        .window_log_max(27)
        .context("Chat archive uses an unsupported compression window")?;
    let mut buffer = vec![0_u8; CHUNK_BYTES];
    loop {
        anyhow::ensure!(!cancelled(), "Archived chat loading was superseded");
        let count = decoder
            .read(&mut buffer)
            .context("Could not decode this archived chat; its original file is unchanged")?;
        if count == 0 {
            break;
        }
        output
            .write_all(&buffer[..count])
            .context("Not enough temporary disk space to open this archived chat")?;
    }
    output.seek(SeekFrom::Start(0))?;
    Ok((output, artifact))
}

pub struct HistoryCursor {
    file: File,
    source: PathBuf,
    stamp: FileStamp,
    prepared_source: Option<Arc<HistoryArtifact>>,
    agent: AgentKind,
    before: u64,
    snapshot_len: u64,
    pending: Vec<TranscriptMessage>,
    large: Option<LargeRecord>,
    buffer: Vec<u8>,
    exhausted: bool,
    cache_start: u64,
    cache_end: u64,
    pub bytes_read: u64,
    pub(crate) head_end: Option<f64>,
    #[cfg(test)]
    indexed_records: usize,
    seek_head: bool,
    seek_offset: Option<u64>,
}
impl HistoryCursor {
    pub fn open(path: &Path, agent: AgentKind) -> Result<Self> {
        Self::open_cancellable(path, agent, || false)
    }
    pub fn open_cancellable(
        path: &Path,
        agent: AgentKind,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self> {
        if !matches!(
            agent,
            AgentKind::Codex | AgentKind::Claude | AgentKind::Gemini
        ) {
            bail!("This agent has no native transcript adapter");
        }
        let stamp = FileStamp::read(path).context("Could not inspect session transcript")?;
        let input = File::open(path).context("Could not open session transcript")?;
        let (file, prepared_source) =
            if path.extension().is_some_and(|extension| extension == "zst") {
                let (file, artifact) = prepare_archive(input, &cancelled)?;
                anyhow::ensure!(
                    stamp.matches(path),
                    "The archived transcript changed while opening it; retry this chat"
                );
                (file, Some(artifact))
            } else {
                (input, None)
            };
        let before = file.metadata()?.len();
        Ok(Self {
            file,
            source: path.to_path_buf(),
            stamp,
            prepared_source,
            agent,
            before,
            snapshot_len: before,
            pending: Vec::new(),
            large: None,
            buffer: vec![0; CHUNK_BYTES],
            exhausted: false,
            cache_start: u64::MAX,
            cache_end: 0,
            bytes_read: 0,
            head_end: None,
            #[cfg(test)]
            indexed_records: 0,
            seek_head: false,
            seek_offset: None,
        })
    }
    /// Position by encoded source bytes; this never indexes the whole log.
    /// Zero uses a forward first-page reader so the oldest endpoint is exact.
    pub(crate) fn seek_fraction(
        &mut self,
        fraction: f64,
        cancelled: &impl Fn() -> bool,
    ) -> Result<()> {
        anyhow::ensure!(!cancelled(), "History seek was superseded");
        let fraction = if fraction.is_finite() {
            fraction.clamp(0., 1.)
        } else {
            1.
        };
        self.pending.clear();
        self.head_end = None;
        self.seek_offset = None;
        self.exhausted = false;
        self.seek_head = fraction == 0.;
        if self.seek_head {
            if self.before != 0 {
                self.large = None;
            }
            self.before = 0;
            return Ok(());
        }
        let offset = (self.snapshot_len as f64 * fraction) as u64;
        if let Some((start, end)) = self
            .large
            .as_mut()
            .map(|r| r.source_bounds())
            .transpose()?
            .flatten()
        {
            let last_record =
                fraction == 1. && self.record_end_from(end, cancelled)? == self.snapshot_len;
            if last_record || fraction < 1. && offset >= start && offset <= end {
                let large = self.large.as_mut().unwrap();
                large.seek_source(if last_record { u64::MAX } else { offset })?;
                return Ok(());
            }
        }
        self.large = None;
        if fraction == 1. {
            self.before = self.snapshot_len;
            return Ok(());
        }
        self.before = self.record_end_from(offset, cancelled)?;
        self.seek_offset = Some(offset);
        Ok(())
    }
    pub(crate) fn source_fraction(&self) -> f64 {
        if self.snapshot_len == 0 || self.exhausted {
            return 0.;
        }
        if let Some(large) = &self.large
            && large.remaining > 0
        {
            use std::os::unix::fs::FileExt;
            let mut raw = [0; 8];
            if large
                .index
                .read_exact_at(&mut raw, (large.remaining - 1) * INDEX_BYTES + 8)
                .is_ok()
            {
                return u64::from_le_bytes(raw) as f64 / self.snapshot_len as f64;
            }
        }
        self.before as f64 / self.snapshot_len as f64
    }
    fn record_end_from(&mut self, offset: u64, cancelled: &impl Fn() -> bool) -> Result<u64> {
        let mut position = offset.min(self.snapshot_len);
        while position < self.snapshot_len {
            anyhow::ensure!(!cancelled(), "History seek was superseded");
            if !(position >= self.cache_start && position < self.cache_end) {
                let count = (self.snapshot_len - position).min(CHUNK_BYTES as u64) as usize;
                self.file.seek(SeekFrom::Start(position))?;
                self.file.read_exact(&mut self.buffer[..count])?;
                self.bytes_read += count as u64;
                self.cache_start = position;
                self.cache_end = position + count as u64;
            }
            let begin = (position - self.cache_start) as usize;
            let end = (self.cache_end - self.cache_start) as usize;
            if let Some(ix) = self.buffer[begin..end].iter().position(|b| *b == b'\n') {
                return Ok(position + ix as u64 + 1);
            }
            position = self.cache_end;
        }
        Ok(self.snapshot_len)
    }
    fn head_page(
        &mut self,
        limit: usize,
        budget: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<TranscriptMessage>> {
        let mut output = Vec::new();
        let mut used = 0;
        let mut position = 0;
        let mut head_end = 0;
        'records: while position < self.snapshot_len {
            anyhow::ensure!(!cancelled(), "History seek was superseded");
            let indexed_end = if position == 0 {
                self.large
                    .as_mut()
                    .map(|r| r.source_bounds())
                    .transpose()?
                    .flatten()
                    .map(|(_, end)| end)
            } else {
                None
            };
            let after = self.record_end_from(indexed_end.unwrap_or(position), cancelled)?;
            let end = if after > position {
                self.file.seek(SeekFrom::Start(after - 1))?;
                let mut last = [0];
                self.file.read_exact(&mut last)?;
                after - u64::from(last[0] == b'\n')
            } else {
                after
            };
            if end - position > SMALL_RECORD {
                let mut large = if position == 0 && self.large.is_some() {
                    self.large.take().unwrap()
                } else {
                    #[cfg(test)]
                    {
                        self.indexed_records += 1;
                    }
                    LargeRecord::index_cancellable(
                        &mut self.file,
                        position,
                        end,
                        self.agent,
                        cancelled,
                    )?
                };
                let count = large.index.metadata()?.len() / INDEX_BYTES;
                if large.final_answer{
                    let boundary=super::final_answer_boundary();
                    let first_bytes=large.first_text_bytes(&mut self.file)?;
                    if used+boundary.text.len()+first_bytes>budget{if position==0{self.large=Some(large)}break 'records}
                    used+=boundary.text.len();output.push(boundary);large.final_answer=false;
                    // Metadata does not consume the single visible-message
                    // slot. A limit-one head seek must include actual text.
                    head_end=large.source_bounds()?.map_or(after,|(first,_)|first);
                }
                for entry in 0..count {
                    anyhow::ensure!(!cancelled(), "History seek was superseded");
                    large.index.seek(SeekFrom::Start(entry * INDEX_BYTES))?;
                    let mut raw = [0; INDEX_BYTES as usize];
                    large.index.read_exact(&mut raw)?;
                    if large.accepted == 0 || raw[16] != large.accepted {
                        continue;
                    }
                    large.remaining = entry + 1;
                    if let Some(message) = large.next(&mut self.file)? {
                        if used + message.text.len() > budget {
                            if position == 0 {
                                self.large = Some(large);
                            }
                            break 'records;
                        }
                        used += message.text.len();
                        head_end = u64::from_le_bytes(raw[8..16].try_into().unwrap());
                        output.push(message);
                        if output.len() >= limit.clamp(1, 400) || used >= budget {
                            if position == 0 {
                                self.large = Some(large);
                            }
                            break 'records;
                        }
                    }
                }
                if let Some((title,status))=large.terminal.clone(){
                    let message=TranscriptMessage{role:"tool".into(),code_language:None,text:crate::chat_interactions::encode(&crate::chat_interactions::ChatInteraction::Status{title,details:String::new(),status})};
                    if used+message.text.len()>budget{if position==0{self.large=Some(large)}break 'records}
                    large.terminal=None;used+=message.text.len();head_end=after;output.push(message);
                    if output.len()>=limit.clamp(1,400){break 'records}
                }
                if position == 0 {
                    self.large = Some(large);
                }
            } else if end > position {
                self.file.seek(SeekFrom::Start(position))?;
                let mut bytes = vec![0; (end - position) as usize];
                self.file.read_exact(&mut bytes)?;
                self.bytes_read += bytes.len() as u64;
                if !bytes.iter().all(u8::is_ascii_whitespace) {
                    match serde_json::from_slice(&bytes) {
                        Ok(record) => {
                            for message in transcript_record(&record, self.agent)
                                .into_iter()
                                .flat_map(split_message)
                            {
                                if used + message.text.len() > budget {
                                    break 'records;
                                }
                                used += message.text.len();
                                head_end = after;
                                output.push(message);
                                if output.len() >= limit.clamp(1, 400) || used >= budget {
                                    break 'records;
                                }
                            }
                        }
                        Err(_) if after == self.snapshot_len => break,
                        Err(error) => {
                            return Err(error).context("Invalid complete native JSON record");
                        }
                    }
                }
            }
            if position != 0 {
                self.large = None;
            }
            position = after;
        }
        self.head_end = Some(if self.snapshot_len == 0 {
            1.
        } else {
            head_end as f64 / self.snapshot_len as f64
        });
        self.seek_head = false;
        self.before = 0;
        self.exhausted = true;
        self.pending.clear();
        Ok(output)
    }
    pub(crate) fn source_matches(&self) -> bool {
        self.stamp.matches(&self.source)
    }
    pub(crate) fn checkpoint(&self) -> HistoryCheckpoint {
        let mut large = self.large.as_ref().map(|r| LargeCheckpoint {
            artifact: r.artifact.path.clone(),
            remaining: r.remaining,
            accepted: r.accepted,
            role: r.role.clone(),
            label: r.label.clone(),
            decoded_bytes: r.decoded_bytes,
            voice: r.voice,
            final_answer:r.final_answer,
            terminal:r.terminal.clone(),
        });
        let pending = if self.pending.len() == 1
            && let Some(large) = large.as_mut()
            && self.pending[0].role==large.role
        {
            // A budget-delayed giant span is already present in the immutable
            // index. Rewind its one entry instead of duplicating 64KB of text
            // into every page checkpoint across a huge message.
            large.remaining += 1;
            Vec::new()
        } else {
            self.pending.clone()
        };
        HistoryCheckpoint {
            source: self.source.clone(),
            stamp: self.stamp.clone(),
            prepared_source: self
                .prepared_source
                .as_ref()
                .map(|artifact| artifact.path.clone()),
            agent: self.agent,
            before: self.before,
            snapshot_len: self.snapshot_len,
            pending,
            large,
            exhausted: self.exhausted,
            seek_head: self.seek_head,
            seek_offset: self.seek_offset,
        }
    }
    pub(crate) fn artifacts(&self) -> Vec<Arc<HistoryArtifact>> {
        let mut artifacts = Vec::with_capacity(2);
        if let Some(artifact) = &self.prepared_source {
            artifacts.push(artifact.clone());
        }
        if let Some(large) = &self.large {
            artifacts.push(large.artifact.clone());
        }
        artifacts
    }
    pub(crate) fn restore(
        checkpoint: HistoryCheckpoint,
        source_override: Option<PathBuf>,
        resolve: &mut impl FnMut(&Path) -> Result<Arc<HistoryArtifact>>,
    ) -> Result<Self> {
        anyhow::ensure!(
            matches!(
                checkpoint.agent,
                AgentKind::Codex | AgentKind::Claude | AgentKind::Gemini
            ),
            "Unsupported history checkpoint adapter"
        );
        let owned_source = source_override.is_some();
        let source = source_override.unwrap_or_else(|| checkpoint.source.clone());
        anyhow::ensure!(
            owned_source || checkpoint.stamp.matches(&source),
            "Native history changed since the saved viewport"
        );
        anyhow::ensure!(
            checkpoint.before <= checkpoint.snapshot_len
                && checkpoint.pending.len() <= 1024
                && checkpoint
                    .pending
                    .iter()
                    .map(|m| m.text.len())
                    .sum::<usize>()
                    <= 2 * 1024 * 1024,
            "Invalid history checkpoint"
        );
        let prepared_source = checkpoint
            .prepared_source
            .as_ref()
            .map(|path| resolve(path))
            .transpose()?;
        let file = File::open(
            prepared_source
                .as_ref()
                .map_or(source.as_path(), |artifact| artifact.path.as_path()),
        )?;
        anyhow::ensure!(
            file.metadata()?.len() >= checkpoint.snapshot_len,
            "Native history was truncated"
        );
        let large = if let Some(r) = checkpoint.large {
            let artifact = resolve(&r.artifact)?;
            let index = File::open(&artifact.path)?;
            anyhow::ensure!(
                r.remaining <= index.metadata()?.len() / INDEX_BYTES
                    && r.accepted <= 4
                    && r.role.len() <= 64
                    && r.label.as_ref().is_none_or(|s| s.len() <= 1024),
                "Invalid large-record checkpoint"
            );
            Some(LargeRecord {
                index,
                artifact,
                remaining: r.remaining,
                accepted: r.accepted,
                role: r.role,
                label: r.label,
                decoded_bytes: r.decoded_bytes,
                voice: r.voice,
                final_answer:r.final_answer,
                terminal:r.terminal,
            })
        } else {
            None
        };
        Ok(Self {
            file,
            stamp: FileStamp::read(&source)?,
            source,
            prepared_source,
            agent: checkpoint.agent,
            before: checkpoint.before,
            snapshot_len: checkpoint.snapshot_len,
            pending: checkpoint.pending,
            large,
            buffer: vec![0; CHUNK_BYTES],
            exhausted: checkpoint.exhausted,
            cache_start: u64::MAX,
            cache_end: 0,
            bytes_read: 0,
            head_end: None,
            #[cfg(test)]
            indexed_records: 0,
            seek_head: checkpoint.seek_head,
            seek_offset: checkpoint.seek_offset,
        })
    }
    pub fn has_older(&self) -> bool {
        self.seek_head
            || !self.pending.is_empty()
            || !self.exhausted
                && (self.before > 0 || self.large.as_ref().is_some_and(|r| r.remaining > 0 || r.final_answer || r.terminal.is_some()))
    }
    #[cfg(test)]
    pub fn page(&mut self, limit: usize) -> Result<Vec<TranscriptMessage>> {
        self.page_budget(limit, PAGE_BYTES)
    }
    pub fn page_budget(&mut self, limit: usize, budget: usize) -> Result<Vec<TranscriptMessage>> {
        self.page_budget_cancellable(limit, budget, &|| false)
    }
    pub(crate) fn page_budget_cancellable(
        &mut self,
        limit: usize,
        budget: usize,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Vec<TranscriptMessage>> {
        if self.seek_head {
            return self.head_page(limit, budget, cancelled);
        }
        if self.exhausted && self.pending.is_empty() {
            return Ok(Vec::new());
        }
        let mut messages = Vec::new();
        let mut bytes = 0;
        while messages.len() < limit.clamp(1, 400) {
            anyhow::ensure!(!cancelled(), "History seek was superseded");
            let Some(message) = self.next_message_cancellable(cancelled)? else {
                break;
            };
            if bytes + message.text.len() > budget {
                self.pending.push(message);
                break;
            }
            bytes += message.text.len();
            messages.push(message);
            if bytes >= budget {
                break;
            }
        }
        messages.reverse();
        Ok(messages)
    }
    /// Match only the newest record at the local/native seam, retaining every
    /// earlier repeated prompt. Large bodies use their indexed length/bookends.
    pub(crate) fn skip_local_copy_cancellable(
        &mut self,
        fingerprint: &crate::store::LocalFingerprint,
        expected: &rusqlite::blob::Blob<'_>,
        cancelled: &impl Fn() -> bool,
    ) -> Result<bool> {
        let Some(last) = self.next_message_cancellable(cancelled)? else {
            return Ok(false);
        };
        let mut matches = false;
        if last.role == fingerprint.role {
            if let Some(large) = self.large.as_mut() {
                let raw_body = fingerprint.raw_digest.is_some();
                let bytes = fingerprint
                    .raw_digest
                    .map_or(fingerprint.bytes, |(bytes, _)| bytes);
                if large.decoded_bytes == bytes
                    && (raw_body || last.text.ends_with(&fingerprint.tail))
                {
                    for entry in 0..=large.remaining {
                        anyhow::ensure!(!cancelled(), "History seek was superseded");
                        large.index.seek(SeekFrom::Start(entry * INDEX_BYTES))?;
                        let mut raw = [0; INDEX_BYTES as usize];
                        large.index.read_exact(&mut raw)?;
                        if raw[16] == large.accepted {
                            let first = decode_span(
                                &mut self.file,
                                u64::from_le_bytes(raw[..8].try_into().unwrap()),
                                u64::from_le_bytes(raw[8..16].try_into().unwrap()),
                            )?;
                            matches = raw_body || first.starts_with(&fingerprint.prefix);
                            break;
                        }
                    }
                }
                if matches {
                    let mut comparison = BodyComparison::new(expected, fingerprint, true);
                    for entry in 0..=large.remaining {
                        anyhow::ensure!(!cancelled(), "History seek was superseded");
                        large.index.seek(SeekFrom::Start(entry * INDEX_BYTES))?;
                        let mut raw = [0; INDEX_BYTES as usize];
                        large.index.read_exact(&mut raw)?;
                        if raw[16] == large.accepted {
                            let text = decode_span(
                                &mut self.file,
                                u64::from_le_bytes(raw[..8].try_into().unwrap()),
                                u64::from_le_bytes(raw[8..16].try_into().unwrap()),
                            )?;
                            if !comparison.feed(text.as_bytes())? {
                                break;
                            }
                        }
                    }
                    matches = comparison.complete();
                }
            } else if self.pending.iter().all(|message| message.role == last.role || super::is_final_answer_boundary(message)) {
                let mut comparison = BodyComparison::new(expected, fingerprint, false);
                for text in self
                    .pending
                    .iter()
                    .filter(|message|!super::is_final_answer_boundary(message))
                    .map(|m| m.text.as_str())
                    .chain(std::iter::once(last.text.as_str()))
                {
                    if !comparison.feed(text.as_bytes())? {
                        break;
                    }
                }
                matches = comparison.complete();
            }
        }
        if matches {
            // A native boundary belongs to the body being skipped. Leaving it
            // at the seam would move it before the complete local prompt/answer
            // pair and would prevent the next prompt overlap from matching.
            self.pending.clear();
            self.large = None;
        } else {
            self.pending.push(last);
        }
        Ok(matches)
    }
    fn next_message_cancellable(
        &mut self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<TranscriptMessage>> {
        loop {
            anyhow::ensure!(!cancelled(), "History seek was superseded");
            if let Some(message) = self.pending.pop() {
                return Ok(Some(message));
            }
            if let Some(record) = self.large.as_mut() {
                if let Some(message) = record.next(&mut self.file)? {
                    return Ok(Some(message));
                }
                self.large = None;
            }
            if self.exhausted {
                return Ok(None);
            }
            let Some((start, end)) = self.previous_record_cancellable(cancelled)? else {
                self.exhausted = true;
                return Ok(None);
            };
            if end - start > SMALL_RECORD {
                #[cfg(test)]
                {
                    self.indexed_records += 1;
                }
                match LargeRecord::index_cancellable(
                    &mut self.file,
                    start,
                    end,
                    self.agent,
                    cancelled,
                ) {
                    Ok(mut record) => {
                        if let Some(offset) = self.seek_offset.take() {
                            record.seek_source(offset)?;
                        }
                        self.large = Some(record);
                    }
                    Err(_) if end == self.snapshot_len => continue,
                    Err(error) => return Err(error).context("Invalid complete native JSON record"),
                }
            } else {
                self.seek_offset = None;
                self.file.seek(SeekFrom::Start(start))?;
                let mut bytes = vec![0; (end - start) as usize];
                self.file.read_exact(&mut bytes)?;
                self.bytes_read += bytes.len() as u64;
                if bytes.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                match serde_json::from_slice(&bytes) {
                    Ok(value) => {
                        self.pending = transcript_record(&value, self.agent)
                            .into_iter()
                            .flat_map(split_message)
                            .collect();
                    }
                    // A CLI may be in the middle of writing the last line.
                    Err(_) if end == self.snapshot_len => continue,
                    Err(error) => {
                        return Err(error)
                            .context("Native transcript contains an invalid complete JSON record");
                    }
                }
            }
        }
    }
    /// Locate the preceding line by bounded backward reads. Start/end delimit
    /// the JSON bytes excluding its trailing newline; no entire-line allocation.
    fn previous_record_cancellable(
        &mut self,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Option<(u64, u64)>> {
        let mut end = self.before;
        if end == 0 {
            return Ok(None);
        }
        self.file.seek(SeekFrom::Start(end - 1))?;
        let mut last = [0];
        self.file.read_exact(&mut last)?;
        if last[0] == b'\n' {
            end -= 1;
        }
        let mut position = end;
        while position > 0 {
            anyhow::ensure!(!cancelled(), "History seek was superseded");
            let start = position.saturating_sub(CHUNK_BYTES as u64);
            let count = (position - start) as usize;
            let (block_start, block_count) =
                if position > self.cache_start && position <= self.cache_end {
                    (self.cache_start, (position - self.cache_start) as usize)
                } else {
                    self.file.seek(SeekFrom::Start(start))?;
                    self.file.read_exact(&mut self.buffer[..count])?;
                    self.bytes_read += count as u64;
                    self.cache_start = start;
                    self.cache_end = position;
                    (start, count)
                };
            if let Some(ix) = self.buffer[..block_count].iter().rposition(|b| *b == b'\n') {
                let line_start = block_start + ix as u64 + 1;
                self.before = line_start;
                return Ok(Some((line_start, end)));
            }
            position = block_start;
        }
        self.before = 0;
        Ok(Some((0, end)))
    }
}

/// Exact seam verification, streamed only after a cheap candidate match. A
/// length/bookend match alone can hide a different output with a changed middle.
pub(crate) struct BodyComparison<'a, 'conn> {
    expected: &'a rusqlite::blob::Blob<'conn>,
    scratch: Vec<u8>,
    offset: usize,
    length: u64,
    digest: Option<([u8; 32], sha2::Sha256)>,
    equal: bool,
}
impl<'a, 'conn> BodyComparison<'a, 'conn> {
    pub(crate) fn new(
        expected: &'a rusqlite::blob::Blob<'conn>,
        fingerprint: &crate::store::LocalFingerprint,
        raw: bool,
    ) -> Self {
        use sha2::Digest;
        let (length, digest) = if raw && let Some((bytes, digest)) = fingerprint.raw_digest {
            (bytes, Some(digest))
        } else {
            (fingerprint.bytes, fingerprint.digest)
        };
        Self {
            expected,
            scratch: Vec::new(),
            offset: 0,
            length,
            digest: digest.map(|digest| (digest, sha2::Sha256::new())),
            equal: digest.is_some() || length <= expected.size() as u64,
        }
    }
    pub(crate) fn feed(&mut self, text: &[u8]) -> Result<bool> {
        if !self.equal || text.len() as u64 > self.length.saturating_sub(self.offset as u64) {
            self.equal = false;
            return Ok(false);
        }
        if let Some((_, digest)) = &mut self.digest {
            use sha2::Digest;
            digest.update(text);
            self.offset += text.len();
            return Ok(true);
        }
        let scratch_size = text.len().min(CHUNK_BYTES);
        if self.scratch.len() < scratch_size {
            self.scratch.resize(scratch_size, 0);
        }
        for chunk in text.chunks(CHUNK_BYTES) {
            self.expected
                .read_at_exact(&mut self.scratch[..chunk.len()], self.offset)?;
            if self.scratch[..chunk.len()] != *chunk {
                self.equal = false;
                return Ok(false);
            }
            self.offset += chunk.len();
        }
        Ok(true)
    }
    pub(crate) fn complete(&self) -> bool {
        use sha2::Digest;
        self.equal
            && self.offset as u64 == self.length
            && self.digest.as_ref().is_none_or(|(expected, actual)| {
                let actual: [u8; 32] = actual.clone().finalize().into();
                *expected == actual
            })
    }
}

pub(crate) fn split_message(message: TranscriptMessage) -> Vec<TranscriptMessage> {
    if message.text.len() <= CHUNK_BYTES {
        return vec![message];
    }
    if message.role == "tool"
        && let Some((card, mut context)) =
            crate::chat_interactions::decode_with_context(&message.text)
    {
        let mut at_line_start = true;
        return crate::chat_interactions::continuations(card, CHUNK_BYTES / 2)
            .into_iter()
            .map(|card| {
                let text = crate::chat_interactions::encode_with_context(&card, context.as_deref());
                if let crate::chat_interactions::ChatInteraction::Speaker { text: spoken, .. } =
                    &card
                {
                    for line in spoken.split_inclusive('\n') {
                        let trimmed = line.trim_start();
                        if at_line_start {
                            let marker = trimmed.as_bytes().first().copied().unwrap_or_default();
                            let length = trimmed.bytes().take_while(|byte| *byte == marker).count();
                            if matches!(marker, b'`' | b'~') && length >= 3 {
                                if let Some(open) = &context {
                                    let same = if open.starts_with("~~~") {
                                        marker == b'~'
                                    } else {
                                        marker == b'`'
                                    };
                                    if same && trimmed[length..].trim().is_empty() {
                                        context = None;
                                    }
                                } else {
                                    let language = trimmed[length..]
                                        .trim()
                                        .chars()
                                        .take(61)
                                        .collect::<String>();
                                    context = Some(if marker == b'~' {
                                        format!("~~~{language}")
                                    } else {
                                        language
                                    });
                                }
                            }
                        }
                        at_line_start = line.ends_with('\n');
                    }
                }
                TranscriptMessage {
                    role: "tool".into(),
                    code_language: None,
                    text,
                }
            })
            .collect();
    }
    let tool = if message.role == "tool" {
        serde_json::from_str::<serde_json::Value>(&message.text).ok()
    } else {
        None
    };
    let mut text = tool
        .as_ref()
        .and_then(|v| v["details"].as_str())
        .unwrap_or(&message.text);
    let mut pieces = Vec::new();
    let mut context: Option<String> = None;
    while !text.is_empty() {
        let mut end = text.len().min(CHUNK_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if end < text.len()
            && let Some(newline) = text[..end].rfind('\n')
            && newline > CHUNK_BYTES / 2
        {
            end = newline + 1;
        }
        let chunk = &text[..end];
        let start_context = context.clone();
        for line in chunk.lines() {
            let line = line.trim_start();
            if let Some(rest) = line
                .strip_prefix("```")
                .or_else(|| line.strip_prefix("~~~"))
            {
                if context.is_some() {
                    context = None;
                } else {
                    let language = rest.trim().chars().take(61).collect::<String>();
                    context = Some(if line.starts_with("~~~") {
                        format!("~~~{language}")
                    } else {
                        language
                    });
                }
            }
        }
        let output = if let Some(value) = &tool {
            serde_json::json!({"orbit_tool":true,"label":value["label"],"details":chunk})
                .to_string()
        } else {
            chunk.to_owned()
        };
        pieces.push(TranscriptMessage {
            code_language: if message.role == "tool" { None } else { start_context },
            role: message.role.clone(),
            text: output,
        });
        text = &text[end..];
    }
    pieces
}
fn sanitize_chunk(role: &str, text: &str) -> Option<String> {
    if text.trim().is_empty() {
        return Some(text.to_owned());
    }
    presentation::sanitize_transcript_text(role, text).map(|clean| {
        if clean == text {
            clean
        } else if clean == text.trim() {
            text.to_owned()
        } else {
            clean
        }
    })
}

struct LargeRecord {
    index: File,
    artifact: Arc<HistoryArtifact>,
    remaining: u64,
    accepted: u8,
    role: String,
    label: Option<String>,
    decoded_bytes: u64,
    voice: Option<crate::chat_interactions::Speaker>,
    final_answer:bool,
    terminal:Option<(String,crate::chat_interactions::InteractionStatus)>,
}
impl LargeRecord {
    fn first_text_bytes(&mut self,source:&mut File)->Result<usize>{
        let count=self.index.metadata()?.len()/INDEX_BYTES;
        for entry in 0..count{self.index.seek(SeekFrom::Start(entry*INDEX_BYTES))?;let mut raw=[0;INDEX_BYTES as usize];self.index.read_exact(&mut raw)?;if self.accepted!=0&&raw[16]==self.accepted{return Ok(decode_span(source,u64::from_le_bytes(raw[..8].try_into().unwrap()),u64::from_le_bytes(raw[8..16].try_into().unwrap()))?.len())}}
        Ok(0)
    }
    fn source_bounds(&mut self) -> Result<Option<(u64, u64)>> {
        let total = self.index.metadata()?.len() / INDEX_BYTES;
        if total == 0 {
            return Ok(None);
        }
        self.index.seek(SeekFrom::Start(0))?;
        let mut first = [0; 8];
        self.index.read_exact(&mut first)?;
        self.index
            .seek(SeekFrom::Start((total - 1) * INDEX_BYTES + 8))?;
        let mut last = [0; 8];
        self.index.read_exact(&mut last)?;
        Ok(Some((u64::from_le_bytes(first), u64::from_le_bytes(last))))
    }
    fn seek_source(&mut self, offset: u64) -> Result<()> {
        let total = self.index.metadata()?.len() / INDEX_BYTES;
        let mut low = 0;
        let mut high = total;
        while low < high {
            let mid = (low + high) / 2;
            self.index.seek(SeekFrom::Start(mid * INDEX_BYTES))?;
            let mut raw = [0; 8];
            self.index.read_exact(&mut raw)?;
            if u64::from_le_bytes(raw) <= offset {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        self.remaining = low.max(1).min(total);
        Ok(())
    }
    fn index_cancellable(
        source: &mut File,
        start: u64,
        end: u64,
        agent: AgentKind,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("orbit-history-spans-{}", uuid::Uuid::new_v4()));
        let index = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let mut result = Self {
            index,
            artifact: HistoryArtifact::own(path),
            remaining: 0,
            accepted: 0,
            role: String::new(),
            label: None,
            decoded_bytes: 0,
            voice: None,
            final_answer:false,
            terminal:None,
        };
        source.seek(SeekFrom::Start(start))?;
        let mut scan = SpanScanner {
            reader: BufReader::with_capacity(
                CHUNK_BYTES,
                CancelRead {
                    inner: source.take(end - start),
                    cancelled,
                },
            ),
            position: start,
            index: &mut result.index,
            entries: 0,
            role: String::new(),
            root_type: String::new(),
            payload_type: String::new(),
            phase:String::new(),
            terminal_status:String::new(),
            terminal_error:false,
            sidechain: false,
            tool_name: String::new(),
            tool_status: String::new(),
            decoded: [0; 5],
            content_type: String::new(),
            text_context: 0,
        };
        scan.value(&mut Vec::new(), 0)?;
        scan.whitespace()?;
        if scan.peek()?.is_some() {
            bail!("Unexpected native transcript data after JSON value");
        }
        result.remaining = scan.entries;
        result.role = scan.role;
        if agent == AgentKind::Codex && scan.root_type == "response_item" {
            if matches!(scan.payload_type.as_str(), "message" | "agent_message")
                && matches!(result.role.as_str(), "user" | "assistant")
            {
                result.accepted = 1;
            } else if matches!(
                scan.payload_type.as_str(),
                "function_call" | "custom_tool_call"
            ) {
                result.accepted = 3;
                result.role = "tool".into();
                result.label = Some(format!("Used {}", scan.tool_name));
            } else if scan.payload_type == "function_call_output" {
                result.accepted = 4;
                result.role = "tool".into();
                result.label = Some("Tool output".into());
            }
        } else if agent == AgentKind::Claude
            && matches!(scan.root_type.as_str(), "user" | "assistant")
            && !scan.sidechain
            && matches!(result.role.as_str(), "user" | "assistant")
        {
            result.accepted = 2;
        }
        if agent == AgentKind::Codex
            && scan.root_type == "event_msg"
            && scan.payload_type == "transcript_segment"
        {
            result.voice = match result.role.as_str() {
                "user" => Some(crate::chat_interactions::Speaker::User),
                "assistant" => Some(crate::chat_interactions::Speaker::Assistant),
                _ => None,
            };
            if result.voice.is_some() {
                result.accepted = 1;
                result.role = "tool".into();
            }
        }
        if agent == AgentKind::Gemini
            && matches!(scan.root_type.as_str(), "user" | "gemini" | "assistant")
        {
            result.role = if scan.root_type == "user" {
                "user"
            } else {
                "assistant"
            }
            .into();
            result.accepted = 2;
        }
        if agent == AgentKind::Codex
            && scan.payload_type == "message"
            && scan.content_type == "tool"
        {
            result.accepted = 4;
            result.role = "tool".into();
            result.label = Some(format!("{} · {}", scan.tool_name, scan.tool_status));
        }
        result.decoded_bytes = scan.decoded[result.accepted as usize];
        let final_phase=scan.phase=="final_answer";
        let terminal=if agent==AgentKind::Codex && scan.root_type=="event_msg" && matches!(scan.payload_type.as_str(),"task_complete"|"turn_complete"|"turn_aborted"){
            use crate::chat_interactions::InteractionStatus as Status;
            let (title,status)=if scan.payload_type=="turn_aborted" || matches!(scan.terminal_status.as_str(),"interrupted"|"cancelled"|"canceled"|"aborted"){("Stopped",Status::Cancelled)}else if scan.terminal_error || matches!(scan.terminal_status.as_str(),"failed"|"error"){("Agent failed",Status::Failed)}else{("Completed",Status::Completed)};
            Some((title.into(),status))
        }else{None};
        // Large helper/control prompts are filtered by their first decoded chunk
        // before any continuation is made visible.
        if result.accepted != 0 && result.role != "tool" {
            for entry in 0..result.remaining {
                result.index.seek(SeekFrom::Start(entry * INDEX_BYTES))?;
                let mut raw = [0; INDEX_BYTES as usize];
                result.index.read_exact(&mut raw)?;
                if raw[16] == result.accepted {
                    let text = decode_span(
                        source,
                        u64::from_le_bytes(raw[..8].try_into().unwrap()),
                        u64::from_le_bytes(raw[8..16].try_into().unwrap()),
                    )?;
                    let approval = result.role == "assistant"
                        && text.trim_start().starts_with('{')
                        && [
                            "\"risk_level\"",
                            "\"user_authorization\"",
                            "\"outcome\"",
                            "\"rationale\"",
                        ]
                        .iter()
                        .all(|key| text.contains(key));
                    if presentation::sanitize_transcript_text(&result.role, &text).is_none()
                        || approval
                    {
                        result.accepted = 0;
                    }
                    break;
                }
            }
        }
        result.final_answer=agent==AgentKind::Codex && result.accepted!=0 && result.decoded_bytes>0 && result.role=="assistant" && final_phase;
        result.terminal=terminal;
        Ok(result)
    }
    fn next(&mut self, source: &mut File) -> Result<Option<TranscriptMessage>> {
        while self.remaining > 0 {
            self.remaining -= 1;
            self.index
                .seek(SeekFrom::Start(self.remaining * INDEX_BYTES))?;
            let mut raw = [0; INDEX_BYTES as usize];
            self.index.read_exact(&mut raw)?;
            if self.accepted == 0 || raw[16] != self.accepted {
                continue;
            }
            let text = decode_span(
                source,
                u64::from_le_bytes(raw[..8].try_into().unwrap()),
                u64::from_le_bytes(raw[8..16].try_into().unwrap()),
            )?;
            if text.trim().is_empty() && (raw[18] != 0 || self.voice.is_some()) {
                continue;
            }
            let code_context = if raw[17] == 0 {
                None
            } else {
                Some(String::from_utf8_lossy(&raw[24..24 + (raw[17] - 1) as usize]).into_owned())
            };
            let card = match (self.voice, raw[18]) {
                (Some(speaker), _) => Some(crate::chat_interactions::ChatInteraction::Speaker {
                    speaker,
                    text: text.clone(),
                }),
                (_, 3) => Some(crate::chat_interactions::ChatInteraction::Speaker {
                    speaker: crate::chat_interactions::Speaker::User,
                    text: text.clone(),
                }),
                (_, 4) => Some(crate::chat_interactions::ChatInteraction::Speaker {
                    speaker: crate::chat_interactions::Speaker::Assistant,
                    text: text.clone(),
                }),
                (_, 1 | 2) => Some(crate::chat_interactions::ChatInteraction::Delegation {
                    title: if raw[18] == 1 {
                        "Voice task"
                    } else {
                        "Voice context"
                    }
                    .into(),
                    details: text.clone(),
                    status: crate::chat_interactions::InteractionStatus::Started,
                    agent: None,
                }),
                _ => None,
            };
            if let Some(card) = card {
                return Ok(Some(TranscriptMessage {
                    code_language: None,
                    role: "tool".into(),
                    text: crate::chat_interactions::encode_with_context(
                        &card,
                        code_context.as_deref(),
                    ),
                }));
            }
            // Tool envelopes must reach the typed decoder before any code fence.
            let code_language = if self.role == "tool" { None } else { code_context };
            if let Some(text) = if code_language.is_some() {
                Some(text)
            } else {
                sanitize_chunk(&self.role, &text)
            } {
                return Ok(Some(TranscriptMessage {
                    code_language,
                    role: self.role.clone(),
                    text: if let Some(label) = &self.label {
                        serde_json::json!({"orbit_tool":true,"label":label,"details":text})
                            .to_string()
                    } else {
                        text
                    },
                }));
            }
        }
        if std::mem::take(&mut self.final_answer){return Ok(Some(super::final_answer_boundary()))}
        if let Some((title,status))=self.terminal.take(){return Ok(Some(TranscriptMessage{role:"tool".into(),code_language:None,text:crate::chat_interactions::encode(&crate::chat_interactions::ChatInteraction::Status{title,details:String::new(),status})}))}
        Ok(None)
    }
}
fn decode_span(file: &mut File, start: u64, end: u64) -> Result<String> {
    file.seek(SeekFrom::Start(start))?;
    let mut raw = vec![b'"'; (end - start) as usize + 2];
    file.read_exact(&mut raw[1..(end - start) as usize + 1])?;
    serde_json::from_slice(&raw).context("Could not decode native text continuation")
}

struct CancelRead<'a, R> {
    inner: R,
    cancelled: &'a dyn Fn() -> bool,
}
impl<R: Read> Read for CancelRead<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        if (self.cancelled)() {
            return Err(std::io::Error::other("History seek was superseded"));
        }
        self.inner.read(bytes)
    }
}

struct SpanScanner<'a, R: Read> {
    reader: BufReader<R>,
    position: u64,
    index: &'a mut File,
    entries: u64,
    role: String,
    root_type: String,
    payload_type: String,
    phase:String,
    terminal_status:String,
    terminal_error:bool,
    sidechain: bool,
    tool_name: String,
    tool_status: String,
    decoded: [u64; 5],
    content_type: String,
    text_context: u8,
}
impl<R: Read> SpanScanner<'_, R> {
    fn peek(&mut self) -> Result<Option<u8>> {
        Ok(self.reader.fill_buf()?.first().copied())
    }
    fn byte(&mut self) -> Result<Option<u8>> {
        let value = self.peek()?;
        if value.is_some() {
            self.reader.consume(1);
            self.position += 1;
        }
        Ok(value)
    }
    fn whitespace(&mut self) -> Result<()> {
        while self.peek()?.is_some_and(|b| b.is_ascii_whitespace()) {
            self.byte()?;
        }
        Ok(())
    }
    fn expect(&mut self, b: u8) -> Result<()> {
        if self.byte()? != Some(b) {
            bail!("Malformed native JSON record");
        }
        Ok(())
    }
    fn entry(&mut self, start: u64, end: u64, kind: u8, context: Option<&str>) -> Result<()> {
        if kind == 0 || end == start || self.text_context == 5 {
            return Ok(());
        }
        self.index.write_all(&start.to_le_bytes())?;
        self.index.write_all(&end.to_le_bytes())?;
        let mut extra = [0; 72];
        extra[0] = kind;
        extra[2] = self.text_context;
        if let Some(language) = context {
            let bytes = language.as_bytes();
            let len = bytes.len().min(64);
            extra[1] = len as u8 + 1;
            extra[8..8 + len].copy_from_slice(&bytes[..len]);
        }
        self.index.write_all(&extra)?;
        self.entries += 1;
        Ok(())
    }
    fn string(&mut self, kind: u8) -> Result<String> {
        self.expect(b'"')?;
        let mut start = self.position;
        let mut small = Vec::with_capacity(128);
        small.push(b'"');
        let mut suppressed: Option<String> = None;
        let mut suppressed_depth = 0u32;
        let mut tag: Option<(u64, String)> = None;
        let mut ticks = 0;
        let mut fenced = false;
        let mut inline_code = false;
        let mut text_started = false;
        let mut text_json = false;
        let mut json_quote = false;
        let mut json_escape = false;
        let mut fence_delimiter = b'`';
        let mut tick_delimiter = b'`';
        let mut language = String::new();
        let mut collecting_language = false;
        let mut start_context: Option<String> = None;
        self.text_context = 0;
        let mut delegation_depth = 0u32;
        let mut line_start = self.position;
        let mut prefix = String::new();
        let mut prefix_possible = true;
        loop {
            let before = self.position;
            let b = self.byte()?.context("Unfinished native JSON string")?;
            let mut tag_finished = false;
            if b == b'"' {
                self.entry(
                    start,
                    before,
                    if suppressed.is_some() { 0 } else { kind },
                    start_context.as_deref(),
                )?;
                break;
            }
            if b < 0x20 {
                bail!("Invalid native JSON string control character");
            }
            if small.len() < 512 {
                small.push(b);
            }
            if matches!(kind, 1 | 2) {
                if !text_started && !b.is_ascii_whitespace() && b != b'\\' {
                    text_started = true;
                    text_json = matches!(b, b'{' | b'[');
                }
                if b != b'\\' && json_escape {
                    json_escape = false;
                }
                if suppressed.is_none() && (b == b'`' || b == b'~') {
                    if tick_delimiter == b {
                        ticks += 1;
                    } else {
                        ticks = 1;
                        tick_delimiter = b;
                    }
                    if ticks == 1 && b == b'`' && !fenced {
                        inline_code = !inline_code;
                    }
                    if ticks == 3 && (!fenced || b == fence_delimiter) {
                        inline_code = false;
                        fenced = !fenced;
                        fence_delimiter = b;
                        language.clear();
                        collecting_language = fenced;
                    }
                } else {
                    ticks = 0;
                }
                if b == b'<' && ((!fenced && !inline_code && !json_quote) || suppressed.is_some()) {
                    tag = Some((before, String::new()));
                } else if let Some((at, name)) = &mut tag {
                    if b == b'>' {
                        tag_finished = true;
                        if let Some((recognized, closing, single)) =
                            presentation::harness_tag(name, self.role == "assistant")
                        {
                            if let Some(open) = &suppressed {
                                if recognized == open {
                                    if closing {
                                        suppressed_depth -= 1;
                                        if suppressed_depth == 0 {
                                            suppressed = None;
                                            start = self.position;
                                            text_started = false;
                                            text_json = false;
                                            json_quote = false;
                                            json_escape = false;
                                        }
                                    } else if !single {
                                        suppressed_depth = suppressed_depth.saturating_add(1);
                                    }
                                }
                            } else {
                                self.entry(start, *at, kind, start_context.as_deref())?;
                                if !closing && !single {
                                    suppressed = Some(recognized.to_owned());
                                    suppressed_depth = 1;
                                }
                                start = self.position;
                            }
                        } else if suppressed.is_none() && name == "realtime_delegation" {
                            self.entry(start, *at, kind, start_context.as_deref())?;
                            delegation_depth = delegation_depth.saturating_add(1);
                            self.text_context = 1;
                            start = self.position;
                        } else if suppressed.is_none()
                            && name == "/realtime_delegation"
                            && delegation_depth > 0
                        {
                            self.entry(start, *at, kind, start_context.as_deref())?;
                            delegation_depth -= 1;
                            if delegation_depth == 0 {
                                self.text_context = 0;
                            }
                            start = self.position;
                        } else if suppressed.is_none()
                            && delegation_depth > 0
                            && matches!(
                                name.as_str(),
                                "input"
                                    | "/input"
                                    | "transcript_delta"
                                    | "/transcript_delta"
                                    | "source"
                                    | "/source"
                            )
                        {
                            self.entry(start, *at, kind, start_context.as_deref())?;
                            self.text_context = match name.as_str() {
                                "transcript_delta" => 2,
                                "source" => 5,
                                _ => 1,
                            };
                            start = self.position;
                            line_start = self.position;
                            prefix.clear();
                            prefix_possible = true;
                        } else if suppressed.is_none() && (name == "final" || name == "/final") {
                            self.entry(start, *at, kind, start_context.as_deref())?;
                            start = self.position;
                        }
                        tag = None;
                    } else if name.len() < 128 && b.is_ascii() {
                        name.push(b as char);
                    } else {
                        tag = None;
                    }
                }
                if delegation_depth > 0
                    && matches!(self.text_context, 2..=4)
                    && prefix_possible
                    && !fenced
                    && !inline_code
                    && suppressed.is_none()
                    && !tag_finished
                    && tag.is_none()
                    && b != b'\\'
                {
                    if prefix.is_empty() && b.is_ascii_whitespace() {
                        line_start = self.position;
                    } else {
                        prefix.push(b as char);
                        if prefix == "user:" || prefix == "assistant:" {
                            self.entry(start, line_start, kind, start_context.as_deref())?;
                            self.text_context = if prefix == "user:" { 3 } else { 4 };
                            start = self.position;
                            prefix_possible = false;
                        } else if !"user:".starts_with(&prefix)
                            && !"assistant:".starts_with(&prefix)
                        {
                            prefix_possible = false;
                        }
                    }
                }
            }
            let mut decoded_count = 1;
            if collecting_language && !matches!(b, b'`' | b'~') && b != b'\\' && language.len() < 64
            {
                language.push(b as char);
            }
            if b == b'\\' {
                let escaped = self.byte()?.context("Unfinished native JSON escape")?;
                if small.len() < 512 {
                    small.push(escaped);
                }
                if text_json && suppressed.is_none() {
                    if json_escape {
                        json_escape = false;
                    } else if escaped == b'"' {
                        json_quote = !json_quote;
                    } else if escaped == b'\\' && json_quote {
                        json_escape = true;
                    }
                }
                if escaped == b'n' || escaped == b'r' {
                    collecting_language = false;
                    line_start = self.position;
                    prefix.clear();
                    prefix_possible = true;
                }
                if escaped == b'u' {
                    let mut digits = [0; 4];
                    for hex in &mut digits {
                        *hex = self.byte()?.context("Unfinished native Unicode escape")?;
                        if !hex.is_ascii_hexdigit() {
                            bail!("Invalid Unicode escape");
                        }
                        if small.len() < 512 {
                            small.push(*hex);
                        }
                    }
                    let unit = u16::from_str_radix(std::str::from_utf8(&digits)?, 16)?;
                    decoded_count = char::from_u32(unit as u32).map_or(4, |c| c.len_utf8()) as u64;
                    if (0xd800..=0xdbff).contains(&unit) {
                        self.expect(b'\\')?;
                        self.expect(b'u')?;
                        if small.len() < 510 {
                            small.extend_from_slice(b"\\u");
                        }
                        let mut low = [0; 4];
                        for hex in &mut low {
                            *hex = self.byte()?.context("Unfinished native Unicode pair")?;
                            if small.len() < 512 {
                                small.push(*hex);
                            }
                        }
                        let low = u16::from_str_radix(std::str::from_utf8(&low)?, 16)?;
                        if !(0xdc00..=0xdfff).contains(&low) {
                            bail!("Invalid native Unicode pair");
                        }
                    }
                } else if !b"\"\\/bfnrt".contains(&escaped) {
                    bail!("Invalid JSON escape");
                }
            } else if b >= 0x80 {
                let extra = if b < 0xe0 {
                    1
                } else if b < 0xf0 {
                    2
                } else {
                    3
                };
                decoded_count += extra;
                for _ in 0..extra {
                    let next = self.byte()?.context("Unfinished UTF-8")?;
                    if next & 0xc0 != 0x80 {
                        bail!("Invalid UTF-8");
                    }
                    if small.len() < 512 {
                        small.push(next);
                    }
                }
            }
            self.decoded[kind as usize] += decoded_count;
            if self.position - start >= (CHUNK_BYTES - 256) as u64 && tag.is_none() {
                self.entry(
                    start,
                    self.position,
                    if suppressed.is_some() { 0 } else { kind },
                    start_context.as_deref(),
                )?;
                start = self.position;
                start_context = if fenced {
                    Some(if fence_delimiter == b'~' {
                        format!("~~~{language}")
                    } else {
                        language.clone()
                    })
                } else {
                    None
                };
            }
        }
        small.push(b'"');
        Ok(serde_json::from_slice::<String>(&small).unwrap_or_default())
    }
    fn value(&mut self, path: &mut Vec<String>, depth: usize) -> Result<()> {
        if depth > 128 {
            bail!("Native JSON nesting exceeds 128 levels");
        }
        self.whitespace()?;
        if matches!(path.as_slice(),[parent,key] if parent=="payload" && key=="error") && self.peek()?.is_some_and(|byte|byte!=b'n'){
            self.terminal_error=true;
        }
        match self.peek()? {
            Some(b'{') => {
                let content_block = path.len() == 3
                    && matches!(path[0].as_str(), "payload" | "message")
                    && path[1] == "content"
                    && path[2] == "[]";
                let first_entry = self.entries;
                if content_block {
                    self.content_type.clear();
                }
                self.byte()?;
                self.whitespace()?;
                if self.peek()? == Some(b'}') {
                    self.byte()?;
                    return Ok(());
                }
                loop {
                    let key = self.string(0)?;
                    self.whitespace()?;
                    self.expect(b':')?;
                    path.push(key);
                    self.value(path, depth + 1)?;
                    path.pop();
                    self.whitespace()?;
                    match self.byte()? {
                        Some(b'}') => break,
                        Some(b',') => {
                            self.whitespace()?;
                        }
                        _ => bail!("Malformed native JSON object"),
                    }
                }
                if content_block
                    && !matches!(
                        self.content_type.as_str(),
                        "text" | "input_text" | "output_text" | "tool"
                    )
                {
                    for entry in first_entry..self.entries {
                        self.index.seek(SeekFrom::Start(entry * INDEX_BYTES + 16))?;
                        self.index.write_all(&[0])?;
                    }
                    self.index.seek(SeekFrom::End(0))?;
                }
            }
            Some(b'[') => {
                self.byte()?;
                self.whitespace()?;
                if self.peek()? == Some(b']') {
                    self.byte()?;
                    return Ok(());
                }
                loop {
                    path.push("[]".into());
                    self.value(path, depth + 1)?;
                    path.pop();
                    self.whitespace()?;
                    match self.byte()? {
                        Some(b']') => break,
                        Some(b',') => {}
                        _ => bail!("Malformed native JSON array"),
                    }
                }
            }
            Some(b'"') => {
                let kind = match path.as_slice() {
                    [a, b, c, d, e]
                        if a == "payload"
                            && b == "content"
                            && c == "[]"
                            && d == "state"
                            && matches!(e.as_str(), "output" | "error") =>
                    {
                        4
                    }
                    [a] if a == "content" => 2,
                    [a, b] if a == "content" && b == "[]" => 2,
                    [a, b, c] if a == "content" && b == "[]" && c == "text" => 2,
                    [a, b] if a == "payload" && b == "output" => 4,
                    [a, b] if a == "payload" && b == "text" => 1,
                    [a, b] if a == "payload" && matches!(b.as_str(), "arguments" | "input") => 3,
                    [a, b] if matches!(a.as_str(), "payload" | "message") && b == "content" => {
                        if a == "payload" { 1 } else { 2 }
                    }
                    [a, b, c]
                        if matches!(a.as_str(), "payload" | "message")
                            && b == "content"
                            && c == "[]" =>
                    {
                        if a == "payload" {
                            1
                        } else {
                            2
                        }
                    }
                    [a, b, c, d]
                        if matches!(a.as_str(), "payload" | "message")
                            && b == "content"
                            && c == "[]"
                            && d == "text" =>
                    {
                        if a == "payload" {
                            1
                        } else {
                            2
                        }
                    }
                    _ => 0,
                };
                let value = self.string(kind)?;
                match path.as_slice() {
                    [a, b, c, d]
                        if matches!(a.as_str(), "payload" | "message")
                            && b == "content"
                            && c == "[]"
                            && d == "type" =>
                    {
                        self.content_type = value
                    }

                    [a, b, c, d]
                        if a == "payload" && b == "content" && c == "[]" && d == "tool" =>
                    {
                        self.tool_name = value
                    }
                    [a, b, c, d, e]
                        if a == "payload"
                            && b == "content"
                            && c == "[]"
                            && d == "state"
                            && e == "status" =>
                    {
                        self.tool_status = value
                    }
                    [key] if key == "type" => self.root_type = value,
                    [parent, key] if parent == "payload" && key == "type" => {
                        self.payload_type = value
                    }
                    [parent,key] if parent=="payload" && key=="phase"=>{self.phase=value}
                    [parent,key] if parent=="payload" && key=="status"=>{self.terminal_status=value}
                    [parent, key]
                        if matches!(parent.as_str(), "payload" | "message") && key == "role" =>
                    {
                        self.role = value
                    }
                    [parent, key]
                        if parent == "payload"
                            && key == "author"
                            && self.role.is_empty()
                            && matches!(value.as_str(), "user" | "assistant") =>
                    {
                        self.role = value
                    }
                    [parent, key] if parent == "payload" && key == "name" => self.tool_name = value,
                    _ => {}
                }
            }
            Some(_) => {
                let mut scalar = Vec::new();
                while self
                    .peek()?
                    .is_some_and(|b| !b.is_ascii_whitespace() && !b",]}".contains(&b))
                {
                    let b = self.byte()?.unwrap();
                    if scalar.len() < 32 {
                        scalar.push(b);
                    }
                }
                if scalar.is_empty() {
                    bail!("Malformed native JSON scalar");
                }
                let value: serde_json::Value = serde_json::from_slice(&scalar)?;
                if path.as_slice() == ["isSidechain"] {
                    self.sidechain = value.as_bool().unwrap_or(false);
                }
            }
            None => bail!("Unfinished native JSON record"),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn path() -> PathBuf {
        std::env::temp_dir().join(format!("orbit-native-lazy-{}.jsonl", uuid::Uuid::new_v4()))
    }
    fn record(file: &mut File, text: &str) {
        writeln!(file,"{}",serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}})).unwrap();
    }
    #[test]
    fn fractional_seek_opens_real_head_tail_and_middle_without_scanning_the_log() {
        let path = path();
        let mut file = File::create(&path).unwrap();
        for n in 0..20_000 {
            record(&mut file, &format!("message {n:05} {}", "x".repeat(80)));
        }
        drop(file);
        for (fraction, expected) in [
            (0., "message 00000"),
            (1., "message 19999"),
            (0.5, "message 10000"),
        ] {
            let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
            cursor.seek_fraction(fraction, &|| false).unwrap();
            let page = cursor
                .page_budget_cancellable(10, PAGE_BYTES, &|| false)
                .unwrap();
            let visible = page.iter().map(|m| m.text.as_str()).collect::<String>();
            assert!(visible.contains(expected), "fraction {fraction}: {visible}");
            assert!(
                cursor.bytes_read < 512 * 1024,
                "seek read {} bytes",
                cursor.bytes_read
            );
            if fraction == 0. {
                assert!(!cursor.has_older());
            }
        }
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn fractional_seek_inside_a_giant_record_and_cancellation_stay_bounded() {
        let path = path();
        let mut file = File::create(&path).unwrap();
        let text = format!(
            "REAL START\n{}MIDDLE LANDMARK\n{}REAL END",
            "a".repeat(2 * 1024 * 1024),
            "b".repeat(2 * 1024 * 1024)
        );
        record(&mut file, &text);
        drop(file);
        let mut head = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        head.seek_fraction(0., &|| false).unwrap();
        let page = head
            .page_budget_cancellable(10, PAGE_BYTES, &|| false)
            .unwrap();
        assert!(page[0].text.starts_with("REAL START"));
        assert!(!head.has_older());
        assert!(
            head.page(200).unwrap().is_empty(),
            "retained head index must not expose an older second page"
        );
        assert!(page.iter().map(|m| m.text.len()).sum::<usize>() <= PAGE_BYTES);
        let mut mid = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        mid.seek_fraction(0.6, &|| false).unwrap();
        let page = mid
            .page_budget_cancellable(10, PAGE_BYTES, &|| false)
            .unwrap();
        assert!(page.iter().map(|m| m.text.len()).sum::<usize>() <= PAGE_BYTES);
        assert!(!page.is_empty());
        assert!(page.iter().any(|m| m.text.contains('b')));
        assert!(!page.iter().any(|m| m.text.contains("REAL END")));
        let indexed = mid.indexed_records;
        let read = mid.bytes_read;
        mid.seek_fraction(0.75, &|| false).unwrap();
        let second = mid
            .page_budget_cancellable(10, PAGE_BYTES, &|| false)
            .unwrap();
        assert_eq!(
            mid.indexed_records, indexed,
            "same giant record must reuse its span index"
        );
        assert!(mid.bytes_read - read < 128 * 1024);
        assert!(!second.is_empty());
        assert!(!second.iter().any(|m| m.text.contains("REAL END")));

        let cancelled = std::cell::Cell::new(0);
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        cursor.seek_fraction(0., &|| false).unwrap();
        assert!(
            cursor
                .page_budget_cancellable(200, PAGE_BYTES, &|| {
                    cancelled.set(cancelled.get() + 1);
                    cancelled.get() > 5
                })
                .is_err()
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn giant_plugin_context_is_skipped_and_real_text_and_literal_code_survive() {
        let path = path();
        let mut file = File::create(&path).unwrap();
        let context =
            "- Figma (figma@openai-curated-remote)\n```xml\nprivate context\n```\n".repeat(300_000);
        let text = format!(
            "Before.\n<recommended_plugins source=\"native\">{context}<recommended_plugins>nested</recommended_plugins></recommended_plugins>\nReal request.\n```xml\n<recommended_plugins>literal example</recommended_plugins>\n```\nUse `<recommended_plugins>` in code."
        );
        assert!(text.len() > 16 * 1024 * 1024);
        record(&mut file, &text);
        drop(file);
        let original = std::fs::metadata(&path).unwrap().len();
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let page = cursor.page(200).unwrap();
        let visible: String = page.iter().map(|m| m.text.as_str()).collect();
        assert!(visible.contains("Before.") && visible.contains("Real request."));
        assert!(visible.contains("<recommended_plugins>literal example</recommended_plugins>"));
        assert!(visible.contains("Use `<recommended_plugins>` in code."));
        assert!(!visible.contains("figma@") && !visible.contains("private context"));
        assert!(!cursor.has_older());
        assert_eq!(std::fs::metadata(&path).unwrap().len(), original);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn giant_ordinary_json_examples_keep_literal_harness_tag_strings() {
        let path = path();
        let mut file = File::create(&path).unwrap();
        let text=serde_json::json!({"literal":"<recommended_plugins>Example</recommended_plugins>","padding":"large user data ".repeat(30_000)}).to_string();
        record(&mut file, &text);
        drop(file);
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let mut pages = Vec::new();
        while cursor.has_older() {
            pages.push(cursor.page(200).unwrap());
        }
        pages.reverse();
        let visible: String = pages.into_iter().flatten().map(|m| m.text).collect();
        assert!(visible == text);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn gemini_jsonl_pages_text_array_and_giant_message_without_metadata() {
        let path = path();
        let mut file = File::create(&path).unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"sessionId":"session", "kind":"session"})
        )
        .unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"user", "content":[{"text":"Question"}]})
        )
        .unwrap();
        let text = "Gemini answer.\n".repeat(20_000);
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"gemini", "content":[{"text":text}]})
        )
        .unwrap();
        drop(file);
        let mut cursor = HistoryCursor::open(&path, AgentKind::Gemini).unwrap();
        let mut pages = Vec::new();
        while cursor.has_older() {
            pages.push(cursor.page(200).unwrap());
        }
        pages.reverse();
        let messages: Vec<_> = pages.into_iter().flatten().collect();
        assert_eq!(messages[0].role, "user");
        assert_eq!(messages[0].text, "Question");
        assert!(messages[1..].iter().all(|m| m.role == "assistant"));
        assert_eq!(
            messages[1..]
                .iter()
                .map(|m| m.text.as_str())
                .collect::<String>(),
            text
        );
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn twenty_mb_thread_opens_only_its_tail_then_reaches_the_first_turn() {
        let path = path();
        let mut file = File::create(&path).unwrap();
        record(&mut file, "The first turn");
        let text = "a".repeat(1024);
        for _ in 0..18_000 {
            record(&mut file, &text);
        }
        record(&mut file, "The latest turn");
        drop(file);
        let original = std::fs::metadata(&path).unwrap().len();
        assert!(original > 16 * 1024 * 1024);
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let first = cursor.page(20).unwrap();
        assert_eq!(first.last().unwrap().text, "The latest turn");
        assert!(
            cursor.bytes_read < 256 * 1024,
            "initial load should not read the whole transcript"
        );
        assert!(cursor.pending.capacity() <= 4);
        let mut seen = first.len();
        let mut oldest = String::new();
        while cursor.has_older() {
            let page = cursor.page(200).unwrap();
            assert!(page.iter().map(|m| m.text.len()).sum::<usize>() <= PAGE_BYTES);
            if let Some(first) = page.first() {
                oldest = first.text.clone();
            }
            seen += page.len();
        }
        assert_eq!(seen, 18_002);
        assert_eq!(oldest, "The first turn");
        assert_eq!(std::fs::metadata(&path).unwrap().len(), original);
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn a_single_twenty_mb_message_is_losslessly_paged_with_bounded_buffers() {
        let path = path();
        let mut file = File::create(&path).unwrap();
        file.write_all(br#"{"type":"response_item","payload":{"content":[{"type":"output_text","text":"START "#).unwrap();
        let block = b"a \\n\\uD83D\\uDE80 ";
        let repetitions = 1_300_000;
        let mut buffered = std::io::BufWriter::new(file);
        for _ in 0..repetitions {
            buffered.write_all(block).unwrap();
        }
        buffered
            .write_all(
                br#" END"}],"role":"assistant","type":"message"}}
"#,
            )
            .unwrap();
        drop(buffered);
        let original = std::fs::metadata(&path).unwrap().len();
        assert!(original > 16 * 1024 * 1024);
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let mut pages = 0;
        let mut bytes = 0;
        let mut tail = String::new();
        let mut head = String::new();
        while cursor.has_older() {
            let page = cursor.page(20).unwrap();
            let count = page.iter().map(|m| m.text.len()).sum::<usize>();
            assert!(count <= PAGE_BYTES);
            assert!(cursor.buffer.capacity() <= CHUNK_BYTES);
            assert!(cursor.pending.len() <= 1);
            if pages == 0 {
                tail = page.last().unwrap().text.clone();
            }
            if let Some(m) = page.first() {
                head = m.text.clone();
            }
            bytes += count;
            pages += 1;
        }
        assert!(pages > 20);
        assert!(head.starts_with("START "));
        assert!(tail.ends_with(" END"));
        assert_eq!(
            bytes,
            "START ".len() + repetitions * "a \n🚀 ".len() + " END".len()
        );
        assert_eq!(std::fs::metadata(&path).unwrap().len(), original);
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn malformed_complete_records_fail_and_unfinished_tails_do_not_hide_earlier_turns() {
        let path = path();
        std::fs::write(&path, b"not JSON\n").unwrap();
        assert!(
            HistoryCursor::open(&path, AgentKind::Codex)
                .unwrap()
                .page(20)
                .is_err()
        );
        let mut file = File::create(&path).unwrap();
        record(&mut file, "Valid earlier turn");
        file.write_all(
            br#"{"type":"response_item","payload":{"type":"message","content":[{"text":""#,
        )
        .unwrap();
        file.write_all(&vec![b'x'; 256 * 1024]).unwrap();
        drop(file);
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        assert_eq!(cursor.page(20).unwrap()[0].text, "Valid earlier turn");
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn large_embedded_media_and_controls_are_not_rendered_as_chat() {
        let path = path();
        let metadata = "BASE64DATA".repeat(80_000);
        let text = format!(
            "<environment_context>{}</environment_context>\nReal prompt after large metadata",
            "machine context ".repeat(50_000)
        );
        std::fs::write(&path,format!("{}\n{}\n",serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"image","source":{"type":"base64","data":metadata}},{"type":"text","text":"Actual user text"}]}}),serde_json::json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":text}]}}))).unwrap();
        let mut cursor = HistoryCursor::open(&path, AgentKind::Claude).unwrap();
        let page = cursor.page(20).unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].text, "Actual user text");
        assert_eq!(page[1].text.trim(), "Real prompt after large metadata");
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn an_in_progress_snapshot_tail_stays_incomplete_when_the_native_file_grows() {
        for size in [20, 256 * 1024] {
            let path = path();
            let mut file = File::create(&path).unwrap();
            record(&mut file, "Earlier complete message");
            file.write_all(br#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":""#).unwrap();
            file.write_all(&vec![b'x'; size]).unwrap();
            drop(file);
            let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
            let mut append = OpenOptions::new().append(true).open(&path).unwrap();
            append.write_all(b"\"}]}}\n").unwrap();
            drop(append);
            let page = cursor.page(20).unwrap();
            assert_eq!(page.len(), 1);
            assert_eq!(page[0].text, "Earlier complete message");
            drop(cursor);
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn oversized_claude_content_uses_the_same_lazy_adapter() {
        let path = path();
        let text = "Claude ".repeat(30_000);
        std::fs::write(&path,serde_json::json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":text}]}}).to_string()).unwrap();
        let mut cursor = HistoryCursor::open(&path, AgentKind::Claude).unwrap();
        let mut chunks = Vec::new();
        while cursor.has_older() {
            let mut page = cursor.page(1).unwrap();
            page.append(&mut chunks);
            chunks = page;
        }
        assert_eq!(chunks.into_iter().map(|m| m.text).collect::<String>(), text);
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn giant_canonical_voice_segments_keep_every_byte_in_bounded_typed_pages() {
        let path = path();
        let text = "Voice 😀 words\n".repeat(1_100_000);
        assert!(text.len() > 16 * 1024 * 1024);
        std::fs::write(&path,format!("{}\n",serde_json::json!({"type":"event_msg","payload":{"type":"transcript_segment","role":"assistant","text":text}}))).unwrap();
        let original = std::fs::metadata(&path).unwrap().len();
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let mut decoded = 0;
        let mut pages = 0;
        while cursor.has_older() {
            let page = cursor.page(40).unwrap();
            assert!(
                page.iter().map(|message| message.text.len()).sum::<usize>() <= super::PAGE_BYTES
            );
            for message in page {
                let card = crate::chat_interactions::decode(&message.text)
                    .expect("independent typed continuation");
                let crate::chat_interactions::ChatInteraction::Speaker { speaker, text } = card
                else {
                    panic!("voice speaker")
                };
                assert_eq!(speaker, crate::chat_interactions::Speaker::Assistant);
                decoded += text.len();
            }
            pages += 1;
        }
        assert_eq!(decoded, text.len());
        assert!(pages > 20);
        assert_eq!(std::fs::metadata(&path).unwrap().len(), original);
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn giant_delegation_continuations_keep_speakers_and_hide_source_metadata() {
        let path = path();
        let input = "Inspect the project 😀. ".repeat(30_000);
        let user = "User voice words. ".repeat(20_000);
        let text = format!(
            "<realtime_delegation><source>{}</source><input>{input}</input><transcript_delta>user:{user}\nassistant: Ready for the next step</transcript_delta></realtime_delegation>",
            "PRIVATE_SOURCE".repeat(20_000)
        );
        std::fs::write(&path,format!("{}\n",serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}))).unwrap();
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let mut all = vec![];
        while cursor.has_older() {
            let mut page = cursor.page(10).unwrap();
            page.append(&mut all);
            all = page;
        }
        let mut task = String::new();
        let mut user_text = String::new();
        let mut assistant_text = String::new();
        for message in all {
            assert!(!message.text.contains("PRIVATE_SOURCE"));
            assert!(!message.text.contains("transcript_delta"));
            if message.text.trim().is_empty() {
                continue;
            }
            match crate::chat_interactions::decode(&message.text)
                .expect("typed delegation continuation")
            {
                crate::chat_interactions::ChatInteraction::Delegation { details, .. } => {
                    task.push_str(&details)
                }
                crate::chat_interactions::ChatInteraction::Speaker {
                    speaker: crate::chat_interactions::Speaker::User,
                    text,
                } => user_text.push_str(&text),
                crate::chat_interactions::ChatInteraction::Speaker {
                    speaker: crate::chat_interactions::Speaker::Assistant,
                    text,
                } => assistant_text.push_str(&text),
                _ => panic!("unexpected interaction"),
            }
        }
        assert_eq!(task, input);
        assert_eq!(user_text.trim(), user.trim());
        assert_eq!(assistant_text.trim(), "Ready for the next step");
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn giant_voice_harness_context_cannot_leak_nested_delegation_or_final_tags() {
        let path = path();
        let text = format!(
            "<realtime_conversation><realtime_delegation><input>{}</input><transcript_delta>user: internal</transcript_delta><final>internal</final></realtime_delegation></realtime_conversation> Actual request",
            "HIDDEN_INSTRUCTIONS".repeat(20_000)
        );
        std::fs::write(&path,format!("{}\n",serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}))).unwrap();
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let page = cursor.page(20).unwrap();
        assert_eq!(page.len(), 1);
        assert_eq!(page[0].text.trim(), "Actual request");
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn giant_tool_output_with_code_fences_stays_typed_and_lossless(){
        let path=path();let source=format!("```swift\n{}\n```", "let text = \"literal \\n\"; // *source*\n".repeat(12_000));
        std::fs::write(&path,format!("{}\n",serde_json::json!({"type":"response_item","payload":{"type":"function_call_output","output":source}}))).unwrap();
        let mut cursor=HistoryCursor::open(&path,AgentKind::Codex).unwrap();let mut all=vec![];
        while cursor.has_older(){let mut page=cursor.page(20).unwrap();page.append(&mut all);all=page;}
        let mut actual=String::new();for message in all{assert_eq!(message.role,"tool");assert!(message.code_language.is_none());let v:serde_json::Value=serde_json::from_str(&message.text).unwrap();assert_eq!(v["orbit_tool"],true);actual.push_str(v["details"].as_str().unwrap());}
        assert_eq!(actual,source);drop(cursor);std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn encoded_long_speaker_continuations_never_fall_back_to_garbled_tool_json() {
        let text = "line\n".repeat(12_000);
        let card = crate::chat_interactions::ChatInteraction::Speaker {
            speaker: crate::chat_interactions::Speaker::User,
            text: text.clone(),
        };
        let pieces = super::split_message(super::TranscriptMessage {
            role: "tool".into(),
            code_language: None,
            text: crate::chat_interactions::encode(&card),
        });
        assert!(pieces.len() > 1);
        let mut result = String::new();
        for piece in pieces {
            assert!(piece.text.len() <= super::CHUNK_BYTES + 256);
            let crate::chat_interactions::ChatInteraction::Speaker { text, .. } =
                crate::chat_interactions::decode(&piece.text).unwrap()
            else {
                panic!("speaker")
            };
            result.push_str(&text);
        }
        assert_eq!(result, text);
    }
    #[test]
    fn giant_agent_message_author_is_safe_role_fallback_and_media_stays_hidden() {
        let path = path();
        let text = "Actual assistant reply. ".repeat(10_000);
        std::fs::write(&path,format!("{}\n",serde_json::json!({"type":"response_item","payload":{"type":"agent_message","author":"assistant","content":[{"type":"encrypted_content","text":"PRIVATE_ENCRYPTED"},{"type":"input_text","text":text}]}}))).unwrap();
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let mut all = vec![];
        while cursor.has_older() {
            let mut page = cursor.page(10).unwrap();
            page.append(&mut all);
            all = page;
        }
        assert!(all.iter().all(|message| message.role == "assistant"));
        assert_eq!(
            all.into_iter()
                .map(|message| message.text)
                .collect::<String>(),
            text
        );
        drop(cursor);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn giant_voice_fenced_code_keeps_style_and_speaker_across_lazy_pages() {
        for delimiter in ["```", "~~~"] {
            let path = path();
            let source = "let value: u64 = 1234; // source\n".repeat(12_000);
            let spoken = format!(
                "Before the code.\n{delimiter}rust\n{source}{delimiter}\nAfter [the file](/project/a.rs:8)."
            );
            assert!(spoken.len() > 128 * 1024);
            std::fs::write(&path,format!("{}\n",serde_json::json!({"type":"event_msg","payload":{"type":"transcript_segment","role":"assistant","text":spoken}}))).unwrap();
            let original = std::fs::metadata(&path).unwrap().len();
            let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
            let mut all = vec![];
            let mut pages = 0;
            while cursor.has_older() {
                let mut page = cursor.page(2).unwrap();
                assert!(
                    page.iter().map(|message| message.text.len()).sum::<usize>()
                        <= super::PAGE_BYTES
                );
                page.append(&mut all);
                all = page;
                pages += 1;
            }
            assert!(pages >= 3);
            let mut decoded = String::new();
            let mut continuations = 0;
            let mut code_chunks = 0;
            let mut after_link = false;
            for message in all {
                let (card, context) =
                    crate::chat_interactions::decode_with_context(&message.text).unwrap();
                let crate::chat_interactions::ChatInteraction::Speaker { speaker, text } = card
                else {
                    panic!("speaker")
                };
                assert_eq!(speaker, crate::chat_interactions::Speaker::Assistant);
                decoded.push_str(&text);
                continuations += usize::from(context.is_some());
                let blocks = crate::presentation::message_blocks(&message.role, &message.text);
                assert!(blocks.iter().any(|block| matches!(
                    block,
                    crate::presentation::MessageBlock::Voice {
                        speaker: crate::chat_interactions::Speaker::Assistant,
                        ..
                    }
                )));
                for block in blocks {
                    match block {
                        crate::presentation::MessageBlock::Code {
                            language,
                            text,
                            highlights,
                        } => {
                            assert_eq!(language, "rust");
                            assert!(!text.is_empty());
                            assert!(
                                highlights
                                    .iter()
                                    .any(|(range, _)| text.get(range.clone()) == Some("let"))
                            );
                            code_chunks += 1;
                        }
                        crate::presentation::MessageBlock::Voice {
                            text, highlights, ..
                        } => {
                            assert!(!text.contains("value: u64"), "code must stay in a code box");
                            after_link |= text=="After the file." && highlights.iter().any(|(_, style)|matches!(style,crate::presentation::InlineStyle::Link(url) if url.as_ref()=="/project/a.rs:8"));
                        }
                        _ => panic!("unexpected speaker block"),
                    }
                }
            }
            assert_eq!(decoded, spoken);
            assert!(continuations >= 3 && code_chunks >= 4);
            assert!(after_link);
            assert_eq!(std::fs::metadata(&path).unwrap().len(), original);
            drop(cursor);
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn medium_voice_fenced_code_envelopes_retain_context_when_split() {
        let source = "let value = 42;\n".repeat(6000);
        let spoken = format!("```rust\n{source}```\nFinished.");
        let card = crate::chat_interactions::ChatInteraction::Speaker {
            speaker: crate::chat_interactions::Speaker::User,
            text: spoken.clone(),
        };
        let pieces = super::split_message(super::TranscriptMessage {
            role: "tool".into(),
            code_language: None,
            text: crate::chat_interactions::encode(&card),
        });
        assert!(pieces.len() >= 3);
        let mut decoded = String::new();
        for piece in pieces {
            let card = crate::chat_interactions::decode(&piece.text).unwrap();
            let crate::chat_interactions::ChatInteraction::Speaker { speaker, text } = card else {
                panic!("speaker")
            };
            assert_eq!(speaker, crate::chat_interactions::Speaker::User);
            decoded.push_str(&text);
            let blocks = crate::presentation::message_blocks(&piece.role, &piece.text);
            assert!(blocks.iter().any(|block|matches!(block, crate::presentation::MessageBlock::Code {language,..} if language=="rust")));
            assert!(blocks.iter().any(|block| matches!(
                block,
                crate::presentation::MessageBlock::Voice {
                    speaker: crate::chat_interactions::Speaker::User,
                    ..
                }
            )));
        }
        assert_eq!(decoded, spoken);
    }
    #[test]
    fn compressed_twenty_mb_history_pages_and_restores_without_redecoding() {
        let plain = path();
        let archive = plain.with_extension("jsonl.zst");
        let mut file = File::create(&plain).unwrap();
        let body = "native archive 中😀 ".repeat(1900);
        for n in 0..700 {
            record(&mut file, &format!("marker-{n} {body}"));
        }
        drop(file);
        assert!(std::fs::metadata(&plain).unwrap().len() > 20 * 1024 * 1024);
        let mut encoder = zstd::stream::read::Encoder::new(File::open(&plain).unwrap(), 1).unwrap();
        std::io::copy(&mut encoder, &mut File::create(&archive).unwrap()).unwrap();
        std::fs::remove_file(plain).unwrap();
        let original = FileStamp::read(&archive).unwrap();
        let mut cursor = HistoryCursor::open(&archive, AgentKind::Codex).unwrap();
        let first = cursor.page(4).unwrap();
        assert!(first.last().unwrap().text.starts_with("marker-699 "));
        assert!(
            cursor.bytes_read < 1024 * 1024,
            "Paging reads only a bounded decoded tail"
        );
        let checkpoint = cursor.checkpoint();
        let held: std::collections::HashMap<_, _> = cursor
            .artifacts()
            .into_iter()
            .map(|a| (a.path.clone(), a))
            .collect();
        let prepared = checkpoint.prepared_source.clone().unwrap();
        assert_eq!(
            std::fs::metadata(&prepared).unwrap().permissions().mode() & 0o777,
            0o600
        );
        drop(cursor);
        let mut resolve = |name: &Path| {
            held.get(name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing prepared source"))
        };
        let mut restored = HistoryCursor::restore(checkpoint.clone(), None, &mut resolve).unwrap();
        assert_eq!(restored.prepared_source.as_ref().unwrap().path, prepared);
        let mut count = first.len();
        while restored.has_older() {
            let page = restored.page(20).unwrap();
            assert!(page.iter().map(|m| m.text.len()).sum::<usize>() <= PAGE_BYTES);
            for message in &page {
                assert!(message.text.contains("中😀"));
            }
            count += page.len();
        }
        assert_eq!(count, 700);
        assert!(original.matches(&archive), "Never rewrite a native archive");
        drop(restored);
        drop(held);
        assert!(
            !prepared.exists(),
            "The last paging owner releases its private decoded source"
        );
        std::fs::remove_file(archive).unwrap();
    }

    #[test]
    fn compressed_history_can_be_cancelled_and_corruption_never_appears_as_empty_history() {
        let archive = path().with_extension("jsonl.zst");
        let data = vec![b'x'; 2 * 1024 * 1024];
        let encoded = zstd::stream::encode_all(data.as_slice(), 1).unwrap();
        std::fs::write(&archive, &encoded).unwrap();
        let original = FileStamp::read(&archive).unwrap();
        let calls = std::cell::Cell::new(0);
        let result = HistoryCursor::open_cancellable(&archive, AgentKind::Codex, || {
            calls.set(calls.get() + 1);
            calls.get() >= 3
        });
        assert!(result.err().unwrap().to_string().contains("superseded"));
        assert_eq!(calls.get(), 3);
        assert!(original.matches(&archive));
        for bad in [
            b"not a compressed frame".as_slice(),
            &encoded[..encoded.len() - 1],
        ] {
            std::fs::write(&archive, bad).unwrap();
            assert!(HistoryCursor::open(&archive, AgentKind::Codex).is_err());
        }
        std::fs::remove_file(archive).unwrap();
    }

    #[test]
    #[ignore = "read-only inspection of an explicitly selected installed Codex archive"]
    fn installed_codex_archive_opens_and_paginates() {
        let path = PathBuf::from(
            std::env::var_os("CODE_ARCHIVE_SMOKE_PATH")
                .expect("select a native .jsonl.zst archive"),
        );
        let original = FileStamp::read(&path).unwrap();
        let mut cursor = HistoryCursor::open(&path, AgentKind::Codex).unwrap();
        let page = cursor.page(40).unwrap();
        assert!(!page.is_empty());
        assert!(page.iter().map(|message| message.text.len()).sum::<usize>() <= PAGE_BYTES);
        let checkpoint = cursor.checkpoint();
        let held: std::collections::HashMap<_, _> = cursor
            .artifacts()
            .into_iter()
            .map(|a| (a.path.clone(), a))
            .collect();
        let mut resolve = |name: &Path| {
            held.get(name)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("missing prepared source"))
        };
        let mut restored = HistoryCursor::restore(checkpoint, None, &mut resolve).unwrap();
        let earlier = restored.page(40).unwrap();
        assert!(
            earlier
                .iter()
                .map(|message| message.text.len())
                .sum::<usize>()
                <= PAGE_BYTES
        );
        assert!(original.matches(&path));
        println!(
            "ARCHIVE_SMOKE_OK tail_rows={} earlier_rows={} decoded_tail_bytes={}",
            page.len(),
            earlier.len(),
            cursor.bytes_read
        );
    }
}
