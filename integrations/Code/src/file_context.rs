//! Explicit, bounded context snapshots. No background indexing or model calls.
use crate::context_refs::{MAX_EXCERPT, Reference};
use anyhow::{Context, Result, bail, ensure};
use std::{
    fs::{self, File, OpenOptions},
    io::Read,
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{ffi::OsStrExt, fs::OpenOptionsExt},
    },
    path::{Component, Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
const SCAN_BYTES: usize = 8 * 1024 * 1024;
const WALK_ENTRIES: usize = 256;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lines {
    pub first: u64,
    pub last: u64,
}
impl Lines {
    pub fn parse(text: &str) -> Result<Option<Self>> {
        let text = text.trim();
        if text.is_empty() {
            return Ok(None);
        }
        let (first, last) = text.split_once('-').unwrap_or((text, text));
        let first = first
            .trim()
            .parse::<u64>()
            .context("Use a line or range, such as 12-40")?;
        let last = last
            .trim()
            .parse::<u64>()
            .context("Use a line or range, such as 12-40")?;
        ensure!(
            first > 0 && last >= first && last <= 1_000_000,
            "Choose a valid range between lines 1 and 1,000,000"
        );
        Ok(Some(Self { first, last }))
    }
}
#[derive(Clone, Debug)]
pub struct Preview {
    pub reference: Reference,
    pub lines: Arc<[String]>,
    pub files: usize,
    pub omitted: bool,
    pub icon: &'static str,
}
fn check(cancel: &AtomicBool) -> Result<()> {
    ensure!(!cancel.load(Ordering::Acquire), "Context preview cancelled");
    Ok(())
}
fn confined(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
    .canonicalize()
    .context("The selected path is unavailable")?;
    ensure!(
        path.starts_with(root),
        "Choose a file or folder inside the current checkout"
    );
    Ok(path)
}
/// Open each canonical relative component against an owned directory FD. An
/// agent replacing a path with a symlink cannot redirect the read outside root.
fn open_file(root: &Path, path: &Path) -> Result<File> {
    let relative = path.strip_prefix(root)?;
    let parts: Vec<_> = relative
        .components()
        .map(|part| match part {
            Component::Normal(part) => Ok(part),
            _ => Err(anyhow::anyhow!("Invalid context path")),
        })
        .collect::<Result<_>>()?;
    ensure!(!parts.is_empty(), "Select a regular file");
    let mut directory = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(root)?;
    for (index, part) in parts.iter().enumerate() {
        let name = std::ffi::CString::new(part.as_bytes())?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if index + 1 < parts.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
        ensure!(
            fd >= 0,
            "The context file changed or could not be opened safely"
        );
        let file = unsafe { File::from_raw_fd(fd) };
        if index + 1 == parts.len() {
            ensure!(
                file.metadata()?.is_file(),
                "Only regular text files can be attached"
            );
            return Ok(file);
        }
        directory = file;
    }
    unreachable!()
}
fn text(
    root: &Path,
    path: &Path,
    range: Option<Lines>,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<(String, bool)> {
    let mut file = open_file(root, path)?;
    let mut output = Vec::with_capacity(limit.min(8192));
    let mut buffer = [0u8; 8192];
    let mut line = 1u64;
    let mut scanned = 0;
    let mut omitted = false;
    let mut found = range.is_none();
    'read: loop {
        check(cancel)?;
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        scanned += count;
        ensure!(
            scanned <= SCAN_BYTES,
            "The line range is beyond the bounded scan. Choose an earlier range or smaller file"
        );
        for byte in &buffer[..count] {
            if let Some(range) = range
                && line > range.last
            {
                break 'read;
            }
            if range.is_none_or(|range| line >= range.first) {
                found = true;
                ensure!(
                    *byte != 0,
                    "Binary files cannot be attached as text; use a supported media attachment"
                );
                if output.len() == limit {
                    omitted = true;
                    break 'read;
                }
                output.push(*byte);
            }
            if *byte == b'\n' {
                line += 1;
            }
        }
    }
    ensure!(
        found,
        "The selected line range is beyond the end of this file"
    );
    let value = match String::from_utf8(output) {
        Ok(value) => value,
        Err(error) if omitted && error.utf8_error().error_len().is_none() => {
            let end = error.utf8_error().valid_up_to();
            let mut bytes = error.into_bytes();
            bytes.truncate(end);
            String::from_utf8(bytes)?
        }
        Err(_) => bail!("This file is not UTF-8 text"),
    };
    Ok((value, omitted))
}
pub fn prepare(
    root: &Path,
    path: &Path,
    range: Option<Lines>,
    cancel: &AtomicBool,
) -> Result<Preview> {
    check(cancel)?;
    let root = root.canonicalize().context("The checkout is unavailable")?;
    let path = confined(&root, path)?;
    let directory = fs::metadata(&path)?.is_dir();
    ensure!(
        !directory || range.is_none(),
        "Line ranges apply to individual files"
    );
    let relative = path.strip_prefix(&root)?.to_string_lossy();
    let label = if relative.is_empty() {
        root.file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned()
    } else {
        relative.into_owned()
    };
    let title = match range {
        Some(range) => format!("{label}:{}-{}", range.first, range.last),
        None => label,
    };
    let mut excerpt = String::new();
    let mut files = 0;
    let mut omitted = directory; // Folder snapshots skip hidden/build paths and deep subtrees.
    if directory {
        let mut seen = 0;
        for entry in walkdir::WalkDir::new(&path)
            .min_depth(1)
            .max_depth(5)
            .follow_links(false)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|entry| {
                entry.depth() == 0
                    || (!entry.file_name().to_string_lossy().starts_with('.')
                        && !["node_modules", "target", "vendor", "build", "dist"]
                            .iter()
                            .any(|name| entry.file_name() == std::ffi::OsStr::new(name)))
            })
        {
            check(cancel)?;
            seen += 1;
            if seen > WALK_ENTRIES {
                omitted = true;
                break;
            }
            let Ok(entry) = entry else {
                omitted = true;
                continue;
            };
            if !entry.file_type().is_file() {
                continue;
            }
            let filename = entry.path().strip_prefix(&root)?.to_string_lossy();
            let header = format!("\n--- File: {filename} ---\n");
            let remaining = MAX_EXCERPT.saturating_sub(excerpt.len() + header.len());
            if remaining < 64 {
                omitted = true;
                break;
            }
            match text(&root, entry.path(), None, remaining.min(4096), cancel) {
                Ok((body, cut)) => {
                    excerpt.push_str(&header);
                    excerpt.push_str(&body);
                    files += 1;
                    omitted |= cut;
                }
                Err(_) => omitted = true,
            }
        }
        ensure!(
            files > 0,
            "No readable UTF-8 files were found in this folder"
        );
    } else {
        let header = format!("File: {title}\n\n");
        ensure!(
            header.len() < 4096,
            "The file path is too long for a context snapshot"
        );
        let (body, cut) = text(&root, &path, range, MAX_EXCERPT - header.len(), cancel)?;
        excerpt = header + &body;
        omitted = cut;
        files = 1;
    }
    check(cancel)?;
    let reference = Reference {
        id: format!("file:{}", uuid::Uuid::new_v4()),
        title,
        project: root.to_string_lossy().into_owned(),
        provider: if directory { "Folder" } else { "File" }.into(),
        excerpt: excerpt.into(),
        older_omitted: omitted,
    };
    ensure!(reference.valid(), "The context metadata exceeds its budget");
    let lines = reference
        .excerpt
        .lines()
        .map(str::to_owned)
        .collect::<Vec<_>>()
        .into();
    Ok(Preview {
        reference,
        lines,
        files,
        omitted,
        icon: if directory {
            "folder"
        } else {
            crate::files::icon_for_path(&path)
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("code-context-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn line_ranges_are_exact_utf8_bounded_and_never_expand_workspace_access() {
        let fixture = Fixture::new();
        let root = fixture.0.join("project");
        fs::create_dir(&root).unwrap();
        fs::write(root.join("code.rs"), "first\n日本語 🦀\nthird\nfourth\n").unwrap();
        let preview = prepare(
            &root,
            Path::new("code.rs"),
            Lines::parse("2-3").unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(
            preview.reference.excerpt.as_ref(),
            "File: code.rs:2-3\n\n日本語 🦀\nthird\n"
        );
        assert!(!preview.omitted);
        assert_eq!(preview.icon, "file-code");
        assert!(preview.reference.valid());
        fs::write(fixture.0.join("outside.txt"), "outside").unwrap();
        std::os::unix::fs::symlink(fixture.0.join("outside.txt"), root.join("escape.txt")).unwrap();
        assert!(
            prepare(
                &root,
                Path::new("escape.txt"),
                None,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            prepare(
                &root,
                Path::new("../outside.txt"),
                None,
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(
            prepare(
                &root,
                Path::new("code.rs"),
                Some(Lines {
                    first: 90,
                    last: 100
                }),
                &AtomicBool::new(false)
            )
            .is_err()
        );
        assert!(prepare(&root, Path::new("code.rs"), None, &AtomicBool::new(true)).is_err());
        assert!(Lines::parse("5-2").is_err());
        assert!(Lines::parse("0").is_err());
    }
    #[test]
    fn folder_snapshots_skip_generated_hidden_and_binary_files_with_explicit_omissions() {
        let fixture = Fixture::new();
        fs::create_dir(fixture.0.join("target")).unwrap();
        fs::write(fixture.0.join("target/hidden.rs"), "generated-secret").unwrap();
        fs::write(fixture.0.join(".env"), "private-secret").unwrap();
        fs::write(fixture.0.join("binary.png"), b"\0\xff").unwrap();
        fs::write(fixture.0.join("a.rs"), "actual-source").unwrap();
        fs::write(fixture.0.join("b.md"), "文".repeat(MAX_EXCERPT)).unwrap();
        let preview = prepare(&fixture.0, Path::new("."), None, &AtomicBool::new(false)).unwrap();
        assert!(preview.omitted && preview.reference.older_omitted);
        assert_eq!(preview.files, 2);
        assert!(preview.reference.excerpt.len() <= MAX_EXCERPT);
        assert!(preview.reference.excerpt.contains("actual-source"));
        assert!(
            !preview.reference.excerpt.contains("private-secret")
                && !preview.reference.excerpt.contains("generated-secret")
        );
        assert!(std::str::from_utf8(preview.reference.excerpt.as_bytes()).is_ok());
        let one = prepare(&fixture.0, Path::new("b.md"), None, &AtomicBool::new(false)).unwrap();
        assert!(one.omitted && one.reference.excerpt.len() <= MAX_EXCERPT);
    }
}
