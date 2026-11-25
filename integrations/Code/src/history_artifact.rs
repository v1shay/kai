//! Private immutable paging artifacts and cheap source fingerprints.
use anyhow::Result;
use std::{
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub(crate) struct FileStamp {
    len: u64,
    dev: u64,
    ino: u64,
    modified: i64,
    nanos: i64,
}
impl FileStamp {
    pub(crate) fn read(path: &Path) -> Result<Self> {
        let m = std::fs::metadata(path)?;
        Ok(Self {
            len: m.len(),
            dev: m.dev(),
            ino: m.ino(),
            modified: m.mtime(),
            nanos: m.mtime_nsec(),
        })
    }
    pub(crate) fn matches(&self, path: &Path) -> bool {
        Self::read(path).is_ok_and(|s| s == *self)
    }
}
/// Immutable private generated index/envelope. Checkpoints retain its pathname
/// without keeping one file descriptor per visited page.
pub(crate) struct HistoryArtifact {
    pub(crate) path: PathBuf,
}
impl HistoryArtifact {
    pub(crate) fn own(path: PathBuf) -> Arc<Self> {
        Arc::new(Self { path })
    }
    pub(crate) fn linked(path: &Path) -> Result<Arc<Self>> {
        let target =
            std::env::temp_dir().join(format!("code-history-resume-{}", uuid::Uuid::new_v4()));
        Self::link_to(path, &target)?;
        Ok(Self::own(target))
    }
    pub(crate) fn link_to(source: &Path, target: &Path) -> Result<()> {
        let metadata = std::fs::symlink_metadata(source)?;
        anyhow::ensure!(metadata.is_file(), "Invalid history artifact");
        if std::fs::hard_link(source, target).is_err() {
            // Cross-volume fallback copies small generated indices only. Never
            // replay or copy an entire giant source message to restore a view.
            anyhow::ensure!(
                metadata.len() <= 2 * 1024 * 1024,
                "History artifact cannot be linked on this filesystem"
            );
            std::fs::copy(source, target)?;
        }
        std::fs::set_permissions(target, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
}
impl Drop for HistoryArtifact {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}
