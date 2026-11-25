//! Native Git/GitHub work runs on the existing background executor. Credentials
//! stay inside Git/gh; no token is read into renderer state or written to settings.
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            ffi::{OsStrExt, OsStringExt},
            fs::OpenOptionsExt,
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
const OUTPUT: usize = 8 * 1024 * 1024;
const FILES: usize = 4096;
const FILE_BYTES: u64 = 512 * 1024 * 1024;
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Counts {
    pub added: u64,
    pub removed: u64,
    pub binary: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FileChange {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub status: String,
    pub counts: Counts,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Summary {
    pub files: Vec<FileChange>,
    pub total: usize,
    pub added: u64,
    pub removed: u64,
    pub binaries: usize,
}
impl Summary {
    pub fn from_files(files: Vec<FileChange>) -> Self {
        Self {
            total: files.len(),
            added: files.iter().map(|f| f.counts.added).sum(),
            removed: files.iter().map(|f| f.counts.removed).sum(),
            binaries: files.iter().filter(|f| f.counts.binary).count(),
            files,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostedRepo {
    pub host: String,
    pub owner: String,
    pub name: String,
}
impl HostedRepo {
    pub fn slug(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }
    pub fn url(&self) -> String {
        format!("https://{}/{}/{}", self.host, self.owner, self.name)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    pub remote: bool,
    pub current: bool,
    pub upstream: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkout {
    pub root: PathBuf,
    pub git_dir: PathBuf,
    pub common_dir: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u64,
    pub behind: u64,
    pub remotes: Vec<String>,
    pub remote: Option<String>,
    pub remote_url: Option<String>,
    pub hosted: Option<HostedRepo>,
    pub push_hosted: Option<HostedRepo>,
    pub remote_repos: Vec<(String, HostedRepo)>,
    pub branches: Vec<Branch>,
    pub changes: Summary,
    pub conflicted: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub repository: Option<HostedRepo>,
    pub title: String,
    pub url: String,
    pub state: String,
    pub draft: bool,
    pub head: String,
    pub base: String,
    pub head_oid: String,
    pub author: String,
    pub review: String,
    pub checks: String,
    pub mergeable: String,
    pub added: u64,
    pub removed: u64,
    pub files: u64,
}
impl PullRequest {
    pub fn glyph(&self) -> (&'static str, u32) {
        if self.state == "MERGED" {
            ("git-merge", 0xb39aff)
        } else if self.state == "CLOSED" {
            ("git-pull-request-closed", 0xff8e91)
        } else if self.checks == "FAILURE"
            || self.checks == "ERROR"
            || self.mergeable == "CONFLICTING"
            || self.review == "CHANGES_REQUESTED"
        {
            ("circle-x", 0xff8e91)
        } else if self.draft {
            ("git-pull-request-draft", 0xa8b0bf)
        } else if self.checks == "PENDING"
            || self.checks == "EXPECTED"
            || self.review == "REVIEW_REQUIRED"
        {
            ("circle-dot", 0xecc87a)
        } else {
            ("git-pull-request", 0x8ad9a0)
        }
    }
}
#[derive(Clone, Debug, Default)]
pub struct PullPage {
    pub login: String,
    pub parent: Option<HostedRepo>,
    pub default_branch: String,
    pub permission: String,
    pub private: bool,
    pub items: Vec<PullRequest>,
    pub cursor: Option<String>,
    pub total: u64,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub checkout: Option<Checkout>,
    pub pulls: PullPage,
    pub gh_available: bool,
    pub github_error: Option<String>,
}
#[derive(Clone, Debug)]
pub struct SyncPreview {
    pub checkout: Checkout,
    pub fingerprint: String,
}
#[derive(Clone, Debug)]
pub struct SyncResult {
    pub commit: String,
    pub committed: bool,
    pub pushed: bool,
    pub push_error: Option<String>,
    pub warning: Option<String>,
}
#[derive(Clone, Debug, Deserialize)]
pub struct Commit {
    pub oid: String,
    pub subject: String,
    pub author: String,
    pub date: String,
}
#[derive(Clone, Debug, Default)]
pub struct Account {
    pub login: String,
    pub url: String,
    pub total: u64,
    pub commits: u64,
    pub prs: u64,
    pub reviews: u64,
    pub issues: u64,
    pub days: Vec<(String, u64)>,
    pub repositories: Vec<(String, String, bool)>,
}
#[derive(Clone)]
pub struct Client {
    pub gh: Option<PathBuf>,
}
struct OwnedChild(Child, bool);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        if !self.1 {
            unsafe {
                libc::kill(-(self.0.id() as i32), libc::SIGKILL);
            }
            let _ = self.0.wait();
        }
    }
}
#[derive(Debug)]
struct Output {
    ok: bool,
    code: Option<i32>,
    bytes: Vec<u8>,
    error: String,
}
fn redact(text: &str) -> String {
    let token = regex::Regex::new(
        r"(?i)(?:gh[pousr]_[A-Za-z0-9_]+|github_pat_[A-Za-z0-9_]+|https?://[^\s/@]+(?::[^\s/@]*)?@)",
    )
    .unwrap();
    let value = token.replace_all(text, "[credential redacted]");
    value
        .chars()
        .filter(|c| *c == '\n' || !c.is_control())
        .take(2400)
        .collect()
}
fn run(command: &mut Command, timeout: Duration, cancel: &AtomicBool) -> Result<Output> {
    ensure!(!cancel.load(Ordering::Acquire), "Operation cancelled");
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = OwnedChild(
        command.spawn().context("Could not launch source control")?,
        false,
    );
    let mut out = child
        .0
        .stdout
        .take()
        .context("Missing source-control output")?;
    let mut err = child
        .0
        .stderr
        .take()
        .context("Missing source-control diagnostics")?;
    for fd in [out.as_raw_fd(), err.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        ensure!(
            flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "Cannot read source-control output"
        );
    }
    let deadline = Instant::now() + timeout;
    let mut streams = [Vec::new(), Vec::new()];
    let mut eof = [false, false];
    let mut exited: Option<Instant> = None;
    loop {
        ensure!(!cancel.load(Ordering::Acquire), "Operation cancelled");
        ensure!(
            Instant::now() < deadline,
            "Source-control operation timed out; refresh to check its result before retrying"
        );
        for (i, reader) in [&mut out as &mut dyn Read, &mut err as &mut dyn Read]
            .into_iter()
            .enumerate()
        {
            if eof[i] {
                continue;
            }
            let mut buf = [0; 8192];
            for _ in 0..8 {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        eof[i] = true;
                        break;
                    }
                    Ok(n) => {
                        ensure!(
                            streams[i].len() + n <= if i == 0 { OUTPUT } else { 128 * 1024 },
                            "Source-control output exceeded its limit"
                        );
                        streams[i].extend_from_slice(&buf[..n]);
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(e) => return Err(e.into()),
                }
            }
        }
        if crate::runtime::child_exit_pending(child.0.id() as i32)? {
            let at = exited.get_or_insert_with(Instant::now);
            if eof.iter().all(|v| *v) || at.elapsed() > Duration::from_millis(80) {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(8));
    }
    // Keep PID owned until the descendant group has been retired.
    unsafe {
        libc::kill(-(child.0.id() as i32), libc::SIGKILL);
    }
    let status = child.0.wait()?;
    child.1 = true;
    Ok(Output {
        ok: status.success(),
        code: status.code(),
        bytes: std::mem::take(&mut streams[0]),
        error: redact(&String::from_utf8_lossy(&streams[1])),
    })
}
fn checked(command: &mut Command, timeout: Duration, cancel: &AtomicBool) -> Result<Vec<u8>> {
    let out = run(command, timeout, cancel)?;
    ensure!(
        out.ok,
        "{}",
        if out.error.trim().is_empty() {
            format!("Source-control command exited with status {:?}", out.code)
        } else {
            out.error
        }
    );
    Ok(out.bytes)
}
fn git(root: &Path) -> Command {
    let mut c = Command::new("/usr/bin/git");
    crate::process_env::apply(&mut c);
    c.args([
        "--literal-pathspecs",
        "--no-pager",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "color.ui=false",
        "-c",
        "core.quotePath=false",
        "-C",
    ])
    .arg(root);
    for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
        c.env_remove(key);
    }
    c.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never");
    c
}
fn local(root: &Path, args: &[&str], cancel: &AtomicBool) -> Result<Vec<u8>> {
    checked(git(root).args(args), Duration::from_secs(12), cancel)
}
fn value(root: &Path, args: &[&str], cancel: &AtomicBool) -> Result<String> {
    Ok(String::from_utf8(local(root, args, cancel)?)?
        .trim_end_matches('\n')
        .into())
}
fn optional(root: &Path, args: &[&str], cancel: &AtomicBool) -> Option<String> {
    value(root, args, cancel).ok().filter(|v| !v.is_empty())
}
fn path_line(bytes: Vec<u8>) -> PathBuf {
    PathBuf::from(OsString::from_vec(
        bytes.strip_suffix(b"\n").unwrap_or(&bytes).to_vec(),
    ))
}
pub fn parse_remote(raw: &str) -> Option<HostedRepo> {
    let (host, path) = if let Some(v) = raw.strip_prefix("git@") {
        v.split_once(':')?
    } else if raw.starts_with("https://") || raw.starts_with("ssh://") {
        let rest = raw.split_once("://")?.1;
        let (authority, path) = rest.split_once('/')?;
        let authority = authority.rsplit('@').next()?;
        let host = authority.split(':').next()?;
        (host, path)
    } else {
        return None;
    };
    if !valid_host(host) {
        return None;
    }
    let path = path
        .trim_end_matches('/')
        .strip_suffix(".git")
        .unwrap_or(path.trim_end_matches('/'));
    let (owner, name) = path.split_once('/')?;
    if [owner, name].iter().any(|s| {
        s.is_empty()
            || s.len() > 128
            || !s
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }) || owner == "."
        || owner == ".."
        || name == "."
        || name == ".."
    {
        return None;
    }
    Some(HostedRepo {
        host: host.to_ascii_lowercase(),
        owner: owner.into(),
        name: name.into(),
    })
}
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() < 254
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
        && !host.starts_with('.')
        && !host.contains("..")
}
fn numstats(bytes: &[u8]) -> BTreeMap<PathBuf, Counts> {
    let mut map = BTreeMap::new();
    let mut fields = bytes.split(|b| *b == 0);
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        let mut parts = record.splitn(3, |b| *b == b'\t');
        let Some(a) = parts.next() else { continue };
        let Some(d) = parts.next() else { continue };
        let Some(mut path) = parts.next() else {
            continue;
        };
        if path.is_empty() {
            let _old = fields.next();
            path = fields.next().unwrap_or_default();
        }
        if !path.is_empty() {
            map.insert(
                PathBuf::from(OsString::from_vec(path.to_vec())),
                Counts {
                    added: std::str::from_utf8(a)
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                    removed: std::str::from_utf8(d)
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(0),
                    binary: a == b"-" || d == b"-",
                },
            );
        }
    }
    map
}
fn count_untracked(path: &Path, cancel: &AtomicBool) -> Result<Counts> {
    let meta = fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(Counts {
            added: 1,
            ..Default::default()
        });
    }
    if !meta.is_file() {
        return Ok(Counts {
            binary: true,
            ..Default::default()
        });
    }
    ensure!(
        meta.len() <= FILE_BYTES,
        "A changed file exceeds the 512 MB review limit"
    );
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let mut buf = [0; 64 * 1024];
    let mut lines = 0;
    let mut last = None;
    let mut binary = false;
    loop {
        ensure!(!cancel.load(Ordering::Acquire), "Review cancelled");
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        binary |= buf[..n].contains(&0);
        lines += buf[..n].iter().filter(|b| **b == b'\n').count() as u64;
        last = buf[..n].last().copied();
    }
    Ok(Counts {
        added: if binary {
            0
        } else {
            lines + u64::from(last.is_some_and(|b| b != b'\n'))
        },
        removed: 0,
        binary,
    })
}
pub fn discover(root: &Path, cancel: &AtomicBool) -> Result<Option<Checkout>> {
    discover_inner(root, cancel, true)
}
fn discover_inner(root: &Path, cancel: &AtomicBool, detailed: bool) -> Result<Option<Checkout>> {
    let root = root
        .canonicalize()
        .context("Project directory is missing")?;
    let probe = run(
        git(&root).args(["rev-parse", "--show-toplevel"]),
        Duration::from_secs(8),
        cancel,
    )?;
    if !probe.ok {
        if probe.error.contains("not a git repository") {
            return Ok(None);
        }
        bail!("{}", probe.error)
    }
    let repo = path_line(probe.bytes).canonicalize()?;
    let git_dir = path_line(local(&repo, &["rev-parse", "--absolute-git-dir"], cancel)?);
    let common = path_line(local(
        &repo,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        cancel,
    )?);
    let branch = optional(
        &repo,
        &["symbolic-ref", "--quiet", "--short", "HEAD"],
        cancel,
    );
    let head = optional(&repo, &["rev-parse", "--verify", "HEAD"], cancel);
    let upstream = optional(
        &repo,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
        cancel,
    );
    let mut ahead = 0;
    let mut behind = 0;
    if upstream.is_some() {
        if let Ok(pair) = value(
            &repo,
            &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"],
            cancel,
        ) {
            let mut fields = pair.split_whitespace();
            ahead = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            behind = fields.next().and_then(|v| v.parse().ok()).unwrap_or(0)
        }
    } else if head.is_some() {
        ahead = value(&repo, &["rev-list", "--count", "HEAD"], cancel)?
            .parse()
            .unwrap_or(0)
    }
    let remotes: Vec<_> = value(&repo, &["remote"], cancel)?
        .lines()
        .map(str::to_owned)
        .collect();
    let remote = branch
        .as_ref()
        .and_then(|b| {
            optional(
                &repo,
                &["config", "--get", &format!("branch.{b}.pushRemote")],
                cancel,
            )
        })
        .or_else(|| optional(&repo, &["config", "--get", "remote.pushDefault"], cancel))
        .or_else(|| {
            branch
                .as_ref()
                .and_then(|b| {
                    optional(
                        &repo,
                        &["config", "--get", &format!("branch.{b}.remote")],
                        cancel,
                    )
                })
                .filter(|v| v != ".")
        })
        .or_else(|| remotes.iter().find(|v| v.as_str() == "origin").cloned())
        .or_else(|| (remotes.len() == 1).then(|| remotes[0].clone()));
    let remote_url = remote
        .as_ref()
        .and_then(|r| optional(&repo, &["remote", "get-url", "--push", r], cancel));
    let push_hosted = remote_url.as_deref().and_then(parse_remote);
    let mut remote_repos = vec![];
    for name in &remotes {
        if let Some(url) = optional(&repo, &["remote", "get-url", name], cancel) {
            if let Some(hosted) = parse_remote(&url) {
                remote_repos.push((name.clone(), hosted));
            }
        }
    }
    let hosted = remote_repos
        .iter()
        .find(|(name, _)| name == "upstream")
        .map(|(_, r)| r.clone())
        .or_else(|| push_hosted.clone());
    let filters = crate::diff_view::filter_overrides(&repo, cancel).map_err(anyhow::Error::msg)?;
    let mut status = git(&repo);
    for filter in &filters {
        status.arg("-c").arg(filter);
    }
    let raw = checked(
        status.args(["status", "--porcelain=v1", "-z", "--untracked-files=all"]),
        Duration::from_secs(15),
        cancel,
    )?;
    let mut stats = if detailed {
        workspace_counts(&repo, &filters, head.is_some(), cancel)?
    } else {
        BTreeMap::new()
    };
    let mut fields = raw.split(|b| *b == 0);
    let mut files = Vec::new();
    let mut conflicted = false;
    while let Some(record) = fields.next() {
        if record.is_empty() {
            continue;
        }
        ensure!(record.len() >= 4, "Invalid Git status record");
        ensure!(
            files.len() < FILES,
            "More than 4096 changed files; narrow the checkout before syncing"
        );
        let code = &record[..2];
        let path = PathBuf::from(OsString::from_vec(record[3..].to_vec()));
        ensure!(
            path.components()
                .all(|p| matches!(p, std::path::Component::Normal(_))),
            "Invalid Git path"
        );
        let old_path = if code.contains(&b'R') || code.contains(&b'C') {
            Some(PathBuf::from(OsString::from_vec(
                fields.next().context("Incomplete rename record")?.to_vec(),
            )))
        } else {
            None
        };
        let conflict = code.contains(&b'U') || code == b"AA" || code == b"DD";
        conflicted |= conflict;
        let status = if conflict {
            "Conflict"
        } else if code == b"??" {
            "Untracked"
        } else if old_path.is_some() {
            "Renamed"
        } else if code.contains(&b'D') {
            "Deleted"
        } else if code.contains(&b'A') {
            "Added"
        } else {
            "Modified"
        };
        let counts = if !detailed {
            Counts::default()
        } else if code == b"??" || head.is_none() && repo.join(&path).exists() {
            count_untracked(&repo.join(&path), cancel)?
        } else {
            stats.remove(&path).unwrap_or_default()
        };
        files.push(FileChange {
            path,
            old_path,
            status: status.into(),
            counts,
        });
    }
    let refs = value(
        &repo,
        &[
            "for-each-ref",
            "--count=512",
            "--sort=-committerdate",
            "--format=%(refname:short)%09%(HEAD)%09%(upstream:short)%09%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
        cancel,
    )?;
    let branches = refs
        .lines()
        .filter_map(|line| {
            let f: Vec<_> = line.split('\t').collect();
            (f.len() == 4 && !f[0].ends_with("/HEAD")).then(|| Branch {
                name: f[0].into(),
                remote: f[3].starts_with("refs/remotes/"),
                current: f[1] == "*",
                upstream: f[2].into(),
            })
        })
        .collect();
    Ok(Some(Checkout {
        root: repo,
        git_dir,
        common_dir: common,
        branch,
        head,
        upstream,
        ahead,
        behind,
        remotes,
        remote,
        remote_url,
        hosted,
        push_hosted,
        remote_repos,
        branches,
        changes: Summary::from_files(files),
        conflicted,
    }))
}
fn fingerprint(repo: &Checkout, cancel: &AtomicBool) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(serde_json::to_vec(repo)?);
    let index = repo.git_dir.join("index");
    if let Ok(mut file) = File::open(index) {
        let mut buf = [0; 64 * 1024];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
    }
    use std::os::unix::fs::MetadataExt;
    let mut total = 0;
    for change in &repo.changes.files {
        ensure!(!cancel.load(Ordering::Acquire), "Review cancelled");
        let path = repo.root.join(&change.path);
        let Ok(meta) = fs::symlink_metadata(&path) else {
            hash.update(b"absent");
            continue;
        };
        hash.update(meta.mode().to_le_bytes());
        if meta.file_type().is_symlink() {
            hash.update(fs::read_link(path)?.as_os_str().as_bytes());
            continue;
        }
        if !meta.is_file() {
            hash.update(b"gitlink-or-directory");
            continue;
        }
        total += meta.len();
        ensure!(
            total <= FILE_BYTES,
            "Changed content exceeds the 512 MB review limit"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let mut buf = [0; 64 * 1024];
        loop {
            ensure!(!cancel.load(Ordering::Acquire), "Review cancelled");
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}
pub fn preview(root: &Path, cancel: &AtomicBool) -> Result<SyncPreview> {
    let checkout =
        discover(root, cancel)?.context("This folder does not have a Git repository yet")?;
    ensure!(
        !checkout.conflicted,
        "Resolve merge conflicts before committing"
    );
    ensure!(
        checkout.branch.is_some(),
        "Create or check out a branch before syncing detached HEAD"
    );
    let fingerprint = fingerprint(&checkout, cancel)?;
    Ok(SyncPreview {
        checkout,
        fingerprint,
    })
}
struct Lease(File);
impl Drop for Lease {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
fn lease(repo: &Checkout) -> Result<Lease> {
    let f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .open(repo.common_dir.join("kai-source-control.lock"))?;
    ensure!(
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "Another source-control action is running for this repository"
    );
    Ok(Lease(f))
}
struct Temp(PathBuf);
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}
fn reference(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() < 256
            && !value.starts_with('-')
            && !value.contains('\0')
            && !value.contains('\n'),
        "Invalid Git reference"
    );
    Ok(())
}
impl Client {
    pub fn discover() -> Self {
        let home = dirs::home_dir().unwrap_or_default();
        let gh = crate::process_env::dirs(&home)
            .into_iter()
            .map(|p| p.join("gh"))
            .find(|p| p.is_file());
        Self { gh }
    }
    fn trusted_host(&self, root: &Path, host: &str) -> Result<bool> {
        if host == "github.com" || host.ends_with(".ghe.com") {
            return Ok(true);
        }
        // A repository remote must not redirect an inherited enterprise token
        // to an arbitrary server. Other hosts require an existing gh login.
        type Hosts = std::collections::HashSet<String>;
        static HOSTS: std::sync::OnceLock<std::sync::Mutex<Option<(Instant, Hosts)>>> =
            std::sync::OnceLock::new();
        let cache = HOSTS.get_or_init(Default::default);
        if let Some((at, hosts)) = cache.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return Ok(hosts.contains(host));
            }
        }
        let mut cmd = Command::new(self.gh.as_ref().context("GitHub CLI is not installed")?);
        crate::process_env::apply(&mut cmd);
        cmd.current_dir(root)
            .args(["auth", "status", "--active", "--json", "hosts"])
            .env("GH_HOST", "github.com")
            .env("GH_PROMPT_DISABLED", "1")
            .env_remove("GH_DEBUG");
        for name in [
            "GH_TOKEN",
            "GITHUB_TOKEN",
            "GH_ENTERPRISE_TOKEN",
            "GITHUB_ENTERPRISE_TOKEN",
        ] {
            cmd.env_remove(name);
        }
        let output = run(&mut cmd, Duration::from_secs(15), &AtomicBool::new(false))?;
        let json: Value = serde_json::from_slice(&output.bytes)
            .context("GitHub CLI could not confirm its configured hosts")?;
        let hosts: Hosts = json["hosts"]
            .as_object()
            .map(|hosts| hosts.keys().cloned().collect())
            .unwrap_or_default();
        let trusted = hosts.contains(host);
        *cache.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), hosts));
        Ok(trusted)
    }
    fn command(&self, root: &Path, host: &str) -> Result<Command> {
        ensure!(valid_host(host), "Invalid GitHub hostname");
        ensure!(
            self.trusted_host(root, host)?,
            "Sign in to this GitHub Enterprise host with gh before connecting it"
        );
        let mut c = Command::new(
            self.gh
                .as_ref()
                .context("Install GitHub CLI to connect your existing GitHub account")?,
        );
        crate::process_env::apply(&mut c);
        c.current_dir(root)
            .env("GH_HOST", host)
            .env("GH_PROMPT_DISABLED", "1")
            .env("NO_COLOR", "1")
            .env("GH_PAGER", "")
            .env_remove("GH_DEBUG");
        Ok(c)
    }
    fn json(&self, root: &Path, host: &str, args: &[&str], cancel: &AtomicBool) -> Result<Value> {
        let bytes = checked(
            self.command(root, host)?.args(args),
            Duration::from_secs(30),
            cancel,
        )?;
        Ok(serde_json::from_slice(&bytes)?)
    }
    pub fn load(&self, root: &Path, cancel: &AtomicBool) -> Result<Snapshot> {
        let mut checkout = discover_inner(root, cancel, false)?;
        if let Some(checkout) = checkout.as_mut() {
            checkout.changes.files.clear();
        }
        let mut snapshot = Snapshot {
            checkout,
            pulls: Default::default(),
            gh_available: self.gh.is_some(),
            github_error: None,
        };
        if let Some(repo) = snapshot.checkout.as_ref().and_then(|r| r.hosted.as_ref()) {
            match self.pulls(root, repo, None, cancel) {
                Ok(page) => {
                    if let Some(parent) = page.parent.clone().filter(|parent| parent != repo) {
                        if let Some(checkout) = snapshot.checkout.as_mut() {
                            checkout.hosted = Some(parent.clone());
                        }
                        match self.pulls(root, &parent, None, cancel) {
                            Ok(parent_page) => snapshot.pulls = parent_page,
                            Err(error) => snapshot.github_error = Some(error.to_string()),
                        }
                    } else {
                        snapshot.pulls = page;
                    }
                }
                Err(error) => snapshot.github_error = Some(error.to_string()),
            }
        }
        Ok(snapshot)
    }
    pub fn pulls(
        &self,
        root: &Path,
        repo: &HostedRepo,
        cursor: Option<&str>,
        cancel: &AtomicBool,
    ) -> Result<PullPage> {
        let query = r#"query($owner:String!,$name:String!,$cursor:String){viewer{login} repository(owner:$owner,name:$name){parent{url} defaultBranchRef{name} viewerPermission isPrivate pullRequests(first:30,after:$cursor,states:OPEN,orderBy:{field:UPDATED_AT,direction:DESC}){totalCount pageInfo{hasNextPage endCursor} nodes{repository{name url owner{login}} number title url state isDraft headRefName baseRefName headRefOid additions deletions changedFiles reviewDecision mergeable author{login} commits(last:1){nodes{commit{statusCheckRollup{state}}}}}}}}"#;
        let mut args = vec![
            "api", "graphql", "-f", "query=", "-F", "owner=", "-F", "name=",
        ];
        let q = format!("query={query}");
        let o = format!("owner={}", repo.owner);
        let n = format!("name={}", repo.name);
        args[3] = &q;
        args[5] = &o;
        args[7] = &n;
        let cursor_arg = cursor.map(|v| format!("cursor={v}"));
        if let Some(value) = cursor_arg.as_ref() {
            args.extend(["-F", value]);
        }
        let data = self.json(root, &repo.host, &args, cancel)?;
        ensure!(
            data.get("errors").is_none(),
            "{}",
            redact(&data["errors"].to_string())
        );
        let r = &data["data"]["repository"];
        ensure!(
            r.is_object(),
            "Repository is not accessible with this GitHub account"
        );
        let page = &r["pullRequests"];
        let mut result = PullPage {
            login: text(&data["data"]["viewer"], "login"),
            parent: r["parent"]["url"].as_str().and_then(parse_remote),
            default_branch: text(&r["defaultBranchRef"], "name"),
            permission: text(r, "viewerPermission"),
            private: r["isPrivate"].as_bool().unwrap_or(false),
            total: page["totalCount"].as_u64().unwrap_or(0),
            ..Default::default()
        };
        if page["pageInfo"]["hasNextPage"] == true {
            result.cursor = page["pageInfo"]["endCursor"].as_str().map(str::to_owned)
        }
        if let Some(rows) = page["nodes"].as_array() {
            result.items = rows.iter().map(parse_pull).collect();
        }
        Ok(result)
    }
    pub fn mine(
        &self,
        root: &Path,
        host: &str,
        login: &str,
        cancel: &AtomicBool,
    ) -> Result<PullPage> {
        let login = if login.is_empty() {
            text(&self.json(root, host, &["api", "user"], cancel)?, "login")
        } else {
            login.to_owned()
        };
        ensure!(
            !login.is_empty()
                && login
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
            "No signed-in GitHub account"
        );
        let query = r#"query($search:String!){search(type:ISSUE,first:100,query:$search){issueCount nodes{... on PullRequest{repository{name url owner{login}} number title url state isDraft headRefName baseRefName headRefOid additions deletions changedFiles reviewDecision mergeable author{login} commits(last:1){nodes{commit{statusCheckRollup{state}}}}}}}}"#;
        let value = self.json(
            root,
            host,
            &[
                "api",
                "graphql",
                "-f",
                &format!("query={query}"),
                "-f",
                &format!("search=is:pr is:open author:{login} sort:updated-desc"),
            ],
            cancel,
        )?;
        ensure!(
            value.get("errors").is_none(),
            "GitHub could not load your pull requests"
        );
        let s = &value["data"]["search"];
        Ok(PullPage {
            login,
            total: s["issueCount"].as_u64().unwrap_or(0),
            items: s["nodes"]
                .as_array()
                .map(|rows| rows.iter().map(parse_pull).collect())
                .unwrap_or_default(),
            ..Default::default()
        })
    }
    pub fn sync(
        &self,
        preview: &SyncPreview,
        message: &str,
        push: bool,
        cancel: &AtomicBool,
    ) -> Result<SyncResult> {
        let expected = &preview.checkout;
        let _lease = lease(expected)?;
        let current = crate::github::preview(&expected.root, cancel)?;
        ensure!(
            current.fingerprint == preview.fingerprint,
            "Workspace, index, branch or remote changed. Refresh the review before syncing"
        );
        ensure!(
            !push || expected.behind == 0,
            "The branch is behind its upstream. Pull or reconcile it before pushing"
        );
        if push {
            ensure!(
                expected.remote.is_some(),
                "Add a remote or publish this repository before pushing"
            );
        }
        let mut committed = false;
        let mut warning = None;
        if !expected.changes.files.is_empty() {
            ensure!(
                !message.trim().is_empty() && message.len() <= 16 * 1024 && !message.contains('\0'),
                "Enter a commit message"
            );
            let guard_path = expected.git_dir.join("index.lock");
            let _guard_file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&guard_path)
                .context("Git's index is busy; let the other Git command finish")?;
            let _guard = Temp(guard_path.clone());
            let index = expected.git_dir.join("index");
            let temp = Temp(
                expected
                    .git_dir
                    .join(format!("kai-index-{}", uuid::Uuid::new_v4())),
            );
            if index.exists() {
                fs::copy(&index, &temp.0)?;
            } else {
                checked(
                    git(&expected.root)
                        .env("GIT_INDEX_FILE", &temp.0)
                        .args(["read-tree", "--empty"]),
                    Duration::from_secs(12),
                    cancel,
                )?;
            }
            checked(
                git(&expected.root)
                    .env("GIT_INDEX_FILE", &temp.0)
                    .args(["add", "--all", "--", "."]),
                Duration::from_secs(90),
                cancel,
            )?;
            let staged = crate::github::preview(&expected.root, cancel)?;
            ensure!(
                staged.fingerprint == preview.fingerprint,
                "Files changed while staging. Refresh the review; the real index was preserved"
            );
            let message_path = Temp(
                expected
                    .git_dir
                    .join(format!("kai-message-{}", uuid::Uuid::new_v4())),
            );
            let mut m = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&message_path.0)?;
            m.write_all(message.as_bytes())?;
            m.sync_all()?;
            let commit_result = checked(
                git(&expected.root)
                    .env("GIT_INDEX_FILE", &temp.0)
                    .args(["commit", "--file"])
                    .arg(&message_path.0),
                Duration::from_secs(120),
                cancel,
            );
            if let Err(error) = commit_result {
                let fresh = AtomicBool::new(false);
                let head = optional(&expected.root, &["rev-parse", "--verify", "HEAD"], &fresh);
                let tree = optional(&expected.root, &["rev-parse", "HEAD^{tree}"], &fresh);
                let staged = checked(
                    git(&expected.root)
                        .env("GIT_INDEX_FILE", &temp.0)
                        .arg("write-tree"),
                    Duration::from_secs(12),
                    &fresh,
                )
                .ok()
                .and_then(|v| String::from_utf8(v).ok())
                .map(|v| v.trim().to_owned());
                let branch = optional(&expected.root, &["symbolic-ref", "--short", "HEAD"], &fresh);
                if head.is_some()
                    && head != expected.head
                    && tree.is_some()
                    && tree == staged
                    && branch == expected.branch
                {
                    warning = Some(format!(
                        "The commit was saved; its Git hook or output reported: {error}. Review before pushing."
                    ));
                } else {
                    return Err(error);
                }
            }
            committed = true;
            fs::rename(&temp.0,&index).context("The commit was saved, but Git’s index could not be published; refresh before continuing")?;
        }
        let commit = value(&expected.root, &["rev-parse", "HEAD"], cancel)?;
        let mut result = SyncResult {
            commit,
            committed,
            pushed: false,
            push_error: None,
            warning,
        };
        if push && result.warning.is_none() {
            match self.push(expected, cancel) {
                Ok(()) => result.pushed = true,
                Err(e) => result.push_error = Some(e.to_string()),
            }
        }
        Ok(result)
    }
    fn credential_git(&self, root: &Path, hosted: Option<&HostedRepo>) -> Command {
        let mut c = git(root);
        if let (Some(gh), Some(repo)) = (&self.gh, hosted) {
            if !self.trusted_host(root, &repo.host).unwrap_or(false) {
                return c;
            }
            let path = gh.to_string_lossy().replace('\'', "'\\''");
            c.arg("-c")
                .arg(format!("credential.https://{}.helper=", repo.host));
            c.arg("-c").arg(format!(
                "credential.https://{}.helper=!'{}' auth git-credential",
                repo.host, path
            ));
        }
        c
    }
    fn push(&self, repo: &Checkout, cancel: &AtomicBool) -> Result<()> {
        let remote = repo.remote.as_deref().context("No push remote")?;
        reference(remote)?;
        let branch = repo
            .branch
            .as_deref()
            .context("Detached HEAD cannot be pushed")?;
        reference(branch)?;
        let upstream_merge = optional(
            &repo.root,
            &["config", "--get", &format!("branch.{branch}.merge")],
            cancel,
        )
        .filter(|v| v.starts_with("refs/heads/"));
        let branch_remote = optional(
            &repo.root,
            &["config", "--get", &format!("branch.{branch}.remote")],
            cancel,
        );
        let destination = upstream_merge
            .filter(|_| branch_remote.as_deref() == Some(remote))
            .unwrap_or_else(|| format!("refs/heads/{branch}"));
        let refspec = format!("HEAD:{destination}");
        checked(
            self.credential_git(&repo.root, repo.push_hosted.as_ref())
                .args(["push", "--set-upstream", remote, &refspec]),
            Duration::from_secs(90),
            cancel,
        )?;
        Ok(())
    }
    pub fn switch_branch(
        &self,
        root: &Path,
        name: &str,
        create: bool,
        cancel: &AtomicBool,
    ) -> Result<()> {
        reference(name)?;
        let repo = discover(root, cancel)?.context("Not a Git repository")?;
        let _lease = lease(&repo)?;
        local(&repo.root, &["check-ref-format", "--branch", name], cancel)?;
        let mut c = git(&repo.root);
        c.arg("switch");
        if create {
            c.arg("-c");
        } else if repo.branches.iter().any(|b| b.remote && b.name == name) {
            c.arg("--track");
        }
        c.arg(name);
        checked(&mut c, Duration::from_secs(30), cancel)?;
        Ok(())
    }
    pub fn fetch(&self, root: &Path, cancel: &AtomicBool) -> Result<()> {
        let repo = discover(root, cancel)?.context("Not a Git repository")?;
        let _lease = lease(&repo)?;
        let remote = repo.remote.as_deref().context("No remote to fetch")?;
        checked(
            self.credential_git(&repo.root, repo.push_hosted.as_ref())
                .args(["fetch", "--prune", "--", remote]),
            Duration::from_secs(90),
            cancel,
        )?;
        Ok(())
    }
    pub fn pull(&self, root: &Path, cancel: &AtomicBool) -> Result<()> {
        let repo = discover(root, cancel)?.context("Not a Git repository")?;
        let _lease = lease(&repo)?;
        ensure!(
            repo.changes.files.is_empty(),
            "Commit or stash workspace changes before pulling"
        );
        ensure!(
            repo.upstream.is_some(),
            "Push this branch to set its upstream first"
        );
        checked(
            self.credential_git(&repo.root, repo.push_hosted.as_ref())
                .args(["pull", "--ff-only"]),
            Duration::from_secs(90),
            cancel,
        )?;
        Ok(())
    }
    pub fn initialize(&self, root: &Path, cancel: &AtomicBool) -> Result<()> {
        ensure!(
            discover(root, cancel)?.is_none(),
            "This folder already belongs to a Git repository"
        );
        local(root, &["init", "--initial-branch=main"], cancel)?;
        Ok(())
    }
    pub fn publish(
        &self,
        root: &Path,
        name: &str,
        private: bool,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let hosted = parse_remote(&format!("https://github.com/{name}"));
        ensure!(
            hosted.is_some(),
            "Use owner/repository for the new GitHub repository"
        );
        if discover(root, cancel)?.is_none() {
            self.initialize(root, cancel)?;
        }
        let repo = discover(root, cancel)?.context("Repository was not initialized")?;
        let _lease = lease(&repo)?;
        ensure!(
            !repo.remotes.iter().any(|r| r == "origin"),
            "This checkout already has origin; it was not replaced"
        );
        checked(
            self.command(root, "github.com")?
                .args([
                    "repo",
                    "create",
                    name,
                    if private { "--private" } else { "--public" },
                    "--source",
                ])
                .arg(&repo.root)
                .args(["--remote", "origin"]),
            Duration::from_secs(45),
            cancel,
        )?;
        Ok(())
    }
    pub fn create_pull(
        &self,
        root: &Path,
        title: &str,
        body: &str,
        base: &str,
        draft: bool,
        cancel: &AtomicBool,
    ) -> Result<String> {
        ensure!(
            !title.trim().is_empty() && title.len() <= 512 && body.len() <= 64 * 1024,
            "Enter a PR title and a description under 64 KB"
        );
        reference(base)?;
        let mut repo = discover(root, cancel)?.context("Not a Git repository")?;
        let _lease = lease(&repo)?;
        // Resolve the same canonical base used by the PR hub, including forks
        // that do not have a separately configured upstream remote.
        if let Some(hosted) = repo.hosted.as_ref() {
            if let Some(parent) = self.pulls(root, hosted, None, cancel)?.parent {
                repo.hosted = Some(parent);
            }
        }
        let hosted = repo
            .hosted
            .as_ref()
            .context("The selected remote is not a GitHub repository")?;
        let head = repo
            .branch
            .as_deref()
            .context("Check out a branch before creating a PR")?;
        ensure!(
            head != base || repo.push_hosted.as_ref().is_some_and(|push| push != hosted),
            "Create a feature branch before opening a PR against this base"
        );
        ensure!(
            repo.upstream.is_some() && repo.ahead == 0,
            "Push this branch before creating its pull request"
        );
        let head = if let Some(push) = repo
            .push_hosted
            .as_ref()
            .filter(|push| push.owner != hosted.owner)
        {
            format!("{}:{head}", push.owner)
        } else {
            head.to_owned()
        };
        let mut cmd = self.command(root, &hosted.host)?;
        cmd.args([
            "pr",
            "create",
            "--repo",
            &hosted.slug(),
            "--head",
            &head,
            "--base",
            base,
            "--title",
            title,
            "--body",
            body,
        ]);
        if draft {
            cmd.arg("--draft");
        }
        Ok(
            String::from_utf8(checked(&mut cmd, Duration::from_secs(45), cancel)?)?
                .trim()
                .into(),
        )
    }
    pub fn pull_diff(
        &self,
        root: &Path,
        repo: &HostedRepo,
        number: u64,
        cancel: &AtomicBool,
    ) -> Result<String> {
        ensure!(number > 0, "Invalid pull request");
        Ok(String::from_utf8(checked(
            self.command(root, &repo.host)?.args([
                "pr",
                "diff",
                &number.to_string(),
                "--repo",
                &repo.slug(),
            ]),
            Duration::from_secs(30),
            cancel,
        )?)?)
    }
    pub fn checkout_pull(
        &self,
        root: &Path,
        repo: &HostedRepo,
        number: u64,
        cancel: &AtomicBool,
    ) -> Result<()> {
        ensure!(number > 0, "Invalid pull request");
        let checkout = discover(root, cancel)?.context("Not a Git repository")?;
        let _lease = lease(&checkout)?;
        let known = checkout.remote_repos.iter().any(|(_, r)| r == repo);
        let parent = if !known {
            if let Some(source) = checkout.push_hosted.as_ref() {
                self.pulls(root, source, None, cancel)?.parent
            } else {
                None
            }
        } else {
            None
        };
        ensure!(
            known || parent.as_ref() == Some(repo),
            "Open this pull request’s project before checking it out"
        );
        ensure!(
            checkout.changes.files.is_empty(),
            "Commit or stash changes before checking out a PR"
        );
        checked(
            self.command(root, &repo.host)?.args([
                "pr",
                "checkout",
                &number.to_string(),
                "--repo",
                &repo.slug(),
            ]),
            Duration::from_secs(90),
            cancel,
        )?;
        Ok(())
    }
    pub fn merge_pull(
        &self,
        root: &Path,
        repo: &HostedRepo,
        number: u64,
        oid: &str,
        cancel: &AtomicBool,
    ) -> Result<()> {
        ensure!(
            number > 0 && oid.len() == 40 && oid.bytes().all(|b| b.is_ascii_hexdigit()),
            "Refresh this pull request before merging"
        );
        checked(
            self.command(root, &repo.host)?.args([
                "pr",
                "merge",
                &number.to_string(),
                "--repo",
                &repo.slug(),
                "--squash",
                "--match-head-commit",
                oid,
            ]),
            Duration::from_secs(45),
            cancel,
        )?;
        Ok(())
    }
    pub fn account(&self, root: &Path, host: &str, cancel: &AtomicBool) -> Result<Account> {
        let query = r#"query{viewer{login url contributionsCollection{totalCommitContributions totalPullRequestContributions totalPullRequestReviewContributions totalIssueContributions contributionCalendar{totalContributions weeks{contributionDays{date contributionCount}}}} repositories(first:100,ownerAffiliations:[OWNER,COLLABORATOR,ORGANIZATION_MEMBER],orderBy:{field:PUSHED_AT,direction:DESC}){nodes{nameWithOwner url isPrivate}}}}"#;
        let data = self.json(
            root,
            host,
            &["api", "graphql", "-f", &format!("query={query}")],
            cancel,
        )?;
        ensure!(
            data.get("errors").is_none(),
            "GitHub could not load contributions for this account"
        );
        let v = &data["data"]["viewer"];
        let c = &v["contributionsCollection"];
        let mut result = Account {
            login: text(v, "login"),
            url: text(v, "url"),
            total: c["contributionCalendar"]["totalContributions"]
                .as_u64()
                .unwrap_or(0),
            commits: c["totalCommitContributions"].as_u64().unwrap_or(0),
            prs: c["totalPullRequestContributions"].as_u64().unwrap_or(0),
            reviews: c["totalPullRequestReviewContributions"]
                .as_u64()
                .unwrap_or(0),
            issues: c["totalIssueContributions"].as_u64().unwrap_or(0),
            ..Default::default()
        };
        if let Some(weeks) = c["contributionCalendar"]["weeks"].as_array() {
            for day in weeks
                .iter()
                .flat_map(|w| w["contributionDays"].as_array().into_iter().flatten())
            {
                result.days.push((
                    text(day, "date"),
                    day["contributionCount"].as_u64().unwrap_or(0),
                ));
            }
        }
        if let Some(repos) = v["repositories"]["nodes"].as_array() {
            result.repositories = repos
                .iter()
                .map(|r| {
                    (
                        text(r, "nameWithOwner"),
                        text(r, "url"),
                        r["isPrivate"] == true,
                    )
                })
                .collect();
        }
        Ok(result)
    }
    pub fn clone_repo(&self, url: &str, destination: &Path, cancel: &AtomicBool) -> Result<()> {
        let repo = parse_remote(url).context("Enter a GitHub HTTPS or SSH repository URL")?;
        ensure!(
            !destination.exists(),
            "Choose a new, empty destination folder"
        );
        ensure!(
            destination.is_absolute(),
            "Use an absolute destination folder"
        );
        let parent = destination.parent().context("Missing clone destination")?;
        ensure!(parent.is_dir(), "Choose an existing parent folder");
        checked(
            self.credential_git(parent, Some(&repo))
                .args(["clone", "--", url])
                .arg(destination),
            Duration::from_secs(120),
            cancel,
        )?;
        Ok(())
    }
    pub fn history(&self, root: &Path, cancel: &AtomicBool) -> Result<Vec<Commit>> {
        let bytes = local(
            root,
            &["log", "-100", "--format=%H%x00%s%x00%an%x00%aI%x00", "--"],
            cancel,
        )?;
        let fields: Vec<_> = bytes.split(|b| *b == 0).collect();
        Ok(fields
            .chunks(4)
            .filter(|f| f.len() == 4)
            .map(|f| Commit {
                oid: String::from_utf8_lossy(f[0]).trim().into(),
                subject: String::from_utf8_lossy(f[1]).into(),
                author: String::from_utf8_lossy(f[2]).into(),
                date: String::from_utf8_lossy(f[3]).into(),
            })
            .collect())
    }
    pub fn commit_diff(&self, root: &Path, oid: &str, cancel: &AtomicBool) -> Result<String> {
        ensure!(
            (40..=64).contains(&oid.len()) && oid.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid commit"
        );
        Ok(String::from_utf8(local(
            root,
            &[
                "show",
                "--format=",
                "--no-ext-diff",
                "--no-textconv",
                "--patch",
                oid,
                "--",
            ],
            cancel,
        )?)?)
    }
}
fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or("").into()
}
fn parse_pull(p: &Value) -> PullRequest {
    PullRequest {
        repository: p["repository"]["url"].as_str().and_then(parse_remote),
        number: p["number"].as_u64().unwrap_or(0),
        title: text(p, "title"),
        url: text(p, "url"),
        state: text(p, "state"),
        draft: p["isDraft"] == true,
        head: text(p, "headRefName"),
        base: text(p, "baseRefName"),
        head_oid: text(p, "headRefOid"),
        author: text(&p["author"], "login"),
        review: text(p, "reviewDecision"),
        checks: p["commits"]["nodes"]
            .as_array()
            .and_then(|v| v.last())
            .map(|v| text(&v["commit"]["statusCheckRollup"], "state"))
            .unwrap_or_default(),
        mergeable: text(p, "mergeable"),
        added: p["additions"].as_u64().unwrap_or(0),
        removed: p["deletions"].as_u64().unwrap_or(0),
        files: p["changedFiles"].as_u64().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    struct Repo {
        base: PathBuf,
        root: PathBuf,
        remote: PathBuf,
    }
    impl Drop for Repo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }
    impl Repo {
        fn new() -> Self {
            let base =
                std::env::temp_dir().join(format!("kai-github-test-{}", uuid::Uuid::new_v4()));
            let root = base.join("repo");
            let remote = base.join("remote.git");
            fs::create_dir_all(&root).unwrap();
            let r = Self { base, root, remote };
            r.cmd(&["init", "--initial-branch=main"]);
            r.cmd(&["config", "user.email", "kai-test@example.invalid"]);
            r.cmd(&["config", "user.name", "Kai Test"]);
            r.cmd(&["config", "commit.gpgsign", "false"]);
            r.cmd(&["config", "core.hooksPath", ".git/hooks"]);
            r.write("hello.rs", "fn old() {}\n");
            r.cmd(&["add", "."]);
            r.cmd(&["commit", "-m", "Initial"]);
            assert!(
                Command::new("/usr/bin/git")
                    .args(["init", "--bare"])
                    .arg(&r.remote)
                    .output()
                    .unwrap()
                    .status
                    .success()
            );
            r.cmd(&["remote", "add", "origin", r.remote.to_str().unwrap()]);
            r.cmd(&["push", "-u", "origin", "main"]);
            r
        }
        fn cmd(&self, args: &[&str]) -> String {
            let out = Command::new("/usr/bin/git")
                .arg("-C")
                .arg(&self.root)
                .args(args)
                .env_remove("GIT_INDEX_FILE")
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().into()
        }
        fn write(&self, name: &str, contents: &str) {
            let path = self.root.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, contents).unwrap();
        }
    }
    fn cancel() -> AtomicBool {
        AtomicBool::new(false)
    }
    #[test]
    fn remotes_and_credentials_are_normalized_without_shell_or_token_exposure() {
        for url in [
            "git@github.com:octo/repo.git",
            "https://github.com/octo/repo.git",
            "ssh://git@github.com/octo/repo.git",
        ] {
            assert_eq!(parse_remote(url).unwrap().slug(), "octo/repo");
        }
        for url in [
            "-u bad",
            "https://evil.example/../repo",
            "https://github.com/owner/repo/extra",
            "https://bad..host/owner/repo",
        ] {
            assert!(parse_remote(url).is_none());
        }
        let s = redact(
            "remote: ghp_abc123secret https://user:password@github.com/repo github_pat_topsecret",
        );
        assert!(!s.contains("password") && !s.contains("topsecret") && !s.contains("abc123"));
        let stats = numstats(
            b"1\t2\tfile with space.rs\0-\t-\tbinary.png\04\t5\t\0old name.rs\0new name.rs\0",
        );
        assert_eq!(stats[Path::new("new name.rs")].added, 4);
        assert!(stats[Path::new("binary.png")].binary);
    }
    #[test]
    fn discovery_counts_staged_unstaged_untracked_deleted_renamed_and_ignored() {
        let r = Repo::new();
        r.write(".gitignore", "ignored/\n");
        r.cmd(&["add", ".gitignore"]);
        r.cmd(&["commit", "-m", "Ignore"]);
        r.write("hello.rs", "fn changed() {}\nfn second() {}\n");
        r.cmd(&["add", "hello.rs"]);
        r.write(
            "hello.rs",
            "fn changed() {}\nfn second() {}\nfn third() {}\n",
        );
        r.write("new folder/a $(touch nope).txt", "one\ntwo");
        r.write("ignored/secret", "ignored");
        let c = discover(&r.root.join("new folder"), &cancel())
            .unwrap()
            .unwrap();
        assert_eq!(c.root, r.root.canonicalize().unwrap());
        assert_eq!(c.branch.as_deref(), Some("main"));
        assert_eq!(c.changes.total, 2);
        assert_eq!(c.changes.added, 5);
        assert_eq!(c.changes.removed, 1);
        assert!(!r.root.join("nope").exists());
    }
    #[test]
    fn stale_review_does_not_stage_or_commit_anything() {
        let r = Repo::new();
        r.write("hello.rs", "one\n");
        r.cmd(&["add", "hello.rs"]);
        let before = r.cmd(&["diff", "--cached"]);
        let head = r.cmd(&["rev-parse", "HEAD"]);
        let preview = preview(&r.root, &cancel()).unwrap();
        r.write("hello.rs", "newer\n");
        assert!(
            Client { gh: None }
                .sync(&preview, "Should not commit", true, &cancel())
                .unwrap_err()
                .to_string()
                .contains("changed")
        );
        assert_eq!(r.cmd(&["diff", "--cached"]), before);
        assert_eq!(r.cmd(&["rev-parse", "HEAD"]), head);
    }
    #[test]
    fn reviewed_sync_commits_all_and_pushes_then_pushes_without_empty_commit() {
        let r = Repo::new();
        r.write("hello.rs", "fn new() {}\n");
        r.write("new file.txt", "hello\n");
        let preview = preview(&r.root, &cancel()).unwrap();
        let client = Client { gh: None };
        let result = client
            .sync(&preview, "A reviewed commit", true, &cancel())
            .unwrap();
        assert!(result.committed && result.pushed && result.push_error.is_none());
        assert!(r.cmd(&["status", "--porcelain"]).is_empty());
        assert_eq!(
            r.cmd(&["rev-parse", "HEAD"]),
            r.cmd(&["rev-parse", "origin/main"])
        );
        let preview = crate::github::preview(&r.root, &cancel()).unwrap();
        let second = client.sync(&preview, "", true, &cancel()).unwrap();
        assert!(!second.committed && second.pushed);
        assert_eq!(result.commit, second.commit);
    }
    #[test]
    fn linked_worktree_sync_does_not_touch_parent_index_or_branch() {
        let r = Repo::new();
        let work = r.base.join("worktree");
        r.cmd(&["worktree", "add", "-b", "feature", work.to_str().unwrap()]);
        fs::write(work.join("hello.rs"), "feature\n").unwrap();
        r.write("hello.rs", "parent edit\n");
        r.cmd(&["add", "hello.rs"]);
        let index = r.cmd(&["diff", "--cached"]);
        let review = preview(&work, &cancel()).unwrap();
        assert_ne!(review.checkout.git_dir, review.checkout.common_dir);
        let result = Client { gh: None }
            .sync(&review, "Worktree feature", true, &cancel())
            .unwrap();
        assert!(result.pushed, "{:?}", result.push_error);
        assert_eq!(r.cmd(&["branch", "--show-current"]), "main");
        assert_eq!(r.cmd(&["diff", "--cached"]), index);
        assert_eq!(
            fs::read_to_string(r.root.join("hello.rs")).unwrap(),
            "parent edit\n"
        );
    }
    #[test]
    fn failed_push_keeps_commit_and_retry_does_not_duplicate_it() {
        let r = Repo::new();
        let hook = r.remote.join("hooks/pre-receive");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        r.write("hello.rs", "new\n");
        let client = Client { gh: None };
        let result = client
            .sync(
                &preview(&r.root, &cancel()).unwrap(),
                "Keep local commit",
                true,
                &cancel(),
            )
            .unwrap();
        assert!(result.committed && !result.pushed && result.push_error.is_some());
        fs::remove_file(hook).unwrap();
        let retry = client
            .sync(&preview(&r.root, &cancel()).unwrap(), "", true, &cancel())
            .unwrap();
        assert!(!retry.committed && retry.pushed);
        assert_eq!(result.commit, retry.commit);
    }
    #[test]
    fn failed_commit_preserves_real_index_and_lease_rejects_concurrent_mutations() {
        let r = Repo::new();
        r.write("hello.rs", "first\n");
        r.cmd(&["add", "hello.rs"]);
        r.write("hello.rs", "second\n");
        let before = r.cmd(&["diff", "--cached"]);
        let head = r.cmd(&["rev-parse", "HEAD"]);
        let hook = r.root.join(".git/hooks/pre-commit");
        fs::write(&hook, "#!/bin/sh\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(hook, fs::Permissions::from_mode(0o700)).unwrap();
        let review = preview(&r.root, &cancel()).unwrap();
        assert!(
            Client { gh: None }
                .sync(&review, "Rejected by hook", false, &cancel())
                .is_err()
        );
        assert_eq!(r.cmd(&["diff", "--cached"]), before);
        assert_eq!(r.cmd(&["rev-parse", "HEAD"]), head);
        assert!(!r.root.join(".git/index.lock").exists());
        let _lease = lease(&review.checkout).unwrap();
        assert!(lease(&review.checkout).is_err());
    }
    #[test]
    fn interrupted_post_commit_publishes_index_and_retry_pushes_saved_commit() {
        use std::os::unix::fs::PermissionsExt;
        let r = Repo::new();
        r.write("hello.rs", "new code\n");
        let before = r.cmd(&["rev-parse", "HEAD"]);
        let hook = r.root.join(".git/hooks/post-commit");
        fs::write(&hook, "#!/bin/sh\nkill -TERM \"$PPID\"\n").unwrap();
        fs::set_permissions(&hook, fs::Permissions::from_mode(0o700)).unwrap();
        let client = Client { gh: None };
        let saved = client
            .sync(
                &preview(&r.root, &cancel()).unwrap(),
                "Saved before hook stopped",
                true,
                &cancel(),
            )
            .unwrap();
        assert!(saved.committed && !saved.pushed && saved.warning.is_some());
        assert_ne!(r.cmd(&["rev-parse", "HEAD"]), before);
        assert!(r.cmd(&["status", "--porcelain"]).is_empty());
        assert!(!r.root.join(".git/index.lock").exists());
        fs::remove_file(hook).unwrap();
        let retry = client
            .sync(&preview(&r.root, &cancel()).unwrap(), "", true, &cancel())
            .unwrap();
        assert!(!retry.committed && retry.pushed);
        assert_eq!(saved.commit, retry.commit);
    }
    #[test]
    fn fork_pr_uses_canonical_parent_and_owner_qualified_head() {
        use std::os::unix::fs::PermissionsExt;
        let r = Repo::new();
        r.cmd(&[
            "remote",
            "set-url",
            "origin",
            "https://github.com/fork/project.git",
        ]);
        let gh = r.base.join("gh");
        let log = r.base.join("pr-args");
        let response = json!({"data":{"viewer":{"login":"fork"},"repository":{"parent":{"url":"https://github.com/upstream/project"},"defaultBranchRef":{"name":"main"},"pullRequests":{"totalCount":0,"nodes":[],"pageInfo":{"hasNextPage":false}}}}});
        fs::write(&gh, format!("#!/bin/sh\nif [ \"$1\" = api ]; then\nprintf '%s' '{}'\nelse\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s' 'https://github.com/upstream/project/pull/1'\nfi\n", response, log.display())).unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
        let client = Client { gh: Some(gh) };
        let url = client
            .create_pull(&r.root, "Native PR", "Body", "main", true, &cancel())
            .unwrap();
        assert!(url.ends_with("/pull/1"));
        let args = fs::read_to_string(log).unwrap();
        assert!(args.contains("--repo\nupstream/project\n"), "{args}");
        assert!(args.contains("--head\nfork:main\n"), "{args}");
        assert!(args.contains("--draft"));
        let unrelated = HostedRepo {
            host: "github.com".into(),
            owner: "other".into(),
            name: "unrelated".into(),
        };
        assert!(
            client
                .checkout_pull(&r.root, &unrelated, 1, &cancel())
                .unwrap_err()
                .to_string()
                .contains("project")
        );
    }
    #[test]
    fn unborn_repository_sync_and_changed_mode_review_are_supported() {
        use std::os::unix::fs::PermissionsExt;
        let r = Repo::new();
        r.write("script.sh", "#!/bin/sh\nexit 0\n");
        fs::set_permissions(r.root.join("script.sh"), fs::Permissions::from_mode(0o600)).unwrap();
        let reviewed = preview(&r.root, &cancel()).unwrap();
        fs::set_permissions(r.root.join("script.sh"), fs::Permissions::from_mode(0o700)).unwrap();
        assert!(
            Client { gh: None }
                .sync(&reviewed, "Stale mode", false, &cancel())
                .is_err()
        );
        let unborn = r.base.join("new-repo");
        fs::create_dir(&unborn).unwrap();
        let status = Command::new("/usr/bin/git")
            .arg("-C")
            .arg(&unborn)
            .args(["init", "--initial-branch=main"])
            .output()
            .unwrap();
        assert!(status.status.success());
        for (key, value) in [
            ("user.name", "Kai Test"),
            ("user.email", "kai@example.invalid"),
            ("commit.gpgsign", "false"),
        ] {
            assert!(
                Command::new("/usr/bin/git")
                    .arg("-C")
                    .arg(&unborn)
                    .args(["config", key, value])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        fs::write(unborn.join("first.txt"), "First commit\n").unwrap();
        let saved = Client { gh: None }
            .sync(
                &preview(&unborn, &cancel()).unwrap(),
                "Initial workspace",
                false,
                &cancel(),
            )
            .unwrap();
        assert!(saved.committed && !saved.commit.is_empty());
        assert!(
            discover(&unborn, &cancel())
                .unwrap()
                .unwrap()
                .changes
                .files
                .is_empty()
        );
    }
    #[test]
    fn github_statuses_use_actual_checks_reviews_and_merge_state() {
        let p = parse_pull(
            &json!({"number":5,"title":"Fix","state":"OPEN","headRefOid":"ab","reviewDecision":"APPROVED","commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"FAILURE"}}}]}}),
        );
        assert_eq!(p.glyph(), ("circle-x", 0xff8e91));
        let mut merged = p;
        merged.state = "MERGED".into();
        assert_eq!(merged.glyph().0, "git-merge");
    }
}

pub fn patch_counts(patch: &str) -> Counts {
    let mut counts = Counts::default();
    for line in patch.lines() {
        if line.starts_with("Binary files ") || line == "GIT binary patch" {
            counts.binary = true;
        } else if line.starts_with('+') && !line.starts_with("+++") {
            counts.added += 1
        } else if line.starts_with('-') && !line.starts_with("---") {
            counts.removed += 1
        }
    }
    counts
}
pub fn untracked_counts(path: &Path, cancel: &AtomicBool) -> Result<Counts> {
    count_untracked(path, cancel)
}
pub fn workspace_counts(
    root: &Path,
    filters: &[String],
    head: bool,
    cancel: &AtomicBool,
) -> Result<BTreeMap<PathBuf, Counts>> {
    let mut cmd = git(root);
    for filter in filters {
        cmd.arg("-c").arg(filter);
    }
    cmd.args(["diff", "--no-ext-diff", "--no-textconv", "--numstat", "-z"]);
    if head {
        cmd.arg("HEAD");
    }
    cmd.arg("--");
    Ok(numstats(&checked(
        &mut cmd,
        Duration::from_secs(15),
        cancel,
    )?))
}

#[cfg(test)]
mod live_reads {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    #[ignore = "Explicit read-only validation of the installed gh login; never creates or changes GitHub resources"]
    fn github_live_account_and_repository_query() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let client = Client::discover();
        assert!(client.gh.is_some(), "GitHub CLI is not installed");
        let snapshot = client.load(&root, &AtomicBool::new(false)).unwrap();
        let repo = snapshot.checkout.as_ref().unwrap().hosted.as_ref().unwrap();
        eprintln!(
            "GitHub read: {} · {} open pull requests · default {} · account {}",
            repo.slug(),
            snapshot.pulls.total,
            snapshot.pulls.default_branch,
            snapshot.pulls.login
        );
        assert!(
            snapshot.github_error.is_none(),
            "{:?}",
            snapshot.github_error
        );
        let personal = client
            .mine(
                &root,
                &repo.host,
                &snapshot.pulls.login,
                &AtomicBool::new(false),
            )
            .unwrap();
        eprintln!("GitHub personal pull requests: {}", personal.total);
        let account = client
            .account(&root, &repo.host, &AtomicBool::new(false))
            .unwrap();
        assert!(!account.login.is_empty());
        assert!(account.days.len() > 300);
        eprintln!(
            "GitHub contributions: {} · {} days · {} repositories",
            account.total,
            account.days.len(),
            account.repositories.len()
        );
    }
}

#[cfg(test)]
mod robustness_tests {
    use super::*;
    use std::prelude::v1::test;
    #[test]
    fn github_read_processes_cancel_timeout_and_bound_output() {
        let cancel = AtomicBool::new(true);
        assert!(
            run(
                Command::new("/bin/sleep").arg("10"),
                Duration::from_secs(1),
                &cancel
            )
            .is_err()
        );
        let at = Instant::now();
        assert!(
            run(
                Command::new("/bin/sleep").arg("10"),
                Duration::from_millis(40),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .to_string()
            .contains("timed out")
        );
        assert!(at.elapsed() < Duration::from_secs(2));
        assert!(
            run(
                Command::new("/usr/bin/yes").arg("bounded-output"),
                Duration::from_secs(5),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .to_string()
            .contains("limit")
        );
    }
}
