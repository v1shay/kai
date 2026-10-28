//! Bounded restart state. UI snapshots enter one replaceable slot; all validation,
//! JSON serialization, synchronization and atomic file writes happen on demand
//! in one worker. Dropping the store drains its final snapshot without joining UI.
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
};

pub const MAX_STATE_BYTES: usize = 8 * 1024 * 1024;
/// Encoded workflow metadata leaves room for bounded editor drafts, window state
/// and future optional fields inside the complete 8 MiB restart snapshot.
pub const MAX_WORKFLOW_BYTES: usize = 2 * 1024 * 1024;
pub const MAX_EXPANDED_PROJECTS: usize = 512;

/// Serialize directly into a counting sink: escaped strings count by their JSON
/// size without allocating another JSON document or reading transcript files.
pub fn workflow_fits<Q: Serialize + ?Sized, T: Serialize + ?Sized, C: Serialize + ?Sized>(
    queue: &Q,
    teams: &T,
    context: &C,
) -> bool {
    struct Counter {
        remaining: usize,
    }
    impl Write for Counter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > self.remaining {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Workflow metadata budget exceeded",
                ));
            }
            self.remaining -= bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    serde_json::to_writer(
        Counter {
            remaining: MAX_WORKFLOW_BYTES,
        },
        &(queue, teams, context),
    )
    .is_ok()
}

const VERSION: u32 = 1;
const MAX_DRAFTS: usize = 64;
// Eight open tabs plus a blank composer can each contain 64 KiB. JSON may
// expand control characters sixfold; budget that encoding without deleting
// valid editor text. Ordinary snapshots remain much smaller than this cap.
const DRAFT_BYTES: usize = 9 * 6 * 64 * 1024 + MAX_DRAFTS * 512 + 1024;
const SINGLE_DRAFT_BYTES: usize = 64 * 1024;
const MAX_FLUSHERS: usize = 16;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct WindowState {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub maximized: bool,
    pub fullscreen: bool,
}
impl Default for WindowState {
    fn default() -> Self {
        Self {
            x: 0.,
            y: 0.,
            width: 1200.,
            height: 800.,
            maximized: false,
            fullscreen: false,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TranscriptAnchor {
    pub history_offset: usize,
    pub row_index: usize,
    pub pixel_offset: f32,
    pub at_bottom: bool,
}
impl Default for TranscriptAnchor {
    fn default() -> Self {
        Self {
            history_offset: 0,
            row_index: 0,
            pixel_offset: 0.,
            at_bottom: true,
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceState {
    pub activity: crate::sidebar_activity::Saved,
    pub prompt_queue: Vec<crate::prompt_queue::Prompt>,
    pub teams: Vec<crate::teams::Team>,
    pub chat_context: BTreeMap<String, Vec<crate::context_refs::Reference>>,
    pub version: u32,
    pub window: Option<WindowState>,
    pub project: Option<PathBuf>,
    pub execution_path: Option<PathBuf>,
    pub agent: String,
    pub model: Option<String>,
    pub text_morph:bool,
    pub turn_timings:Vec<crate::chat_timing::Record>,
    pub reasoning_efforts:BTreeMap<String,String>,
    pub allow_edits: bool,
    pub codex_approvals: crate::runtime::CodexApprovals,
    pub codex_access: crate::runtime::CodexAccess,
    pub motion: bool,
    pub sidebar: bool,
    pub sidebar_split: bool,
    pub expanded_projects: Vec<PathBuf>,
    pub conversations_visible: bool,
    pub open_tabs: Vec<String>,
    pub active_chat: Option<String>,
    pub drafts: BTreeMap<String, String>,
    pub files_visible: bool,
    pub all_projects: bool,
    pub diff_visible: bool,
    pub selected_diff: Option<PathBuf>,
    pub file_offset: u64,
    pub file_row: usize,
    pub file_pixel: f32,
    pub selected_file: Option<PathBuf>,
    pub file_edit: Option<crate::editor::Draft>,
    pub transcript: Option<TranscriptAnchor>,
    pub appearance: crate::appearance::Appearance,
    pub theme: crate::theme::Theme,
    pub quick_shortcut: crate::global_hotkey::Shortcut,
    pub voice_choice: String,
}
impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            activity: Default::default(),
            prompt_queue: Vec::new(),
            teams: Vec::new(),
            chat_context: BTreeMap::new(),
            version: VERSION,
            window: None,
            project: None,
            execution_path: None,
            agent: "codex".into(),
            model: None,
            text_morph:true,
            turn_timings:Vec::new(),
            reasoning_efforts:BTreeMap::new(),
            allow_edits: false,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            codex_access: Default::default(),
            motion: true,
            sidebar: true,
            sidebar_split: false,
            expanded_projects: Vec::new(),
            conversations_visible: true,
            open_tabs: Vec::new(),
            active_chat: None,
            drafts: BTreeMap::new(),
            files_visible: false,
            all_projects: false,
            diff_visible: false,
            selected_diff: None,
            file_offset: 0,
            file_row: 0,
            file_pixel: 0.,
            selected_file: None,
            file_edit: None,
            transcript: None,
            appearance: Default::default(),
            theme: Default::default(),
            quick_shortcut: Default::default(),
            voice_choice: "cove".into(),
        }
    }
}
impl WorkspaceState {
    /// Does no filesystem work. Discovery/display code validates existence and
    /// clips window coordinates against actual displays after restoration.
    pub fn normalized(mut self) -> Self {
        self.activity.normalize();
        if self.file_edit.as_ref().is_some_and(|draft| !draft.valid()) {
            self.file_edit = None;
        }
        let mut queued_ids = HashSet::new();
        self.prompt_queue.retain(|prompt| {
            crate::prompt_queue::Prompt::valid(prompt) && queued_ids.insert(prompt.id.clone())
        });
        self.prompt_queue.truncate(crate::prompt_queue::MAX_ITEMS);
        let mut queue_context = 0;
        self.prompt_queue.retain(|prompt| {
            queue_context += prompt
                .context
                .iter()
                .map(|r| r.excerpt.len())
                .sum::<usize>();
            queue_context <= 256 * 1024
        });
        self.teams.retain(crate::teams::Team::valid);
        self.teams.truncate(crate::teams::MAX_TEAMS);
        let mut member_ids = HashSet::new();
        self.teams.retain(|team| {
            if team
                .members
                .iter()
                .any(|member| member_ids.contains(&member.session))
            {
                return false;
            }
            member_ids.extend(team.members.iter().map(|member| member.session.clone()));
            true
        });
        self.chat_context.retain(|key, refs| {
            valid_text(key, 4096)
                && !refs.is_empty()
                && refs.len() <= crate::context_refs::MAX_REFERENCES
                && refs.iter().all(crate::context_refs::Reference::valid)
        });
        let mut context_bytes = 0;
        let mut context_scopes = 0;
        self.chat_context.retain(|_, refs| {
            context_scopes += 1;
            context_bytes += refs.iter().map(|r| r.excerpt.len()).sum::<usize>();
            context_scopes <= 64 && context_bytes <= 256 * 1024
        });
        self.version = VERSION;
        self.window = self.window.filter(|w| {
            [w.x, w.y, w.width, w.height].iter().all(|v| v.is_finite())
                && w.x.abs() <= 100_000.
                && w.y.abs() <= 100_000.
                && (64. ..=16_384.).contains(&w.width)
                && (64. ..=16_384.).contains(&w.height)
        });
        self.project = self.project.filter(|p| valid_path(p));
        let mut expanded = HashSet::new();
        self.expanded_projects = self
            .expanded_projects
            .into_iter()
            .filter(|p| valid_path(p) && p != Path::new("/") && expanded.insert(p.clone()))
            .take(MAX_EXPANDED_PROJECTS)
            .collect();
        self.execution_path = self.execution_path.filter(|p| valid_path(p));
        self.selected_file = self.selected_file.filter(|p| valid_path(p));
        self.selected_diff = self.selected_diff.filter(|p| valid_path(p));
        if self.file_row > 1_000_000 {
            self.file_row = 0;
        }
        if !self.file_pixel.is_finite() || self.file_pixel.abs() > 1_000_000. {
            self.file_pixel = 0.;
        }
        if !valid_text(&self.agent, 64) {
            self.agent = "codex".into();
        }
        self.turn_timings.retain(|t|t.key.len()<=128&&t.session.len()<=512&&t.submitted_ms>0);if self.turn_timings.len()>1024{self.turn_timings.drain(..self.turn_timings.len()-1024);}
        self.reasoning_efforts.retain(|key,value|valid_text(key,512)&&((key=="codex:service-tier"&&matches!(value.as_str(),"fast"|"default"))||matches!(value.as_str(),"none"|"minimal"|"low"|"medium"|"high"|"xhigh"|"max"|"ultra")));while self.reasoning_efforts.len()>512{self.reasoning_efforts.pop_first();}
        self.model = self.model.filter(|m| valid_text(m, 256));
        self.active_chat = self.active_chat.filter(|id| valid_text(id, 512));
        let mut seen = HashSet::with_capacity(8);
        self.open_tabs = self
            .open_tabs
            .into_iter()
            .filter(|id| valid_text(id, 512) && seen.insert(id.clone()))
            .take(8)
            .collect();
        if let Some(active) = &self.active_chat
            && !self.open_tabs.contains(active)
        {
            if self.open_tabs.len() == 8 {
                self.open_tabs.pop();
            }
            self.open_tabs.push(active.clone());
        }
        self.transcript = self.transcript.filter(|a| {
            a.history_offset <= 1_000_000_000
                && a.row_index <= 1_000_000
                && a.pixel_offset.is_finite()
                && a.pixel_offset.abs() <= 1_000_000.
        });
        self.appearance = self.appearance.normalized();
        self.theme = self.theme.normalized();
        self.quick_shortcut = self.quick_shortcut.normalized();
        if self.voice_choice != "default"
            && !crate::voice::VOICES.contains(&self.voice_choice.as_str())
        {
            self.voice_choice = "cove".into();
        }
        let mut drafts = BTreeMap::new();
        let mut remaining = DRAFT_BYTES;
        // Preserve the active composer before less relevant cached drafts.
        if let Some(active) = &self.active_chat
            && let Some(text) = self.drafts.remove(active)
        {
            keep_draft(&mut drafts, active.clone(), text, &mut remaining);
        }
        for (id, text) in self.drafts {
            if drafts.len() == MAX_DRAFTS || remaining == 0 {
                break;
            }
            keep_draft(&mut drafts, id, text, &mut remaining);
        }
        self.drafts = drafts;
        self
    }
}
fn valid_text(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)
}
fn valid_path(path: &Path) -> bool {
    let Some(text) = path.to_str() else {
        return false;
    };
    path.is_absolute()
        && text.len() <= 4096
        && !text.contains('\0')
        && !path.components().any(|c| matches!(c, Component::ParentDir))
}
fn escaped_size(ch: char) -> usize {
    match ch {
        '"' | '\\' => 2,
        '\n' | '\r' | '\t' | '\u{8}' | '\u{c}' => 2,
        '\u{0}'..='\u{1f}' => 6,
        _ => ch.len_utf8(),
    }
}
fn keep_draft(out: &mut BTreeMap<String, String>, id: String, text: String, remaining: &mut usize) {
    if !valid_text(&id, 512) || text.is_empty() || out.len() == MAX_DRAFTS {
        return;
    }
    let overhead = id.chars().map(escaped_size).sum::<usize>() + 8;
    if *remaining <= overhead {
        return;
    }
    let budget = *remaining - overhead;
    let (mut used, mut end) = (0, 0);
    for (offset, ch) in text.char_indices() {
        let size = escaped_size(ch);
        if used + size > budget || offset + ch.len_utf8() > SINGLE_DRAFT_BYTES {
            break;
        }
        used += size;
        end = offset + ch.len_utf8();
    }
    if end == 0 {
        return;
    }
    let mut text = text;
    text.truncate(end);
    *remaining -= overhead + used;
    out.insert(id, text);
}
fn read_state(path: &Path) -> io::Result<WorkspaceState> {
    let file = File::open(path)?;
    let mut bytes = Vec::with_capacity(8192);
    file.take((MAX_STATE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Saved workspace exceeds the size limit.",
        ));
    }
    let state: WorkspaceState = serde_json::from_slice(&bytes).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "Saved workspace could not be read.",
        )
    })?;
    if state.version > VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Saved workspace uses a newer format.",
        ));
    }
    Ok(state.normalized())
}
struct Shared {
    pending: Option<(u64, WorkspaceState)>,
    sequence: u64,
    completed: u64,
    last_error: Option<(io::ErrorKind, String)>,
    flushers: Vec<(u64, mpsc::Sender<io::Result<()>>)>,
    closing: bool,
}
impl Shared {
    fn result(&self) -> io::Result<()> {
        self.last_error.as_ref().map_or(Ok(()), |(kind, message)| {
            Err(io::Error::new(*kind, message.clone()))
        })
    }
    fn acknowledge(&mut self) {
        let mut waiting = Vec::with_capacity(MAX_FLUSHERS);
        for (sequence, sender) in std::mem::take(&mut self.flushers) {
            if sequence <= self.completed {
                let _ = sender.send(self.result());
            } else {
                waiting.push((sequence, sender));
            }
        }
        self.flushers = waiting;
    }
}

pub struct StateStore {
    initial: WorkspaceState,
    pub load_error: Option<String>,
    shared: Arc<Mutex<Shared>>,
    wake: SyncSender<()>,
}
impl StateStore {
    /// Small bounded startup read. Missing/corrupt state never prevents startup.
    /// Only inability to launch the saving worker returns an error.
    pub fn new(path: PathBuf) -> io::Result<Self> {
        let (initial, load_error) = match read_state(&path) {
            Ok(state) => (state, None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (WorkspaceState::default(), None),
            Err(e) => (WorkspaceState::default(), Some(e.to_string())),
        };
        let shared = Arc::new(Mutex::new(Shared {
            pending: None,
            sequence: 0,
            completed: 0,
            last_error: None,
            flushers: Vec::with_capacity(MAX_FLUSHERS),
            closing: false,
        }));
        let (wake, receiver) = mpsc::sync_channel(1);
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("code-workspace-state".into())
            .spawn(move || state_worker(path, worker, receiver))?;
        Ok(Self {
            initial,
            load_error,
            shared,
            wake,
        })
    }
    pub fn initial(&self) -> &WorkspaceState {
        &self.initial
    }
    pub fn save(&self, state: WorkspaceState) {
        if let Ok(mut shared) = self.shared.lock() {
            if shared.closing {
                return;
            }
            shared.sequence = shared.sequence.saturating_add(1);
            let sequence = shared.sequence;
            shared.pending = Some((sequence, state));
        }
        match self.wake.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => {}
        }
    }
    /// Acknowledges the most recent snapshot queued before this call, including
    /// newer snapshots that replace it. Await recv outside the UI thread.
    pub fn flush(&self) -> Receiver<io::Result<()>> {
        let (sender, receiver) = mpsc::channel();
        if let Ok(mut shared) = self.shared.lock() {
            if shared.sequence <= shared.completed {
                let _ = sender.send(shared.result());
            } else if shared.flushers.len() < MAX_FLUSHERS {
                let sequence = shared.sequence;
                shared.flushers.push((sequence, sender));
            } else {
                let _ = sender.send(Err(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Too many workspace flush requests.",
                )));
            }
        } else {
            let _ = sender.send(Err(io::Error::other("Workspace worker is unavailable.")));
        }
        let _ = self.wake.try_send(());
        receiver
    }
}
impl Drop for StateStore {
    fn drop(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            shared.closing = true;
        }
        let _ = self.wake.try_send(());
        // Worker owns its final queued snapshot and exits after writing it. Never
        // wait for file synchronization or join a thread while rendering UI.
    }
}
fn state_worker(path: PathBuf, shared: Arc<Mutex<Shared>>, wake: Receiver<()>) {
    loop {
        let disconnected = wake.recv().is_err();
        loop {
            let next = match shared.lock() {
                Ok(mut state) => {
                    state.acknowledge();
                    match state.pending.take() {
                        Some(next) => Some(next),
                        None => {
                            if state.closing || disconnected {
                                return;
                            }
                            None
                        }
                    }
                }
                Err(_) => return,
            };
            let Some((sequence, state)) = next else {
                break;
            };
            let result = write_state(&path, state.normalized());
            if let Ok(mut state) = shared.lock() {
                state.completed = sequence;
                state.last_error = result.err().map(|e| (e.kind(), e.to_string()));
                state.acknowledge();
            } else {
                return;
            }
        }
    }
}
fn write_state(path: &Path, state: WorkspaceState) -> io::Result<()> {
    let bytes = serde_json::to_vec(&state).map_err(io::Error::other)?;
    if bytes.len() > MAX_STATE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Workspace snapshot exceeds the size limit.",
        ));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Workspace state needs a parent directory.",
            )
        })?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".code-workspace-{}.tmp", uuid::Uuid::new_v4()));
    struct Temp(PathBuf);
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.0);
        }
    }
    let guard = Temp(temporary);
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&guard.0)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&guard.0, path)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path =
                std::env::temp_dir().join(format!("code-workspace-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn state(&self) -> PathBuf {
            self.0.join("workspace.json")
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn flushed(store: &StateStore) {
        store
            .flush()
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap();
    }
    #[test]
    fn legacy_workspace_defaults_to_one_project_tree_and_bounds_expanded_roots() {
        let legacy: WorkspaceState =
            serde_json::from_str(r#"{"version":1,"sidebar":true,"conversations_visible":true}"#)
                .unwrap();
        assert!(!legacy.sidebar_split);
        assert!(legacy.expanded_projects.is_empty());
        let mut state = WorkspaceState {
            expanded_projects: vec![
                "/".into(),
                "relative".into(),
                "/work/../other".into(),
                PathBuf::from("/bad\0path"),
                "/work/a".into(),
                "/work/a".into(),
            ],
            ..Default::default()
        };
        state
            .expanded_projects
            .extend((0..1000).map(|n| PathBuf::from(format!("/work/project-{n}"))));
        let state = state.normalized();
        assert_eq!(state.expanded_projects.len(), MAX_EXPANDED_PROJECTS);
        assert_eq!(state.expanded_projects[0], PathBuf::from("/work/a"));
        assert_eq!(
            state.expanded_projects.iter().collect::<HashSet<_>>().len(),
            MAX_EXPANDED_PROJECTS
        );
        assert!(
            state
                .expanded_projects
                .iter()
                .all(|p| valid_path(p) && p != Path::new("/"))
        );
    }
    #[test]
    fn restart_roundtrip_restores_bounded_window_tabs_and_active_draft() {
        let f = Fixture::new();
        let store = StateStore::new(f.state()).unwrap();
        assert!(store.load_error.is_none());
        let state = WorkspaceState {
            window: Some(WindowState {
                x: -1200.,
                y: 24.,
                width: 1440.,
                height: 900.,
                maximized: true,
                fullscreen: false,
            }),
            project: Some(PathBuf::from("/work/project")),
            execution_path: Some(PathBuf::from("/work/project/worktree")),
            active_chat: Some("claude:session".into()),
            open_tabs: vec!["codex:a".into(), "claude:session".into()],
            agent: "claude".into(),
            model: Some("sonnet".into()),
            allow_edits: true,
            motion: false,
            sidebar: false,
            sidebar_split: true,
            expanded_projects: vec!["/work/project".into(), "/other/project".into()],
            drafts: BTreeMap::from([("claude:session".into(), "unfinished 🦀 prompt".into())]),
            files_visible: true,
            selected_file: Some(PathBuf::from("/work/project/main.rs")),
            transcript: Some(TranscriptAnchor {
                history_offset: 42,
                row_index: 9,
                pixel_offset: -14.5,
                at_bottom: false,
            }),
            ..Default::default()
        };
        store.save(state.clone());
        flushed(&store);
        let restored = StateStore::new(f.state()).unwrap();
        let r = restored.initial();
        assert_eq!(r.window, state.window);
        assert_eq!(r.project, state.project);
        assert_eq!(r.execution_path, state.execution_path);
        assert_eq!(r.expanded_projects, state.expanded_projects);
        assert!(r.sidebar_split);
        assert_eq!(r.agent, "claude");
        assert_eq!(r.model.as_deref(), Some("sonnet"));
        assert_eq!(r.open_tabs, state.open_tabs);
        assert_eq!(r.drafts, state.drafts);
        assert_eq!(r.transcript, state.transcript);
        assert!(r.allow_edits && r.files_visible);
        assert!(!r.motion && !r.sidebar);
    }
    #[test]
    fn full_size_drafts_survive_json_escaping_and_restart_without_text_loss() {
        let f = Fixture::new();
        let store = StateStore::new(f.state()).unwrap();
        let mut state = WorkspaceState {
            open_tabs: (0..8).map(|i| format!("chat:{i}")).collect(),
            active_chat: Some("chat:0".into()),
            ..Default::default()
        };
        for id in state
            .open_tabs
            .iter()
            .chain(std::iter::once(&"__new_conversation__".to_owned()))
        {
            // Every character needs escaping, exercising the maximum serialized size.
            state
                .drafts
                .insert(id.clone(), "\u{0}".repeat(SINGLE_DRAFT_BYTES));
        }
        store.save(state.clone());
        flushed(&store);
        assert_eq!(
            StateStore::new(f.state()).unwrap().initial().drafts,
            state.drafts
        );
        state
            .drafts
            .insert("chat:0".into(), "🦀\n".repeat(SINGLE_DRAFT_BYTES / 5));
        store.save(state.clone());
        flushed(&store);
        assert_eq!(
            StateStore::new(f.state()).unwrap().initial().drafts,
            state.drafts
        );
    }
    #[test]
    fn corrupt_oversized_future_and_legacy_state_do_not_block_startup() {
        let f = Fixture::new();
        for bytes in [
            b"{broken".to_vec(),
            vec![b' '; MAX_STATE_BYTES + 1],
            b"{\"version\":999}".to_vec(),
        ] {
            fs::write(f.state(), bytes).unwrap();
            let store = StateStore::new(f.state()).unwrap();
            assert!(store.load_error.is_some());
            assert_eq!(store.initial().agent, "codex");
        }
        fs::write(
            f.state(),
            b"{\"active_chat\":\"legacy:chat\",\"motion\":false}",
        )
        .unwrap();
        let store = StateStore::new(f.state()).unwrap();
        assert!(store.load_error.is_none());
        assert_eq!(store.initial().active_chat.as_deref(), Some("legacy:chat"));
        assert!(!store.initial().motion);
        assert!(store.initial().sidebar);
    }
    #[test]
    fn validation_bounds_paths_numbers_unicode_and_escaped_draft_budget() {
        let mut s = WorkspaceState {
            window: Some(WindowState {
                width: f32::NAN,
                ..Default::default()
            }),
            project: Some(PathBuf::from("relative/path")),
            execution_path: Some(PathBuf::from("/work/../escape")),
            active_chat: Some("active".into()),
            open_tabs: (0..100).map(|i| format!("chat:{i}")).collect(),
            transcript: Some(TranscriptAnchor {
                pixel_offset: f32::INFINITY,
                ..Default::default()
            }),
            ..Default::default()
        };
        s.drafts.insert("active".into(), "🦀\u{0}".repeat(50_000));
        for i in 0..100 {
            s.drafts
                .insert(format!("draft:{i}"), "long\\\"prompt".repeat(1000));
        }
        let s = s.normalized();
        assert!(
            s.window.is_none()
                && s.project.is_none()
                && s.execution_path.is_none()
                && s.transcript.is_none()
        );
        assert_eq!(s.open_tabs.len(), 8);
        assert!(s.open_tabs.contains(&"active".into()));
        assert!(s.drafts.contains_key("active"));
        assert!(s.drafts.len() <= MAX_DRAFTS);
        assert!(serde_json::to_vec(&s).unwrap().len() <= MAX_STATE_BYTES);
        assert!(s.drafts.values().all(|t| t.is_char_boundary(t.len())));
    }
    #[test]
    fn latest_replacement_flush_and_drop_drain_do_not_leave_temp_files() {
        let f = Fixture::new();
        let store = StateStore::new(f.state()).unwrap();
        for i in 0..500 {
            store.save(WorkspaceState {
                model: Some(format!("model-{i}")),
                ..Default::default()
            });
        }
        flushed(&store);
        assert_eq!(
            read_state(&f.state()).unwrap().model.as_deref(),
            Some("model-499")
        );
        store.save(WorkspaceState {
            model: Some("final".into()),
            ..Default::default()
        });
        let done = store.flush();
        drop(store);
        done.recv_timeout(Duration::from_secs(3)).unwrap().unwrap();
        assert_eq!(
            read_state(&f.state()).unwrap().model.as_deref(),
            Some("final")
        );
        assert_eq!(fs::read_dir(&f.0).unwrap().count(), 1);
    }
    #[test]
    fn write_failures_acknowledge_and_existing_snapshot_survives_failed_atomic_write() {
        let f = Fixture::new();
        let path = f.state();
        write_state(&path, WorkspaceState::default()).unwrap();
        let previous = fs::read(&path).unwrap();
        let invalid = WorkspaceState {
            model: Some("x".repeat(MAX_STATE_BYTES + 1)),
            ..Default::default()
        };
        assert!(write_state(&path, invalid).is_err());
        assert_eq!(fs::read(&path).unwrap(), previous);
        let blocked = f.0.join("file-not-directory");
        fs::write(&blocked, b"file").unwrap();
        let store = StateStore::new(blocked.join("state.json")).unwrap();
        store.save(WorkspaceState::default());
        assert!(
            store
                .flush()
                .recv_timeout(Duration::from_secs(3))
                .unwrap()
                .is_err()
        );
    }
}

#[cfg(test)]
mod integration_regressions {
    use super::*;
    use crate::{
        context_refs::Reference,
        discovery::AgentKind,
        teams::{Member, Status, Team},
    };
    fn reference() -> Reference {
        Reference {
            id: "source".into(),
            title: "Findings".into(),
            project: "/other/project".into(),
            provider: "Claude Code".into(),
            excerpt: "Quoted findings".into(),
            older_omitted: true,
        }
    }
    fn team() -> Team {
        Team {
            id: uuid::Uuid::new_v4().to_string(),
            title: "Review".into(),
            project: "/tmp/workspace".into(),
            allow_edits: false,
            created_at: 0,
            context: vec![reference()],
            members: [Status::Working, Status::Complete]
                .into_iter()
                .map(|status| Member {
                    session: uuid::Uuid::new_v4().to_string(),
                    agent: AgentKind::Codex,
                    model: None,
                    task: "Inspect without modifications".into(),
                    status,
                    attempted: true,
                    cancel: Default::default(),
                })
                .collect(),
        }
    }
    #[test]
    fn duplicate_restored_team_owners_are_removed_before_native_runs_can_be_attached() {
        let first = team();
        let mut duplicate = team();
        duplicate.members[1].session = first.members[0].session.clone();
        let normalized = WorkspaceState {
            teams: vec![first.clone(), duplicate],
            ..Default::default()
        }
        .normalized();
        assert_eq!(normalized.teams.len(), 1);
        let json = serde_json::to_string(&normalized).unwrap();
        let mut restored: WorkspaceState = serde_json::from_str(&json).unwrap();
        restored.teams[0].hold_on_restart();
        assert_eq!(restored.teams[0].members[0].status, Status::Held);
        assert_eq!(restored.teams[0].members[1].status, Status::Complete);
        assert!(restored.teams[0].members[0].attempted);
        assert_eq!(restored.teams[0].context[0].project, "/other/project");
        assert_eq!(
            restored.teams[0].members[0].session,
            first.members[0].session
        );
        assert!(!restored.teams[0].allow_edits);
    }
    #[test]
    fn restored_context_prunes_empty_scopes_and_enforces_the_runtime_scope_budget() {
        let mut context = BTreeMap::new();
        for i in 0..100 {
            context.insert(format!("empty-{i}"), vec![]);
        }
        for i in 0..80 {
            context.insert(format!("chat-{i}"), vec![reference()]);
        }
        let state = WorkspaceState {
            chat_context: context,
            ..Default::default()
        }
        .normalized();
        assert!(state.chat_context.len() <= 64);
        assert!(state.chat_context.values().all(|refs| !refs.is_empty()));
        let restored: WorkspaceState =
            serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
        assert_eq!(restored.chat_context.len(), state.chat_context.len());
        assert!(
            restored
                .chat_context
                .values()
                .flatten()
                .all(|r| r.valid() && r.older_omitted)
        );
    }
    #[test]
    fn duplicate_queued_ids_cannot_replay_a_restored_follow_up() {
        let prompt = crate::prompt_queue::Prompt {
            id: uuid::Uuid::new_v4().to_string(),
            session: "chat".into(),
            agent: AgentKind::Codex,
            model: None,
            allow_edits: false,
            codex_access: Default::default(),
            text: "Only once".into(),
            images: vec![],
            context: vec![],
            held: true,
        };
        let state = WorkspaceState {
            prompt_queue: vec![prompt.clone(), prompt],
            ..Default::default()
        }
        .normalized();
        assert_eq!(state.prompt_queue.len(), 1);
        assert!(state.prompt_queue[0].held);
    }
    #[test]
    fn encoded_workflow_budget_counts_escaping_and_leaves_room_for_full_editor_drafts() {
        let mut large = reference();
        large.excerpt = "\u{1}".repeat(crate::context_refs::MAX_EXCERPT).into();
        let teams: Vec<_> = (0..4)
            .map(|i| {
                let mut t = team();
                t.context = vec![large.clone(); if i == 3 { 2 } else { 4 }];
                t
            })
            .collect();
        assert!(workflow_fits(
            &Vec::<crate::prompt_queue::Prompt>::new(),
            &teams,
            &BTreeMap::<String, Vec<Reference>>::new()
        ));
        let queue = vec![crate::prompt_queue::Prompt {
            id: uuid::Uuid::new_v4().to_string(),
            session: "chat".into(),
            agent: AgentKind::Codex,
            model: None,
            allow_edits: false,
            codex_access: Default::default(),
            text: "\u{1}".repeat(crate::prompt_queue::MAX_TEXT),
            images: vec![],
            context: vec![],
            held: true,
        }];
        assert!(queue[0].valid());
        assert!(
            !workflow_fits(&queue, &teams, &BTreeMap::<String, Vec<Reference>>::new()),
            "Raw text fits, but its escaped JSON exceeds the shared2MiB cap"
        );
        let drafts = (0..9)
            .map(|i| (format!("draft-{i}"), "\u{1}".repeat(SINGLE_DRAFT_BYTES)))
            .collect();
        let state = WorkspaceState {
            teams,
            drafts,
            ..Default::default()
        }
        .normalized();
        let bytes = serde_json::to_vec(&state).unwrap();
        assert!(bytes.len() < MAX_STATE_BYTES);
        assert_eq!(state.drafts.len(), 9);
        assert!(
            state
                .drafts
                .values()
                .all(|draft| draft.len() == SINGLE_DRAFT_BYTES)
        );
        assert_eq!(
            state.teams.len(),
            4,
            "Existing workflow metadata must never be silently discarded"
        );
    }
}
