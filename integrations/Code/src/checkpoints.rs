//! Private, streamed, per-prompt workspace snapshots. No Git reset and no shell.
//! Undo is a filesystem transaction with a durable per-file recovery journal;
//! external services, ignored caches and running processes are outside its scope.
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};
use std::{
    ffi::{CString, OsStr, OsString},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
const MAX_FILES: u64 = 200_000;
const MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
const IO_BYTES: usize = 64 * 1024;
const DISK_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const MAX_CHECKPOINTS: u64 = 512;
const MAX_CACHE_ENTRIES: u64 = 200_000;
pub const PAGE: usize = 64;
#[derive(Clone)]
pub struct Repository {
    path: PathBuf,
}
#[derive(Clone, Debug)]
pub struct Checkpoint {
    pub id: String,
    pub session: String,
    pub prompt_key: String,
    pub title: String,
    pub root: PathBuf,
    pub phase: String,
    pub undone: bool,
    pub created: i64,
}
pub struct CleanupGuard {
    repository: Repository,
    id: String,
    settled: Option<async_channel::Receiver<()>>,
}
impl CleanupGuard {
    pub fn disarm(&mut self) {
        self.settled = None;
    }
}
impl Drop for CleanupGuard {
    fn drop(&mut self) {
        if let Some(settled) = self.settled.take() {
            let repository = self.repository.clone();
            let id = self.id.clone();
            let _ = std::thread::Builder::new()
                .name("code-orphan-checkpoint".into())
                .spawn(move || {
                    let _ = settled.recv_blocking();
                    let _ = repository.seal(&id, &AtomicBool::new(false));
                });
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Undo,
    Redo,
}
#[derive(Clone, Debug)]
pub struct Change {
    pub path: PathBuf,
    pub kind: &'static str,
    pub directory:bool,
}
#[derive(Clone, Debug)]
pub struct Preview {
    pub checkpoint: Checkpoint,
    pub changes: Vec<Change>,
    pub total: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
struct Entry {
    kind: i64,
    mode: u32,
    size: u64,
    digest: Vec<u8>,
    target: Vec<u8>,
    blob: String,
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    ensure!(
        !cancel.load(Ordering::Acquire),
        "Checkpoint operation canceled"
    );
    Ok(())
}
fn bytes(path: &Path) -> &[u8] {
    path.as_os_str().as_bytes()
}
fn relative(path: &Path) -> Result<()> {
    ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|c| matches!(c, Component::Normal(_))),
        "Unsafe checkpoint path"
    );
    Ok(())
}
fn cstr(value: &OsStr) -> Result<CString> {
    Ok(CString::new(value.as_bytes())?)
}
fn syscall(result: i32) -> std::io::Result<i32> {
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result)
    }
}
struct OperationLock(File);
impl Drop for OperationLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
struct Temporary(Option<PathBuf>);
impl Drop for Temporary {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = fs::remove_file(path);
        }
    }
}
struct Root {
    fd: File,
    path: PathBuf,
}
impl Root {
    fn open(path: &Path) -> Result<Self> {
        let fd = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)?;
        Ok(Self {
            fd,
            path: path.into(),
        })
    }
    fn parent(&self, path: &Path, create: bool) -> Result<(File, CString)> {
        relative(path)?;
        let mut fd = self.fd.try_clone()?;
        let components: Vec<_> = path.components().collect();
        for part in &components[..components.len() - 1] {
            let name = cstr(part.as_os_str())?;
            let opened = unsafe {
                libc::openat(
                    fd.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            };
            let opened = match syscall(opened) {
                Ok(fd) => fd,
                Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                    syscall(unsafe { libc::mkdirat(fd.as_raw_fd(), name.as_ptr(), 0o755) })?;
                    syscall(unsafe {
                        libc::openat(
                            fd.as_raw_fd(),
                            name.as_ptr(),
                            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                        )
                    })?
                }
                Err(error) => {
                    return Err(error).context("A workspace parent changed or is a symlink");
                }
            };
            fd = unsafe { File::from_raw_fd(opened) };
        }
        Ok((fd, cstr(components.last().unwrap().as_os_str())?))
    }
    fn signature(&self, path: &Path) -> Result<Option<Vec<u8>>> {
        let (parent, name) = match self.parent(path, false) {
            Ok(value) => value,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let fd = unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            )
        };
        let fd = match syscall(fd) {
            Ok(fd) => fd,
            Err(error)
                if matches!(error.raw_os_error(), Some(libc::ELOOP))
                    || error.kind() == std::io::ErrorKind::NotFound =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let file = unsafe { File::from_raw_fd(fd) };
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Ok(None);
        }
        let mut signature = Vec::with_capacity(72);
        for value in [
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mode() as u64,
            metadata.mtime() as u64,
            metadata.mtime_nsec() as u64,
            metadata.ctime() as u64,
            metadata.ctime_nsec() as u64,
        ] {
            signature.extend_from_slice(&value.to_le_bytes());
        }
        Ok(Some(signature))
    }
    fn entry(
        &self,
        path: &Path,
        blobs: Option<&Path>,
        cancel: &AtomicBool,
    ) -> Result<Option<Entry>> {
        cancelled(cancel)?;
        let (parent, name) = match self.parent(path, false) {
            Ok(value) => value,
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        let result = unsafe {
            libc::fstatat(
                parent.as_raw_fd(),
                name.as_ptr(),
                stat.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error.into());
        }
        let stat = unsafe { stat.assume_init() };
        let mode = (stat.st_mode as u32) & 0o7777;
        if stat.st_mode & libc::S_IFMT == libc::S_IFLNK {
            let mut target = vec![0u8; 4097];
            let size = unsafe {
                libc::readlinkat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    target.as_mut_ptr().cast(),
                    target.len(),
                )
            };
            ensure!(
                size >= 0 && (size as usize) < target.len(),
                "Symlink target cannot be captured safely"
            );
            target.truncate(size as usize);
            return Ok(Some(Entry {
                kind: 1,
                mode,
                size: size as u64,
                digest: Sha256::digest(&target).to_vec(),
                target,
                blob: String::new(),
            }));
        }
        if stat.st_mode & libc::S_IFMT == libc::S_IFDIR {
            return Ok(Some(Entry {
                kind: 2,
                mode,
                size: 0,
                digest: vec![],
                target: vec![],
                blob: String::new(),
            }));
        }
        ensure!(
            stat.st_mode & libc::S_IFMT == libc::S_IFREG,
            "Special files cannot be snapshotted safely: {}",
            path.display()
        );
        let fd = syscall(unsafe {
            libc::openat(
                parent.as_raw_fd(),
                name.as_ptr(),
                libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        })?;
        let mut file = unsafe { File::from_raw_fd(fd) };
        let before = file.metadata()?;
        #[allow(clippy::unnecessary_cast)]
        let identity_matches =
            before.dev() == stat.st_dev as u64 && before.ino() == stat.st_ino as u64;
        ensure!(identity_matches, "File changed while opening checkpoint");
        let temporary = blobs.map(|root| root.join(format!(".capture-{}", uuid::Uuid::new_v4())));
        let mut cleanup = Temporary(temporary.clone());
        let mut output = temporary
            .as_ref()
            .map(|path| {
                OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .mode(0o600)
                    .open(path)
            })
            .transpose()?;
        let mut hash = Sha256::new();
        let mut buffer = [0u8; IO_BYTES];
        let mut size = 0;
        loop {
            cancelled(cancel)?;
            let n = file.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            size += n as u64;
            ensure!(size <= MAX_BYTES, "Checkpoint file exceeds 20 GiB budget");
            hash.update(&buffer[..n]);
            if let Some(output) = output.as_mut() {
                output.write_all(&buffer[..n])?;
            }
        }
        let after = file.metadata()?;
        ensure!(
            before.len() == after.len()
                && before.mtime() == after.mtime()
                && before.mtime_nsec() == after.mtime_nsec()
                && before.ctime() == after.ctime()
                && before.ctime_nsec() == after.ctime_nsec()
                && size == before.len(),
            "File changed during checkpoint capture: {}",
            path.display()
        );
        let digest = hash.finalize().to_vec();
        let blob = digest
            .iter()
            .map(|n| format!("{n:02x}"))
            .collect::<String>();
        if let Some(mut output) = output {
            output.flush()?;
            output.sync_all()?;
            drop(output);
            let target = blobs.unwrap().join(&blob);
            if !target.exists() {
                fs::rename(temporary.as_ref().unwrap(), target)?;
            } else {
                fs::remove_file(temporary.as_ref().unwrap())?;
            }
        }
        cleanup.0 = None;
        Ok(Some(Entry {
            kind: 0,
            mode,
            size,
            digest,
            target: vec![],
            blob,
        }))
    }
}
fn table_entry(db: &Connection, table: &str, path: &[u8]) -> Result<Option<Entry>> {
    Ok(db
        .query_row(
            &format!("SELECT kind,mode,size,digest,target,blob FROM {table} WHERE path=?1"),
            [path],
            |r| {
                Ok(Entry {
                    kind: r.get(0)?,
                    mode: r.get(1)?,
                    size: r.get(2)?,
                    digest: r.get(3)?,
                    target: r.get(4)?,
                    blob: r.get(5)?,
                })
            },
        )
        .optional()?)
}
fn same(a: &Option<Entry>, b: &Option<Entry>) -> bool {
    a == b
}
fn private_path(path: &Path) -> bool {
    path.components().any(|c| {
        let value = c.as_os_str().to_string_lossy();
        matches!(value.as_ref(), ".git" | ".hg" | ".svn") || value.starts_with(".code-undo-")
    })
}
fn hidden(path: &Path) -> bool {
    path.components().any(|c| {
        let v = c.as_os_str().to_string_lossy();
        matches!(
            v.as_ref(),
            ".git"
                | ".hg"
                | ".svn"
                | "node_modules"
                | "target"
                | "dist"
                | "build"
                | ".build"
                | ".next"
                | ".nuxt"
                | ".venv"
                | "venv"
                | "__pycache__"
                | ".cache"
                | ".DS_Store"
        ) || v.starts_with(".code-undo-")
    })
}
impl Repository {
    /// Lazy UI handle; creation/validation happens in a worker operation.
    pub fn cleanup_on_orphan(
        &self,
        id: String,
        settled: async_channel::Receiver<()>,
    ) -> CleanupGuard {
        CleanupGuard {
            repository: self.clone(),
            id,
            settled: Some(settled),
        }
    }
    pub fn at(path: PathBuf) -> Self {
        Self { path }
    }
    #[cfg(test)]
    pub fn new(path: PathBuf) -> Result<Self> {
        fs::create_dir_all(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
        let this = Self { path };
        this.index()?;
        Ok(this)
    }
    fn index(&self) -> Result<Connection> {
        fs::create_dir_all(&self.path)?;
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700))?;
        let db = Connection::open(self.path.join("index.sqlite"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;PRAGMA temp_store=FILE;PRAGMA cache_size=-2048;PRAGMA max_page_count=131072;")?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS checkpoints(id TEXT PRIMARY KEY,session TEXT NOT NULL,prompt_key TEXT NOT NULL,title TEXT NOT NULL,root BLOB NOT NULL,dev INTEGER NOT NULL,ino INTEGER NOT NULL,phase TEXT NOT NULL,undone INTEGER NOT NULL DEFAULT 0,created INTEGER NOT NULL,owner INTEGER NOT NULL);CREATE INDEX IF NOT EXISTS checkpoint_sessions ON checkpoints(session,created DESC);CREATE TABLE IF NOT EXISTS blobs(hash TEXT PRIMARY KEY,size INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS blob_usage(id INTEGER PRIMARY KEY CHECK(id=1),total INTEGER NOT NULL);INSERT OR IGNORE INTO blob_usage(id,total) VALUES(1,0);CREATE TABLE IF NOT EXISTS blob_refs(checkpoint TEXT,hash TEXT,PRIMARY KEY(checkpoint,hash));CREATE INDEX IF NOT EXISTS blob_refs_hash ON blob_refs(hash);CREATE TABLE IF NOT EXISTS restore_order(checkpoint TEXT PRIMARY KEY,sequence INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS recovery_usage(checkpoint TEXT PRIMARY KEY,total INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS metadata_usage(checkpoint TEXT PRIMARY KEY,total INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS file_cache(root BLOB,path BLOB,signature BLOB,kind INTEGER,mode INTEGER,size INTEGER,digest BLOB,target BLOB,blob TEXT,PRIMARY KEY(root,path));")?;
        static CHECKED: OnceLock<Mutex<std::collections::HashSet<PathBuf>>> = OnceLock::new();
        let mut checked = CHECKED
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| anyhow::anyhow!("Checkpoint initialization lock unavailable"))?;
        if !checked.contains(&self.path) {
            self.recover_orphans(&db)?;
            self.reconcile_blobs(&db)?;
            checked.insert(self.path.clone());
        }
        Ok(db)
    }
    fn recover_orphans(&self, db: &Connection) -> Result<()> {
        let mut query = db.prepare("SELECT id,owner FROM checkpoints WHERE phase IN ('capturing','recording','sealing','seal-failed')")?;
        let pending = query
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(query);
        for (id, owner) in pending {
            let alive = owner > 0
                && (unsafe { libc::kill(owner, 0) } == 0
                    || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM));
            if !alive {
                db.execute("UPDATE checkpoints SET phase='interrupted' WHERE id=?1 AND owner=?2 AND phase IN ('capturing','recording','sealing','seal-failed')",params![id,owner])?;
            }
        }
        Ok(())
    }
    fn reconcile_blobs(&self, db: &Connection) -> Result<()> {
        let directory = self.path.join("blobs");
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| -> Result<()> {
            for entry in entries {
                let entry = entry?;
                let name = entry.file_name();
                let text = name.to_string_lossy();
                if text.starts_with(".capture-") {
                    let _ = fs::remove_file(entry.path());
                    continue;
                }
                if text.len() != 64 || !text.bytes().all(|b| b.is_ascii_hexdigit()) {
                    continue;
                }
                let registered: bool = db.query_row(
                    "SELECT EXISTS(SELECT 1 FROM blobs WHERE hash=?1)",
                    [text.as_ref()],
                    |r| r.get(0),
                )?;
                if !registered {
                    fs::remove_file(entry.path())?;
                }
            }
            Ok(())
        })();
        if result.is_ok() {
            db.execute_batch("COMMIT")?
        } else {
            let _ = db.execute_batch("ROLLBACK");
        }
        result
    }
    fn lock(&self, id: &str) -> Result<OperationLock> {
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.dir(id)?.join("operation.lock"))?;
        syscall(unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) })
            .context("This checkpoint already has an operation in progress")?;
        Ok(OperationLock(file))
    }
    fn reserve_restore(
        &self,
        id: &str,
        root: &Path,
        phase: &str,
        recovery_bytes: u64,
    ) -> Result<()> {
        let mut db = self.index()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            let mut query=tx.prepare("SELECT root,owner FROM checkpoints WHERE id!=?1 AND phase IN ('capturing','recording','sealing','seal-failed','applying-undo','applying-redo')")?;
            let mut rows = query.query([id])?;
            while let Some(row) = rows.next()? {
                let other = PathBuf::from(OsString::from_vec(row.get(0)?));
                let owner: i32 = row.get(1)?;
                if root.starts_with(&other) || other.starts_with(root) {
                    let alive = unsafe { libc::kill(owner, 0) } == 0
                        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
                    ensure!(
                        !alive,
                        "Another prompt, terminal, or restore owns this workspace. Stop it first."
                    );
                }
            }
        }
        let blobs: u64 =
            tx.query_row("SELECT total FROM blob_usage WHERE id=1", [], |r| r.get(0))?;
        ensure!(
            blobs
                .saturating_add(Self::auxiliary_usage(&tx)?)
                .saturating_add(recovery_bytes)
                .saturating_add(512 * 1024 * 1024)
                <= DISK_BYTES,
            "Recovery storage would exceed 4 GiB. Forget old checkpoints before restoring; no file was changed."
        );
        tx.execute("INSERT INTO recovery_usage(checkpoint,total) VALUES(?1,?2) ON CONFLICT(checkpoint) DO UPDATE SET total=total+excluded.total",params![id,recovery_bytes])?;
        tx.execute(
            "UPDATE checkpoints SET phase=?2,owner=?3 WHERE id=?1",
            params![id, phase, std::process::id()],
        )?;
        tx.commit()?;
        Ok(())
    }
    fn dir(&self, id: &str) -> Result<PathBuf> {
        uuid::Uuid::parse_str(id)?;
        Ok(self.path.join(id))
    }
    fn manifest(&self, id: &str) -> Result<Connection> {
        let db = Connection::open(self.dir(id)?.join("manifest.sqlite"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.execute_batch("PRAGMA journal_mode=WAL;PRAGMA synchronous=FULL;PRAGMA temp_store=FILE;PRAGMA cache_size=-2048;PRAGMA max_page_count=32768;CREATE TABLE IF NOT EXISTS before(path BLOB PRIMARY KEY,kind INTEGER,mode INTEGER,size INTEGER,digest BLOB,target BLOB,blob TEXT);CREATE TABLE IF NOT EXISTS after(path BLOB PRIMARY KEY,kind INTEGER,mode INTEGER,size INTEGER,digest BLOB,target BLOB,blob TEXT);CREATE TABLE IF NOT EXISTS applied(path BLOB PRIMARY KEY);")?;
        Ok(db)
    }
    pub fn get(&self, id: &str) -> Result<Checkpoint> {
        self.index()?.query_row("SELECT id,session,prompt_key,title,root,phase,undone,created FROM checkpoints WHERE id=?1",[id],row).context("This checkpoint was forgotten or is unavailable")
    }
    pub fn list(&self, session: &str, offset: usize) -> Result<Vec<Checkpoint>> {
        let db = self.index()?;
        let mut query=db.prepare("SELECT id,session,prompt_key,title,root,phase,undone,created FROM checkpoints WHERE session=?1 ORDER BY created DESC,rowid DESC LIMIT 64 OFFSET ?2")?;
        Ok(query
            .query_map(params![session, offset], row)?
            .collect::<std::result::Result<_, _>>()?)
    }
    pub fn capture_before(
        &self,
        root: &Path,
        session: &str,
        prompt_key: &str,
        title: &str,
        cancel: &AtomicBool,
    ) -> Result<String> {
        cancelled(cancel)?;
        let root = fs::canonicalize(root)?;
        fs::create_dir_all(&self.path)?;
        let storage = fs::canonicalize(&self.path)?;
        ensure!(
            !root.starts_with(&storage),
            "Checkpoint storage cannot itself be used as the workspace"
        );
        ensure!(
            root.parent().is_some(),
            "The filesystem root cannot be a checkpoint workspace"
        );
        let opened = Root::open(&root)?;
        let metadata = opened.fd.metadata()?;
        let id = uuid::Uuid::new_v4().to_string();
        let mut db = self.index()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let retained: u64 = tx.query_row("SELECT count(*) FROM checkpoints", [], |r| r.get(0))?;
        ensure!(
            retained < MAX_CHECKPOINTS,
            "Checkpoint retention limit reached (512 prompts). Existing snapshots were kept; free checkpoint storage before starting another mutating prompt."
        );
        {
            let mut query=tx.prepare("SELECT root,owner FROM checkpoints WHERE phase IN ('capturing','recording','sealing','seal-failed','applying-undo','applying-redo')")?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                let other = PathBuf::from(OsString::from_vec(row.get(0)?));
                let owner: i32 = row.get(1)?;
                if root.starts_with(&other) || other.starts_with(&root) {
                    let alive = unsafe { libc::kill(owner, 0) } == 0
                        || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM);
                    ensure!(
                        !alive,
                        "Another prompt or restore owns this workspace. Wait for it to finish."
                    );
                }
            }
        }
        tx.execute("INSERT INTO checkpoints(id,session,prompt_key,title,root,dev,ino,phase,created,owner) VALUES(?1,?2,?3,?4,?5,?6,?7,'capturing',?8,?9)",params![id,session,prompt_key,title.chars().take(256).collect::<String>(),bytes(&root),metadata.dev(),metadata.ino(),chrono::Utc::now().timestamp_millis(),std::process::id()])?;
        tx.commit()?;
        let result = (|| {
            let directory = self.dir(&id)?;
            fs::create_dir(&directory)?;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            fs::create_dir_all(self.path.join("blobs"))?;
            fs::set_permissions(self.path.join("blobs"), fs::Permissions::from_mode(0o700))?;
            self.snapshot(&id, &opened, "before", cancel)?;
            self.record_metadata_size(&id)?;
            self.index()?.execute(
                "UPDATE checkpoints SET phase='recording' WHERE id=?1",
                [&id],
            )?;
            Ok(id.clone())
        })();
        if result.is_err() {
            let db = self.index()?;
            let _ = db.execute(
                "UPDATE checkpoints SET phase='capture-failed' WHERE id=?1",
                [&id],
            );
            let _ = self.reconcile_blobs(&db);
        }
        result
    }
    pub fn record_read_only(
        &self,
        root: &Path,
        session: &str,
        prompt_key: &str,
        title: &str,
    ) -> Result<String> {
        let root = fs::canonicalize(root)?;
        let metadata = fs::metadata(&root)?;
        let id = uuid::Uuid::new_v4().to_string();
        let mut db = self.index()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let retained: u64 = tx.query_row("SELECT count(*) FROM checkpoints", [], |r| r.get(0))?;
        ensure!(
            retained < MAX_CHECKPOINTS,
            "Prompt history retention limit reached. Forget an old checkpoint from Prompt history before starting more tasks."
        );
        tx.execute("INSERT INTO checkpoints(id,session,prompt_key,title,root,dev,ino,phase,created,owner) VALUES(?1,?2,?3,?4,?5,?6,?7,'read-only',?8,?9)",params![id,session,prompt_key,title.chars().take(256).collect::<String>(),bytes(&root),metadata.dev(),metadata.ino(),chrono::Utc::now().timestamp_millis(),std::process::id()])?;
        tx.commit()?;
        Ok(id)
    }
    fn auxiliary_usage(db: &Connection) -> Result<u64> {
        Ok(db.query_row("SELECT COALESCE((SELECT sum(total) FROM recovery_usage),0)+COALESCE((SELECT sum(total) FROM metadata_usage),0)",[],|r|r.get(0))?)
    }
    fn record_metadata_size(&self, id: &str) -> Result<()> {
        let directory = self.dir(id)?;
        let mut total = 0u64;
        for name in [
            "manifest.sqlite",
            "manifest.sqlite-wal",
            "manifest.sqlite-shm",
        ] {
            total = total.saturating_add(
                fs::metadata(directory.join(name))
                    .map(|m| m.len())
                    .unwrap_or(0),
            );
        }
        let db = self.index()?;
        db.execute(
            "INSERT OR REPLACE INTO metadata_usage(checkpoint,total) VALUES(?1,?2)",
            params![id, total],
        )?;
        let blobs: u64 =
            db.query_row("SELECT total FROM blob_usage WHERE id=1", [], |r| r.get(0))?;
        ensure!(
            blobs.saturating_add(Self::auxiliary_usage(&db)?) <= DISK_BYTES,
            "Complete checkpoint storage exceeded 4 GiB. Existing data was kept; forget old snapshots before continuing."
        );
        Ok(())
    }
    /// Explicitly forget app-owned backup data. Workspace files and CLI history
    /// are untouched; the caller presents the title and loss of Undo first.
    pub fn forget(&self, id: &str, cancel: &AtomicBool) -> Result<()> {
        cancelled(cancel)?;
        let checkpoint = self.get(id)?;
        ensure!(
            matches!(
                checkpoint.phase.as_str(),
                "ready"
                    | "not-started"
                    | "read-only"
                    | "capture-failed"
                    | "seal-failed"
                    | "interrupted"
                    | "forgetting"
            ),
            "A running, incomplete, or restoring checkpoint cannot be forgotten"
        );
        let directory = self.dir(id)?;
        let _lock = if directory.is_dir() {
            Some(self.lock(id)?)
        } else {
            None
        };
        let mut db = self.index()?;
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "UPDATE checkpoints SET phase='forgetting' WHERE id=?1",
            [id],
        )?;
        tx.execute("DELETE FROM blob_refs WHERE checkpoint=?1", [id])?;
        tx.commit()?;
        // A canceled/crashed GC leaves a durable forgetting state, never a
        // supposedly restorable checkpoint pointing at deleted content.
        let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        {
            let mut query=tx.prepare("SELECT hash,size FROM blobs WHERE NOT EXISTS(SELECT 1 FROM blob_refs WHERE blob_refs.hash=blobs.hash)")?;
            let mut rows = query.query([])?;
            let mut freed = 0u64;
            while let Some(row) = rows.next()? {
                cancelled(cancel)?;
                let hash: String = row.get(0)?;
                let size: u64 = row.get(1)?;
                ensure!(
                    hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                    "Invalid checkpoint content key"
                );
                match fs::remove_file(self.path.join("blobs").join(&hash)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
                freed = freed.saturating_add(size);
            }
            drop(rows);
            drop(query);
            tx.execute("DELETE FROM blobs WHERE NOT EXISTS(SELECT 1 FROM blob_refs WHERE blob_refs.hash=blobs.hash)",[])?;
            tx.execute(
                "UPDATE blob_usage SET total=max(total-?1,0) WHERE id=1",
                [freed],
            )?;
        }
        // Finish private-directory removal before acknowledging a freed slot.
        if directory.exists() {
            ensure!(
                !fs::symlink_metadata(&directory)?.file_type().is_symlink(),
                "Checkpoint directory was replaced"
            );
            fs::remove_dir_all(&directory)?;
        }
        tx.execute("DELETE FROM restore_order WHERE checkpoint=?1", [id])?;
        tx.execute("DELETE FROM recovery_usage WHERE checkpoint=?1", [id])?;
        tx.execute("DELETE FROM metadata_usage WHERE checkpoint=?1", [id])?;
        tx.execute("DELETE FROM checkpoints WHERE id=?1", [id])?;
        tx.execute("DELETE FROM file_cache WHERE root=?1 AND NOT EXISTS(SELECT 1 FROM checkpoints WHERE checkpoints.root=file_cache.root)", [bytes(&checkpoint.root)])?;
        tx.execute("DELETE FROM file_cache WHERE NOT EXISTS(SELECT 1 FROM blobs WHERE blobs.hash=file_cache.blob)", [])?;
        tx.commit()?;
        Ok(())
    }
    pub fn abandon(&self, id: &str) -> Result<()> {
        self.index()?.execute(
            "UPDATE checkpoints SET phase='not-started' WHERE id=?1 AND phase='recording'",
            [id],
        )?;
        Ok(())
    }
    pub fn seal(&self, id: &str, cancel: &AtomicBool) -> Result<()> {
        let checkpoint = self.get(id)?;
        if checkpoint.phase == "read-only" || checkpoint.phase == "not-started" {
            return Ok(());
        }
        let _lock = self.lock(id)?;
        ensure!(
            checkpoint.phase == "recording" || checkpoint.phase == "seal-failed",
            "Checkpoint is not awaiting a final snapshot"
        );
        self.index()?
            .execute("UPDATE checkpoints SET phase='sealing' WHERE id=?1", [id])?;
        let result = (|| {
            let root = self.root(id)?;
            self.snapshot(id, &root, "after", cancel)?;
            self.record_metadata_size(id)?;
            self.index()?
                .execute("UPDATE checkpoints SET phase='ready' WHERE id=?1", [id])?;
            Ok(())
        })();
        if result.is_err() {
            let _ = self.index()?.execute(
                "UPDATE checkpoints SET phase='seal-failed' WHERE id=?1",
                [id],
            );
        }
        result
    }
    fn root(&self, id: &str) -> Result<Root> {
        let db = self.index()?;
        let (path, dev, ino): (Vec<u8>, u64, u64) = db.query_row(
            "SELECT root,dev,ino FROM checkpoints WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        let root = Root::open(&PathBuf::from(OsString::from_vec(path)))?;
        let metadata = root.fd.metadata()?;
        ensure!(
            metadata.dev() == dev && metadata.ino() == ino,
            "The workspace directory was replaced"
        );
        Ok(root)
    }
    fn snapshot(&self, id: &str, root: &Root, table: &str, cancel: &AtomicBool) -> Result<()> {
        // Broad workspaces can contain Kai's private storage. Never enumerate,
        // snapshot, diff, or restore that live, self-mutating subtree.
        let storage = fs::canonicalize(&self.path)?;
        let excluded_storage = storage.strip_prefix(&root.path).ok().map(Path::to_path_buf);
        let mut db = self.manifest(id)?;
        let tx = db.transaction()?;
        tx.execute(&format!("DELETE FROM {table}"), [])?;
        let blobs = self.path.join("blobs");
        let mut cache_connection = self.index()?;
        let cache =
            cache_connection.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        cache
            .execute_batch("CREATE TEMP TABLE cache_seen(path BLOB PRIMARY KEY) WITHOUT ROWID;")?;
        let mut stored: u64 =
            cache.query_row("SELECT total FROM blob_usage WHERE id=1", [], |r| r.get(0))?;
        let auxiliary = Self::auxiliary_usage(&cache)?.saturating_add(640 * 1024 * 1024);
        let mut count = 0u64;
        let mut total = 0u64;
        let mut capture = |path: &Path, tracked: bool| -> Result<()> {
            relative(path)?;
            if excluded_storage.as_ref().is_some_and(|excluded|path.starts_with(excluded)) {
                return Ok(());
            }
            if private_path(path) || (!tracked && hidden(path)) {
                return Ok(());
            }
            cancelled(cancel)?;
            let exists: bool = tx.query_row(
                &format!("SELECT EXISTS(SELECT 1 FROM {table} WHERE path=?1)"),
                [bytes(path)],
                |r| r.get(0),
            )?;
            if exists {
                return Ok(());
            }
            let signature = root.signature(path)?;
            let cached = if let Some(signature) = &signature {
                cache.query_row("SELECT kind,mode,size,digest,target,blob FROM file_cache WHERE root=?1 AND path=?2 AND signature=?3",params![bytes(&root.path),bytes(path),signature],|r|Ok(Entry{kind:r.get(0)?,mode:r.get(1)?,size:r.get(2)?,digest:r.get(3)?,target:r.get(4)?,blob:r.get(5)?})).optional()?
                    .filter(|entry|entry.blob.len()==64&&blobs.join(&entry.blob).metadata().is_ok_and(|m|m.is_file()&&m.len()==entry.size))
            } else {
                None
            };
            let entry = match cached {
                Some(entry) => Some(entry),
                None => {
                    // Reserve against the worst-case new immutable file before
                    // streaming it. Duplicate contents reclaim this space naturally.
                    if let Some(signature) = &signature {
                        let source_size = u64::from_le_bytes(signature[16..24].try_into().unwrap());
                        ensure!(
                            stored.saturating_add(source_size).saturating_add(auxiliary)
                                <= DISK_BYTES,
                            "Complete checkpoint storage would exceed 4 GiB. No prompt was started; existing checkpoints were kept."
                        );
                    }
                    root.entry(path, Some(&blobs), cancel)?
                }
            };
            if let Some(entry) = entry {
                if entry.kind == 0 {
                    ensure!(
                        root.signature(path)? == signature,
                        "File changed during snapshot cache validation: {}",
                        path.display()
                    );
                    if cache.execute(
                        "INSERT OR IGNORE INTO blobs(hash,size) VALUES(?1,?2)",
                        params![entry.blob, entry.size],
                    )? > 0
                    {
                        stored = stored.saturating_add(entry.size);
                        cache.execute("UPDATE blob_usage SET total=?1 WHERE id=1", [stored])?;
                    }
                    ensure!(
                        stored.saturating_add(auxiliary) <= DISK_BYTES,
                        "Checkpoint storage exceeded its 4 GiB budget; snapshots were kept"
                    );
                    cache.execute(
                        "INSERT OR IGNORE INTO blob_refs(checkpoint,hash) VALUES(?1,?2)",
                        params![id, entry.blob],
                    )?;
                    cache.execute(
                        "INSERT OR IGNORE INTO cache_seen(path) VALUES(?1)",
                        [bytes(path)],
                    )?;
                    cache.execute("INSERT OR REPLACE INTO file_cache(root,path,signature,kind,mode,size,digest,target,blob) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![bytes(&root.path),bytes(path),signature,entry.kind,entry.mode,entry.size,entry.digest,entry.target,entry.blob])?;
                }
                count += 1;
                total = total.saturating_add(entry.size);
                ensure!(
                    count <= MAX_FILES && total <= MAX_BYTES,
                    "Workspace exceeds the complete checkpoint budget (200,000 entries / 20 GiB); prompt was not started"
                );
                tx.execute(&format!("INSERT OR REPLACE INTO {table}(path,kind,mode,size,digest,target,blob) VALUES(?1,?2,?3,?4,?5,?6,?7)"),params![bytes(path),entry.kind,entry.mode,entry.size,entry.digest,entry.target,entry.blob])?;
            }
            Ok(())
        };
        let git = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&root.path)
            .args(["rev-parse", "--is-inside-work-tree"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()
            .is_some_and(|o| o.status.success());
        // A private empty Git directory applies the real nested/global ignore
        // semantics to non-repository folders without initializing that folder.
        let helper = self.dir(id)?.join("ignore.git");
        if !git && !helper.exists() {
            let status = Command::new("/usr/bin/git")
                .args(["init", "--bare", "--quiet"])
                .arg(&helper)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()?;
            ensure!(
                status.success(),
                "Git is required to enumerate ignored files safely"
            );
        }
        let mut command = Command::new("/usr/bin/git");
        command.arg("-C").arg(&root.path);
        if !git {
            command
                .arg("--git-dir")
                .arg(&helper)
                .arg("--work-tree")
                .arg(&root.path)
                .args(["-c", "core.bare=false"]);
        }
        command
            .args([
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-t",
                "-z",
                "--",
                ".",
            ])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .stdout(Stdio::piped());
        if let Some(excluded) = &excluded_storage {
            let mut spec = b":(exclude,literal)".to_vec();
            spec.extend_from_slice(bytes(excluded));
            command.arg(OsString::from_vec(spec));
        }
        let mut child = command.spawn()?;
        let result = (|| -> Result<()> {
            let mut reader = BufReader::with_capacity(IO_BYTES, child.stdout.take().unwrap());
            let mut name = Vec::with_capacity(1024);
            loop {
                cancelled(cancel)?;
                name.clear();
                let n = reader.by_ref().take(4097).read_until(0, &mut name)?;
                if n == 0 {
                    break;
                }
                ensure!(
                    name.len() <= 4096 && name.last() == Some(&0),
                    "Workspace contains an unsupported long path"
                );
                name.pop();
                ensure!(
                    name.len() >= 2 && name[1] == b' ',
                    "Git returned an unsupported checkpoint listing"
                );
                let tracked = name[0] != b'?';
                let path = PathBuf::from(OsString::from_vec(name[2..].to_vec()));
                capture(&path, tracked)?;
                for parent in path
                    .ancestors()
                    .skip(1)
                    .filter(|p| !p.as_os_str().is_empty())
                {
                    capture(parent, tracked)?;
                }
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = child.kill();
        }
        let status = child.wait()?;
        result?;
        ensure!(
            status.success(),
            "Git could not enumerate the complete workspace"
        );
        cancelled(cancel)?;
        // Cache metadata is disposable, not retained history. Remove deleted
        // paths and bound all roots together, evicting least recently reused rows.
        cache.execute(
            "DELETE FROM file_cache WHERE root=?1 AND path NOT IN(SELECT path FROM cache_seen)",
            [bytes(&root.path)],
        )?;
        cache.execute("DELETE FROM file_cache WHERE rowid IN(SELECT rowid FROM file_cache ORDER BY rowid DESC LIMIT -1 OFFSET ?1)", [MAX_CACHE_ENTRIES])?;
        cache.commit()?;
        tx.commit()?;
        Ok(())
    }
    pub fn preview(&self, id: &str, direction: Direction, offset: usize) -> Result<Preview> {
        let checkpoint = self.get(id)?;
        ensure!(
            checkpoint.phase == "ready" || checkpoint.phase.starts_with("applying-"),
            "This prompt's file checkpoint is not ready"
        );
        ensure!(
            checkpoint.undone == (direction == Direction::Redo),
            "Choose the current Undo or Redo action"
        );
        let db = self.manifest(id)?;
        let mut total = 0;
        let mut changes = Vec::with_capacity(PAGE);
        each_change(&db, None, |path, before, after| {
            if total >= offset && changes.len() < PAGE {
                changes.push(Change {
                    directory:before.as_ref().is_none_or(|e|e.kind==2)&&after.as_ref().is_none_or(|e|e.kind==2),
                    path: path.to_owned(),
                    kind: match (before.is_some(), after.is_some()) {
                        (false, true) => "Added",
                        (true, false) => "Deleted",
                        _ => "Modified",
                    },
                });
            }
            total += 1;
            Ok(())
        })?;
        Ok(Preview {
            checkpoint,
            changes,
            total,
        })
    }
    /// Select by this chat and canonical workspace, never by Git HEAD or a
    /// different chat's most recent turn. Zero-change prompts are ineligible.
    pub fn latest_completed(
        &self,
        session: &str,
        root: &Path,
        direction: Direction,
    ) -> Result<Option<Checkpoint>> {
        let root = fs::canonicalize(root)?;
        let db = self.index()?;
        let mut query = db.prepare("SELECT id,session,prompt_key,title,root,phase,undone,created FROM checkpoints WHERE session=?1 AND root=?2 AND phase='ready' AND undone=?3 ORDER BY CASE WHEN ?3 THEN COALESCE((SELECT sequence FROM restore_order WHERE checkpoint=checkpoints.id),0) ELSE created END DESC,created DESC,rowid DESC")?;
        let candidates = query.query_map(
            params![session, bytes(&root), direction == Direction::Redo],
            row,
        )?;
        for candidate in candidates {
            let candidate = candidate?;
            let _lock = self.lock(&candidate.id)?;
            let manifest = self.manifest(&candidate.id)?;
            let changed: bool = manifest.query_row("SELECT EXISTS(SELECT 1 FROM before b LEFT JOIN after a ON b.path=a.path WHERE a.path IS NULL OR b.kind!=a.kind OR b.mode!=a.mode OR b.digest!=a.digest OR b.target!=a.target UNION ALL SELECT 1 FROM after a LEFT JOIN before b ON a.path=b.path WHERE b.path IS NULL)",[],|r|r.get(0))?;
            if changed {
                return Ok(Some(candidate));
            }
        }
        Ok(None)
    }
    /// Read the immutable before/after versions for one path in this prompt.
    /// Cached immutable before/after counts. This runs only on a worker and
    /// uses the same bounded alignment as the native diff, never today's HEAD.
    pub fn change_summary(&self,id:&str,cancel:&AtomicBool)->Result<crate::github::Summary>{
        let _lock=self.lock(id)?;let checkpoint=self.get(id)?;
        ensure!(checkpoint.phase=="ready" || checkpoint.phase.starts_with("applying-"),"This prompt's files are still being finalized");
        let mut db=self.manifest(id)?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS change_stats(path BLOB PRIMARY KEY,status TEXT NOT NULL,added INTEGER NOT NULL,removed INTEGER NOT NULL,binary INTEGER NOT NULL);CREATE TABLE IF NOT EXISTS change_stats_meta(id INTEGER PRIMARY KEY,total INTEGER NOT NULL,added INTEGER NOT NULL,removed INTEGER NOT NULL,binaries INTEGER NOT NULL);")?;
        if db.query_row("SELECT 1 FROM change_stats_meta WHERE id=1",[],|r|r.get::<_,i64>(0)).optional()?.is_none(){
            let tx=db.transaction()?;tx.execute("DELETE FROM change_stats",[])?;
            let mut total=0usize;let mut added=0u64;let mut removed=0u64;let mut binaries=0usize;let deadline=std::time::Instant::now()+Duration::from_secs(45);
            each_change(&tx,None,|path,before,after|{
                cancelled(cancel)?;ensure!(std::time::Instant::now()<deadline,"Change counts exceeded their background review budget");
                if before.as_ref().is_none_or(|e|e.kind==2)&&after.as_ref().is_none_or(|e|e.kind==2){return Ok(())}
                let status=match(before.is_some(),after.is_some()){(false,true)=>"Added",(true,false)=>"Deleted",_=>"Modified"};
                let a=self.diff_version(before.clone())?;let b=self.diff_version(after.clone())?;
                let mut position=crate::checkpoint_diff::Position::default();let mut counts=crate::github::Counts::default();
                loop{let page=crate::checkpoint_diff::page(&a,&b,position,cancel)?;for line in page.lines{match line.kind{crate::checkpoint_diff::Kind::Added=>counts.added+=1,crate::checkpoint_diff::Kind::Removed=>counts.removed+=1,crate::checkpoint_diff::Kind::Note if line.text.starts_with("Binary or long-line")=>counts.binary=true,_=>{}}}if let Some(next)=page.next{position=next}else{break}ensure!(std::time::Instant::now()<deadline,"Change counts exceeded their background review budget");}
                total+=1;added+=counts.added;removed+=counts.removed;binaries+=usize::from(counts.binary);
                if total<=4096{tx.execute("INSERT INTO change_stats(path,status,added,removed,binary) VALUES(?1,?2,?3,?4,?5)",params![bytes(path),status,counts.added,counts.removed,counts.binary])?;}
                Ok(())
            })?;
            tx.execute("INSERT INTO change_stats_meta(id,total,added,removed,binaries) VALUES(1,?1,?2,?3,?4)",params![total as i64,added,removed,binaries as i64])?;tx.commit()?;
        }
        let mut result=db.query_row("SELECT total,added,removed,binaries FROM change_stats_meta WHERE id=1",[],|r|Ok(crate::github::Summary{total:r.get::<_,i64>(0)? as usize,added:r.get(1)?,removed:r.get(2)?,binaries:r.get::<_,i64>(3)? as usize,files:vec![]}))?;
        let mut query=db.prepare("SELECT path,status,added,removed,binary FROM change_stats ORDER BY path LIMIT 4096")?;
        result.files=query.query_map([],|r|Ok(crate::github::FileChange{path:PathBuf::from(OsString::from_vec(r.get::<_,Vec<u8>>(0)?)),old_path:None,status:r.get(1)?,counts:crate::github::Counts{added:r.get(2)?,removed:r.get(3)?,binary:r.get(4)?}}))?.collect::<rusqlite::Result<Vec<_>>>()?;Ok(result)
    }
    fn diff_version(&self,entry:Option<Entry>)->Result<crate::checkpoint_diff::Version>{
        let Some(entry)=entry else{return Ok(crate::checkpoint_diff::Version::absent())};
        if entry.kind==0{ensure!(entry.blob.len()==64 && entry.blob.bytes().all(|b|b.is_ascii_hexdigit()),"Invalid checkpoint content key");}
        Ok(crate::checkpoint_diff::Version{file:(entry.kind==0).then(||self.path.join("blobs").join(entry.blob)),inline:entry.target,size:entry.size,mode:entry.mode,kind:match entry.kind{0=>"file",1=>"symlink",_=>"directory"}})
    }
    pub fn diff_page(
        &self,
        id: &str,
        path: &Path,
        position: crate::checkpoint_diff::Position,
        cancel: &AtomicBool,
    ) -> Result<crate::checkpoint_diff::Page> {
        relative(path)?;
        let _lock = self.lock(id)?;
        let checkpoint = self.get(id)?;
        ensure!(
            checkpoint.phase == "ready" || checkpoint.phase.starts_with("applying-"),
            "This prompt's checkpoint is not ready"
        );
        let manifest = self.manifest(id)?;
        let before = table_entry(&manifest, "before", bytes(path))?;
        let after = table_entry(&manifest, "after", bytes(path))?;
        ensure!(before != after, "This path has no changes in this prompt");
        crate::checkpoint_diff::page(&self.diff_version(before)?, &self.diff_version(after)?, position, cancel)
    }
    pub fn apply(&self, id: &str, direction: Direction, cancel: &AtomicBool) -> Result<()> {
        let _lock = self.lock(id)?;
        cancelled(cancel)?;
        let checkpoint = self.get(id)?;
        ensure!(
            checkpoint.undone == (direction == Direction::Redo),
            "This checkpoint has already changed direction"
        );
        let phase = if direction == Direction::Undo {
            "applying-undo"
        } else {
            "applying-redo"
        };
        ensure!(
            checkpoint.phase == "ready" || checkpoint.phase == phase,
            "Checkpoint is not ready to restore"
        );
        let root = self.root(id)?;
        ensure!(
            root.fd.metadata()?.dev() == fs::metadata(self.dir(id)?)?.dev(),
            "Safe atomic undo requires checkpoint storage on the workspace's filesystem; snapshots were kept and no file was changed"
        );
        let db = self.manifest(id)?;
        if checkpoint.phase == "ready" {
            db.execute("DELETE FROM applied", [])?;
        }
        let storage = fs::canonicalize(&self.path)?;
        let excluded_storage = storage.strip_prefix(&root.path).ok();
        let mut recovery_bytes = 0u64;
        each_change(&db, None, |path, before, after| {
            cancelled(cancel)?;
            ensure!(!excluded_storage.is_some_and(|excluded|path.starts_with(excluded)), "Private checkpoint storage cannot be restored as a workspace file");
            let applied: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM applied WHERE path=?1)",
                [bytes(path)],
                |r| r.get(0),
            )?;
            let (source, target) = if direction == Direction::Undo {
                (after, before)
            } else {
                (before, after)
            };
            if let Some(entry) = source.as_ref().filter(|e| e.kind != 2) {
                recovery_bytes = recovery_bytes.saturating_add(entry.size);
            }
            let current = root.entry(path, None, cancel)?;
            if !applied && checkpoint.phase == phase && same(&current, target) {
                // The last process may have stopped after atomic rename but
                // before journaling it. Matching the exact desired bytes/mode
                // recovers that step without rewriting anything.
                db.execute(
                    "INSERT OR IGNORE INTO applied(path) VALUES(?1)",
                    [bytes(path)],
                )?;
                return Ok(());
            }
            let expected = if applied { target } else { source };
            ensure!(
                same(&current, expected),
                "File changed outside this prompt; nothing was overwritten: {}",
                path.display()
            );
            Ok(())
        })?;
        self.reserve_restore(
            id,
            &checkpoint.root,
            phase,
            if checkpoint.phase == phase {
                0
            } else {
                recovery_bytes
            },
        )?;
        // SQLite performs bounded disk sorting: parents first, empty dirs last.
        each_change(&db, Some(direction), |path, before, after| {
            cancelled(cancel)?;
            let done: bool = db.query_row(
                "SELECT EXISTS(SELECT 1 FROM applied WHERE path=?1)",
                [bytes(path)],
                |r| r.get(0),
            )?;
            if done {
                return Ok(());
            }
            let (source, target) = if direction == Direction::Undo {
                (after, before)
            } else {
                (before, after)
            };
            ensure!(
                same(&root.entry(path, None, cancel)?, source),
                "File changed during restore: {}",
                path.display()
            );
            self.restore(id, &root, path, source, target, cancel)?;
            db.execute("INSERT INTO applied(path) VALUES(?1)", [bytes(path)])?;
            Ok(())
        })?;
        let mut index = self.index()?;
        let tx = index.transaction()?;
        tx.execute(
            "UPDATE checkpoints SET phase='ready',undone=?2 WHERE id=?1",
            params![id, direction == Direction::Undo],
        )?;
        tx.execute("INSERT OR REPLACE INTO restore_order(checkpoint,sequence) VALUES(?1,COALESCE((SELECT max(sequence)+1 FROM restore_order),1))",[id])?;
        tx.commit()?;
        Ok(())
    }
    fn restore(
        &self,
        id: &str,
        root: &Root,
        path: &Path,
        source: &Option<Entry>,
        target: &Option<Entry>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let (parent, name) = root.parent(path, target.is_some())?;
        if let Some(entry) = target.as_ref().filter(|entry| entry.kind == 2) {
            let result = unsafe {
                libc::mkdirat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    entry.mode as libc::mode_t,
                )
            };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(std::io::Error::last_os_error().into());
            }
            let fd = syscall(unsafe {
                libc::openat(
                    parent.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
                )
            })?;
            let directory = unsafe { File::from_raw_fd(fd) };
            syscall(unsafe { libc::fchmod(directory.as_raw_fd(), entry.mode as libc::mode_t) })?;
            return Ok(());
        }
        if source.as_ref().is_some_and(|entry| entry.kind == 2) {
            ensure!(
                target.is_none(),
                "Changing a directory to a file needs manual review"
            );
            syscall(unsafe {
                libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), libc::AT_REMOVEDIR)
            })?;
            return Ok(());
        }
        let recovery_dir = self.dir(id)?.join("recovery");
        fs::create_dir_all(&recovery_dir)?;
        fs::set_permissions(&recovery_dir, fs::Permissions::from_mode(0o700))?;
        let recovery = Root::open(&recovery_dir)?;
        ensure!(
            parent.metadata()?.dev() == recovery.fd.metadata()?.dev(),
            "Safe atomic undo requires checkpoint storage on the workspace's filesystem; your snapshots are kept"
        );
        let token = uuid::Uuid::new_v4().to_string();
        let temp_text = format!(".code-undo-{token}");
        let temp = cstr(OsStr::new(&temp_text))?;
        let recovery_name = cstr(OsStr::new(&token))?;
        let temp_path = path.parent().unwrap_or(Path::new("")).join(&temp_text);
        if let Some(entry) = target {
            let prepared = (|| -> Result<()> {
                if entry.kind == 1 {
                    let link = CString::new(entry.target.clone())?;
                    syscall(unsafe {
                        libc::symlinkat(link.as_ptr(), parent.as_raw_fd(), temp.as_ptr())
                    })?;
                } else {
                    ensure!(
                        entry.blob.len() == 64 && entry.blob.bytes().all(|b| b.is_ascii_hexdigit()),
                        "Invalid checkpoint content key"
                    );
                    let fd = syscall(unsafe {
                        libc::openat(
                            parent.as_raw_fd(),
                            temp.as_ptr(),
                            libc::O_WRONLY
                                | libc::O_CREAT
                                | libc::O_EXCL
                                | libc::O_NOFOLLOW
                                | libc::O_CLOEXEC,
                            0o600,
                        )
                    })?;
                    let mut file = unsafe { File::from_raw_fd(fd) };
                    let mut blob = OpenOptions::new()
                        .read(true)
                        .custom_flags(libc::O_NOFOLLOW)
                        .open(self.path.join("blobs").join(&entry.blob))?;
                    let mut hash = Sha256::new();
                    let mut buffer = [0u8; IO_BYTES];
                    loop {
                        cancelled(cancel)?;
                        let n = blob.read(&mut buffer)?;
                        if n == 0 {
                            break;
                        }
                        hash.update(&buffer[..n]);
                        file.write_all(&buffer[..n])?;
                    }
                    ensure!(
                        hash.finalize().as_slice() == entry.digest.as_slice(),
                        "Checkpoint content failed integrity validation"
                    );
                    file.set_permissions(fs::Permissions::from_mode(entry.mode))?;
                    file.sync_all()?;
                }
                Ok(())
            })();
            if let Err(error) = prepared {
                let _ = unsafe { libc::unlinkat(parent.as_raw_fd(), temp.as_ptr(), 0) };
                return Err(error);
            }
            cancelled(cancel)?;
            if source.is_some() {
                rename_flags(&parent, &temp, &parent, &name, true)?;
                let displaced = root.entry(&temp_path, None, &AtomicBool::new(false))?;
                if !same(&displaced, source) {
                    // Preserve both inodes. Roll back only while our replacement
                    // is still at the live path; never overwrite an external edit.
                    if same(&root.entry(path, None, &AtomicBool::new(false))?, target) {
                        rename_flags(&parent, &temp, &parent, &name, true)?;
                    }
                    bail!(
                        "File changed concurrently; both versions were preserved: {}",
                        path.display()
                    );
                }
                rename_flags(&parent, &temp, &recovery.fd, &recovery_name, false)?;
            } else {
                rename_flags(&parent, &temp, &parent, &name, false)?;
            }
        } else if source.is_some() {
            rename_flags(&parent, &name, &recovery.fd, &recovery_name, false)?;
            let displaced = recovery.entry(Path::new(&token), None, &AtomicBool::new(false))?;
            if !same(&displaced, source) {
                let _ = rename_flags(&recovery.fd, &recovery_name, &parent, &name, false);
                bail!(
                    "File changed concurrently; its contents were preserved: {}",
                    path.display()
                );
            }
        }
        parent.sync_all()?;
        recovery.fd.sync_all()?;
        Ok(())
    }
}
/// Atomic exchange or no-replace rename. Unlike ordinary rename, a racing new
/// file is never overwritten. Displaced file inodes remain in recovery storage.
fn rename_flags(
    from: &File,
    source: &CString,
    to: &File,
    target: &CString,
    exchange: bool,
) -> Result<()> {
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::renameatx_np(
            from.as_raw_fd(),
            source.as_ptr(),
            to.as_raw_fd(),
            target.as_ptr(),
            if exchange {
                libc::RENAME_SWAP
            } else {
                libc::RENAME_EXCL
            },
        )
    };
    #[cfg(target_os = "linux")]
    let result = unsafe {
        libc::syscall(
            libc::SYS_renameat2,
            from.as_raw_fd(),
            source.as_ptr(),
            to.as_raw_fd(),
            target.as_ptr(),
            if exchange {
                libc::RENAME_EXCHANGE
            } else {
                libc::RENAME_NOREPLACE
            },
        ) as i32
    };
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let result = -1;
    syscall(result)
        .context("Could not atomically restore file; no existing destination was overwritten")?;
    Ok(())
}
fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Checkpoint> {
    Ok(Checkpoint {
        id: r.get(0)?,
        session: r.get(1)?,
        prompt_key: r.get(2)?,
        title: r.get(3)?,
        root: PathBuf::from(OsString::from_vec(r.get(4)?)),
        phase: r.get(5)?,
        undone: r.get(6)?,
        created: r.get(7)?,
    })
}
fn each_change(
    db: &Connection,
    direction: Option<Direction>,
    mut visit: impl FnMut(&Path, &Option<Entry>, &Option<Entry>) -> Result<()>,
) -> Result<()> {
    let target = if direction == Some(Direction::Undo) {
        "b"
    } else {
        "a"
    };
    let order = if direction.is_some() {
        format!(
            "CASE WHEN {target}.kind=2 THEN 0 WHEN {target}.path IS NULL AND COALESCE(b.kind,a.kind)=2 THEN 2 ELSE 1 END, CASE WHEN {target}.path IS NULL AND COALESCE(b.kind,a.kind)=2 THEN -length(p.path) ELSE length(p.path) END, p.path"
        )
    } else {
        "p.path".into()
    };
    let sql = format!(
        "SELECT p.path FROM (SELECT path FROM before UNION SELECT path FROM after) p LEFT JOIN before b ON b.path=p.path LEFT JOIN after a ON a.path=p.path WHERE b.path IS NULL OR a.path IS NULL OR b.kind!=a.kind OR b.mode!=a.mode OR b.digest!=a.digest OR b.target!=a.target ORDER BY {order}"
    );
    let mut query = db.prepare(&sql)?;
    let mut rows = query.query([])?;
    while let Some(row) = rows.next()? {
        let value: Vec<u8> = row.get(0)?;
        let path = PathBuf::from(OsString::from_vec(value.clone()));
        relative(&path)?;
        let before = table_entry(db, "before", &value)?;
        let after = table_entry(db, "after", &value)?;
        visit(&path, &before, &after)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    struct Fixture {
        path: PathBuf,
        root: PathBuf,
        repo: Repository,
    }
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("code-checkpoint-test-{}", uuid::Uuid::new_v4()));
            let root = path.join("workspace");
            fs::create_dir_all(&root).unwrap();
            let repo = Repository::new(path.join("private")).unwrap();
            Self { path, root, repo }
        }
        fn start(&self, key: &str) -> String {
            self.repo
                .capture_before(
                    &self.root,
                    "session",
                    key,
                    "Make changes",
                    &AtomicBool::new(false),
                )
                .unwrap()
        }
        fn seal(&self, id: &str) {
            self.repo.seal(id, &AtomicBool::new(false)).unwrap()
        }
        fn apply(&self, id: &str, direction: Direction) {
            self.repo
                .apply(id, direction, &AtomicBool::new(false))
                .unwrap()
        }
    }
    #[test]
    fn nested_checkpoint_storage_is_excluded_and_survives_undo_and_redo() {
        for git in [false, true] {
            let mut fixture = Fixture::new();
            let private = fixture.root.join("Kai [private]");
            fixture.repo = Repository::new(private.clone()).unwrap();
            fs::write(private.join("sentinel.txt"), "private-before").unwrap();
            fs::write(fixture.root.join("work.txt"), "before").unwrap();
            if git {
                assert!(Command::new("/usr/bin/git").arg("-C").arg(&fixture.root).args(["init","--quiet"]).status().unwrap().success());
                assert!(Command::new("/usr/bin/git").arg("-C").arg(&fixture.root).args(["add","--","Kai [private]/sentinel.txt"]).status().unwrap().success());
            }
            let id = fixture.start("nested-storage");
            fs::write(fixture.root.join("work.txt"), "after").unwrap();
            fs::write(private.join("sentinel.txt"), "private-live").unwrap();
            fixture.seal(&id);
            let preview = fixture.repo.preview(&id, Direction::Undo, 0).unwrap();
            assert!(preview.changes.iter().all(|change| !change.path.starts_with("Kai [private]")));
            fixture.apply(&id, Direction::Undo);
            assert_eq!(fs::read_to_string(fixture.root.join("work.txt")).unwrap(), "before");
            assert_eq!(fs::read_to_string(private.join("sentinel.txt")).unwrap(), "private-live");
            fixture.apply(&id, Direction::Redo);
            assert_eq!(fs::read_to_string(fixture.root.join("work.txt")).unwrap(), "after");
            assert_eq!(fs::read_to_string(private.join("sentinel.txt")).unwrap(), "private-live");
            assert!(fixture.repo.capture_before(&private, "session", "invalid", "private", &AtomicBool::new(false)).is_err());
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
    #[test]
    fn immutable_change_counts_match_the_saved_prompt_and_survive_later_edits(){
        let f=Fixture::new();fs::write(f.root.join("old.rs"),"alpha\nbeta\n").unwrap();fs::write(f.root.join("delete.txt"),"old\n").unwrap();
        let id=f.start("summary");fs::write(f.root.join("old.rs"),"alpha\ngamma\n").unwrap();fs::remove_file(f.root.join("delete.txt")).unwrap();fs::create_dir_all(f.root.join("src/nested")).unwrap();fs::write(f.root.join("src/nested/new.rs"),"one\ntwo\n").unwrap();fs::write(f.root.join("image.bin"),[0,255,12]).unwrap();f.seal(&id);
        let first=f.repo.change_summary(&id,&AtomicBool::new(false)).unwrap();assert_eq!((first.total,first.added,first.removed,first.binaries),(4,3,2,1));
        assert!(first.files.iter().any(|v|v.path==Path::new("src/nested/new.rs")&&v.counts.added==2));
        fs::write(f.root.join("old.rs"),"later\nindependent\nedits\n").unwrap();let reopened=Repository::at(f.repo.path.clone());let cached=reopened.change_summary(&id,&AtomicBool::new(false)).unwrap();assert_eq!((cached.total,cached.added,cached.removed),(4,3,2));
    }
    #[test]
    fn prompt_undo_redo_preserves_modified_deleted_new_binary_files_and_modes_after_restart() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("original.rs"), "before\n").unwrap();
        fs::write(fixture.root.join("deleted.bin"), [0, 1, 255, 0]).unwrap();
        fs::set_permissions(
            fixture.root.join("original.rs"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let id = fixture.start("prompt-one");
        fs::write(fixture.root.join("original.rs"), "after\n").unwrap();
        fs::set_permissions(
            fixture.root.join("original.rs"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        fs::remove_file(fixture.root.join("deleted.bin")).unwrap();
        fs::create_dir(fixture.root.join("new")).unwrap();
        fs::write(fixture.root.join("new/added.txt"), "new file").unwrap();
        fixture.seal(&id);
        let preview = fixture.repo.preview(&id, Direction::Undo, 0).unwrap();
        assert_eq!(preview.total, 4);
        assert!(
            preview
                .changes
                .iter()
                .any(|c| c.path == Path::new("deleted.bin") && c.kind == "Deleted")
        );
        let restarted = Repository::new(fixture.path.join("private")).unwrap();
        assert_eq!(
            restarted.list("session", 0).unwrap()[0].prompt_key,
            "prompt-one"
        );
        restarted
            .apply(&id, Direction::Undo, &AtomicBool::new(false))
            .unwrap();
        assert_eq!(
            fs::read_to_string(fixture.root.join("original.rs")).unwrap(),
            "before\n"
        );
        assert_eq!(
            fs::read(fixture.root.join("deleted.bin")).unwrap(),
            [0, 1, 255, 0]
        );
        assert!(!fixture.root.join("new").exists());
        assert_eq!(
            fs::metadata(fixture.root.join("original.rs"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o755
        );
        fixture.apply(&id, Direction::Redo);
        assert_eq!(
            fs::read_to_string(fixture.root.join("original.rs")).unwrap(),
            "after\n"
        );
        assert!(!fixture.root.join("deleted.bin").exists());
        assert_eq!(
            fs::read_to_string(fixture.root.join("new/added.txt")).unwrap(),
            "new file"
        );
    }
    #[test]
    fn conflicts_and_cancellation_refuse_to_overwrite_current_edits() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("file"), "before").unwrap();
        let id = fixture.start("p");
        fs::write(fixture.root.join("file"), "agent").unwrap();
        fixture.seal(&id);
        fs::write(fixture.root.join("file"), "my later edit").unwrap();
        let error = fixture
            .repo
            .apply(&id, Direction::Undo, &AtomicBool::new(false))
            .unwrap_err();
        assert!(error.to_string().contains("outside this prompt"));
        assert_eq!(
            fs::read_to_string(fixture.root.join("file")).unwrap(),
            "my later edit"
        );
        fs::write(fixture.root.join("file"), "agent").unwrap();
        assert!(
            fixture
                .repo
                .apply(&id, Direction::Undo, &AtomicBool::new(true))
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(fixture.root.join("file")).unwrap(),
            "agent"
        );
        assert!(!fixture.repo.get(&id).unwrap().undone);
    }
    #[test]
    fn nested_git_ignores_and_caches_stay_out_of_complete_snapshots() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join(".gitignore"), "custom-cache/\n*.log\n").unwrap();
        fs::create_dir_all(fixture.root.join("custom-cache")).unwrap();
        fs::write(fixture.root.join("custom-cache/large.txt"), "ignored").unwrap();
        fs::write(fixture.root.join("app.log"), "ignored").unwrap();
        fs::create_dir_all(fixture.root.join("target")).unwrap();
        fs::write(fixture.root.join("target/cache"), "ignored").unwrap();
        fs::create_dir_all(fixture.root.join("src")).unwrap();
        fs::write(fixture.root.join("src/.gitignore"), "secret.tmp\n").unwrap();
        fs::write(fixture.root.join("src/secret.tmp"), "ignored").unwrap();
        fs::write(fixture.root.join("src/main.rs"), "before").unwrap();
        let id = fixture.start("p");
        let manifest = fixture.repo.manifest(&id).unwrap();
        let count: u64 = manifest
            .query_row(
                "SELECT count(*) FROM before WHERE path IN (?1,?2,?3,?4)",
                params![
                    b"app.log".as_slice(),
                    b"custom-cache/large.txt".as_slice(),
                    b"target/cache".as_slice(),
                    b"src/secret.tmp".as_slice()
                ],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 0);
        fs::write(fixture.root.join("src/main.rs"), "after").unwrap();
        fixture.seal(&id);
        fixture.apply(&id, Direction::Undo);
        assert_eq!(
            fs::read_to_string(fixture.root.join("custom-cache/large.txt")).unwrap(),
            "ignored"
        );
    }
    #[test]
    fn symlinks_are_preserved_without_following_them_and_replaced_parents_are_rejected() {
        let fixture = Fixture::new();
        let outside = fixture.path.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep"), "outside").unwrap();
        fs::create_dir(fixture.root.join("src")).unwrap();
        fs::write(fixture.root.join("src/file"), "before").unwrap();
        symlink(&outside, fixture.root.join("link")).unwrap();
        let id = fixture.start("p");
        fs::write(fixture.root.join("src/file"), "after").unwrap();
        fs::remove_file(fixture.root.join("link")).unwrap();
        symlink("src", fixture.root.join("link")).unwrap();
        fixture.seal(&id);
        fixture.apply(&id, Direction::Undo);
        assert_eq!(fs::read_link(fixture.root.join("link")).unwrap(), outside);
        assert_eq!(
            fs::read_to_string(fixture.path.join("outside/keep")).unwrap(),
            "outside"
        );
        fixture.apply(&id, Direction::Redo);
        fs::rename(fixture.root.join("src"), fixture.root.join("old-src")).unwrap();
        symlink(&outside, fixture.root.join("src")).unwrap();
        assert!(
            fixture
                .repo
                .apply(&id, Direction::Undo, &AtomicBool::new(false))
                .is_err()
        );
        assert_eq!(
            fs::read_to_string(fixture.path.join("outside/keep")).unwrap(),
            "outside"
        );
        assert!(relative(Path::new("../escape")).is_err());
        assert!(relative(Path::new("/absolute")).is_err());
    }
    #[test]
    fn overlapping_prompts_and_restores_are_serialized_and_failed_captures_release_the_root() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("file"), "before").unwrap();
        let id = fixture.start("one");
        assert!(
            fixture
                .repo
                .capture_before(
                    &fixture.root,
                    "other",
                    "two",
                    "Second",
                    &AtomicBool::new(false)
                )
                .is_err()
        );
        fixture.repo.abandon(&id).unwrap();
        let next = fixture.start("two");
        fs::write(fixture.root.join("file"), "after").unwrap();
        fixture.seal(&next);
        let active = fixture.start("three");
        assert!(
            fixture
                .repo
                .apply(&next, Direction::Undo, &AtomicBool::new(false))
                .is_err()
        );
        fixture.repo.abandon(&active).unwrap();
        fixture.apply(&next, Direction::Undo);
        assert!(
            fixture
                .repo
                .capture_before(
                    &fixture.root,
                    "session",
                    "cancel",
                    "Canceled",
                    &AtomicBool::new(true)
                )
                .is_err()
        );
        let final_id = fixture.start("four");
        fixture.repo.abandon(&final_id).unwrap();
    }
    #[test]
    fn unchanged_snapshots_reuse_verified_cached_blobs_and_readonly_records_never_claim_undo() {
        let fixture = Fixture::new();
        fs::write(fixture.root.join("file"), "unchanged").unwrap();
        let first = fixture.start("same-one");
        fixture.seal(&first);
        let second = fixture.start("same-two");
        fixture.seal(&second);
        let db = fixture.repo.index().unwrap();
        let count: u64 = db
            .query_row("SELECT count(*) FROM blobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(
            fixture
                .repo
                .preview(&second, Direction::Undo, 0)
                .unwrap()
                .total,
            0
        );
        let readonly = fixture
            .repo
            .record_read_only(&fixture.root, "session", "readonly", "Inspect files")
            .unwrap();
        fixture
            .repo
            .seal(&readonly, &AtomicBool::new(false))
            .unwrap();
        assert!(fixture.repo.preview(&readonly, Direction::Undo, 0).is_err());
    }
}

#[cfg(test)]
mod recovery_tests {
    use super::*;
    fn fixture() -> (PathBuf, PathBuf, Repository) {
        let path =
            std::env::temp_dir().join(format!("code-restore-recovery-{}", uuid::Uuid::new_v4()));
        let root = path.join("workspace");
        fs::create_dir_all(&root).unwrap();
        let repo = Repository::new(path.join("private")).unwrap();
        (path, root, repo)
    }
    #[test]
    fn forgetting_shared_blobs_is_reference_aware_and_never_changes_workspace_files() {
        let (path, root, repo) = fixture();
        fs::write(root.join("file"), "shared original").unwrap();
        let cancel = AtomicBool::new(false);
        let first = repo
            .capture_before(&root, "s", "one", "First", &cancel)
            .unwrap();
        repo.seal(&first, &cancel).unwrap();
        let second = repo
            .capture_before(&root, "s", "two", "Second", &cancel)
            .unwrap();
        repo.seal(&second, &cancel).unwrap();
        repo.forget(&first, &cancel).unwrap();
        let count: u64 = repo
            .index()
            .unwrap()
            .query_row("SELECT count(*) FROM blobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        assert!(repo.get(&second).is_ok());
        assert!(repo.get(&first).is_err());
        repo.forget(&second, &cancel).unwrap();
        let count: u64 = repo
            .index()
            .unwrap()
            .query_row("SELECT count(*) FROM blobs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        assert_eq!(
            fs::read_to_string(root.join("file")).unwrap(),
            "shared original"
        );
        let third = repo
            .capture_before(&root, "s", "three", "Third", &cancel)
            .unwrap();
        assert!(repo.forget(&third, &cancel).is_err());
        repo.abandon(&third).unwrap();
        repo.forget(&third, &cancel).unwrap();
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn redo_follows_actual_undo_order_for_two_turns_editing_the_same_file() {
        let (path, root, repo) = fixture();
        let file = root.join("file");
        std::fs::write(&file, "zero").unwrap();
        let cancel = AtomicBool::new(false);
        let first = repo
            .capture_before(&root, "chat", "first", "First", &cancel)
            .unwrap();
        std::fs::write(&file, "one").unwrap();
        repo.seal(&first, &cancel).unwrap();
        let second = repo
            .capture_before(&root, "chat", "second", "Second", &cancel)
            .unwrap();
        std::fs::write(&file, "two").unwrap();
        repo.seal(&second, &cancel).unwrap();
        repo.apply(&second, Direction::Undo, &cancel).unwrap();
        repo.apply(&first, Direction::Undo, &cancel).unwrap();
        assert_eq!(
            repo.latest_completed("chat", &root, Direction::Redo)
                .unwrap()
                .unwrap()
                .id,
            first
        );
        repo.apply(&first, Direction::Redo, &cancel).unwrap();
        assert_eq!(
            repo.latest_completed("chat", &root, Direction::Redo)
                .unwrap()
                .unwrap()
                .id,
            second
        );
        repo.apply(&second, Direction::Redo, &cancel).unwrap();
        assert_eq!(std::fs::read_to_string(file).unwrap(), "two");
        std::fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn prompt_diffs_and_latest_undo_use_only_the_exact_pass_after_restart() {
        let (path, root, repo) = fixture();
        fs::write(root.join("first.rs"), "first before\n").unwrap();
        fs::write(root.join("second.rs"), "second before\n").unwrap();
        let cancel = AtomicBool::new(false);
        let first = repo
            .capture_before(&root, "chat", "one", "First pass", &cancel)
            .unwrap();
        fs::write(root.join("first.rs"), "first after\n").unwrap();
        repo.seal(&first, &cancel).unwrap();
        let second = repo
            .capture_before(&root, "chat", "two", "Second pass", &cancel)
            .unwrap();
        fs::write(root.join("second.rs"), "second after\n").unwrap();
        fs::write(root.join("new.txt"), "created in second\n").unwrap();
        repo.seal(&second, &cancel).unwrap();
        let repo = Repository::new(path.join("private")).unwrap();
        assert_eq!(
            repo.latest_completed("chat", &root, Direction::Undo)
                .unwrap()
                .unwrap()
                .id,
            second
        );
        assert!(
            repo.latest_completed("other-chat", &root, Direction::Undo)
                .unwrap()
                .is_none()
        );
        assert!(
            repo.diff_page(&second, Path::new("first.rs"), Default::default(), &cancel)
                .is_err()
        );
        let patch = repo
            .diff_page(&second, Path::new("second.rs"), Default::default(), &cancel)
            .unwrap();
        assert!(
            patch
                .lines
                .iter()
                .any(|line| line.kind == crate::checkpoint_diff::Kind::Removed
                    && line.text == "second before")
        );
        assert!(
            patch
                .lines
                .iter()
                .any(|line| line.kind == crate::checkpoint_diff::Kind::Added
                    && line.text == "second after")
        );
        assert!(
            repo.diff_page(&second, Path::new("../escape"), Default::default(), &cancel)
                .is_err()
        );
        repo.apply(&second, Direction::Undo, &cancel).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("first.rs")).unwrap(),
            "first after\n"
        );
        assert_eq!(
            fs::read_to_string(root.join("second.rs")).unwrap(),
            "second before\n"
        );
        assert!(!root.join("new.txt").exists());
        assert_eq!(
            repo.latest_completed("chat", &root, Direction::Redo)
                .unwrap()
                .unwrap()
                .id,
            second
        );
        repo.apply(&second, Direction::Redo, &cancel).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("new.txt")).unwrap(),
            "created in second\n"
        );
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn dead_owner_capture_records_are_interrupted_and_can_be_forgotten() {
        let (path, root, repo) = fixture();
        fs::write(root.join("file"), "before").unwrap();
        let cancel = AtomicBool::new(false);
        let id = repo
            .capture_before(&root, "s", "p", "Crashed prompt", &cancel)
            .unwrap();
        let db = repo.index().unwrap();
        db.execute("UPDATE checkpoints SET owner=2000000000 WHERE id=?1", [&id])
            .unwrap();
        repo.recover_orphans(&db).unwrap();
        assert_eq!(repo.get(&id).unwrap().phase, "interrupted");
        assert!(repo.preview(&id, Direction::Undo, 0).is_err());
        repo.forget(&id, &cancel).unwrap();
        assert_eq!(fs::read_to_string(root.join("file")).unwrap(), "before");
        let next = repo
            .capture_before(&root, "s", "next", "Next prompt", &cancel)
            .unwrap();
        repo.abandon(&next).unwrap();
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn tracked_build_paths_are_restorable_and_cache_metadata_is_pruned() {
        let (path, root, repo) = fixture();
        fs::create_dir(root.join("build")).unwrap();
        fs::write(root.join("build/source.rs"), "before").unwrap();
        fs::write(root.join("old"), "old").unwrap();
        assert!(
            Command::new("/usr/bin/git")
                .arg("-C")
                .arg(&root)
                .args(["init", "--quiet"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("/usr/bin/git")
                .arg("-C")
                .arg(&root)
                .args(["add", "build/source.rs"])
                .status()
                .unwrap()
                .success()
        );
        let cancel = AtomicBool::new(false);
        let id = repo
            .capture_before(&root, "s", "p", "Tracked build source", &cancel)
            .unwrap();
        fs::write(root.join("build/source.rs"), "after").unwrap();
        fs::remove_file(root.join("old")).unwrap();
        repo.seal(&id, &cancel).unwrap();
        let stale: u64 = repo
            .index()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM file_cache WHERE path=?1",
                [b"old".as_slice()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0);
        assert!(
            repo.preview(&id, Direction::Undo, 0)
                .unwrap()
                .changes
                .iter()
                .any(|change| change.path == Path::new("build/source.rs"))
        );
        repo.apply(&id, Direction::Undo, &cancel).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("build/source.rs")).unwrap(),
            "before"
        );
        repo.forget(&id, &cancel).unwrap();
        let cached: u64 = repo
            .index()
            .unwrap()
            .query_row("SELECT count(*) FROM file_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cached, 0);
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn a_crash_after_atomic_rename_is_recovered_without_overwriting_new_edits() {
        let (path, root, repo) = fixture();
        for name in ["a", "b"] {
            fs::write(root.join(name), "before").unwrap();
        }
        let cancel = AtomicBool::new(false);
        let id = repo
            .capture_before(&root, "s", "p", "Prompt", &cancel)
            .unwrap();
        for name in ["a", "b"] {
            fs::write(root.join(name), "after").unwrap();
        }
        repo.seal(&id, &cancel).unwrap();
        let manifest = repo.manifest(&id).unwrap();
        let before = table_entry(&manifest, "before", b"a").unwrap();
        let after = table_entry(&manifest, "after", b"a").unwrap();
        repo.reserve_restore(&id, &root, "applying-undo", 10)
            .unwrap();
        repo.restore(
            &id,
            &repo.root(&id).unwrap(),
            Path::new("a"),
            &after,
            &before,
            &cancel,
        )
        .unwrap();
        drop(manifest);
        let restarted = Repository::new(path.join("private")).unwrap();
        restarted.apply(&id, Direction::Undo, &cancel).unwrap();
        for name in ["a", "b"] {
            assert_eq!(fs::read_to_string(root.join(name)).unwrap(), "before");
        }
        restarted.apply(&id, Direction::Redo, &cancel).unwrap();
        for name in ["a", "b"] {
            assert_eq!(fs::read_to_string(root.join(name)).unwrap(), "after");
        }
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn atomic_exchange_preserves_an_edit_that_races_after_preflight() {
        let (path, root, repo) = fixture();
        fs::write(root.join("file"), "before").unwrap();
        let cancel = AtomicBool::new(false);
        let id = repo
            .capture_before(&root, "s", "p", "Prompt", &cancel)
            .unwrap();
        fs::write(root.join("file"), "agent").unwrap();
        repo.seal(&id, &cancel).unwrap();
        let manifest = repo.manifest(&id).unwrap();
        let before = table_entry(&manifest, "before", b"file").unwrap();
        let after = table_entry(&manifest, "after", b"file").unwrap();
        fs::write(root.join("file"), "my racing edit").unwrap();
        assert!(
            repo.restore(
                &id,
                &repo.root(&id).unwrap(),
                Path::new("file"),
                &after,
                &before,
                &cancel
            )
            .is_err()
        );
        assert_eq!(
            fs::read_to_string(root.join("file")).unwrap(),
            "my racing edit"
        );
        assert!(!repo.get(&id).unwrap().undone);
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn orphaned_ui_launch_seals_after_owned_process_cleanup() {
        let (path, root, repo) = fixture();
        fs::write(root.join("file"), "before").unwrap();
        let cancel = AtomicBool::new(false);
        let id = repo
            .capture_before(&root, "s", "p", "Prompt", &cancel)
            .unwrap();
        fs::write(root.join("file"), "after").unwrap();
        let (tx, rx) = async_channel::bounded::<()>(1);
        drop(repo.cleanup_on_orphan(id.clone(), rx));
        assert_eq!(repo.get(&id).unwrap().phase, "recording");
        drop(tx);
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while repo.get(&id).unwrap().phase != "ready" {
            assert!(std::time::Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(5));
        }
        repo.apply(&id, Direction::Undo, &cancel).unwrap();
        assert_eq!(fs::read_to_string(root.join("file")).unwrap(), "before");
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn partial_capture_failure_releases_reservations_and_does_not_leave_unaccounted_blobs() {
        let (path, root, repo) = fixture();
        fs::write(root.join("a-file"), "data").unwrap();
        let fifo = root.join("z-pipe");
        fs::write(&fifo, "tracked first").unwrap();
        assert!(
            Command::new("/usr/bin/git")
                .arg("-C")
                .arg(&root)
                .args(["init", "--quiet"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("/usr/bin/git")
                .arg("-C")
                .arg(&root)
                .args(["add", "--", "a-file", "z-pipe"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .unwrap()
                .success()
        );
        fs::remove_file(&fifo).unwrap();
        let name = cstr(fifo.as_os_str()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        let cancel = AtomicBool::new(false);
        assert!(
            repo.capture_before(&root, "s", "bad", "Prompt", &cancel)
                .is_err()
        );
        assert_eq!(fs::read_dir(path.join("private/blobs")).unwrap().count(), 0);
        fs::remove_file(fifo).unwrap();
        let id = repo
            .capture_before(&root, "s", "good", "Prompt", &cancel)
            .unwrap();
        repo.abandon(&id).unwrap();
        let retained: u64 = repo
            .index()
            .unwrap()
            .query_row(
                "SELECT count(*) FROM checkpoints WHERE phase IN ('capturing','recording')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(retained, 0);
        fs::remove_dir_all(path).unwrap();
    }
    #[test]
    fn storage_limits_fail_before_launch_and_existing_snapshots_can_be_forgotten() {
        let (path, root, repo) = fixture();
        fs::write(root.join("file"), "data").unwrap();
        let cancel = AtomicBool::new(false);
        repo.index()
            .unwrap()
            .execute("UPDATE blob_usage SET total=?1 WHERE id=1", [DISK_BYTES])
            .unwrap();
        assert!(
            repo.capture_before(&root, "s", "quota", "Prompt", &cancel)
                .is_err()
        );
        assert_eq!(fs::read_to_string(root.join("file")).unwrap(), "data");
        let existing = repo.list("s", 0).unwrap();
        assert_eq!(existing.len(), 1);
        assert_eq!(existing[0].phase, "capture-failed");
        repo.forget(&existing[0].id, &cancel).unwrap();
        fs::remove_dir_all(path).unwrap();
    }
}
