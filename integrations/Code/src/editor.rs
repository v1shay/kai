//! Bounded local source editing. IO stays on workers; optimistic saves compare
//! complete bytes and preserve a displaced conflicting inode for recovery.
use anyhow::{Context, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    ffi::CString,
    fs::File,
    io::{Read, Write},
    os::unix::{
        ffi::OsStrExt,
        fs::MetadataExt,
        io::{AsRawFd, FromRawFd},
    },
    path::{Component, Path, PathBuf},
    sync::Arc,
};
pub const LIMIT: usize = crate::input::MAX_DRAFT_BYTES;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Document {
    pub path: PathBuf,
    pub text: Arc<str>,
    root: PathBuf,
    root_identity: (u64, u64),
    parent_identity: (u64, u64),
    identity: (u64, u64),
    mode: u32,
    digest: [u8; 32],
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub struct Draft {
    pub document: Document,
    pub text: String,
}
impl Draft {
    pub fn valid(&self) -> bool {
        self.text.len() <= LIMIT
            && self.document.text.len() <= LIMIT
            && !self.document.text.contains('\0')
            && <[u8; 32]>::from(Sha256::digest(self.document.text.as_bytes()))
                == self.document.digest
            && self.document.root.is_absolute()
            && self
                .document
                .root
                .components()
                .all(|part| matches!(part, Component::RootDir | Component::Normal(_)))
            && self.document.path.is_absolute()
            && self
                .document
                .path
                .strip_prefix(&self.document.root)
                .is_ok_and(|relative| {
                    !relative.as_os_str().is_empty()
                        && relative
                            .components()
                            .all(|c| matches!(c, Component::Normal(_)))
                })
    }
}
impl Document {
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn matches(&self, text: &str) -> bool {
        <[u8; 32]>::from(Sha256::digest(text.as_bytes())) == self.digest
    }
}
pub fn verify(document: &Document) -> Result<()> {
    let (root, parent, name) = parent(&document.root, &document.path)?;
    ensure!(
        identity(&root)? == document.root_identity
            && identity(&parent)? == document.parent_identity,
        "Project path was replaced; your draft is preserved"
    );
    let mut file = at(&parent, &name, libc::O_RDONLY, 0)?;
    ensure!(
        identity(&file)? == document.identity
            && file.metadata()?.mode() & 0o777 == document.mode
            && <[u8; 32]>::from(Sha256::digest(bytes(&mut file)?)) == document.digest,
        "File changed outside this editor; your restored draft is preserved. Copy it or discard it before loading the current file."
    );
    Ok(())
}
fn identity(file: &File) -> Result<(u64, u64)> {
    let m = file.metadata()?;
    Ok((m.dev(), m.ino()))
}
fn name(value: &std::ffi::OsStr) -> Result<CString> {
    Ok(CString::new(value.as_bytes())?)
}
fn at(parent: &File, path: &CString, flags: i32, mode: u32) -> Result<File> {
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            path.as_ptr(),
            flags | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            mode,
        )
    };
    ensure!(
        fd >= 0,
        "Cannot open file safely: {}",
        std::io::Error::last_os_error()
    );
    Ok(unsafe { File::from_raw_fd(fd) })
}
fn parent(root: &Path, path: &Path) -> Result<(File, File, CString)> {
    let relative = path
        .strip_prefix(root)
        .context("File is outside this project")?;
    let parts = relative
        .components()
        .map(|c| match c {
            Component::Normal(n) => Ok(n),
            _ => anyhow::bail!("Unsafe editor path"),
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(!parts.is_empty(), "Choose a file");
    let root_name = name(root.as_os_str())?;
    let fd = unsafe {
        libc::open(
            root_name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    ensure!(fd >= 0, "Project folder is unavailable");
    let root_fd = unsafe { File::from_raw_fd(fd) };
    let mut parent = root_fd.try_clone()?;
    for part in &parts[..parts.len() - 1] {
        parent = at(&parent, &name(part)?, libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
    }
    Ok((root_fd, parent, name(parts[parts.len() - 1])?))
}
fn bytes(file: &mut File) -> Result<Vec<u8>> {
    let metadata = file.metadata()?;
    ensure!(
        metadata.is_file() && metadata.nlink() == 1,
        "Editing requires a regular file with one link"
    );
    ensure!(
        metadata.len() <= LIMIT as u64,
        "Large file remains in paged preview; editing is limited to 64 KiB"
    );
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((LIMIT + 1) as u64).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= LIMIT, "File grew while loading");
    Ok(bytes)
}
pub fn load(root: &Path, path: &Path) -> Result<Document> {
    let canonical_root = root.canonicalize()?;
    let path = path
        .strip_prefix(root)
        .map(|relative| canonical_root.join(relative))
        .unwrap_or_else(|_| path.to_owned());
    let root = canonical_root;
    let (root_fd, parent, name) = parent(&root, &path)?;
    let mut file = at(&parent, &name, libc::O_RDONLY, 0)?;
    let initial = file.metadata()?;
    let data = bytes(&mut file)?;
    let final_meta = file.metadata()?;
    ensure!(
        initial.len() == final_meta.len()
            && initial.mtime() == final_meta.mtime()
            && initial.mtime_nsec() == final_meta.mtime_nsec()
            && initial.ctime() == final_meta.ctime()
            && initial.ctime_nsec() == final_meta.ctime_nsec(),
        "File changed while opening; retry"
    );
    ensure!(!data.contains(&0), "Binary file remains in preview");
    let digest = Sha256::digest(&data).into();
    let text = String::from_utf8(data).context("Editing requires UTF-8 text")?;
    Ok(Document {
        path,
        text: text.into(),
        root,
        root_identity: identity(&root_fd)?,
        parent_identity: identity(&parent)?,
        identity: identity(&file)?,
        mode: final_meta.mode() & 0o777,
        digest,
    })
}
pub fn save(document: &Document, text: &str) -> Result<Document> {
    ensure!(
        text.len() <= LIMIT && !text.contains('\0'),
        "Edited text exceeds the limit or contains binary data"
    );
    let (root, parent, name) = parent(&document.root, &document.path)?;
    ensure!(
        identity(&root)? == document.root_identity
            && identity(&parent)? == document.parent_identity,
        "Project path was replaced; your draft is preserved"
    );
    let mut current = at(&parent, &name, libc::O_RDONLY, 0)?;
    ensure!(
        identity(&current)? == document.identity
            && current.metadata()?.mode() & 0o777 == document.mode
            && <[u8; 32]>::from(Sha256::digest(bytes(&mut current)?)) == document.digest,
        "File changed outside this editor; reload or copy your draft before saving"
    );
    let temporary = PathBuf::from(format!(".code-restore-editor-{}.tmp", uuid::Uuid::new_v4()));
    let temporary_name = name_fn(&temporary)?;
    let mut output = at(
        &parent,
        &temporary_name,
        libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL,
        0o600,
    )?;
    let mut published = false;
    let result = (|| -> Result<()> {
        output.write_all(text.as_bytes())?;
        ensure!(
            unsafe { libc::fchmod(output.as_raw_fd(), document.mode as libc::mode_t) } == 0,
            "Could not preserve file permissions"
        );
        output.sync_all()?;
        // A second full comparison closes the potentially slow staging interval.
        let mut current = at(&parent, &name, libc::O_RDONLY, 0)?;
        ensure!(
            identity(&current)? == document.identity
                && current.metadata()?.mode() & 0o777 == document.mode
                && <[u8; 32]>::from(Sha256::digest(bytes(&mut current)?)) == document.digest,
            "File changed while preparing save; your draft is preserved"
        );
        #[cfg(target_os = "macos")]
        let swapped = unsafe {
            libc::renameatx_np(
                parent.as_raw_fd(),
                temporary_name.as_ptr(),
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_SWAP,
            )
        };
        #[cfg(target_os = "linux")]
        let swapped = unsafe {
            libc::syscall(
                libc::SYS_renameat2,
                parent.as_raw_fd(),
                temporary_name.as_ptr(),
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::RENAME_EXCHANGE,
            ) as i32
        };
        ensure!(
            swapped == 0,
            "Could not atomically save file: {}",
            std::io::Error::last_os_error()
        );
        published = true;
        let mut displaced = at(&parent, &temporary_name, libc::O_RDONLY, 0)?;
        if identity(&displaced)? != document.identity
            || displaced.metadata()?.mode() & 0o777 != document.mode
            || <[u8; 32]>::from(Sha256::digest(bytes(&mut displaced)?)) != document.digest
        {
            // Never delete the concurrent version. Both complete files survive.
            parent.sync_all()?;
            anyhow::bail!(
                "Concurrent edit preserved at {}. The editor version is at the original path; compare both before continuing.",
                document.path.parent().unwrap().join(&temporary).display()
            );
        }
        Ok(())
    })();
    if result.is_ok() || !published {
        unsafe { libc::unlinkat(parent.as_raw_fd(), temporary_name.as_ptr(), 0) };
    }
    result?;
    parent.sync_all()?;
    load(&document.root, &document.path)
}
fn name_fn(path: &Path) -> Result<CString> {
    name(path.as_os_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn root() -> PathBuf {
        let root = std::env::temp_dir().join(format!("code-editor-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        root.canonicalize().unwrap()
    }
    #[test]
    fn saves_utf8_permissions_and_rejects_external_edits_without_overwrite() {
        let root = root();
        let path = root.join("main.rs");
        std::fs::write(&path, "let café = 1;\r\n").unwrap();
        let doc = load(&root, &path).unwrap();
        let next = save(&doc, "let café = 2;\r\n").unwrap();
        assert_eq!(&*next.text, "let café = 2;\r\n");
        std::fs::write(&path, "external").unwrap();
        assert!(save(&next, "overwrite").is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external");
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn restart_draft_preserves_original_baseline_and_rejects_later_external_changes() {
        let root = root();
        let path = root.join("file.rs");
        std::fs::write(&path, "original\n").unwrap();
        let document = load(&root, &path).unwrap();
        let draft = Draft {
            document,
            text: "unsaved 雪\n".into(),
        };
        let json = serde_json::to_string(&draft).unwrap();
        let restored: Draft = serde_json::from_str(&json).unwrap();
        assert!(restored.valid());
        assert!(verify(&restored.document).is_ok());
        std::fs::write(&path, "external\n").unwrap();
        assert!(verify(&restored.document).is_err());
        assert!(save(&restored.document, &restored.text).is_err());
        assert_eq!(restored.text, "unsaved 雪\n");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "external\n");
        let mut invalid = restored;
        invalid.document.path = root.join("../escape");
        assert!(!invalid.valid());
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn refuses_symlinks_parent_substitution_binary_and_large_files() {
        let root = root();
        let dir = root.join("src");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join("a");
        std::fs::write(&path, "before").unwrap();
        let doc = load(&root, &path).unwrap();
        std::fs::rename(&dir, root.join("old")).unwrap();
        std::os::unix::fs::symlink(root.join("old"), &dir).unwrap();
        assert!(save(&doc, "after").is_err());
        assert_eq!(std::fs::read(root.join("old/a")).unwrap(), b"before");
        let large = root.join("large");
        std::fs::write(&large, vec![b'a'; LIMIT + 1]).unwrap();
        assert!(load(&root, &large).is_err());
        std::fs::write(&large, [0u8, 1]).unwrap();
        assert!(load(&root, &large).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
