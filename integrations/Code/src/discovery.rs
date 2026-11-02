//! Bounded discovery of installed agents, recent sessions, and local projects.
//! Only session metadata and small transcript samples are read; credentials are never opened.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, File},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};
use walkdir::WalkDir;

#[path = "discovery_agents.rs"]
mod other_agents;

const MAX_SESSIONS: usize = 2_400;
const MAX_SESSIONS_PER_PROJECT: usize = 100;
const MAX_LOG_SAMPLES: usize = 1_200;
const MAX_SESSION_ENTRIES: usize = 16_000;
const SAMPLE_BYTES: usize = 64 * 1024;
const MAX_PROJECT_ENTRIES: usize = 2_000;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum AgentKind {
    Ninja,
    Codex,
    Claude,
    Gemini,
    OpenCode,
    Antigravity,
    Cursor,
    Grok,
}

impl AgentKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ninja => "Ninja",
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::Gemini => "Gemini CLI",
            Self::OpenCode => "OpenCode",
            Self::Antigravity => "Antigravity",
            Self::Cursor => "Cursor",
            Self::Grok => "Grok Build",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            Self::Ninja => "ninja",
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::Gemini => "gemini",
            Self::OpenCode => "opencode",
            Self::Antigravity => "agy",
            Self::Cursor => "cursor-agent",
            Self::Grok => "grok",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Agent {
    pub kind: AgentKind,
    pub name: String,
    pub executable: Option<PathBuf>,
    pub available: bool,
}

#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub agent: AgentKind,
    pub project_path: PathBuf,
    /// Original CLI working directory; project_path is the navigation group.
    #[serde(default)]
    pub execution_path: Option<PathBuf>,
    pub updated_at: i64,
    pub external_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub agents: Vec<Agent>,
    pub projects: Vec<Project>,
    pub sessions: Vec<SessionSummary>,
}

/// Resolve a provider without waiting for saved-history/project discovery.
pub fn discover_agent(kind: AgentKind, home: &Path) -> Agent {
    let executable = if kind==AgentKind::Ninja {crate::ninja::executable(crate::ninja::Harness::Codex,home).or_else(||crate::ninja::executable(crate::ninja::Harness::Claude,home))} else {find_executable(kind, home)};
    Agent { kind, name:kind.name().to_owned(), available:executable.is_some(), executable }
}

pub fn discover(home: &Path, extra_root: &Path) -> Discovery {
    let agents = [
        AgentKind::Ninja,
        AgentKind::Codex,
        AgentKind::Claude,
        AgentKind::Gemini,
        AgentKind::OpenCode,
        AgentKind::Antigravity,
        AgentKind::Cursor,
        AgentKind::Grok,
    ]
    .into_iter()
    .map(|kind| discover_agent(kind, home))
    .collect();

    let metadata = native_metadata(home);
    let (mut sessions, _) = native_sessions(home, &metadata, None);
    let mut seen_sessions: HashSet<_> = sessions.iter().map(|session| session.id.clone()).collect();
    let known_paths: HashSet<_> = sessions
        .iter()
        .filter_map(|session| session.transcript_path.clone())
        .collect();
    let roots = [
        (home.join(".codex/sessions"), AgentKind::Codex),
        (home.join(".codex/archived_sessions"), AgentKind::Codex),
        (home.join(".claude/projects"), AgentKind::Claude),
    ];
    let mut files = Vec::new();
    let mut inspected = 0;
    for (root, kind) in roots {
        for entry in WalkDir::new(root)
            .follow_links(false)
            .max_depth(8)
            .into_iter()
        {
            inspected += 1;
            if inspected > MAX_SESSION_ENTRIES {
                break;
            }
            let Ok(entry) = entry else { continue };
            if !entry.file_type().is_file()
                || entry.path().extension().is_none_or(|ext| ext != "jsonl")
            {
                continue;
            }
            if known_paths.contains(entry.path()) {
                continue;
            }
            // Claude subagent logs are not independently resumable conversations.
            if entry
                .path()
                .components()
                .any(|part| part.as_os_str() == "subagents")
            {
                continue;
            }
            let modified = entry
                .metadata()
                .ok()
                .and_then(|meta| meta.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            files.push((modified, entry.into_path(), kind));
        }
        if inspected > MAX_SESSION_ENTRIES {
            break;
        }
    }
    files.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    let imported = files
        .into_iter()
        .take(MAX_LOG_SAMPLES)
        .filter_map(|(modified, path, kind)| read_session(&path, kind, modified))
        .filter(|session| seen_sessions.insert(session.id.clone()))
        .collect::<Vec<_>>();
    sessions.extend(imported);
    sessions.extend(other_agents::sessions(home, None));
    for session in &mut sessions {
        apply_project_metadata(session, &metadata);
    }
    sessions.sort_unstable_by(|a, b| b.updated_at.cmp(&a.updated_at));
    // Keep summaries from quiet workspaces even if one busy agent produces
    // hundreds of recent conversations in another project.
    let mut project_counts = HashMap::new();
    sessions.retain(|session| {
        if !usable_project(&session.project_path) {
            return false;
        }
        let count = project_counts
            .entry(session.project_path.clone())
            .or_insert(0usize);
        *count += 1;
        *count <= MAX_SESSIONS_PER_PROJECT
    });
    sessions.truncate(MAX_SESSIONS);

    let mut project_paths: HashSet<_> = metadata
        .labels
        .keys()
        .filter(|path| usable_project(path))
        .cloned()
        .collect();
    for session in &sessions {
        if usable_project(&session.project_path) {
            project_paths.insert(normalize_path(&session.project_path));
        }
    }
    if usable_project(extra_root) {
        project_paths.insert(normalize_path(extra_root));
    }
    scan_projects(home, extra_root, &mut project_paths);
    let mut projects: Vec<_> = project_paths
        .into_iter()
        .map(|path| Project {
            name: metadata.labels.get(&path).cloned().unwrap_or_else(|| {
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            }),
            path,
        })
        .collect();
    projects.sort_by_cached_key(|project| (project.name.to_lowercase(), project.path.clone()));
    Discovery {
        agents,
        projects,
        sessions,
    }
}

/// Index a selected workspace independently of the global recent-session cap.
/// Native database reads are read-only; transcript bodies remain lazy.
pub fn discover_project(home: &Path, project: &Path) -> Vec<SessionSummary> {
    let project = normalize_path(project);
    if !usable_project(&project) {
        return Vec::new();
    }
    let metadata = native_metadata(home);
    let (mut sessions, indexed) = native_sessions(home, &metadata, Some(&project));
    sessions.extend(other_agents::sessions(home, Some(&project)));
    // Claude's per-workspace directory allows older quiet projects to load
    // directly without competing with every other project's recent logs.
    // Native storage replaces every non-alphanumeric UTF-16 unit, not just
    // slashes; long names use a 200-character prefix plus a native hash.
    // Reference: code.claude.com/docs/en/sessions#where-transcripts-are-stored.
    let encoded: String = project
        .to_string_lossy()
        .encode_utf16()
        .map(|unit| {
            if unit <= 127 && (unit as u8).is_ascii_alphanumeric() {
                char::from(unit as u8)
            } else {
                '-'
            }
        })
        .collect();
    let lookup = &encoded[..encoded.len().min(200)];
    let claude_root = home.join(".claude/projects");
    let prefix = format!("{lookup}-");
    let mut roots = if encoded.len() <= 200 {
        vec![claude_root.join(&encoded)]
    } else {
        Vec::new()
    };
    if let Ok(entries) = fs::read_dir(&claude_root) {
        roots.extend(
            entries
                .take(MAX_PROJECT_ENTRIES)
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
                .map(|entry| entry.path()),
        );
    }
    let mut claude_logs: Vec<_> = roots
        .into_iter()
        .flat_map(|root| {
            WalkDir::new(root)
                .follow_links(false)
                .max_depth(1)
                .into_iter()
        })
        .take(2_000)
        .filter_map(Result::ok)
        .filter(|entry| {
            entry.file_type().is_file()
                && entry
                    .path()
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
        })
        .map(|entry| {
            let modified = entry
                .metadata()
                .ok()
                .and_then(|metadata| metadata.modified().ok())
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);
            (modified, entry.into_path())
        })
        .collect();
    claude_logs.sort_unstable_by(|a, b| b.0.cmp(&a.0));
    sessions.extend(
        claude_logs
            .into_iter()
            .take(1_000)
            .filter_map(|(modified, path)| read_session(&path, AgentKind::Claude, modified))
            .filter(|session| session.project_path.starts_with(&project)),
    );
    // An indexed Codex installation answers this workspace directly. Only
    // files-only installations need to sample the date-organized Codex logs;
    // avoid rerunning global agent discovery and repository scans on every click.
    if !indexed {
        let mut files = Vec::new();
        let mut inspected = 0;
        for directory in [".codex/sessions", ".codex/archived_sessions"] {
            for entry in WalkDir::new(home.join(directory))
                .follow_links(false)
                .max_depth(8)
                .into_iter()
                .take(MAX_SESSION_ENTRIES - inspected)
                .inspect(|_| inspected += 1)
                .filter_map(Result::ok)
            {
                if !entry.file_type().is_file()
                    || entry.path().extension().is_none_or(|ext| ext != "jsonl")
                {
                    continue;
                }
                let modified = entry
                    .metadata()
                    .ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                files.push((modified, entry.into_path()));
            }
        }
        files.sort_unstable_by(|a, b| b.0.cmp(&a.0));
        for (modified, path) in files.into_iter().take(MAX_LOG_SAMPLES) {
            if let Some(mut session) = read_session(&path, AgentKind::Codex, modified) {
                apply_project_metadata(&mut session, &metadata);
                if session.project_path.starts_with(&project) {
                    sessions.push(session);
                }
            }
        }
    }
    let mut seen = HashSet::new();
    sessions.retain(|session| seen.insert(session.id.clone()));
    sessions.sort_unstable_by(|a, b| b.updated_at.cmp(&a.updated_at));
    sessions.truncate(1_000);
    sessions
}

#[derive(Default)]
struct NativeMetadata {
    labels: HashMap<PathBuf, String>,
    thread_roots: HashMap<String, PathBuf>,
    canonical_paths: RefCell<HashMap<PathBuf, PathBuf>>,
    unprojected_root: Option<PathBuf>,
}
impl NativeMetadata {
    fn normalize(&self, path: &Path) -> PathBuf {
        if let Some(normalized) = self.canonical_paths.borrow().get(path) {
            return normalized.clone();
        }
        let normalized = normalize_path(path);
        self.canonical_paths
            .borrow_mut()
            .insert(path.into(), normalized.clone());
        normalized
    }
}

fn usable_project(path: &Path) -> bool {
    path.is_absolute() && path.parent().is_some() && path.is_dir()
}

fn native_metadata(home: &Path) -> NativeMetadata {
    let mut result = NativeMetadata {
        unprojected_root: Some(unprojected_root(&normalize_path(home))),
        ..Default::default()
    };
    let path = home.join(".codex/.codex-global-state.json");
    let Ok(file) = File::open(path) else {
        return result;
    };
    // Extract only known navigation fields from this UI metadata file.
    let Ok(value) = serde_json::from_reader::<_, Value>(file.take(2 * 1024 * 1024)) else {
        return result;
    };
    let mut projects = HashMap::new();
    if let Some(local_projects) = value.get("local-projects").and_then(Value::as_object) {
        for (id, project) in local_projects.iter().take(1_000) {
            if let Some(roots) = project.get("rootPaths").and_then(Value::as_array) {
                for root in roots.iter().filter_map(Value::as_str).take(20) {
                    let path = normalize_path(Path::new(root));
                    if !usable_project(&path) {
                        continue;
                    }
                    let name = project
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            path.file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("Project")
                        });
                    result.labels.insert(path.clone(), name.to_owned());
                    projects.entry(id.to_owned()).or_insert(path);
                }
            }
        }
    }
    for key in ["electron-saved-workspace-roots", "active-workspace-roots"] {
        if let Some(roots) = value.get(key).and_then(Value::as_array) {
            for root in roots.iter().filter_map(Value::as_str).take(1_000) {
                let path = normalize_path(Path::new(root));
                if usable_project(&path) {
                    result.labels.entry(path.clone()).or_insert_with(|| {
                        path.file_name().unwrap().to_string_lossy().into_owned()
                    });
                }
            }
        }
    }
    if let Some(labels) = value
        .get("electron-workspace-root-labels")
        .and_then(Value::as_object)
    {
        for (root, label) in labels.iter().take(1_000) {
            let path = normalize_path(Path::new(root));
            if usable_project(&path)
                && let Some(label) = label.as_str()
            {
                result.labels.insert(path, label.to_owned());
            }
        }
    }
    if let Some(hints) = value
        .get("thread-workspace-root-hints")
        .and_then(Value::as_object)
    {
        for (thread, root) in hints.iter().take(20_000) {
            if let Some(root) = root.as_str() {
                result
                    .thread_roots
                    .insert(thread.clone(), normalize_path(Path::new(root)));
            }
        }
    }
    if let Some(assignments) = value
        .get("thread-project-assignments")
        .and_then(Value::as_object)
    {
        for (thread, assignment) in assignments.iter().take(20_000) {
            let id = assignment
                .as_str()
                .or_else(|| assignment.get("projectId").and_then(Value::as_str));
            if let Some(root) = id.and_then(|id| projects.get(id)) {
                result.thread_roots.insert(thread.clone(), root.clone());
            }
        }
    }
    result
}

fn apply_project_metadata(session: &mut SessionSummary, metadata: &NativeMetadata) {
    if session.execution_path.is_none() {
        session.execution_path = Some(session.project_path.clone());
    }
    if session.agent == AgentKind::Codex
        && let Some(root) = session
            .external_id
            .as_ref()
            .and_then(|id| metadata.thread_roots.get(id))
    {
        session.project_path = root.clone();
    }
    session.project_path = metadata.normalize(&session.project_path);
    if let Some(root) = &metadata.unprojected_root
        && session.project_path.parent() == Some(root.as_path())
        && session
            .project_path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| uuid::Uuid::parse_str(name).is_ok())
    {
        session.project_path = root.clone();
    }
}

/// Navigation group for standalone chats; each CLI runs in a UUID child.
pub fn unprojected_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Orbit/Chats")
}

fn codex_databases(home: &Path) -> Vec<PathBuf> {
    codex_databases_in(&home.join(".codex"))
}
fn codex_databases_in(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut databases: Vec<_> = entries
        .take(MAX_PROJECT_ENTRIES)
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            name.starts_with("state_") && name.ends_with(".sqlite")
        })
        .collect();
    databases.sort();
    databases.into_iter().rev().take(3).collect()
}

/// Worker-only, indexed native metadata lookup. Never opens rollout bodies or
/// modifies Codex's database; an early SessionId event may precede its DB row.
pub fn codex_transcript_path(home: &Path, id: &str) -> Option<PathBuf> {
    codex_transcript_in(&home.join(".codex"), id)
}
fn codex_transcript_in(root: &Path, id: &str) -> Option<PathBuf> {
    codex_thread_field(root, id, "SELECT rollout_path FROM threads WHERE id=?1 LIMIT 1").map(PathBuf::from)
}
/// Provider identity is durable thread metadata, independent of HTTP vs WS.
/// Reuse the same bounded, read-only index lookup as transcript discovery.
pub(crate) fn codex_model_provider_in(root: &Path, id: &str) -> Option<String> {
    codex_thread_field(root, id, "SELECT model_provider FROM threads WHERE id=?1 LIMIT 1")
        .filter(|p| p.len() <= 512 && !p.chars().any(char::is_control))
}
fn codex_thread_field(root: &Path, id: &str, query: &str) -> Option<String> {
    use rusqlite::{Connection, OpenFlags, OptionalExtension};
    if id.is_empty() { return None; }
    for database in codex_databases_in(root) {
        let Ok(connection) = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else { continue; };
        let _ = connection.busy_timeout(std::time::Duration::from_millis(100));
        if let Some(value) = connection.query_row(query, [id], |row| row.get::<_, String>(0)).optional().ok().flatten().filter(|v| !v.is_empty()) {
            return Some(value);
        }
    }
    None
}

/// Fast indexed lookup for runtime SessionId/Finish events. Archive directory
/// recovery belongs to explicit history opens, never per streaming update.
pub(crate) fn codex_existing_transcript_path(home: &Path, id: &str) -> Option<PathBuf> {
    codex_transcript_path(home, id)
        .as_deref()
        .and_then(existing_transcript)
}
pub(crate) fn codex_existing_transcript_in(root: &Path, id: &str) -> Option<PathBuf> {
    codex_transcript_in(root, id)
        .as_deref()
        .and_then(existing_transcript)
}

/// Resolve an old or archived rollout without reading conversation bodies.
/// Fast path is the native indexed row; bounded filename recovery only runs
/// when neither the index nor the last saved location exists.
pub(crate) fn recover_codex_transcript(
    home: &Path,
    id: &str,
    previous: Option<&Path>,
) -> Option<PathBuf> {
    let indexed = codex_transcript_path(home, id);
    if let Some(path) = indexed.as_deref().and_then(existing_transcript) {
        return Some(path);
    }
    if let Some(path) = previous.and_then(existing_transcript) {
        return Some(path);
    }
    // IDs are a single filename component. Never use a supplied ID as a path
    // or accept substring matches (notably parent versus fork identifiers).
    if id.is_empty()
        || id.len() > 128
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return None;
    }
    let expected = format!("-{id}.jsonl");
    let exact = format!("{id}.jsonl");
    let codex = home.join(".codex");
    // An indexed basename gives an O(1) archived lookup in the common case.
    if let Some(name) = indexed.as_ref().and_then(|path| path.file_name()) {
        let archived = codex.join("archived_sessions").join(name);
        if let Some(path) = existing_transcript(&archived) {
            return Some(path);
        }
    }
    let roots = [codex.join("archived_sessions"), codex.join("sessions")];
    let mut inspected = 0;
    for root in roots {
        for entry in WalkDir::new(root)
            .follow_links(false)
            .max_depth(8)
            .into_iter()
        {
            inspected += 1;
            if inspected > MAX_SESSION_ENTRIES {
                return None;
            }
            let Ok(entry) = entry else { continue };
            let name = entry.file_name().to_string_lossy();
            let name = name.strip_suffix(".zst").unwrap_or(&name);
            if entry.file_type().is_file() && (name == exact || name.ends_with(&expected)) {
                return existing_transcript(entry.path());
            }
        }
    }
    None
}

/// Codex compacts old rollouts beside their original JSONL path. Prefer plain
/// JSONL if both are present; the history worker streams compressed sources.
fn existing_transcript(path: &Path) -> Option<PathBuf> {
    if path.extension().is_some_and(|extension| extension == "zst") {
        let plain = path.with_extension("");
        if plain.is_file() {
            return Some(plain);
        }
    }
    if path.is_file() {
        return Some(path.to_owned());
    }
    let mut compressed = path.as_os_str().to_owned();
    compressed.push(".zst");
    let compressed = PathBuf::from(compressed);
    compressed.is_file().then_some(compressed)
}

fn native_sessions(
    home: &Path,
    metadata: &NativeMetadata,
    project: Option<&Path>,
) -> (Vec<SessionSummary>, bool) {
    use rusqlite::{Connection, OpenFlags};
    let mut sessions = Vec::new();
    let mut indexed = false;
    // Newest supported native schema wins. Do not inspect any other tables.
    for database in codex_databases(home) {
        let Ok(connection) = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        ) else {
            continue;
        };
        let _ = connection.busy_timeout(std::time::Duration::from_millis(200));
        let mut values = vec![rusqlite::types::Value::Integer(if project.is_some() {
            1_000
        } else {
            100
        })];
        let mut predicate = String::new();
        if let Some(project) = project {
            let path = project.to_string_lossy().into_owned();
            let prefix = format!(
                "{}/%",
                path.trim_end_matches('/')
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            );
            values.push(path.into());
            values.push(prefix.into());
            predicate = " AND (cwd=?2 OR cwd LIKE ?3 ESCAPE '\\'".into();
            let assigned: Vec<_> = metadata
                .thread_roots
                .iter()
                .filter(|(_, root)| root.starts_with(project))
                .take(10_000)
                .map(|(thread, _)| thread.clone())
                .collect();
            if !assigned.is_empty() {
                predicate.push_str(" OR id IN (");
                for (index, id) in assigned.into_iter().enumerate() {
                    if index != 0 {
                        predicate.push(',');
                    }
                    predicate.push_str(&format!("?{}", values.len() + 1));
                    values.push(id.into());
                }
                predicate.push(')');
            }
            predicate.push(')');
        }
        let query = format!(
            "SELECT id,cwd,substr(title,1,4096),rollout_path,updated_at FROM (
                SELECT id,cwd,title,rollout_path,updated_at,
                ROW_NUMBER() OVER(PARTITION BY cwd ORDER BY updated_at DESC) AS position
                FROM threads WHERE archived=0 AND source NOT LIKE '%subagent%'{predicate}
             ) WHERE position<=?1 ORDER BY updated_at DESC LIMIT 12000"
        );
        let Ok(mut statement) = connection.prepare(&query) else {
            continue;
        };
        let Ok(rows) = statement.query_map(rusqlite::params_from_iter(values), |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
            ))
        }) else {
            continue;
        };
        indexed = true;
        for (id, cwd, title, transcript, updated_at) in rows.flatten() {
            let mut session = SessionSummary {
                id: format!("codex:{id}"),
                title: String::new(),
                agent: AgentKind::Codex,
                project_path: PathBuf::from(cwd),
                execution_path: None,
                updated_at,
                external_id: Some(id),
                transcript_path: Some(PathBuf::from(transcript)),
            };
            apply_project_metadata(&mut session, metadata);
            if !usable_project(&session.project_path)
                || project.is_some_and(|project| !session.project_path.starts_with(project))
            {
                continue;
            }
            let clean_native_title = clean_title(&title);
            let invalid_native_title = clean_native_title.is_none();
            let repaired_title = clean_native_title
                .or_else(|| {
                    session
                        .transcript_path
                        .as_ref()
                        .and_then(|path| read_session(path, AgentKind::Codex, updated_at))
                        .map(|session| session.title)
                })
                .unwrap_or_else(|| "Codex session".into());
            if invalid_native_title && repaired_title == "Codex session" {
                continue;
            }
            session.title = repaired_title;
            sessions.push(session);
        }
        break;
    }
    (sessions, indexed)
}

fn find_executable(kind: AgentKind, home: &Path) -> Option<PathBuf> {
    if kind == AgentKind::Grok {
        // Grok also installs a generic `agent` shim. Never confuse it with Cursor.
        for name in ["grok", "agent"] {
            let path = home.join(".grok/bin").join(name);
            if is_executable(&path) {
                return Some(path);
            }
        }
    }
    crate::process_env::dirs(home)
        .into_iter()
        .map(|dir| dir.join(kind.command()))
        .find(|path| is_executable(path))
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = path.metadata() else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn sample_log(path: &Path) -> Option<Vec<Value>> {
    sample_log_parts(path, true)
}
fn sample_log_head(path: &Path) -> Option<Vec<Value>> {
    sample_log_parts(path, false)
}
fn sample_log_parts(path: &Path, include_tail: bool) -> Option<Vec<Value>> {
    let mut file = File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let mut head = Vec::with_capacity(SAMPLE_BYTES);
    (&mut file)
        .take(SAMPLE_BYTES as u64)
        .read_to_end(&mut head)
        .ok()?;
    let mut lines: Vec<Value> = head
        .split(|byte| *byte == b'\n')
        .filter_map(|line| serde_json::from_slice(line).ok())
        .collect();
    if include_tail && length > SAMPLE_BYTES as u64 {
        let start = length
            .saturating_sub(SAMPLE_BYTES as u64)
            .max(SAMPLE_BYTES as u64);
        file.seek(SeekFrom::Start(start)).ok()?;
        let mut tail = Vec::with_capacity(SAMPLE_BYTES);
        (&mut file)
            .take(SAMPLE_BYTES as u64)
            .read_to_end(&mut tail)
            .ok()?;
        // The first tail line may start in the middle of an enormous JSON record.
        lines.extend(
            tail.split(|byte| *byte == b'\n')
                .skip(1)
                .filter_map(|line| serde_json::from_slice::<Value>(line).ok()),
        );
    }
    Some(lines)
}

fn read_session(path: &Path, kind: AgentKind, modified: i64) -> Option<SessionSummary> {
    let records = sample_log(path)?;
    let mut external_id = None;
    let mut project = None;
    let mut title = None;
    let mut custom_title = None;
    let mut saw_internal_prompt = false;
    for record in &records {
        match kind {
            AgentKind::Codex => {
                let payload = record.get("payload").unwrap_or(record);
                if record.get("type").and_then(Value::as_str) == Some("session_meta") {
                    if payload.get("source").is_some_and(|source| {
                        source.get("subagent").is_some() || source.as_str() == Some("subagent")
                    }) {
                        return None;
                    }
                    external_id = payload.get("id").and_then(Value::as_str).map(str::to_owned);
                    project = payload
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(PathBuf::from);
                }
                if title.is_none() {
                    let event = payload.get("type").and_then(Value::as_str) == Some("user_message");
                    let user = payload.get("role").and_then(Value::as_str) == Some("user");
                    if event || user {
                        let raw = payload
                            .get("message")
                            .and_then(Value::as_str)
                            .or_else(|| payload.get("content").and_then(Value::as_str))
                            .or_else(|| {
                                payload
                                    .get("content")
                                    .and_then(Value::as_array)
                                    .and_then(|parts| {
                                        parts.iter().find_map(|part| {
                                            part.get("text").and_then(Value::as_str)
                                        })
                                    })
                            });
                        saw_internal_prompt |= raw.is_some_and(|text| {
                            !text.trim().is_empty()
                                && crate::presentation::sanitize_transcript_text("user", text)
                                    .is_none()
                        });
                        title = payload
                            .get("message")
                            .and_then(Value::as_str)
                            .and_then(clean_title)
                            .or_else(|| message_title(payload.get("content")));
                    }
                }
            }
            AgentKind::Claude => {
                if record.get("isSidechain").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                if external_id.is_none() {
                    external_id = record
                        .get("sessionId")
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                }
                if project.is_none() {
                    project = record.get("cwd").and_then(Value::as_str).map(PathBuf::from);
                }
                if record.get("type").and_then(Value::as_str) == Some("custom-title") {
                    custom_title = record
                        .get("customTitle")
                        .and_then(Value::as_str)
                        .and_then(clean_title);
                }
                if title.is_none() && record.get("type").and_then(Value::as_str) == Some("user") {
                    title = message_title(
                        record
                            .get("message")
                            .and_then(|message| message.get("content")),
                    );
                }
            }
            _ => {}
        }
    }
    if saw_internal_prompt && title.is_none() && custom_title.is_none() {
        return None;
    }
    let external_id = external_id?;
    let project_path = project.filter(|path| path.is_absolute())?;
    Some(SessionSummary {
        id: format!("{}:{external_id}", kind.command()),
        title: custom_title
            .or(title)
            .unwrap_or_else(|| format!("{} session", kind.name())),
        agent: kind,
        execution_path: Some(project_path.clone()),
        project_path,
        updated_at: modified,
        external_id: Some(external_id),
        transcript_path: Some(path.to_owned()),
    })
}

fn message_title(content: Option<&Value>) -> Option<String> {
    let content = content?;
    content.as_str().and_then(clean_title).or_else(|| {
        content.as_array()?.iter().find_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .and_then(clean_title)
        })
    })
}

/// Temporary native verification sessions are kept on disk but do not belong
/// in a daily project list. The exact prefixes are owned by Kai's test probes.
pub fn internal_probe_project(path:&std::path::Path)->bool{
    let Some(name)=path.file_name().and_then(|s|s.to_str())else{return false};
    if !["kai-agent-ui-","kai-v060-packaged-"].iter().any(|prefix|name.starts_with(prefix)){return false}
    let normalize=|p:&std::path::Path|p.strip_prefix("/private").map(|rest|std::path::Path::new("/").join(rest)).unwrap_or_else(|_|p.to_owned());
    let parent=path.parent().map(normalize);
    parent.as_deref()==Some(std::path::Path::new("/tmp")) || parent==Some(normalize(&std::env::temp_dir()))
}

pub fn clean_title(text: &str) -> Option<String> {
    let sanitized = crate::presentation::sanitize_transcript_text("user", text)?;
    let text = if sanitized.starts_with("# Files mentioned by the user:") || sanitized.starts_with("## Files mentioned by the user:") {
        sanitized.split_once("## My request:").map(|(_,request)|request).unwrap_or(sanitized.as_str())
    }else{sanitized.as_str()};
    let mut title = String::new();
    for word in text.split_whitespace().take(40) {
        if !title.is_empty() {
            title.push(' ');
        }
        title.extend(word.chars().take(80));
        if title.chars().count() >= 80 {
            break;
        }
    }
    let truncated = title.chars().count() > 80;
    title = title.chars().take(80).collect();
    if truncated {
        title.push('…');
    }
    (!title.is_empty()).then_some(title)
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

fn scan_projects(home: &Path, extra_root: &Path, found: &mut HashSet<PathBuf>) {
    let mut queue: VecDeque<_> = ["Documents", "Developer", "Projects", "Code", "code"]
        .into_iter()
        .map(|name| (home.join(name), 0usize))
        .collect();
    if usable_project(extra_root) {
        queue.push_front((extra_root.to_owned(), 0));
    }
    let mut seen = HashSet::new();
    let mut inspected = 0;
    while let Some((dir, depth)) = queue.pop_front() {
        if !dir.is_dir() || !seen.insert(normalize_path(&dir)) {
            continue;
        }
        inspected += 1;
        if inspected > MAX_PROJECT_ENTRIES {
            break;
        }
        if dir.join(".git").exists() {
            found.insert(normalize_path(&dir));
            continue;
        }
        if depth >= 3 {
            continue;
        }
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.take(MAX_PROJECT_ENTRIES).flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.')
                || matches!(
                    name.as_ref(),
                    "node_modules" | "target" | "vendor" | "Library" | "Applications"
                )
            {
                continue;
            }
            if entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
            {
                queue.push_back((entry.path(), depth + 1));
                if queue.len() >= MAX_PROJECT_ENTRIES {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> PathBuf {
        let root = std::env::temp_dir().join(format!("orbit-discovery-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    #[test]
    fn codex_provider_metadata_is_indexed_exact_and_read_only(){
        let root=std::env::temp_dir().join(format!("kai-provider-index-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&root).unwrap();
        let db=rusqlite::Connection::open(root.join("state_5.sqlite")).unwrap();db.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,model_provider TEXT,rollout_path TEXT);").unwrap();
        for(id,provider)in [("app-chat","openai"),("kai-chat","kai-openai-https"),("custom-chat","custom-provider")]{db.execute("INSERT INTO threads VALUES(?1,?2,?3)",rusqlite::params![id,provider,format!("/{id}.jsonl")]).unwrap();}
        let before=std::fs::read(root.join("state_5.sqlite")).unwrap();
        for(id,provider)in [("app-chat","openai"),("kai-chat","kai-openai-https"),("custom-chat","custom-provider")]{assert_eq!(codex_model_provider_in(&root,id).as_deref(),Some(provider));assert_eq!(codex_transcript_in(&root,id),Some(PathBuf::from(format!("/{id}.jsonl"))));}
        assert!(codex_model_provider_in(&root,"unknown").is_none());assert!(codex_model_provider_in(&root,"").is_none());assert_eq!(std::fs::read(root.join("state_5.sqlite")).unwrap(),before);
        drop(db);std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn standalone_native_chat_rediscovery_keeps_group_and_execution_directory_separate() {
        let home = temp_dir();
        let group = unprojected_root(&home);
        let workspace = group.join(uuid::Uuid::new_v4().to_string());
        let project = home.join("real-project");
        let logs = home.join(".claude/projects/fixture");
        for path in [&workspace, &project, &logs] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(
            logs.join("standalone.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "type":"user", "sessionId":"standalone-native", "cwd": workspace,
                    "message":{"content":[{"type":"text","text":"An unprojected chat"}]}
                })
            ),
        )
        .unwrap();
        let found = discover(&home, &project);
        let chat = found
            .sessions
            .iter()
            .find(|s| s.external_id.as_deref() == Some("standalone-native"))
            .unwrap();
        assert_eq!(chat.project_path, group);
        assert_eq!(chat.execution_path.as_ref(), Some(&workspace));
        assert!(!found.projects.iter().any(|p| p.path == workspace));
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn codex_child_rollout_lookup_is_indexed_read_only_metadata() {
        let home = temp_dir();
        fs::create_dir(home.join(".codex")).unwrap();
        let database = home.join(".codex/state_5.sqlite");
        let native = rusqlite::Connection::open(&database).unwrap();
        native.execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,rollout_path TEXT); INSERT INTO threads VALUES('parent','/parent.jsonl'),('child','/child.jsonl');").unwrap();
        let plan: String = native
            .query_row(
                "EXPLAIN QUERY PLAN SELECT rollout_path FROM threads WHERE id=?1 LIMIT 1",
                ["child"],
                |row| row.get(3),
            )
            .unwrap();
        assert!(plan.contains("SEARCH") && plan.contains("INDEX"), "{plan}");
        let before = fs::read(&database).unwrap();
        assert_eq!(
            codex_transcript_path(&home, "child"),
            Some(PathBuf::from("/child.jsonl"))
        );
        assert_eq!(codex_transcript_path(&home, "missing"), None);
        assert_eq!(codex_transcript_path(&home, "child' OR 1=1 --"), None);
        assert_eq!(fs::read(&database).unwrap(), before);
        drop(native);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn missing_codex_rollout_recovers_index_archive_and_exact_relocated_id() {
        let home = temp_dir();
        let codex = home.join(".codex");
        fs::create_dir_all(codex.join("archived_sessions")).unwrap();
        fs::create_dir_all(codex.join("sessions/2026/10/01")).unwrap();
        let stale = home.join("gone/rollout-2026-10-01-child.jsonl");
        let archived = codex
            .join("archived_sessions")
            .join(stale.file_name().unwrap());
        fs::write(&archived, "archive").unwrap();
        let database = codex.join("state_5.sqlite");
        let native = rusqlite::Connection::open(&database).unwrap();
        native
            .execute_batch("CREATE TABLE threads(id TEXT PRIMARY KEY,rollout_path TEXT);")
            .unwrap();
        native
            .execute(
                "INSERT INTO threads VALUES(?1,?2)",
                rusqlite::params!["child", stale.to_string_lossy()],
            )
            .unwrap();
        let before = fs::read(&database).unwrap();
        assert_eq!(
            recover_codex_transcript(&home, "child", Some(&stale)),
            Some(archived.clone())
        );
        let current = codex.join("sessions/2026/10/01/rollout-2026-10-01-child.jsonl");
        fs::rename(&archived, &current).unwrap();
        assert_eq!(
            recover_codex_transcript(&home, "child", Some(&stale)),
            Some(current)
        );
        assert_eq!(
            recover_codex_transcript(&home, "parent", Some(&stale)),
            None
        );
        assert_eq!(recover_codex_transcript(&home, "ild", Some(&stale)), None);
        assert_eq!(recover_codex_transcript(&home, "../../child", None), None);
        assert_eq!(fs::read(&database).unwrap(), before);
        drop(native);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn compacted_codex_rollout_recovers_sibling_and_archive_preferring_plain() {
        let home = temp_dir();
        let codex = home.join(".codex");
        fs::create_dir_all(codex.join("archived_sessions")).unwrap();
        let plain = codex.join("sessions/2026/10/01/rollout-2026-10-01-child.jsonl");
        fs::create_dir_all(plain.parent().unwrap()).unwrap();
        let compressed = plain.with_extension("jsonl.zst");
        // Resolver inspects metadata only. Decoder validity is tested in the
        // lazy history adapter, without decoding at discovery/startup.
        fs::write(&compressed, b"not read by path resolution").unwrap();
        assert_eq!(
            recover_codex_transcript(&home, "child", Some(&plain)),
            Some(compressed.clone())
        );
        fs::write(&plain, b"plain wins").unwrap();
        assert_eq!(
            recover_codex_transcript(&home, "child", Some(&compressed)),
            Some(plain.clone())
        );
        fs::remove_file(&plain).unwrap();
        let archived = codex
            .join("archived_sessions")
            .join(compressed.file_name().unwrap());
        fs::rename(&compressed, &archived).unwrap();
        assert_eq!(
            recover_codex_transcript(&home, "child", Some(&plain)),
            Some(archived)
        );
        assert_eq!(
            recover_codex_transcript(&home, "parent", Some(&plain)),
            None
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn discovers_codex_and_claude_metadata_without_reading_credentials() {
        let home = temp_dir();
        let project = home.join("Projects/hello");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(home.join(".codex/sessions/2026/10/11")).unwrap();
        fs::create_dir_all(home.join(".claude/projects/hello")).unwrap();
        let codex = format!(
            "{}\n{}\n",
            serde_json::json!({"type":"session_meta","payload":{"id":"cx1","cwd":project}}),
            serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"Build a native client"}})
        );
        fs::write(home.join(".codex/sessions/2026/10/11/a.jsonl"), codex).unwrap();
        let claude = serde_json::json!({"type":"user","sessionId":"cl1","cwd":project,"message":{"content":[{"type":"text","text":"Fix the layout"}]}});
        fs::write(
            home.join(".claude/projects/hello/b.jsonl"),
            format!("{claude}\n"),
        )
        .unwrap();
        fs::write(home.join(".codex/auth.json"), "this must never be parsed").unwrap();
        let result = discover(&home, &project);
        assert_eq!(result.sessions.len(), 2);
        assert!(
            result.sessions.iter().any(
                |session| session.id == "codex:cx1" && session.title == "Build a native client"
            )
        );
        assert!(
            result
                .sessions
                .iter()
                .any(|session| session.id == "claude:cl1")
        );
        assert_eq!(result.projects.len(), 1);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn sample_reads_metadata_and_tail_without_loading_large_records() {
        use std::io::Write;
        let root = temp_dir();
        let path = root.join("session.jsonl");
        let mut file = File::create(&path).unwrap();
        writeln!(file, "{}", serde_json::json!({"type":"user","sessionId":"large","cwd":root,"message":{"content":"Original title"}})).unwrap();
        file.write_all(&vec![b'x'; SAMPLE_BYTES * 8]).unwrap();
        writeln!(file).unwrap();
        writeln!(
            file,
            "{}",
            serde_json::json!({"type":"custom-title","customTitle":"Renamed conversation"})
        )
        .unwrap();
        drop(file);
        let session = read_session(&path, AgentKind::Claude, 1).unwrap();
        assert_eq!(session.title, "Renamed conversation");
        assert!(sample_log(&path).unwrap().len() <= 2);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn saved_native_projects_and_quiet_workspace_sessions_survive_busy_projects() {
        let home = temp_dir();
        let busy = home.join("busy");
        let quiet = home.join("outside-scanned-folders");
        let worktree = home.join("temporary-worktree");
        for path in [&busy, &quiet, &worktree, &home.join(".codex")] {
            fs::create_dir_all(path).unwrap();
        }
        fs::write(home.join(".codex/.codex-global-state.json"), serde_json::json!({
            "electron-saved-workspace-roots":["/",busy],
            "local-projects":{"quiet-project":{"name":"Actual user project","rootPaths":[quiet]}},
            "thread-project-assignments":{"quiet-thread":{"projectId":"quiet-project","projectKind":"local"}}
        }).to_string()).unwrap();
        let mut connection =
            rusqlite::Connection::open(home.join(".codex/state_5.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE threads(id TEXT PRIMARY KEY,cwd TEXT,title TEXT,rollout_path TEXT,
            updated_at INTEGER,archived INTEGER,source TEXT);",
            )
            .unwrap();
        let transaction = connection.transaction().unwrap();
        for i in 0..1201 {
            transaction
                .execute(
                    "INSERT INTO threads VALUES(?1,?2,?3,'',?4,0,'cli')",
                    rusqlite::params![
                        format!("busy-{i}"),
                        busy.to_string_lossy(),
                        format!("Busy session {i}"),
                        1000 + i
                    ],
                )
                .unwrap();
        }
        transaction.execute("INSERT INTO threads VALUES('quiet-thread',?1,'A real earlier question','',1,0,'cli')",
            [worktree.to_string_lossy()]).unwrap();
        transaction.execute("INSERT INTO threads VALUES('helper',?1,'Harness helper','',2000,0,'{\"subagent\":{}}')",
            [busy.to_string_lossy()]).unwrap();
        transaction.commit().unwrap();
        drop(connection);
        let found = discover(&home, Path::new("/"));
        assert!(
            found
                .projects
                .iter()
                .all(|project| project.path != Path::new("/"))
        );
        assert!(
            found
                .projects
                .iter()
                .any(|project| project.path == quiet && project.name == "Actual user project")
        );
        assert!(
            found
                .sessions
                .iter()
                .any(|session| session.id == "codex:quiet-thread"
                    && session.project_path == quiet
                    && session.execution_path.as_deref() == Some(worktree.as_path()))
        );
        assert!(
            !found
                .sessions
                .iter()
                .any(|session| session.id == "codex:helper")
        );
        assert_eq!(
            found
                .sessions
                .iter()
                .filter(|session| session.project_path == busy)
                .count(),
            100
        );
        assert_eq!(discover_project(&home, &busy).len(), 1000);
        assert_eq!(discover_project(&home, &quiet).len(), 1);
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn scoped_discovery_keeps_files_only_codex_and_nested_claude_chats() {
        let home = temp_dir();
        let project = home.join("project with_spaces");
        let nested = project.join("child");
        fs::create_dir_all(&nested).unwrap();
        let logs = home.join(".codex/sessions/2026/10/12");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join("session.jsonl"),format!("{}\n{}\n",serde_json::json!({"type":"session_meta","payload":{"id":"files-only","cwd":project}}),serde_json::json!({"type":"event_msg","payload":{"type":"user_message","message":"Native file chat"}}))).unwrap();
        let encoded: String = nested
            .to_string_lossy()
            .chars()
            .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '-' })
            .collect();
        let claude = home.join(".claude/projects").join(encoded);
        fs::create_dir_all(&claude).unwrap();
        fs::write(claude.join("session.jsonl"),format!("{}\n",serde_json::json!({"type":"user","sessionId":"nested-chat","cwd":nested,"message":{"content":[{"type":"text","text":"Nested workspace question"}]}}))).unwrap();
        let found = discover_project(&home, &project);
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|session| session.id == "codex:files-only"));
        assert!(
            found
                .iter()
                .any(|session| session.id == "claude:nested-chat"
                    && session.execution_path.as_ref() == Some(&nested))
        );
        fs::remove_dir_all(home).unwrap();
    }

    #[test]
    fn imported_titles_skip_injected_history_and_use_actual_user_prompt() {
        let root = temp_dir();
        let path = root.join("rollout.jsonl");
        let records = [
            serde_json::json!({"type":"session_meta","payload":{"id":"real-thread","cwd":root}}),
            serde_json::json!({"type":"response_item","payload":{"role":"user","content":[
                {"text":"The following is the Codex agent history added since the last turn"},
                {"text":"<environment_context>injected context</environment_context>\nBuild a focused native client"}
            ]}}),
        ];
        fs::write(
            &path,
            records
                .iter()
                .map(|record| format!("{record}\n"))
                .collect::<String>(),
        )
        .unwrap();
        assert_eq!(
            read_session(&path, AgentKind::Codex, 1).unwrap().title,
            "Build a focused native client"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn diagnostic_projects_are_recognized_only_in_owned_temporary_locations(){
        assert!(internal_probe_project(std::path::Path::new("/private/tmp/kai-agent-ui-fixture")));
        assert!(internal_probe_project(&std::env::temp_dir().join("kai-agent-ui-reasoning-fixture")));
        assert!(!internal_probe_project(std::path::Path::new("/Users/person/projects/kai-agent-ui-client")));
        assert!(!internal_probe_project(std::path::Path::new("/private/tmp/my-project")));
        assert_eq!(clean_title("# Files mentioned by the user:\n## sample.png: /tmp/sample.png\n## My request:\nBuild a useful browser").as_deref(),Some("Build a useful browser"));
    }
    #[test]
    fn titles_are_unicode_safe_and_bounded() {
        let title = clean_title(&"🦀".repeat(200)).unwrap();
        assert!(title.chars().count() <= 81);
        assert_eq!(clean_title("\n  hello   world \n").unwrap(), "hello world");
        assert!(clean_title("<environment_context> secret").is_none());
    }
}
