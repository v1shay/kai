//! Sparse fence context for byte slices inside an existing SQLite text cell.
use crate::presentation::HarnessState;
use anyhow::{Result, ensure};
use rusqlite::blob::Blob;
use std::{
    fs::{File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    os::unix::fs::OpenOptionsExt,
    sync::Arc,
};
const CHUNK: usize = 64 * 1024;
const ENTRY: u64 = 2048;
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct Fence {
    language: Option<String>,
    delimiter: u8,
    length: u8,
    line: Vec<u8>,
    overflow: bool,
}
impl Fence {
    fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            if byte == b'\n' {
                let line = String::from_utf8_lossy(&self.line);
                let line = line.trim_start();
                let delimiter = line.as_bytes().first().copied().unwrap_or(0);
                let length = line.bytes().take_while(|b| *b == delimiter).count();
                if matches!(delimiter, b'`' | b'~') && length >= 3 {
                    if self.language.is_some() {
                        if delimiter == self.delimiter
                            && length >= self.length as usize
                            && line[length..].trim().is_empty()
                            && !self.overflow
                        {
                            self.language = None;
                        }
                    } else {
                        self.language = Some(line[length..].trim().chars().take(64).collect());
                        self.delimiter = delimiter;
                        self.length = length.min(255) as u8;
                    }
                }
                self.line.clear();
                self.overflow = false;
            } else if self.line.len() < 128 {
                self.line.push(byte);
            } else if !byte.is_ascii_whitespace() {
                self.overflow = true;
            }
        }
    }
}
#[derive(Default, serde::Serialize, serde::Deserialize)]
struct ScanState {
    fence: Fence,
    harness: HarnessState,
}
impl ScanState {
    fn feed(&mut self, byte: u8, assistant: bool) -> Option<std::ops::Range<usize>> {
        let hidden_before = self.harness.hidden();
        let range = self
            .harness
            .feed(byte, self.fence.language.is_some(), assistant);
        if (!hidden_before && !self.harness.hidden()) || byte == b'\n' {
            self.fence.feed(&[byte]);
        }
        range
    }
    fn language(&self) -> Option<String> {
        self.fence.language.as_ref().map(|language| {
            if self.fence.delimiter == b'~' {
                format!("~~~{language}")
            } else {
                language.clone()
            }
        })
    }
}

type FilteredContext = (Option<String>, Vec<std::ops::Range<usize>>, Option<usize>);

pub struct PlainIndex {
    file: File,
    artifact: Arc<crate::history_artifact::HistoryArtifact>,
}
impl PlainIndex {
    pub(crate) fn artifact(&self) -> Arc<crate::history_artifact::HistoryArtifact> {
        self.artifact.clone()
    }
    pub(crate) fn restore(artifact: Arc<crate::history_artifact::HistoryArtifact>) -> Result<Self> {
        Ok(Self {
            file: File::open(&artifact.path)?,
            artifact,
        })
    }
    pub fn new_cancellable(
        blob: &Blob<'_>,
        assistant: bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<Self> {
        let path =
            std::env::temp_dir().join(format!("orbit-local-fences-{}", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let mut index = Self {
            file,
            artifact: crate::history_artifact::HistoryArtifact::own(path),
        };
        let mut state = ScanState::default();
        let mut buffer = vec![0; CHUNK];
        let length = blob.size() as usize;
        for offset in (0..length).step_by(CHUNK) {
            ensure!(!cancelled(), "History seek was superseded");
            let encoded = serde_json::to_vec(&state)?;
            ensure!(
                encoded.len() + 4 <= ENTRY as usize,
                "Sparse history state exceeded its fixed budget"
            );
            let mut entry = [0; ENTRY as usize];
            entry[..4].copy_from_slice(&(encoded.len() as u32).to_le_bytes());
            entry[4..4 + encoded.len()].copy_from_slice(&encoded);
            index.file.write_all(&entry)?;
            let count = (length - offset).min(CHUNK);
            blob.read_at_exact(&mut buffer[..count], offset)?;
            for &byte in &buffer[..count] {
                state.feed(byte, assistant);
            }
        }
        Ok(index)
    }
    /// Filter a physical text slice using the exact context at its byte offset.
    /// The index and replay use constant-sized buffers, including hidden spans.
    pub fn filter(
        &mut self,
        blob: &Blob<'_>,
        offset: usize,
        end: usize,
        assistant: bool,
        body: &[u8],
    ) -> Result<FilteredContext> {
        let block = offset / CHUNK;
        let begin = block * CHUNK;
        self.file.seek(SeekFrom::Start(block as u64 * ENTRY))?;
        let mut entry = [0; ENTRY as usize];
        self.file.read_exact(&mut entry)?;
        let size = u32::from_le_bytes(entry[..4].try_into().unwrap()) as usize;
        ensure!(size <= ENTRY as usize - 4, "Invalid sparse history state");
        let mut state: ScanState = serde_json::from_slice(&entry[4..4 + size])?;
        ensure!(body.len() == end - offset, "Invalid cached history slice");
        // The caller already read the body from the incremental BLOB. Replay
        // only the sparse prefix and tiny tag lookahead, rather than reading
        // every visible byte from SQLite a second time.
        let mut prefix = vec![0; offset - begin];
        blob.read_at_exact(&mut prefix, begin)?;
        for &byte in &prefix {
            state.feed(byte, assistant);
        }
        let language = state.language();
        let mut hidden = Vec::new();
        for &byte in body {
            if let Some(range) = state.feed(byte, assistant) {
                hidden.push(range);
            }
        }
        let lookahead_end = end.saturating_add(256).min(blob.size() as usize);
        let mut lookahead = [0; 256];
        blob.read_at_exact(&mut lookahead[..lookahead_end - end], end)?;
        for &byte in &lookahead[..lookahead_end - end] {
            if let Some(range) = state.feed(byte, assistant) {
                hidden.push(range);
            }
        }
        if let Some(range) = state.harness.pending_range() {
            hidden.push(range);
        }
        let skip_before = hidden
            .iter()
            .find(|range| range.start <= offset && range.end >= end)
            .map(|range| range.start);
        hidden = hidden
            .into_iter()
            .filter_map(|range| {
                let start = range.start.max(offset);
                let stop = range.end.min(end);
                (start < stop).then_some(start - offset..stop - offset)
            })
            .collect();
        hidden.sort_by_key(|range| range.start);
        Ok((language, hidden, skip_before))
    }
}
