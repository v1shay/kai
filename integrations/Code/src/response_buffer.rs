//! UTF-8 responses stay small in RAM and spill once into a private, owned file.
//! Sources become immutable when published, so worker/UI snapshots can share
//! them without sharing a writable descriptor or copying a large response.
use crate::history_artifact::HistoryArtifact;
use anyhow::{Result, anyhow, ensure};
use std::{
    fmt,
    fs::{File, OpenOptions},
    io::{self, Write},
    ops::Range,
    os::unix::fs::{FileExt, OpenOptionsExt},
    sync::Arc,
};

pub(crate) const MEMORY_BYTES: usize = 256 * 1024;
const COPY_BYTES: usize = 64 * 1024;

#[derive(Clone)]
pub(crate) enum ResponseSource {
    Memory(String),
    File {
        artifact: Arc<HistoryArtifact>,
        len: usize,
    },
}
impl fmt::Debug for ResponseSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Responses can contain private source code and tool output.
        match self {
            Self::Memory(text) => f.debug_struct("Memory").field("len", &text.len()).finish(),
            Self::File { artifact, len } => f
                .debug_struct("File")
                .field("path", &artifact.path)
                .field("len", len)
                .finish(),
        }
    }
}
impl PartialEq for ResponseSource {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Memory(a), Self::Memory(b)) => a == b,
            (
                Self::File {
                    artifact: a,
                    len: al,
                },
                Self::File {
                    artifact: b,
                    len: bl,
                },
            ) => a.path == b.path && al == bl,
            _ => false,
        }
    }
}
impl Eq for ResponseSource {}
impl ResponseSource {
    pub(crate) fn len(&self) -> usize {
        match self {
            Self::Memory(text) => text.len(),
            Self::File { len, .. } => *len,
        }
    }
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }
    #[cfg(test)]
    pub(crate) fn read_range(&self, range: Range<usize>) -> Result<String> {
        match self {
            Self::Memory(text) => memory_range(text, range),
            Self::File { artifact, len } => file_range(&File::open(&artifact.path)?, *len, range),
        }
    }
    pub(crate) fn tail(&self, max_bytes: usize) -> Result<String> {
        let start = self.len().saturating_sub(max_bytes);
        match self {
            Self::Memory(text) => {
                let mut start = start;
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                Ok(text[start..].to_owned())
            }
            Self::File { artifact, len } => {
                let file = File::open(&artifact.path)?;
                file_range(&file, *len, tail_start(&file, start, *len)?..*len)
            }
        }
    }
    #[cfg(test)]
    pub(crate) fn matches_at(&self, offset: usize, text: &str) -> Result<bool> {
        match self {
            Self::Memory(value) => {
                Ok(value.get(offset..offset.saturating_add(text.len())) == Some(text))
            }
            Self::File { artifact, len } => {
                file_matches(&File::open(&artifact.path)?, *len, offset, text)
            }
        }
    }
    pub(crate) fn copy_range_to(
        &self,
        start: usize,
        end: usize,
        target: &mut impl Write,
    ) -> Result<()> {
        match self {
            Self::Memory(text) => copy_memory(text, start..end, target),
            Self::File { artifact, len } => {
                copy_file(&File::open(&artifact.path)?, *len, start..end, target)
            }
        }
    }
}

enum Storage {
    Memory(String),
    File {
        file: File,
        artifact: Arc<HistoryArtifact>,
    },
}
pub(crate) struct ResponseBuffer {
    storage: Storage,
    len: usize,
    error: Option<anyhow::Error>,
    failed: bool,
    first_non_whitespace: Option<usize>,
}
impl Default for ResponseBuffer {
    fn default() -> Self {
        Self {
            storage: Storage::Memory(String::new()),
            len: 0,
            error: None,
            failed: false,
            first_non_whitespace: None,
        }
    }
}
impl From<String> for ResponseBuffer {
    fn from(text: String) -> Self {
        if text.len() <= MEMORY_BYTES && text.capacity() <= MEMORY_BYTES {
            Self {
                len: text.len(),
                first_non_whitespace: text
                    .char_indices()
                    .find(|(_, ch)| !ch.is_whitespace())
                    .map(|(at, _)| at),
                storage: Storage::Memory(text),
                ..Self::default()
            }
        } else {
            let mut buffer = Self::default();
            buffer.push_str(&text);
            buffer
        }
    }
}
impl ResponseBuffer {
    #[cfg(test)]
    pub(crate) fn from_source(source: ResponseSource) -> Self {
        match source {
            ResponseSource::Memory(text) => text.into(),
            source => {
                let mut buffer = Self::default();
                if let Err(error) = source.copy_range_to(0, source.len(), &mut buffer) {
                    buffer.record(error);
                }
                buffer
            }
        }
    }
    pub(crate) fn len(&self) -> usize {
        self.len
    }
    pub(crate) fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub(crate) fn error(&self) -> Option<&anyhow::Error> {
        self.error.as_ref()
    }
    #[cfg(test)]
    pub(crate) fn take_error(&mut self) -> Option<anyhow::Error> {
        self.error.take()
    }
    pub(crate) fn has_non_whitespace(&self) -> bool {
        self.first_non_whitespace.is_some()
    }
    fn record(&mut self, error: anyhow::Error) {
        self.failed = true;
        if self.error.is_none() {
            self.error = Some(error.context("Could not retain the agent response"));
        }
    }
    pub(crate) fn push_str(&mut self, text: &str) {
        if self.failed || text.is_empty() {
            return;
        }
        if let Err(error) = self.append(text) {
            self.record(error);
        }
    }
    fn append(&mut self, text: &str) -> Result<()> {
        let next = self
            .len
            .checked_add(text.len())
            .ok_or_else(|| anyhow!("Response length overflow"))?;
        if let Storage::Memory(memory) = &mut self.storage {
            if next <= MEMORY_BYTES {
                if memory.capacity() < next {
                    let capacity = memory
                        .capacity()
                        .max(2048)
                        .saturating_mul(2)
                        .min(MEMORY_BYTES)
                        .max(next);
                    memory.try_reserve_exact(capacity - memory.len())?;
                }
                memory.push_str(text);
            } else {
                let path =
                    std::env::temp_dir().join(format!("code-response-{}", uuid::Uuid::new_v4()));
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .read(true)
                    .write(true)
                    .mode(0o600)
                    .open(&path)?;
                let artifact = HistoryArtifact::own(path);
                for chunk in memory.as_bytes().chunks(COPY_BYTES) {
                    file.write_all(chunk)?;
                }
                for chunk in text.as_bytes().chunks(COPY_BYTES) {
                    file.write_all(chunk)?;
                }
                self.storage = Storage::File { file, artifact };
            }
        } else if let Storage::File { file, .. } = &mut self.storage {
            // Positional writes cannot inherit a read cursor from preview work.
            let mut offset = self.len;
            for chunk in text.as_bytes().chunks(COPY_BYTES) {
                if let Err(error) = file.write_all_at(chunk, offset as u64) {
                    let _ = file.set_len(self.len as u64);
                    return Err(error.into());
                }
                offset += chunk.len();
            }
        }
        if self.first_non_whitespace.is_none() {
            self.first_non_whitespace = text
                .char_indices()
                .find(|(_, ch)| !ch.is_whitespace())
                .map(|(at, _)| self.len + at);
        }
        self.len = next;
        Ok(())
    }
    pub(crate) fn truncate(&mut self, len: usize) {
        if self.failed || len >= self.len {
            return;
        }
        if matches!(&self.storage, Storage::File { artifact, .. } if Arc::strong_count(artifact) > 1)
        {
            // Published sources expose an immutable prefix. Appends can share
            // it, but a correction must copy before changing those bytes.
            let mut replacement = Self::default();
            match self.copy_range_to(0, len, &mut replacement) {
                Ok(()) => *self = replacement,
                Err(error) => self.record(error),
            }
            return;
        }
        let result = match &mut self.storage {
            Storage::Memory(text) => {
                if text.is_char_boundary(len) {
                    text.truncate(len);
                    Ok(())
                } else {
                    Err(anyhow!("Response truncation splits a UTF-8 character"))
                }
            }
            Storage::File { file, .. } => file_boundary(file, len).and_then(|valid| {
                ensure!(valid, "Response truncation splits a UTF-8 character");
                file.set_len(len as u64)?;
                Ok(())
            }),
        };
        if let Err(error) = result {
            self.record(error);
            return;
        }
        self.len = len;
        if self.first_non_whitespace.is_some_and(|first| first >= len) {
            self.first_non_whitespace = None;
        }
    }
    #[cfg(test)]
    pub(crate) fn read_range(&self, range: Range<usize>) -> Result<String> {
        match &self.storage {
            Storage::Memory(text) => memory_range(text, range),
            Storage::File { file, .. } => file_range(file, self.len, range),
        }
    }
    #[cfg(test)]
    pub(crate) fn tail(&self, max_bytes: usize) -> Result<String> {
        let mut start = self.len.saturating_sub(max_bytes);
        match &self.storage {
            Storage::Memory(text) => {
                while !text.is_char_boundary(start) {
                    start += 1;
                }
                Ok(text[start..].to_owned())
            }
            Storage::File { file, .. } => {
                file_range(file, self.len, tail_start(file, start, self.len)?..self.len)
            }
        }
    }
    pub(crate) fn matches_at(&self, offset: usize, text: &str) -> Result<bool> {
        match &self.storage {
            Storage::Memory(value) => {
                Ok(value.get(offset..offset.saturating_add(text.len())) == Some(text))
            }
            Storage::File { file, .. } => file_matches(file, self.len, offset, text),
        }
    }
    #[cfg(test)]
    pub(crate) fn slice_matches(&self, range: Range<usize>, text: &str) -> Result<bool> {
        if range.end.checked_sub(range.start) != Some(text.len()) {
            return Ok(false);
        }
        self.matches_at(range.start, text)
    }
    pub(crate) fn ends_with(&mut self, text: &str) -> bool {
        let Some(start) = self.len.checked_sub(text.len()) else {
            return false;
        };
        match self.matches_at(start, text) {
            Ok(matches) => matches,
            Err(error) => {
                self.record(error);
                false
            }
        }
    }
    pub(crate) fn copy_range_to(
        &self,
        start: usize,
        end: usize,
        target: &mut impl Write,
    ) -> Result<()> {
        match &self.storage {
            Storage::Memory(text) => copy_memory(text, start..end, target),
            Storage::File { file, .. } => copy_file(file, self.len, start..end, target),
        }
    }
    pub(crate) fn snapshot(&self) -> Result<ResponseSource> {
        ensure!(!self.failed, "Response retention previously failed");
        Ok(match &self.storage {
            Storage::Memory(text) => ResponseSource::Memory(text.clone()),
            Storage::File { artifact, .. } => ResponseSource::File {
                artifact: artifact.clone(),
                len: self.len,
            },
        })
    }
    pub(crate) fn into_source(self) -> Result<ResponseSource> {
        if self.failed {
            return Err(self
                .error
                .unwrap_or_else(|| anyhow!("Response retention previously failed")));
        }
        Ok(match self.storage {
            Storage::Memory(text) => ResponseSource::Memory(text),
            Storage::File { file, artifact } => {
                drop(file);
                ResponseSource::File {
                    artifact,
                    len: self.len,
                }
            }
        })
    }
}
impl fmt::Debug for ResponseBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResponseBuffer")
            .field("len", &self.len)
            .field("failed", &self.failed)
            .finish()
    }
}
impl PartialEq<str> for ResponseBuffer {
    fn eq(&self, other: &str) -> bool {
        self.len == other.len() && self.matches_at(0, other).unwrap_or(false)
    }
}
impl PartialEq<&str> for ResponseBuffer {
    fn eq(&self, other: &&str) -> bool {
        self == *other
    }
}
impl From<&str> for ResponseBuffer {
    fn from(text: &str) -> Self {
        let mut buffer = Self::default();
        buffer.push_str(text);
        buffer
    }
}

impl Write for ResponseBuffer {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.failed {
            return Err(io::Error::other("Response retention previously failed"));
        }
        let text = match std::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => {
                self.record(error.into());
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Response data must be UTF-8",
                ));
            }
        };
        self.push_str(text);
        if self.failed {
            Err(io::Error::other(
                self.error
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_default(),
            ))
        } else {
            Ok(bytes.len())
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        if self.failed {
            return Err(io::Error::other("Response retention previously failed"));
        }
        Ok(()) // File writes are unbuffered; durability is owned by session persistence.
    }
}

#[cfg(test)]
fn memory_range(text: &str, range: Range<usize>) -> Result<String> {
    Ok(text
        .get(range)
        .ok_or_else(|| anyhow!("Invalid response range"))?
        .to_owned())
}
fn file_boundary(file: &File, at: usize) -> Result<bool> {
    if at == 0 {
        return Ok(true);
    }
    let mut byte = [0];
    file.read_exact_at(&mut byte, at as u64)?;
    Ok(byte[0] & 0xc0 != 0x80)
}
fn tail_start(file: &File, mut at: usize, len: usize) -> Result<usize> {
    while at < len && !file_boundary(file, at)? {
        at += 1;
    }
    Ok(at)
}
fn checked_range(file: &File, len: usize, range: &Range<usize>) -> Result<()> {
    ensure!(
        range.start <= range.end && range.end <= len,
        "Invalid response range"
    );
    ensure!(
        range.start == len || file_boundary(file, range.start)?,
        "Response range splits UTF-8"
    );
    ensure!(
        range.end == len || file_boundary(file, range.end)?,
        "Response range splits UTF-8"
    );
    Ok(())
}
fn file_range(file: &File, len: usize, range: Range<usize>) -> Result<String> {
    checked_range(file, len, &range)?;
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(range.len())?;
    bytes.resize(range.len(), 0);
    for (i, chunk) in bytes.chunks_mut(COPY_BYTES).enumerate() {
        file.read_exact_at(chunk, (range.start + i * COPY_BYTES) as u64)?;
    }
    Ok(String::from_utf8(bytes)?)
}
fn file_matches(file: &File, len: usize, at: usize, text: &str) -> Result<bool> {
    let Some(end) = at.checked_add(text.len()).filter(|end| *end <= len) else {
        return Ok(false);
    };
    let mut bytes = [0; COPY_BYTES];
    for (i, chunk) in text.as_bytes().chunks(COPY_BYTES).enumerate() {
        file.read_exact_at(&mut bytes[..chunk.len()], (at + i * COPY_BYTES) as u64)?;
        if &bytes[..chunk.len()] != chunk {
            return Ok(false);
        }
    }
    // Equal UTF-8 bytes imply valid boundaries, including the empty-string case.
    Ok(at == len || end > at || file_boundary(file, at)?)
}
fn copy_memory(text: &str, range: Range<usize>, target: &mut impl Write) -> Result<()> {
    let text = text
        .get(range)
        .ok_or_else(|| anyhow!("Invalid response range"))?;
    let mut at = 0;
    while at < text.len() {
        let mut end = (at + COPY_BYTES).min(text.len());
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        target.write_all(&text.as_bytes()[at..end])?;
        at = end;
    }
    Ok(())
}
fn visit_file(
    file: &File,
    len: usize,
    range: Range<usize>,
    visitor: &mut impl FnMut(&str) -> Result<()>,
) -> Result<()> {
    checked_range(file, len, &range)?;
    let mut bytes = [0; COPY_BYTES];
    let mut at = range.start;
    while at < range.end {
        let mut count = (range.end - at).min(COPY_BYTES);
        file.read_exact_at(&mut bytes[..count], at as u64)?;
        if at + count < range.end {
            while std::str::from_utf8(&bytes[..count]).is_err() {
                count -= 1;
                ensure!(count + 3 >= COPY_BYTES, "Invalid UTF-8 response artifact");
            }
        }
        visitor(std::str::from_utf8(&bytes[..count])?)?;
        at += count;
    }
    Ok(())
}
fn copy_file(file: &File, len: usize, range: Range<usize>, target: &mut impl Write) -> Result<()> {
    visit_file(file, len, range, &mut |text| {
        target.write_all(text.as_bytes())?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn giant_unicode_response_spills_and_publishes_without_resident_body() {
        let chunk = "let 🌸 = 日本語;\n".repeat(1024);
        let mut buffer = ResponseBuffer::default();
        let mut bytes = 0;
        while bytes <= 20 * 1024 * 1024 {
            buffer.push_str(&chunk);
            bytes += chunk.len();
        }
        assert_eq!(buffer.len(), bytes);
        assert!(buffer.error().is_none());
        assert!(buffer.has_non_whitespace());
        let Storage::File { artifact, .. } = &buffer.storage else {
            panic!("Large responses must spill");
        };
        let path = artifact.path.clone();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(buffer.matches_at(0, &chunk).unwrap());
        assert!(buffer.ends_with(&chunk));
        for budget in [0, 1, 2, 3, 4, 101, COPY_BYTES] {
            let tail = buffer.tail(budget).unwrap();
            assert!(tail.len() <= budget);
            assert!(chunk.repeat(8).ends_with(&tail));
        }
        let expected_tail = buffer.tail(99).unwrap();
        let source = buffer.into_source().unwrap();
        assert!(path.exists());
        assert_eq!(source.len(), bytes);
        assert!(!source.is_empty());
        assert_eq!(source.tail(99).unwrap(), expected_tail);
        assert!(source.matches_at(bytes - chunk.len(), &chunk).unwrap());
        assert_eq!(source.read_range(0..chunk.len()).unwrap(), chunk);
        let cloned = source.clone();
        assert_eq!(source, cloned);
        drop(source);
        assert!(path.exists());
        drop(cloned);
        assert!(
            !path.exists(),
            "Last owner must remove private response artifacts"
        );
    }

    #[test]
    fn snapshots_keep_immutable_prefixes_after_appends_and_unicode_corrections() {
        let prefix = "🌸x".repeat(MEMORY_BYTES / 5 + 10);
        let mut buffer = ResponseBuffer::from(prefix.clone());
        let snapshot = buffer.snapshot().unwrap();
        buffer.push_str("tail");
        assert_eq!(snapshot.len(), prefix.len());
        assert_eq!(snapshot.tail(5).unwrap(), "🌸x");
        buffer.truncate(prefix.len() - 5);
        buffer.push_str("corrected 🦀");
        assert!(buffer.error().is_none());
        assert_eq!(snapshot.tail(5).unwrap(), "🌸x");
        assert!(buffer.ends_with("corrected 🦀"));
        assert!(!snapshot.matches_at(snapshot.len(), "tail").unwrap());
        let restored = ResponseBuffer::from_source(snapshot.clone());
        assert!(restored.error().is_none());
        assert_eq!(restored.len(), prefix.len());
        assert!(restored.matches_at(0, &prefix).unwrap());
        assert!(matches!(restored.storage, Storage::File { .. }));
    }

    #[test]
    fn bounded_utf8_copy_handles_every_buffer_boundary() {
        let text = "a🌸日本\n".repeat(30000);
        let buffer = ResponseBuffer::from(text.clone());
        let mut target = ResponseBuffer::default();
        buffer.copy_range_to(0, buffer.len(), &mut target).unwrap();
        assert!(target.error().is_none());
        assert_eq!(target, text.as_str());
        assert!(target.slice_matches(0..1, "a").unwrap());
        assert!(!target.slice_matches(0..2, "a").unwrap());
        assert!(target.read_range(2..3).is_err());
        let mut subset = Vec::new();
        buffer.copy_range_to(1, 5, &mut subset).unwrap();
        assert_eq!(subset, "🌸".as_bytes());
        let source = target.into_source().unwrap();
        assert_eq!(source.tail(3).unwrap(), "\n");
    }

    #[test]
    fn read_and_truncate_failures_stay_observable_and_never_publish_success() {
        let mut buffer = ResponseBuffer::from("🌸x".repeat(MEMORY_BYTES));
        let original = buffer.len();
        buffer.truncate(1);
        assert_eq!(buffer.len(), original);
        assert!(buffer.take_error().is_some());
        buffer.push_str("must not silently continue");
        assert_eq!(buffer.len(), original);
        assert!(buffer.into_source().is_err());

        let mut buffer = ResponseBuffer::from("x".repeat(MEMORY_BYTES + 1));
        if let Storage::File { file, artifact } = &mut buffer.storage {
            *file = File::open(&artifact.path).unwrap(); // Deterministic read-only write failure.
        }
        let original = buffer.len();
        buffer.push_str("new output");
        assert!(buffer.error().is_some());
        assert_eq!(buffer.len(), original);
        assert!(buffer.into_source().is_err());
    }

    #[test]
    fn whitespace_tracking_and_small_corrections_are_constant_space() {
        let mut overallocated = String::with_capacity(2 * MEMORY_BYTES);
        overallocated.push_str("small");
        let compact = ResponseBuffer::from(overallocated);
        assert!(
            matches!(&compact.storage, Storage::Memory(text) if text.capacity() <= MEMORY_BYTES)
        );
        let mut buffer = ResponseBuffer::from(" \t\u{2003}");
        assert!(!buffer.has_non_whitespace());
        buffer.push_str("🦀");
        assert!(buffer.has_non_whitespace());
        buffer.truncate(5);
        assert!(buffer.error().is_none());
        assert!(!buffer.has_non_whitespace());
        buffer.push_str("a");
        assert!(buffer.has_non_whitespace());
        assert_eq!(buffer, " \t\u{2003}a");
        buffer.truncate(0);
        assert!(buffer.is_empty());
        assert!(!buffer.has_non_whitespace());
        assert!(matches!(
            buffer.into_source().unwrap(),
            ResponseSource::Memory(_)
        ));
    }

    #[test]
    fn dropped_buffers_remove_spill_and_write_rejects_invalid_utf8() {
        let mut buffer = ResponseBuffer::from("x".repeat(MEMORY_BYTES + 1));
        let path = match &buffer.storage {
            Storage::File { artifact, .. } => artifact.path.clone(),
            _ => unreachable!(),
        };
        assert!(buffer.write_all(&[0xff]).is_err());
        assert!(buffer.error().is_some());
        assert_eq!(buffer.len(), MEMORY_BYTES + 1);
        drop(buffer);
        assert!(!path.exists());
    }
}
