//! Private local prompt snapshots. Mutations are revision checked and atomic;
//! callers clear drafts only after this worker acknowledges durable storage.
use crate::context_refs::Reference;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::{Mutex, OnceLock},
};
const MAX_BYTES: usize = 1024 * 1024;
const MAX_PAYLOAD: usize = 256 * 1024;
pub const MAX_ENTRIES: usize = 32;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Stash,
    Template,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub title: String,
    pub text: std::sync::Arc<str>,
    pub context: Vec<Reference>,
    pub project: String,
    pub provider: String,
    pub kind: Kind,
}
impl Entry {
    pub fn valid(&self) -> bool {
        uuid::Uuid::parse_str(&self.id).is_ok()
            && !self.title.trim().is_empty()
            && self.title.len() <= 256
            && !self.title.chars().any(char::is_control)
            && self.text.len() <= crate::input::MAX_DRAFT_BYTES
            && (!self.text.trim().is_empty() || !self.context.is_empty())
            && self.context.len() <= crate::context_refs::MAX_REFERENCES
            && self.context.iter().all(Reference::valid)
            && self.project.len() <= 4096
            && self.provider.len() <= 64
            && (self.kind != Kind::Template || self.context.is_empty())
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Library {
    pub revision: u64,
    pub entries: Vec<Entry>,
}
impl Library {
    fn valid(&self) -> bool {
        self.entries.len() <= MAX_ENTRIES
            && self.entries.iter().all(Entry::valid)
            && self
                .entries
                .iter()
                .enumerate()
                .all(|(i, entry)| !self.entries[..i].iter().any(|old| old.id == entry.id))
            && self
                .entries
                .iter()
                .map(|entry| {
                    entry.text.len() + entry.context.iter().map(|r| r.excerpt.len()).sum::<usize>()
                })
                .sum::<usize>()
                <= MAX_PAYLOAD
    }
}
pub enum Operation {
    Save(Entry),
    Rename(String, String),
    Remove(String),
}
pub fn load(path: &Path) -> Result<Library> {
    let mut file = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Library::default()),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        file.metadata()?.is_file(),
        "The prompt library is not a regular file"
    );
    ensure!(
        file.metadata()?.len() <= MAX_BYTES as u64,
        "The prompt library exceeds its storage budget"
    );
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "The prompt library exceeds its storage budget"
    );
    let library: Library = serde_json::from_slice(&bytes)
        .context("The saved prompt library could not be decoded; existing data was preserved")?;
    ensure!(
        library.valid(),
        "The saved prompt library is invalid; existing data was preserved"
    );
    Ok(library)
}
pub fn apply(path: &Path, expected_revision: u64, operation: Operation) -> Result<Library> {
    static WRITES: OnceLock<Mutex<()>> = OnceLock::new();
    let _guard = WRITES
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let mut library = load(path)?;
    ensure!(
        library.revision == expected_revision,
        "The library changed while saving. Reload it; your draft is preserved"
    );
    match operation {
        Operation::Save(entry) => {
            ensure!(
                entry.valid(),
                "Use a nonempty prompt name and a bounded text/context draft"
            );
            library.entries.insert(0, entry);
        }
        Operation::Remove(id) => library.entries.retain(|entry| entry.id != id),
        Operation::Rename(id, title) => {
            let entry = library
                .entries
                .iter_mut()
                .find(|entry| entry.id == id)
                .context("This saved prompt no longer exists")?;
            entry.title = title;
        }
    }
    ensure!(
        library.valid(),
        "The prompt library reached its budget. Remove an unused saved prompt; your draft is preserved"
    );
    library.revision = library
        .revision
        .checked_add(1)
        .context("The library revision is exhausted")?;
    let bytes = serde_json::to_vec(&library)?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "The encoded prompt library reached its budget; your draft is preserved"
    );
    let parent = path.parent().context("Prompt storage is unavailable")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".prompt-library-{}.tmp", uuid::Uuid::new_v4()));
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(temporary.clone());
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    File::open(parent)?.sync_all()?;
    Ok(library)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn entry(text: &str) -> Entry {
        Entry {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Saved prompt".into(),
            text: text.into(),
            context: vec![],
            project: "/project".into(),
            provider: "codex".into(),
            kind: Kind::Stash,
        }
    }
    #[test]
    fn private_atomic_library_roundtrip_rename_delete_and_stale_revision_preserve_existing_work() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("code-prompts-{}", uuid::Uuid::new_v4()));
        let path = root.join("prompts.json");
        let first = entry("Exact multiline\n日本語 🦀");
        let id = first.id.clone();
        let saved = apply(&path, 0, Operation::Save(first)).unwrap();
        assert_eq!(saved.revision, 1);
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(apply(&path, 0, Operation::Save(entry("stale"))).is_err());
        assert_eq!(load(&path).unwrap().entries.len(), 1);
        let renamed = apply(
            &path,
            1,
            Operation::Rename(id.clone(), "Reusable task".into()),
        )
        .unwrap();
        assert_eq!(renamed.entries[0].title, "Reusable task");
        assert_eq!(
            renamed.entries[0].text.as_ref(),
            "Exact multiline\n日本語 🦀"
        );
        let deleted = apply(&path, 2, Operation::Remove(id)).unwrap();
        assert!(deleted.entries.is_empty());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn escaped_payload_and_invalid_or_symlink_storage_fail_without_replacing_saved_data() {
        let root = std::env::temp_dir().join(format!("code-prompts-{}", uuid::Uuid::new_v4()));
        let path = root.join("prompts.json");
        apply(&path, 0, Operation::Save(entry("retained"))).unwrap();
        let original = fs::read(&path).unwrap();
        let mut large = entry(&"\u{1}".repeat(crate::input::MAX_DRAFT_BYTES));
        large.context = (0..4)
            .map(|i| Reference {
                id: format!("context-{i}"),
                title: "Source".into(),
                project: "/source".into(),
                provider: "Codex".into(),
                excerpt: "\u{1}".repeat(crate::context_refs::MAX_EXCERPT).into(),
                older_omitted: true,
            })
            .collect();
        apply(&path, 1, Operation::Save(large.clone())).unwrap();
        let retained = fs::read(&path).unwrap();
        assert!(apply(&path, 2, Operation::Save(large)).is_err());
        assert_eq!(fs::read(&path).unwrap(), retained);
        let link = root.join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        assert!(load(&link).is_err());
        assert_ne!(retained, original);
        fs::remove_dir_all(root).unwrap();
    }
}
