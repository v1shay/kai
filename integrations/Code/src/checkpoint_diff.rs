//! Streaming, bounded line alignment between two immutable prompt versions.
//! This never consults Git HEAD or the current working tree.
use anyhow::{Result, ensure};
use std::{
    collections::VecDeque,
    fs::OpenOptions,
    io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};
const LOOKAHEAD: usize = 24;
const LINE_BYTES: usize = 16 * 1024;
const PAGE_BYTES: usize = 256 * 1024;
const PAGE_ROWS: usize = 160;
#[derive(Clone, Debug)]
pub struct Version {
    pub file: Option<PathBuf>,
    pub inline: Vec<u8>,
    pub size: u64,
    pub mode: u32,
    pub kind: &'static str,
}
impl Version {
    pub fn absent() -> Self {
        Self {
            file: None,
            inline: vec![],
            size: 0,
            mode: 0,
            kind: "absent",
        }
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Position {
    pub before: u64,
    pub after: u64,
    pub old_line: u64,
    pub new_line: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Context,
    Added,
    Removed,
    Header,
    Note,
}
#[derive(Clone, Debug)]
pub struct Line {
    pub kind: Kind,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct Page {
    pub lines: Vec<Line>,
    pub next: Option<Position>,
}
trait ReadSeek: Read + Seek {}
impl<T: Read + Seek> ReadSeek for T {}
struct Input {
    reader: BufReader<Box<dyn ReadSeek>>,
    pending: VecDeque<Vec<u8>>,
    offset: u64,
    line: u64,
    eof: bool,
}
impl Input {
    fn open(version: &Version, offset: u64, line: u64) -> Result<Self> {
        let mut reader: Box<dyn ReadSeek> = if let Some(path) = &version.file {
            let file = OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(path)?;
            ensure!(
                file.metadata()?.is_file() && file.metadata()?.len() == version.size,
                "Checkpoint content changed"
            );
            Box::new(file)
        } else {
            Box::new(Cursor::new(version.inline.clone()))
        };
        reader.seek(SeekFrom::Start(offset))?;
        Ok(Self {
            reader: BufReader::with_capacity(32 * 1024, reader),
            pending: VecDeque::new(),
            offset,
            line: line.max(1),
            eof: false,
        })
    }
    fn fill(&mut self, cancel: &AtomicBool) -> Result<()> {
        while !self.eof && self.pending.len() < LOOKAHEAD {
            ensure!(!cancel.load(Ordering::Acquire), "Diff canceled");
            let mut bytes = Vec::new();
            let n = self
                .reader
                .by_ref()
                .take((LINE_BYTES + 1) as u64)
                .read_until(b'\n', &mut bytes)?;
            if n == 0 {
                self.eof = true;
                break;
            }
            ensure!(
                n <= LINE_BYTES && !bytes.contains(&0) && std::str::from_utf8(&bytes).is_ok(),
                "binary-or-long-line"
            );
            self.pending.push_back(bytes);
        }
        Ok(())
    }
    fn pop(&mut self) -> Option<Vec<u8>> {
        let bytes = self.pending.pop_front()?;
        self.offset += bytes.len() as u64;
        self.line += 1;
        Some(bytes)
    }
}
fn push(lines: &mut Vec<Line>, kind: Kind, bytes: Vec<u8>) {
    let newline = bytes.last() == Some(&b'\n');
    let mut text = String::from_utf8(bytes).expect("validated line");
    if newline {
        text.pop();
        if text.ends_with('\r') {
            text.pop();
        }
    }
    lines.push(Line { kind, text });
    if !newline {
        lines.push(Line {
            kind: Kind::Note,
            text: "\\ No newline at end of file".into(),
        });
    }
}
fn position(a: &Input, b: &Input) -> Position {
    Position {
        before: a.offset,
        after: b.offset,
        old_line: a.line,
        new_line: b.line,
    }
}
/// Generates exact unified hunks with bounded 24-line alignment lookahead.
/// Alignment is deliberately bounded rather than promising a minimal edit script.
pub fn page(
    before: &Version,
    after: &Version,
    cursor: Position,
    cancel: &AtomicBool,
) -> Result<Page> {
    let result = text_page(before, after, cursor, cancel);
    match result {
        Err(error) if error.to_string() == "binary-or-long-line" => Ok(Page {
            lines: vec![Line {
                kind: Kind::Note,
                text: format!(
                    "Binary or long-line content · before {} bytes ({}) → after {} bytes ({}). File bytes remain fully included in Undo/Redo.",
                    before.size, before.kind, after.size, after.kind
                ),
            }],
            next: None,
        }),
        result => result,
    }
}
fn text_page(
    before: &Version,
    after: &Version,
    cursor: Position,
    cancel: &AtomicBool,
) -> Result<Page> {
    let mut a = Input::open(before, cursor.before, cursor.old_line)?;
    let mut b = Input::open(after, cursor.after, cursor.new_line)?;
    let mut output = Vec::new();
    if cursor.before == 0 && cursor.after == 0 {
        output.push(Line {
            kind: Kind::Header,
            text: "--- Before this prompt".into(),
        });
        output.push(Line {
            kind: Kind::Header,
            text: "+++ After this prompt".into(),
        });
        if before.mode != after.mode || before.kind != after.kind {
            output.push(Line {
                kind: Kind::Note,
                text: format!(
                    "{} {:04o} → {} {:04o}",
                    before.kind, before.mode, after.kind, after.mode
                ),
            });
        }
    }
    loop {
        let mut context = VecDeque::new();
        a.fill(cancel)?;
        b.fill(cancel)?;
        while a.pending.front().is_some() && a.pending.front() == b.pending.front() {
            context.push_back((a.line, b.line, a.pop().unwrap()));
            b.pop();
            if context.len() > 3 {
                context.pop_front();
            }
            a.fill(cancel)?;
            b.fill(cancel)?;
        }
        if a.pending.is_empty() && b.pending.is_empty() {
            return Ok(Page {
                lines: output,
                next: None,
            });
        }
        let (old_start, new_start) = context
            .front()
            .map_or((a.line, b.line), |(old, new, _)| (*old, *new));
        let mut old_count = context.len() as u64;
        let mut new_count = old_count;
        let header = output.len();
        output.push(Line {
            kind: Kind::Header,
            text: String::new(),
        });
        for (_, _, bytes) in context {
            push(&mut output, Kind::Context, bytes);
        }
        let mut equal = 0;
        loop {
            a.fill(cancel)?;
            b.fill(cancel)?;
            if a.pending.is_empty() && b.pending.is_empty() {
                break;
            }
            if a.pending.front().is_some() && a.pending.front() == b.pending.front() {
                push(&mut output, Kind::Context, a.pop().unwrap());
                b.pop();
                old_count += 1;
                new_count += 1;
                equal += 1;
                if equal == 3 {
                    break;
                }
            } else {
                equal = 0;
                let mut anchor = None;
                for (i, left) in a.pending.iter().enumerate() {
                    for (j, right) in b.pending.iter().enumerate() {
                        if left == right && anchor.is_none_or(|(x, y)| i + j < x + y) {
                            anchor = Some((i, j));
                        }
                    }
                }
                let (removed, added) = anchor.unwrap_or((
                    usize::from(!a.pending.is_empty()),
                    usize::from(!b.pending.is_empty()),
                ));
                for _ in 0..removed {
                    push(&mut output, Kind::Removed, a.pop().unwrap());
                    old_count += 1;
                }
                for _ in 0..added {
                    push(&mut output, Kind::Added, b.pop().unwrap());
                    new_count += 1;
                }
            }
            if output.len() >= PAGE_ROWS
                || output.iter().map(|line| line.text.len()).sum::<usize>() >= PAGE_BYTES
            {
                break;
            }
        }
        output[header].text = format!(
            "@@ -{},{} +{},{} @@",
            if old_count == 0 {
                old_start - 1
            } else {
                old_start
            },
            old_count,
            if new_count == 0 {
                new_start - 1
            } else {
                new_start
            },
            new_count
        );
        if output.len() >= PAGE_ROWS
            || output.iter().map(|line| line.text.len()).sum::<usize>() >= PAGE_BYTES
        {
            return Ok(Page {
                lines: output,
                next: Some(position(&a, &b)),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn version(text: &str) -> Version {
        Version {
            file: None,
            inline: text.as_bytes().to_vec(),
            size: text.len() as u64,
            mode: 0o644,
            kind: "file",
        }
    }
    fn all_pages(before: &Version, after: &Version) -> Vec<Line> {
        let mut position = Position::default();
        let mut lines = vec![];
        for _ in 0..1000 {
            let page = page(before, after, position, &AtomicBool::new(false)).unwrap();
            assert!(page.lines.len() <= PAGE_ROWS + 2 * LOOKAHEAD + 12);
            lines.extend(page.lines);
            if let Some(next) = page.next {
                assert!(next.before > position.before || next.after > position.after);
                position = next;
            } else {
                return lines;
            }
        }
        panic!("diff paging did not terminate");
    }
    fn apply_hunks(before: &str, patch: &[Line]) -> String {
        let original = before.lines().collect::<Vec<_>>();
        let mut cursor = 0;
        let mut output = Vec::new();
        let mut in_hunk = false;
        for line in patch {
            match line.kind {
                Kind::Header if line.text.starts_with("@@") => {
                    let part = line
                        .text
                        .split_whitespace()
                        .nth(1)
                        .unwrap()
                        .trim_start_matches('-');
                    let (start, count) = part.split_once(',').unwrap();
                    let start = start.parse::<usize>().unwrap();
                    let count = count.parse::<usize>().unwrap();
                    let next = if count == 0 { start } else { start - 1 };
                    assert!(next >= cursor);
                    output.extend_from_slice(&original[cursor..next]);
                    cursor = next;
                    in_hunk = true;
                }
                Kind::Context if in_hunk => {
                    assert_eq!(original[cursor], line.text);
                    output.push(line.text.as_str());
                    cursor += 1;
                }
                Kind::Removed if in_hunk => {
                    assert_eq!(original[cursor], line.text);
                    cursor += 1;
                }
                Kind::Added if in_hunk => output.push(line.text.as_str()),
                _ => {}
            }
        }
        output.extend_from_slice(&original[cursor..]);
        format!("{}\n", output.join("\n"))
    }
    #[test]
    fn streamed_hunks_apply_exactly_across_pages_insertions_deletions_and_unicode() {
        let before = (0..1800)
            .map(|n| format!("line {n} 雪\n"))
            .collect::<String>();
        let mut lines = before.lines().map(str::to_owned).collect::<Vec<_>>();
        lines.splice(6..6, ["inserted 雨".into(), "another insertion".into()]);
        lines.drain(200..450);
        for (ix, line) in lines.iter_mut().enumerate().skip(550).take(650) {
            *line = format!("replacement {ix}");
        }
        let after = format!("{}\n", lines.join("\n"));
        let patch = all_pages(&version(&before), &version(&after));
        assert_eq!(apply_hunks(&before, &patch), after);
        assert!(
            patch
                .iter()
                .filter(|line| line.kind == Kind::Header && line.text.starts_with("@@"))
                .count()
                > 3
        );
        assert!(patch.len() < before.lines().count() + after.lines().count());
    }
    #[test]
    fn additions_deletions_binary_long_lines_and_no_newline_are_explicit() {
        let added = all_pages(&Version::absent(), &version("a\nb\n"));
        assert_eq!(apply_hunks("", &added), "a\nb\n");
        let deleted = all_pages(&version("a\nb\n"), &Version::absent());
        assert!(
            deleted
                .iter()
                .filter(|line| line.kind == Kind::Removed)
                .count()
                == 2
        );
        let no_newline = all_pages(&version("old"), &version("new"));
        assert!(
            no_newline
                .iter()
                .any(|line| line.text == "\\ No newline at end of file")
        );
        for text in ["a\0b".to_owned(), "x".repeat(LINE_BYTES + 200)] {
            let page = page(
                &Version::absent(),
                &version(&text),
                Position::default(),
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(page.next.is_none());
            assert!(page.lines[0].text.contains("Binary or long-line"));
        }
        assert!(
            page(
                &version("old"),
                &version("new"),
                Position::default(),
                &AtomicBool::new(true)
            )
            .is_err()
        );
    }
}
