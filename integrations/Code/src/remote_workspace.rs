use super::github_ui::button;
use super::*;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::marker::Send;
use std::os::fd::AsRawFd;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

#[path = "remote_composer.rs"]
mod composer;
pub(super) use composer::ComposerPanel;

type Job = Box<dyn FnOnce() + Send>;

struct Worker {
    queue: async_channel::Sender<Job>,
    pending: async_channel::Receiver<Job>,
}

impl Worker {
    fn start(name: &str) -> Option<Self> {
        let (queue, pending) = async_channel::bounded::<Job>(1);
        let jobs = pending.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                while let Ok(job) = jobs.recv_blocking() {
                    job();
                }
            })
            .ok()?;
        Some(Self { queue, pending })
    }

    fn submit<T: Send + 'static>(
        &self,
        replace: bool,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> async_channel::Receiver<T> {
        let (send, receive) = async_channel::bounded(1);
        let job: Job = Box::new(move || {
            let _ = send.try_send(work());
        });
        if let Err(async_channel::TrySendError::Full(job)) = self.queue.try_send(job)
            && replace
        {
            let _ = self.pending.try_recv();
            let _ = self.queue.try_send(job);
        }
        receive
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
struct Device {
    node: String,
    device: String,
    name: String,
    #[serde(default)]
    account: String,
    online: bool,
    allowed: bool,
    compatible: bool,
    #[serde(default)]
    control: bool,
    #[serde(default)]
    file_project: String,
    #[serde(default)]
    terminal: bool,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    form_factor: String,
    #[serde(default)]
    capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct Approval {
    peer: Device,
    #[serde(default)]
    control_requested: bool,
    #[serde(default)]
    project_requested: String,
    #[serde(default)]
    terminal_requested: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RemoteAgentApproval {
    approval_request_id: String,
    request: runtime::NativeRequest,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
struct Status {
    enabled: bool,
    auth_url: Option<String>,
    #[serde(default)]
    system_tailnet: bool,
    #[serde(default, deserialize_with = "null_default")]
    requests: Vec<Approval>,
    #[serde(default, deserialize_with = "null_default")]
    trusted: Vec<Device>,
}

#[derive(Clone, Default, PartialEq, Eq, Deserialize)]
struct Directory {
    #[serde(default, deserialize_with = "null_default")]
    projects: Vec<RemoteProject>,
    #[serde(default, deserialize_with = "null_default")]
    chats: Vec<RemoteChat>,
    #[serde(default)]
    next_offset: usize,
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
struct RemoteProject {
    path: String,
    name: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
struct RemoteChat {
    id: String,
    title: String,
    agent: String,
    #[serde(default)]
    project: String,
    #[serde(default)]
    updated_at: i64,
    #[serde(default)]
    terminal: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
struct DeviceKey(String, String);

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Fold {
    Device(DeviceKey),
    Project(DeviceKey, String),
    Chats(DeviceKey),
}

impl Device {
    fn key(&self) -> DeviceKey {
        DeviceKey(self.node.clone(), self.device.clone())
    }
    fn supports(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|value| value == capability)
    }

    fn has_paired_access(&self) -> bool {
        self.control && self.supports("workspace.paired-access")
    }

    fn has_project_files(&self, project: &str) -> bool {
        self.has_paired_access() || self.file_project == project
    }

    fn glyph(&self) -> &'static str {
        match (self.platform.as_str(), self.form_factor.as_str()) {
            (_, "laptop") => "machine-laptop",
            ("linux" | "freebsd", _) => "machine-server",
            _ => "machine-desktop",
        }
    }
}

#[derive(Clone)]
struct TreeQuery {
    device: DeviceKey,
    project: Option<String>,
    offset: usize,
}

#[derive(Default)]
struct Branch {
    open: bool,
    projects_open: HashSet<String>,
    chats_open: bool,
    directory: Option<Directory>,
    project_pages: HashMap<String, Directory>,
    issue: Option<String>,
}

#[derive(Clone, PartialEq, Eq)]
enum TreeRow {
    Setup,
    Device(Device),
    Project(Device, RemoteProject, bool),
    Chats(Device, bool),
    Chat(Device, RemoteChat, bool),
    More(Device, Option<String>, usize),
    Note(u8, String),
    Connect(Device),
}

fn standalone_project(path: &str) -> bool {
    ["Kai", "Kai Unified", "Code", "Orbit"].iter().any(|app| {
        Path::new(path).ends_with(
            Path::new("Library/Application Support")
                .join(app)
                .join("Chats"),
        )
    })
}

impl Branch {
    fn receive(&mut self, project: Option<String>, offset: usize, mut incoming: Directory) -> bool {
        let previous = if let Some(path) = project {
            match self.project_pages.entry(path) {
                std::collections::hash_map::Entry::Vacant(entry) => {
                    entry.insert(incoming);
                    return true;
                }
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            }
        } else if let Some(directory) = &mut self.directory {
            directory
        } else {
            self.directory = Some(incoming);
            return true;
        };
        if offset > 0 {
            let ids: HashSet<_> = previous.chats.iter().map(|chat| chat.id.as_str()).collect();
            let additional = incoming
                .chats
                .drain(..)
                .filter(|chat| !ids.contains(chat.id.as_str()))
                .collect::<Vec<_>>();
            if additional.is_empty() && previous.next_offset == incoming.next_offset {
                return false;
            }
            previous.chats.extend(additional);
            previous.next_offset = incoming.next_offset;
            return true;
        }
        if incoming.next_offset > 0
            && previous.chats.len() > incoming.chats.len()
            && previous.next_offset != incoming.next_offset
        {
            // A refresh updates the first page but preserves pages explicitly loaded earlier.
            let ids: HashSet<_> = incoming.chats.iter().map(|chat| chat.id.as_str()).collect();
            let older = previous
                .chats
                .iter()
                .filter(|chat| !ids.contains(chat.id.as_str()))
                .cloned()
                .collect::<Vec<_>>();
            incoming.chats.extend(older);
            incoming.next_offset = previous.next_offset;
        }
        if *previous == incoming {
            return false;
        }
        *previous = incoming;
        true
    }

    fn rows(
        &self,
        peer: &Device,
        rows: &mut Vec<TreeRow>,
        split: bool,
        motions: &HashMap<Fold, interaction_ui::WorkMotion>,
    ) {
        if let Some(issue) = &self.issue {
            rows.push(TreeRow::Note(1, issue.clone()));
        }
        let Some(directory) = &self.directory else {
            if self.issue.is_none() && peer.online {
                rows.push(TreeRow::Note(1, "Loading projects…".into()));
            }
            return;
        };
        let project_paths: HashSet<_> =
            directory.projects.iter().map(|p| p.path.as_str()).collect();
        let mut grouped: HashMap<&str, Vec<&RemoteChat>> = HashMap::new();
        let mut standalone = Vec::new();
        for chat in &directory.chats {
            let owner = Path::new(&chat.project)
                .ancestors()
                .filter_map(Path::to_str)
                .find(|path| project_paths.contains(path));
            match owner {
                Some(path) if !standalone_project(path) => {
                    grouped.entry(path).or_default().push(chat)
                }
                _ => standalone.push(chat),
            }
        }
        for project in directory
            .projects
            .iter()
            .filter(|p| !standalone_project(&p.path))
        {
            let open = self.projects_open.contains(&project.path);
            rows.push(TreeRow::Project(peer.clone(), project.clone(), open));
            if split
                || (!open
                    && !motions.contains_key(&Fold::Project(peer.key(), project.path.clone())))
            {
                continue;
            }
            let page = self.project_pages.get(&project.path);
            let mut known = HashSet::new();
            for chat in grouped
                .get(project.path.as_str())
                .into_iter()
                .flatten()
                .copied()
                .chain(page.iter().flat_map(|page| &page.chats))
            {
                if known.insert(chat.id.as_str()) {
                    rows.push(TreeRow::Chat(peer.clone(), chat.clone(), true));
                }
            }
            if let Some(offset) = page
                .map(|page| page.next_offset)
                .filter(|&offset| offset > 0)
            {
                rows.push(TreeRow::More(
                    peer.clone(),
                    Some(project.path.clone()),
                    offset,
                ));
            } else if known.is_empty() {
                let text = if page.is_none() && directory.next_offset > 0 && peer.online {
                    "Loading chats…"
                } else {
                    "No chats yet"
                };
                rows.push(TreeRow::Note(2, text.into()));
            }
        }
        rows.push(TreeRow::Chats(peer.clone(), self.chats_open));
        if !split && (self.chats_open || motions.contains_key(&Fold::Chats(peer.key()))) {
            if standalone.is_empty() && directory.next_offset == 0 {
                rows.push(TreeRow::Note(2, "No standalone chats".into()));
            }
            rows.extend(
                standalone
                    .into_iter()
                    .map(|chat| TreeRow::Chat(peer.clone(), chat.clone(), false)),
            );
            if directory.next_offset > 0 {
                rows.push(TreeRow::More(peer.clone(), None, directory.next_offset));
            }
        }
    }
}

#[derive(Deserialize)]
struct Message {
    id: i64,
    role: String,
    text: String,
}

#[derive(Deserialize)]
struct CanonicalHistory {
    messages: Vec<runtime::TranscriptMessage>,
    older: Option<i64>,
    #[serde(default)]
    snapshot: Option<String>,
}

#[derive(Clone, Default)]
struct HistoryPages {
    newest: Arc<Vec<runtime::TranscriptMessage>>,
    cursor: Option<i64>,
    cursor_snapshot: Option<String>,
    requested_head: Option<String>,
}

impl HistoryPages {
    fn receive_head(&mut self, mut page: CanonicalHistory) -> bool {
        self.requested_head = page.snapshot.clone();
        if self.newest.is_empty() {
            self.cursor = page.older;
            self.cursor_snapshot = page.snapshot;
        } else if let Some(start) = self.position_in(&page) {
            let end = start + self.newest.len();
            self.cursor = page.older;
            self.cursor_snapshot = page.snapshot;
            // Reconciliation can fetch past the retained boundary. Keep those
            // rows so the next cursor always refers to the oldest visible row.
            if start == 0 && end == page.messages.len() {
                return false;
            }
        } else {
            let shared = Self::overlap(&self.newest, &page.messages);
            if shared == 0 || shared == page.messages.len() {
                return false;
            }
            let prefix = self.newest.len() - shared;
            Arc::make_mut(&mut self.newest).extend(page.messages.drain(shared..));
            if prefix != 0 {
                // Page numbers are relative to their snapshot. An older read
                // will locate our retained boundary in the current snapshot.
                self.cursor_snapshot = None;
            }
            return true;
        }
        if *self.newest == page.messages {
            return false;
        }
        self.newest = Arc::new(page.messages);
        true
    }

    fn receive_older(&mut self, mut page: CanonicalHistory, adjacent: bool) -> bool {
        if !adjacent {
            let Some(start) = self.position_in(&page) else { return false };
            page.messages.truncate(start);
        }
        self.cursor = page.older;
        self.cursor_snapshot = page.snapshot;
        if page.messages.is_empty() {
            return false;
        }
        page.messages.extend(self.newest.iter().cloned());
        self.newest = Arc::new(page.messages);
        true
    }

    fn position_in(&self, page: &CanonicalHistory) -> Option<usize> {
        if self.newest.is_empty() {
            return Some(page.messages.len());
        }
        page.messages.windows(self.newest.len()).rposition(|section| section == self.newest.as_slice())
    }

    fn overlap(before: &[runtime::TranscriptMessage], after: &[runtime::TranscriptMessage]) -> usize {
        (1..=before.len().min(after.len())).rev()
            .find(|&count| before[before.len() - count..] == after[..count])
            .unwrap_or(0)
    }

    #[cfg(test)]
    fn combined(&self) -> Vec<runtime::TranscriptMessage> {
        self.newest.as_ref().clone()
    }
}

#[derive(Clone)]
struct LiveTranscript {
    rows: Arc<Vec<Row>>,
    flow: Arc<chat_flow::Index>,
    status: String,
}

impl LiveTranscript {
    fn stable_prefix(&self, next: &Self) -> usize {
        self.rows.iter().zip(next.rows.iter()).enumerate().take_while(|(i, (old, new))| {
            old.role == new.role && old.block == new.block
                && self.flow.hidden[*i] == next.flow.hidden[*i]
                && self.flow.work[*i] == next.flow.work[*i]
                && self.flow.commands[*i] == next.flow.commands[*i]
                && self.flow.message_ends[*i] == next.flow.message_ends[*i]
                && self.flow.completion_ends[*i] == next.flow.completion_ends[*i]
        }).count()
    }
}

enum StreamUpdate {
    Connected,
    Workspace,
    Activity(Vec<Activity>),
    Directory,
    Running(LiveTranscript),
    Finished,
    Revoked,
}

struct StreamCancellation(Arc<AtomicBool>);

impl Drop for StreamCancellation {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

fn stream_chat(
    helper: &Path,
    peer: &Device,
    chat: &RemoteChat,
    cancellation: &AtomicBool,
    updates: &async_channel::Sender<StreamUpdate>,
) -> std::io::Result<()> {
    let mut params = request("stream", peer);
    params["chat"] = json!(chat.id);
    if !chat.project.is_empty() && peer.has_project_files(&chat.project) {
        params["project"] = json!(chat.project);
        params["file"] = json!(".");
    }
    let mut child = Command::new(helper)
        .arg("stream")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let result = (|| {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(&serde_json::to_vec(&params)?)?;
        let mut output = child.stdout.take().unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 32_768];
        let mut preview = String::new();
        let mut status = String::from("Working…");
        let mut last_publish = Instant::now();
        let mut dirty = false;
        let mut finished = false;
        let mut active = false;
        let mut connected = false;
        while !cancellation.load(Ordering::Acquire) && !updates.is_closed() {
            let mut descriptor = libc::pollfd {
                fd: output.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = unsafe { libc::poll(&mut descriptor, 1, 80) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(error);
            }
            if ready > 0 {
                let read = output.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                bytes.extend_from_slice(&buffer[..read]);
                if bytes.len() > 8 << 20 {
                    break;
                }
                let mut consumed = 0;
                while let Some(end) = bytes[consumed..].iter().position(|&b| b == b'\n') {
                    let line_end = consumed + end;
                    let frame = serde_json::from_slice::<Value>(&bytes[consumed..line_end]);
                    consumed = line_end + 1;
                    let Ok(frame) = frame else {
                        continue;
                    };
                    if !connected {
                        connected = true;
                        let _ = updates.try_send(StreamUpdate::Connected);
                    }
                    match frame["type"].as_str() {
                        Some("metadata" | "metadata_delta") => {
                            let _ = updates.try_send(StreamUpdate::Workspace);
                        }
                        Some("directory") => { let _ = updates.try_send(StreamUpdate::Directory); }
                        Some("activity") => {
                            if let Ok(items) = serde_json::from_value::<Option<Vec<Activity>>>(
                                frame["activity"].clone(),
                            ) {
                                let items = items.unwrap_or_default();
                                let running = items.iter().any(|item| item.chat == chat.id && item.running());
                                if active != running {
                                    preview.clear();
                                    dirty = false;
                                    finished = false;
                                }
                                active = running;
                                let _ = updates.try_send(StreamUpdate::Activity(items));
                            }
                        }
                        Some("revoked") => {
                            let _ = updates.try_send(StreamUpdate::Revoked);
                            return Ok(());
                        }
                        Some("reset") if frame["chat"] == chat.id => {
                            preview.clear();
                            status = "Working…".into();
                            dirty = false;
                            finished = false;
                        }
                        Some("events") if active && frame["chat"] == chat.id => {
                            if let Some(events) = frame["events"].as_array() {
                                for event in events {
                                    if event == "Finished" || event.get("Failed").is_some() {
                                        finished = true;
                                        dirty = false;
                                        let _ = updates.try_send(StreamUpdate::Finished);
                                    } else if finished {
                                        continue;
                                    } else if let Some(text) = event["ReplaceText"]
                                        .as_str()
                                        .or_else(|| event["ReplacePreview"].as_str())
                                    {
                                        preview = text.to_owned();
                                        dirty = true;
                                    } else if let Some(text) = event["Text"].as_str() {
                                        preview.push_str(text);
                                        dirty = true;
                                    } else if let Some(card) =
                                        event.get("Interaction").and_then(|card| {
                                            serde_json::from_value::<
                                                chat_interactions::ChatInteraction,
                                            >(
                                                card.clone()
                                            )
                                            .ok()
                                        })
                                    {
                                        preview.push_str("\n\n");
                                        preview.push_str(
                                            &chat_interactions::serialize_interaction(&card),
                                        );
                                        preview.push_str("\n\n");
                                        dirty = true;
                                    } else if let Some(text) = event["Status"].as_str() {
                                        status = text.to_owned();
                                        dirty = true;
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                }
                bytes.drain(..consumed);
            }
            if preview.len() > 64 << 10 {
                let start = preview.ceil_char_boundary(preview.len() - (48 << 10));
                preview.drain(..start);
            }
            if dirty && last_publish.elapsed() >= Duration::from_millis(100) {
                let rows = if preview.is_empty() {
                    vec![]
                } else {
                    canonical_remote_rows(
                        chat,
                        vec![runtime::TranscriptMessage {
                            role: "assistant".into(),
                            text: preview.clone(),
                            code_language: None,
                        }],
                    )
                };
                if updates
                    .try_send(StreamUpdate::Running(LiveTranscript {
                        flow: Arc::new(chat_flow::Index::new(&rows)),
                        rows: Arc::new(rows),
                        status: status.clone(),
                    }))
                    .is_ok()
                {
                    dirty = false;
                    last_publish = Instant::now();
                }
            }
        }
        Ok(())
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}

fn remote_history_page(
    helper: &Path,
    peer: &Device,
    chat: &RemoteChat,
    before: i64,
) -> Result<CanonicalHistory, String> {
    let mut query = request("messages", peer);
    query["chat"] = json!(chat.id);
    query["cursor"] = json!(before.to_string());
    if peer.supports("workspace.canonical-history") {
        query["canonical"] = json!(true);
        return call(helper, query);
    }
    let page: Vec<Message> = call_list(helper, query)?;
    let older = (page.len() == 30)
        .then(|| page.iter().map(|m| m.id).min())
        .flatten();
    let messages = page
        .into_iter()
        .map(|row| runtime::TranscriptMessage {
            role: row.role,
            text: row.text,
            code_language: None,
        })
        .collect();
    Ok(CanonicalHistory { messages, older, snapshot: None })
}

/// Page numbers are valid only for the history snapshot that produced them.
fn load_remote_history(
    pages: &mut HistoryPages,
    earlier: bool,
    canonical: bool,
    mut fetch: impl FnMut(i64) -> Result<CanonicalHistory, String>,
) -> Result<bool, String> {
    if earlier {
        let Some(cursor) = pages.cursor else { return Ok(false) };
        if !canonical || pages.cursor_snapshot.is_some() {
            let page = fetch(cursor)?;
            if !canonical || page.snapshot == pages.cursor_snapshot {
                return Ok(pages.receive_older(page, true));
            }
        }
    }
    let mut page = fetch(0)?;
    if !earlier && page.snapshot.is_some() && page.snapshot == pages.requested_head {
        return Ok(false);
    }
    while let Some(cursor) = page.older {
        let needs_context = if earlier {
            pages.position_in(&page).is_none_or(|start| start == 0)
        } else if pages.newest.is_empty() {
            page.messages.len() < 2
        } else {
            pages.position_in(&page).is_none()
                && HistoryPages::overlap(&pages.newest, &page.messages) == 0
        };
        if !needs_context {
            break;
        }
        let mut older = fetch(cursor)?;
        if older.snapshot != page.snapshot {
            // Keep the current view and wait for the next refresh. Joining
            // ordinal pages from different snapshots can skip entire turns.
            if !earlier && pages.newest.is_empty() && !page.messages.is_empty() {
                let updated = pages.receive_head(page);
                pages.requested_head = None;
                return Ok(updated);
            }
            return Ok(false);
        }
        if older.older.is_some_and(|next| next <= cursor) {
            return Err("The host returned an invalid transcript cursor.".into());
        }
        older.messages.append(&mut page.messages);
        page = older;
    }
    Ok(if earlier {
        pages.receive_older(page, false)
    } else {
        pages.receive_head(page)
    })
}

fn canonical_remote_rows(
    chat: &RemoteChat,
    mut messages: Vec<runtime::TranscriptMessage>,
) -> Vec<Row> {
    let provider = agent(&chat.agent);
    for message in &mut messages {
        if message.role == "assistant" {
            message.role = format!("assistant:{}", provider.command());
        }
    }
    Orbit::history_rows(engine::parse_messages(messages))
}

#[derive(Clone, PartialEq, Eq, Deserialize)]
struct Activity {
    chat: String,
    status: String,
    #[serde(default)]
    tail: String,
}

impl Activity {
    fn running(&self) -> bool {
        // Older hosts could include queued entries in their overview.
        !matches!(self.status.to_ascii_lowercase().as_str(),
            "" | "queued" | "finished" | "completed" | "failed" | "stopped" | "interrupted" | "idle")
    }
}

#[derive(Clone, Default, PartialEq, Deserialize)]
struct WorkspaceOverview {
    #[serde(default)]
    tabs: Vec<RemoteChat>,
    #[serde(default)]
    active: String,
    dock: Option<terminal_dock::Dock>,
    #[serde(default)]
    activity: Vec<Activity>,
    #[serde(default, deserialize_with = "null_default")]
    running: Vec<RemoteChat>,
}

fn null_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<T, D::Error> {
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

fn helper() -> Option<PathBuf> {
    Some(std::env::current_exe().ok()?.parent()?.join("kai-server"))
}

fn call<T: DeserializeOwned>(helper: &Path, request: Value) -> Result<T, String> {
    let mut child = Command::new(helper)
        .arg("call")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let input = serde_json::to_vec(&request).map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(&input) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e.to_string());
        }
    }
    let result = child.wait_with_output().map_err(|e| e.to_string())?;
    if !result.status.success() {
        return Err(String::from_utf8_lossy(&result.stderr).trim().to_owned());
    }
    serde_json::from_slice(&result.stdout).map_err(|e| e.to_string())
}

#[derive(Clone)]
pub(super) struct RemoteFiles {
    helper: PathBuf,
    peer: Device,
    pub(super) root: PathBuf,
}

impl RemoteFiles {
    pub(super) fn path(&self, path: &Path) -> Result<PathBuf, String> {
        let path = if path.is_absolute() {
            path.to_owned()
        } else {
            self.root.join(path)
        };
        let allowed = path.strip_prefix(&self.root).is_ok_and(|relative| {
            relative
                .components()
                .all(|component| matches!(component, std::path::Component::Normal(_)))
        });
        allowed
            .then_some(path)
            .ok_or_else(|| "The file is outside the shared project".into())
    }

    pub(super) fn call<T: DeserializeOwned>(
        &self,
        action: &str,
        path: &Path,
        fields: Value,
    ) -> Result<T, String> {
        let path = self.path(path)?;
        let relative = path
            .strip_prefix(&self.root)
            .map_err(|error| error.to_string())?;
        let mut req = request(action, &self.peer);
        req["project"] = json!(self.root);
        req["file"] = json!(if relative.as_os_str().is_empty() {
            Path::new(".")
        } else {
            relative
        });
        if let Some(fields) = fields.as_object() {
            for (key, value) in fields {
                req[key] = value.clone();
            }
        }
        call(&self.helper, req)
    }
}

#[derive(Deserialize)]
struct RemoteTerminal {
    id: String,
    #[serde(default)]
    program: String,
}

fn call_list<T: DeserializeOwned>(helper: &Path, request: Value) -> Result<Vec<T>, String> {
    call::<Option<Vec<T>>>(helper, request).map(Option::unwrap_or_default)
}

fn save_aliases(
    path: &Path,
    aliases: &HashMap<String, String>,
    saved: &Mutex<u64>,
    revision: u64,
) -> anyhow::Result<()> {
    let mut saved = saved.lock().unwrap_or_else(|error| error.into_inner());
    if revision > *saved {
        let temporary = path.with_extension("new");
        std::fs::write(&temporary, serde_json::to_vec(aliases)?)?;
        std::fs::rename(temporary, path)?;
        *saved = revision;
    }
    Ok(())
}

fn request(action: &str, device: &Device) -> Value {
    json!({"action":action,"node":device.node,"device":device.device})
}

fn agent(name: &str) -> AgentKind {
    [
        AgentKind::Codex,
        AgentKind::Claude,
        AgentKind::Gemini,
        AgentKind::OpenCode,
        AgentKind::Antigravity,
        AgentKind::Cursor,
        AgentKind::Grok,
        AgentKind::Ninja,
    ]
    .into_iter()
    .find(|kind| {
        kind.command().eq_ignore_ascii_case(name) || kind.name().eq_ignore_ascii_case(name)
    })
    .unwrap_or(AgentKind::Codex)
}

#[derive(Clone)]
pub(super) struct RemoteChatDrag {
    peer: Device,
    chat: RemoteChat,
}

impl Render for RemoteChatDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().px_3().py_2().rounded_md().bg(rgba(0x242424ee)).child(self.chat.title.clone())
    }
}

struct RemoteCli {
    panel: Entity<terminal_ui::TerminalPanel>,
    selected: bool,
    _subscription: Subscription,
}

// Pinned panes retain their renderer, cursor, draft and network stream.
macro_rules! remote_state {
    ($( $scope:vis $name:ident: $ty:ty, )* ; $( $global:ident: $global_type:ty, )*) => {
        #[derive(Default)]
        struct RemotePane {
            $( $name: $ty, )*
        }

        #[derive(Default)]
        pub(super) struct State {
            $( $scope $name: $ty, )*
            $( $global: $global_type, )*
            next_epoch: u64,
            parked: HashMap<String, RemotePane>,
        }

        impl State {
            fn swap_pane(&mut self, pane: &mut RemotePane) {
                $( std::mem::swap(&mut self.$name, &mut pane.$name); )*
            }
        }
    };
}

remote_state! {
    pane_id: Option<String>,
    split_draft: bool,
    terminal_only: bool,
    browse_worker: Option<Worker>,
    composer_worker: Option<Worker>,
    voice_worker: Option<Worker>,
    controls: composer::Controls,
    activity_worker: Option<Worker>,
    stream_worker: Option<Worker>,
    stream: Option<StreamCancellation>,
    stream_connected: bool,
    live_transcript: Option<LiveTranscript>,
    pending_live_transcript: Option<LiveTranscript>,
    finish_poll_until: Option<Instant>,
    pub(super) decode: text_decode::TextDecode,
    selected: Option<Device>,
    pub(super) files: Option<Entity<files::FileBrowser>>,
    pending_terminal: Option<(uuid::Uuid, bool)>,
    pending_cli: Option<uuid::Uuid>,
    project: Option<String>,
    chat: Option<RemoteChat>,
    pub(super) transcript: Option<Arc<Vec<Row>>>,
    pub(super) flow: Arc<chat_flow::Index>,
    native_history_revision: u64,
    native_history: Option<(u64, Arc<[u8]>)>,
    native_history_pending: bool,
    pages: HistoryPages,
    expanded_work: HashSet<usize>,
    work_motions: HashMap<usize, interaction_ui::WorkMotion>,
    expanded_tools: HashSet<usize>,
    tool_motions: HashMap<usize, interaction_ui::WorkMotion>,
    pub(super) reveal: Option<ListReveal>,
    latest_prompt_end: usize,
    transcript_list: Option<ListState>,
    active_chats: HashSet<String>,
    live: Option<Activity>,
    refreshing_activity: bool,
    activity_revision: u64,
    activity_confirmed_at: Option<Instant>,
    composer: Option<Entity<TextInput>>,
    submitted_preview: Option<(String, usize, Instant)>,
    epoch: u64,
    loading: bool,
    loading_earlier: bool,
    earlier_after_load: bool,
    native_history_end: Option<usize>,
    native_history_start: usize,
    native_earlier_requested: bool,
    refresh_after_load: bool,
    agent_approvals: Vec<RemoteAgentApproval>,
    queue_caption: Option<String>,
    host_task_running: bool,
    host_run_id: Option<String>,
    host_task_terminal: bool,
    history_issue: Option<String>,
    answering_approval: Option<String>,
    sent_from: Option<usize>,
    issue: Option<String>,
    ;
    helper: Option<PathBuf>,
    action_worker: Option<Worker>,
    overview_worker: Option<Worker>,
    overviews: HashMap<DeviceKey, WorkspaceOverview>,
    client_tabs: HashMap<DeviceKey, Vec<RemoteChat>>,
    overview_pending: bool,
    expanded_host_tabs: Option<DeviceKey>,
    tab_group_motion: HashMap<DeviceKey, interaction_ui::WorkMotion>,
    tree_worker: Option<Worker>,
    tree_writer: Option<Worker>,
    tree_branches: HashMap<DeviceKey, Branch>,
    tree_rows: Vec<TreeRow>,
    tree_folds: Vec<[Option<Fold>; 2]>,
    tree_motion: HashMap<Fold, interaction_ui::WorkMotion>,
    tree_scroll: Option<ListState>,
    tree_queue: std::collections::VecDeque<TreeQuery>,
    tree_busy: bool,
    started: bool,
    status: Option<Status>,
    devices: Vec<Device>,
    aliases: HashMap<String, String>,
    alias_file: Option<PathBuf>,
    selection_file: Option<PathBuf>,
    restore_chat: Option<RemoteSelection>,
    alias_revision: u64,
    aliases_saved: Arc<Mutex<u64>>,
    remote_clis: HashMap<workspace_ui::TerminalOwner, RemoteCli>,
    rendering_remote_peer: bool,
    rename: Option<(String, Entity<TextInput>)>,
    drafts: HashMap<(DeviceKey, String), String>,
    pending_sends: HashMap<(DeviceKey, String), (String, Value)>,
    sending: HashSet<(DeviceKey, String)>,
    receipt_checked: HashSet<(DeviceKey, String)>,
    stopping: HashMap<(DeviceKey, String), async_channel::Receiver<()>>,
    closing_tab: Option<(String, Instant)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SavedSend {
    device: DeviceKey,
    destination: String,
    prompt: String,
    command: Value,
}
impl SavedSend {
    pub(super) fn valid(&self) -> bool {
        self.device.0.len() <= 128 && self.device.1.len() <= 128
            && self.destination.len() <= 4096 && self.prompt.len() <= teams::MAX_REMOTE_PROMPT_BYTES
            && self.command["request_id"].as_str().is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
            && self.command["prompt"].as_str().is_some_and(|prompt| prompt.len() <= teams::MAX_REMOTE_PROMPT_BYTES)
            && match self.command["action"].as_str() {
                Some("prompt_chat") => self.command["chat_id"].as_str().is_some_and(|chat| self.destination.strip_prefix("chat:") == Some(chat)),
                Some("create_chat") => self.command["project"].as_str().is_some_and(|project| self.destination.strip_prefix("project:") == Some(project)
                    || self.destination.strip_prefix("split-draft:").and_then(|key| key.split_once(':'))
                        .is_some_and(|(id, path)| uuid::Uuid::parse_str(id).is_ok() && path == project)),
                _ => false,
            }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct SavedTab {
    device: DeviceKey,
    chat: RemoteChat,
}

#[derive(Clone)]
pub(super) struct ProjectDestination {
    device: Device,
    parent: String,
    request: uuid::Uuid,
}

#[derive(Serialize, Deserialize)]
struct RemoteSelection {
    device: DeviceKey,
    chat: RemoteChat,
}

impl State {
    pub(super) fn saved_tabs(&self) -> Vec<SavedTab> {
        self.client_tabs.iter().flat_map(|(device, chats)| chats.iter().map(|chat| SavedTab {
            device: device.clone(), chat: chat.clone(),
        })).take(48).collect()
    }

    pub(super) fn restore_tabs(&mut self, saved: Vec<SavedTab>) {
        for tab in saved.into_iter().take(48) {
            self.remember_tab(tab.device, tab.chat);
        }
    }

    fn remember_tab(&mut self, device: DeviceKey, chat: RemoteChat) {
        if chat.id.is_empty() || chat.id.len() > 512 || !Path::new(&chat.project).is_absolute()
            || (!self.client_tabs.contains_key(&device) && self.client_tabs.len() >= 8)
        {
            return;
        }
        let tabs = self.client_tabs.entry(device).or_default();
        if let Some(known) = tabs.iter_mut().find(|known| known.id == chat.id) {
            *known = chat;
        } else {
            if tabs.len() >= keyboard::MAX_CHAT_TABS {
                tabs.remove(0);
            }
            tabs.push(chat);
        }
    }

    fn tabs_for_device(&self, key: &DeviceKey) -> Vec<RemoteChat> {
        let mut tabs = self.overviews.get(key).map(|overview| overview.tabs.clone()).unwrap_or_default();
        if let Some(local) = self.client_tabs.get(key) {
            for tab in local {
                if !tabs.iter().any(|known| known.id == tab.id) {
                    tabs.push(tab.clone());
                }
            }
        }
        tabs
    }

    pub(super) fn pending_snapshot(&self) -> Vec<SavedSend> {
        self.pending_sends.iter().map(|((device, destination), (prompt, command))| SavedSend {
            device: device.clone(), destination: destination.clone(), prompt: prompt.clone(), command: command.clone(),
        }).collect()
    }

    pub(super) fn restore_pending_sends(&mut self, saved: Vec<SavedSend>) {
        for item in saved.into_iter().filter(SavedSend::valid) {
            self.pending_sends.entry((item.device, item.destination)).or_insert((item.prompt, item.command));
        }
    }

    fn draft_key(&self) -> Option<(DeviceKey, String)> {
        Some((self.selected.as_ref()?.key(), self.chat.as_ref().map_or_else(
            || {
                let project = self.project.as_deref().unwrap_or_default();
                self.pane_id.as_deref().and_then(|id| id.strip_prefix("remote-pane:")).filter(|_| self.split_draft)
                    .map_or_else(|| format!("project:{project}"), |id| format!("split-draft:{id}:{project}"))
            },
            |chat| format!("chat:{}",chat.id),
        )))
    }

    fn selected_chat_running(&self) -> bool {
        self.chat.as_ref().is_some_and(|chat| !self.host_task_terminal &&
            (self.host_task_running || self.active_chats.contains(&chat.id)))
    }

    fn submitting(&self) -> bool {
        self.draft_key().is_some_and(|key| self.sending.contains(&key))
    }

    fn has_pending_send(&self) -> bool {
        self.draft_key().is_some_and(|key| self.pending_sends.contains_key(&key))
    }

    fn remember_draft(&mut self, cx: &App) {
        let (Some(key), Some(input)) = (self.draft_key(), &self.composer) else { return };
        let text = input.read(cx).text();
        if text.is_empty() {
            self.drafts.remove(&key);
        } else if self.drafts.len() < 64 || self.drafts.contains_key(&key) {
            self.drafts.insert(key, text);
        }
    }

    fn history_len(&self) -> usize {
        self.transcript.as_ref().map_or(0, |rows| rows.len())
    }

    pub(super) fn visible_history_len(&self) -> usize {
        self.history_len()
    }

    fn apply_history(
        &mut self,
        rows: Arc<Vec<Row>>,
        prompt_end: usize,
        flow: chat_flow::Index,
        earlier: bool,
        stable_prefix: usize,
    ) {
        let old = self.visible_history_len();
        let new = rows.len();
        let list = self
            .transcript_list
            .get_or_insert_with(|| ListState::new(0, ListAlignment::Bottom, px(768.)));
        let anchor = list.logical_scroll_top();
        let at_end = anchor.item_ix
            >= old
                + self
                    .live_transcript
                    .as_ref()
                    .map_or(0, |live| live.rows.len());
        if earlier && new >= old {
            let prefix = new - old;
            list.splice(0..0, prefix);
            // A work group can change at the page seam.
            let end = (prefix + 2).min(new);
            list.splice(prefix..end, end - prefix);
            if !at_end {
                list.scroll_to(ListOffset {
                    item_ix: anchor.item_ix + prefix,
                    ..anchor
                });
            }
            self.expanded_work = self.expanded_work.iter().map(|ix| ix + prefix).collect();
            self.expanded_tools = self.expanded_tools.iter().map(|ix| ix + prefix).collect();
            self.work_motions.clear();
            self.tool_motions.clear();
            self.decode = Default::default();
        } else {
            let unchanged = stable_prefix.min(old).min(new);
            if old != unchanged || new != unchanged {
                list.splice(unchanged..old, new - unchanged);
            }
            if !at_end && anchor.item_ix >= unchanged {
                list.scroll_to(ListOffset {
                    item_ix: anchor.item_ix.min(new),
                    ..anchor
                });
            }
        }
        self.latest_prompt_end = prompt_end;
        self.transcript = Some(rows);
        self.flow = Arc::new(flow);
        self.dirty_native_history();
    }

    fn receive_live(&mut self, live: LiveTranscript) {
        let base = self.visible_history_len();
        let previous = self
            .live_transcript
            .as_ref()
            .map_or(0, |current| current.rows.len());
        let list = self
            .transcript_list
            .get_or_insert_with(|| ListState::new(0, ListAlignment::Bottom, px(768.)));
        // Stream updates often extend just the last paragraph or add a status
        // caption. Preserve the virtual list's measured rows and scroll anchor.
        let stable = self.live_transcript.as_ref().map_or(0, |old| old.stable_prefix(&live));
        if stable != previous || stable != live.rows.len() {
            list.splice(base + stable..base + previous, live.rows.len() - stable);
            self.dirty_native_history();
        }
        self.live_transcript = Some(live);
    }

    fn project_live_when_prompt_is_saved(&mut self, live: LiveTranscript) -> bool {
        if self.submitted_preview.is_none()
            && self.pages.newest.last().is_some_and(|message| message.role == "user")
        {
            self.receive_live(live);
            true
        } else {
            self.pending_live_transcript = Some(live);
            false
        }
    }

    fn clear_preview(&mut self) -> bool {
        self.live = None;
        self.pending_live_transcript = None;
        let base = self.visible_history_len();
        let Some(live) = self.live_transcript.take() else {
            return false;
        };
        if let Some(list) = &self.transcript_list {
            list.splice(base..base + live.rows.len(), 0);
        }
        self.dirty_native_history();
        true
    }

    fn label(&self, device: &Device) -> String {
        self.aliases
            .get(&device.node)
            .cloned()
            .unwrap_or_else(|| device.name.clone())
    }

    pub(super) fn active(&self) -> bool {
        self.selected.is_some()
    }

    pub(super) fn media_scope(&self) -> Option<String> {
        let (peer, chat) = self.selected.as_ref().zip(self.chat.as_ref())?;
        Some(format!("remote:{}:{}:{}", peer.node, peer.device, chat.id))
    }

    pub(super) fn media_source(&self) -> Option<RemoteFiles> {
        let peer = self.selected.as_ref()?;
        let project = self.project.as_ref()?;
        (peer.online && peer.allowed && peer.has_project_files(project)).then_some(())?;
        Some(RemoteFiles {
            helper: self.helper.clone()?,
            peer: peer.clone(),
            root: PathBuf::from(project),
        })
    }

    pub(super) fn remeasure_media_rows(&self, rows: Vec<std::ops::Range<usize>>) {
        if let Some(list) = &self.transcript_list {
            interaction_ui::remeasure_work_rows(list, rows);
        }
    }

    pub(super) fn close_chat(&mut self) {
        if self.selected.take().is_some() {
            self.invalidate();
            self.persist_selection();
        }
    }

    fn persist_selection(&self) {
        let (Some(path), Some(writer)) = (self.selection_file.clone(), &self.tree_writer) else {
            return;
        };
        let selection = self
            .selected
            .as_ref()
            .zip(self.chat.as_ref())
            .map(|(peer, chat)| RemoteSelection {
                device: peer.key(),
                chat: chat.clone(),
            });
        let _ = writer.submit(true, move || {
            let temporary = path.with_extension("new");
            if let Ok(data) = serde_json::to_vec(&selection)
                && std::fs::write(&temporary, data).is_ok()
            {
                let _ = std::fs::rename(temporary, path);
            }
        });
    }

    fn queue_tree(&mut self, query: TreeQuery) {
        if !self.tree_queue.iter().any(|queued| {
            queued.device == query.device
                && queued.project == query.project
                && queued.offset == query.offset
        }) {
            if self.tree_queue.len() >= 64 {
                self.tree_queue.pop_front();
            }
            self.tree_queue.push_back(query);
        }
    }

    fn dirty_native_history(&mut self) {
        self.native_history_revision = self.native_history_revision.wrapping_add(1);
    }

    fn reset_transcript_view(&mut self) {
        self.native_history = None;
        self.native_history_end = None;
        self.native_history_start = 0;
        self.native_earlier_requested = false;
        self.earlier_after_load = false;
        self.native_history_pending = false;
        self.dirty_native_history();
        self.flow = Default::default();
        self.pages = Default::default();
        self.expanded_work.clear();
        self.expanded_tools.clear();
        self.work_motions.clear();
        self.tool_motions.clear();
        self.reveal = None;
    }

    pub(super) fn row(&self, ix: usize) -> Option<&Row> {
        let base = self.visible_history_len();
        if ix < base {
            self.transcript.as_ref()?.get(ix)
        } else {
            self.live_transcript.as_ref()?.rows.get(ix - base)
        }
    }

    pub(super) fn projection(&self, ix: usize) -> (&chat_flow::Index, usize) {
        let base = self.visible_history_len();
        if ix >= base
            && let Some(live) = &self.live_transcript
        {
            return (&live.flow, base);
        }
        (&self.flow, 0)
    }

    pub(super) fn row_group(&self, ix: usize) -> Option<(Arc<Vec<Row>>, usize)> {
        let base = self.visible_history_len();
        if ix >= base
            && let Some(live) = &self.live_transcript
        {
            return Some((live.rows.clone(), base));
        }
        self.transcript.clone().map(|rows| (rows, 0))
    }

    pub(super) fn provider(&self) -> AgentKind {
        self.chat
            .as_ref()
            .map_or(AgentKind::Codex, |chat| agent(&chat.agent))
    }

    fn invalidate(&mut self) {
        self.next_epoch = self.next_epoch.wrapping_add(1);
        self.epoch = self.next_epoch;
        self.loading = false;
        self.loading_earlier = false;
        self.refresh_after_load = false;
        self.pending_terminal = None;
        self.pending_cli = None;
        self.terminal_only = false;
        self.controls.voice = None;
        self.transcript = None;
        self.reset_transcript_view();
        self.latest_prompt_end = 0;
        self.transcript_list = None;
        self.live = None;
        self.stream = None;
        self.stream_connected = false;
        self.live_transcript = None;
        self.pending_live_transcript = None;
        self.finish_poll_until = None;
        self.decode = Default::default();
        self.refreshing_activity = false;
        self.activity_confirmed_at = None;
        self.agent_approvals.clear();
        self.queue_caption = None;
        self.host_task_running = false;
        self.host_run_id = None;
        self.host_task_terminal = false;
        self.history_issue = None;
        self.answering_approval = None;
        self.sent_from = None;
        self.submitted_preview = None;
        self.issue = None;
    }
}

impl Orbit {
    pub(super) fn remote_work_reveal(&self, index: usize) -> f32 {
        let state = &self.remote_workspace;
        state
            .work_motions
            .get(&index)
            .map_or(f32::from(state.expanded_work.contains(&index)), |motion| {
                motion.progress()
            })
    }

    pub(super) fn remote_work_summary(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        Self::work_disclosure(
            index,
            "Worked".into(),
            self.remote_work_reveal(index),
            cx.listener(move |app, _, _, cx| app.toggle_remote_work(index, cx)),
        )
        .into_any_element()
    }

    fn toggle_remote_work(&mut self, index: usize, cx: &mut Context<Self>) {
        let from = self.remote_work_reveal(index);
        let state = &mut self.remote_workspace;
        state.dirty_native_history();
        let open = if state.expanded_work.remove(&index) {
            false
        } else {
            state.expanded_work.insert(index);
            true
        };
        if self.motion && !native::reduced_motion() {
            state.work_motions.insert(index, interaction_ui::WorkMotion::new(from, open));
        }
        if let Some(list) = &state.transcript_list {
            let (flow, base) = state.projection(index);
            if let Some(span) = index.checked_sub(base).and_then(|index| flow.work.get(index)).cloned().flatten() {
                interaction_ui::remeasure_work_rows(list, Some(span.start + base..span.end + base));
            }
        }
        cx.notify();
    }

    pub(super) fn remote_tool_expanded(&self, index: usize) -> bool {
        self.remote_workspace.expanded_tools.contains(&index)
    }

    pub(super) fn remote_tool_reveal(&self, index: usize) -> f32 {
        self.remote_workspace
            .tool_motions
            .get(&index)
            .map_or(f32::from(self.remote_tool_expanded(index)), |motion| {
                motion.progress()
            })
    }

    pub(super) fn toggle_remote_tool(&mut self, index: usize, cx: &mut Context<Self>) {
        let from = self.remote_tool_reveal(index);
        let state = &mut self.remote_workspace;
        state.dirty_native_history();
        let open = if state.expanded_tools.remove(&index) {
            false
        } else {
            state.expanded_tools.insert(index);
            true
        };
        if self.motion && !native::reduced_motion() {
            state
                .tool_motions
                .insert(index, interaction_ui::WorkMotion::tool(from, open));
        }
        if let Some(list) = &state.transcript_list {
            list.splice(index..index + 1, 1);
        }
        cx.notify();
    }

    fn tick_remote_transcript_motion(&mut self, window: &Window, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let motion = self.motion && !native::reduced_motion();
        let active = window.is_window_active();
        let reveal = ListReveal::advance(&mut state.reveal, motion, true, active);
        let moving = !state.work_motions.is_empty() || !state.tool_motions.is_empty();
        if let Some(list) = &state.transcript_list
            && moving
        {
            interaction_ui::remeasure_work_rows(
                list,
                state
                    .work_motions
                    .keys()
                    .filter_map(|index| state.flow.work.get(*index)?.clone()),
            );
            for index in state.tool_motions.keys().copied() {
                list.splice(index..index + 1, 1);
            }
        }
        state
            .work_motions
            .retain(|_, animation| motion && animation.active());
        state
            .tool_motions
            .retain(|_, animation| motion && animation.active());
        if active && (reveal || !state.work_motions.is_empty() || !state.tool_motions.is_empty()) {
            request_motion_frame(window, cx);
        }
    }

    pub(super) fn start_remote_workspace(&mut self, cx: &mut Context<Self>) {
        if self.remote_workspace.started || !unified::active() {
            return;
        }
        self.remote_workspace.started = true;
        let Some(helper) = helper().filter(|path| path.is_file()) else {
            return;
        };
        let Some(discovery) = Worker::start("Kai remote discovery") else {
            return;
        };
        self.remote_workspace.action_worker = Worker::start("Kai remote actions");
        self.remote_workspace.overview_worker = Worker::start("Kai remote workspace metadata");
        self.remote_workspace.tree_worker = Worker::start("Kai remote projects");
        self.remote_workspace.tree_writer = Worker::start("Kai remote selection");
        self.remote_workspace.helper = Some(helper.clone());
        let labels = self.background_data.join("remote-device-names.json");
        self.remote_workspace.alias_file = Some(labels.clone());
        let selection = self.background_data.join("remote-selected-chat.json");
        self.remote_workspace.selection_file = Some(selection.clone());
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            let (aliases, restore) = executor
                .spawn(async move {
                    let aliases = std::fs::read(labels)
                        .ok()
                        .and_then(|data| serde_json::from_slice(&data).ok())
                        .unwrap_or_default();
                    let restore = std::fs::read(selection)
                        .ok()
                        .and_then(|data| {
                            serde_json::from_slice::<Option<RemoteSelection>>(&data).ok()
                        })
                        .flatten();
                    (aliases, restore)
                })
                .await;
            if this
                .update(cx, |app, _| {
                    app.remote_workspace.aliases = aliases;
                    app.remote_workspace.restore_chat = restore;
                })
                .is_err()
            {
                return;
            }
            loop {
                let binary = helper.clone();
                let result = discovery
                    .submit(false, move || {
                        let mut status = call::<Status>(&binary, json!({"action":"status"}))?;
                        status.trusted.sort_by(|a, b| a.node.cmp(&b.node));
                        status
                            .requests
                            .sort_by(|a, b| a.peer.node.cmp(&b.peer.node));
                        let devices = if status.enabled {
                            call_list::<Device>(&binary, json!({"action":"peers"})).ok()
                        } else {
                            Some(Vec::new())
                        };
                        Ok::<_, String>((status, devices))
                    })
                    .recv()
                    .await;
                if this
                    .update(cx, |app, cx| {
                        if let Ok(Ok((status, devices))) = result {
                            let state = &mut app.remote_workspace;
                            let devices = devices.unwrap_or_else(|| state.devices.clone());
                            if state.status.as_ref() != Some(&status) || state.devices != devices {
                                let mut refresh_composer = false;
                                if let Some(mut selected) = state.selected.take() {
                                    if let Some(fresh) = devices.iter().find(|entry| {
                                        entry.node == selected.node
                                            && entry.device == selected.device
                                    }) {
                                        if fresh.online
                                            && (!fresh.allowed
                                                || fresh.file_project != selected.file_project)
                                        {
                                            if state.files.take().is_some() {
                                                app.files_visible = false;
                                            }
                                        }
                                        if fresh.online && !fresh.allowed {
                                            state.pending_terminal = None;
                                            state.transcript = None;
                                            state.reset_transcript_view();
                                            state.latest_prompt_end = 0;
                                            state.transcript_list = None;
                                            state.active_chats.clear();
                                            state.live = None;
                                            state.stream = None;
                                            state.stream_connected = false;
                                            state.live_transcript = None;
                                            state.refreshing_activity = false;
                                            state.next_epoch = state.next_epoch.wrapping_add(1);
                                            state.epoch = state.next_epoch;
                                            state.loading = false;
                                            state.loading_earlier = false;
                                        }
                                        refresh_composer = fresh.allowed
                                            && fresh.online
                                            && fresh.control
                                            && (!selected.online
                                                || !selected.control
                                                || !selected.supports("workspace.composer"))
                                            && fresh.supports("workspace.composer");
                                        if fresh.online
                                            && (!fresh.allowed || !fresh.control)
                                            && state.controls.voice.is_some()
                                        {
                                            state.clear_preview();
                                            state.controls.voice = None;
                                        }
                                        if fresh.online && !selected.online {
                                            state.receipt_checked.retain(|(device, _)| *device != fresh.key());
                                        }
                                        let mut fresh = fresh.clone();
                                        if !fresh.online {
                                            fresh.allowed = selected.allowed;
                                            fresh.control = selected.control;
                                        }
                                        selected = fresh;
                                    } else {
                                        selected.online = false;
                                    }
                                    state.selected = Some(selected);
                                }
                                state.status = Some(status);
                                state.devices = devices;
                                app.refresh_remote_tree(cx);
                                app.restore_remote_chat(cx);
                                if refresh_composer {
                                    app.refresh_remote_composer(cx);
                                }
                                cx.notify();
                            }
                        }
                    })
                    .is_err()
                {
                    return;
                }
                executor.timer(Duration::from_secs(6)).await;
            }
        })
        .detach();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(Duration::from_secs(3)).await;
                if this
                    .update(cx, |app, cx| {
                        app.start_remote_stream(cx);
                        app.recover_remote_send(cx);
                        if !app.remote_workspace.stream_connected
                            || app
                                .remote_workspace
                                .finish_poll_until
                                .is_some_and(|until| until > Instant::now())
                        {
                            app.fetch_remote_workspace(cx);
                        }
                        app.fetch_remote_activity(cx);
                        let parked = app.remote_workspace.parked.keys().cloned().collect::<Vec<_>>();
                        for id in parked {
                            app.with_remote_pane(&id, |app| {
                                let state = &mut app.remote_workspace;
                                if let Some(peer) = &mut state.selected {
                                    if let Some(fresh) = state.devices.iter().find(|candidate| candidate.key() == peer.key()).cloned() {
                                        if fresh.online && !fresh.allowed && peer.allowed {
                                            state.invalidate();
                                            state.files = None;
                                        }
                                        let peer = state.selected.as_mut().unwrap();
                                        if fresh.online { *peer = fresh; }
                                        else { peer.online = false; }
                                    } else { peer.online = false; }
                                }
                                app.start_remote_stream(cx);
                                if !app.remote_workspace.stream_connected || app.remote_workspace.finish_poll_until.is_some_and(|until| until > Instant::now()) {
                                    app.fetch_remote_workspace(cx);
                                }
                                app.fetch_remote_activity(cx);
                            });
                        }
                        app.refresh_remote_overviews(cx);
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            loop {
                executor.timer(Duration::from_secs(12)).await;
                if this
                    .update(cx, |app, cx| app.refresh_open_remote_branches(cx))
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }

    fn restore_remote_chat(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let Some(saved) = &state.restore_chat else {
            return;
        };
        let Some(peer) = state
            .devices
            .iter()
            .find(|peer| peer.key() == saved.device && peer.online && peer.allowed)
            .cloned()
        else {
            return;
        };
        let saved = state.restore_chat.take().unwrap();
        if state.selected.is_none() {
            let branch = state.tree_branches.entry(peer.key()).or_default();
            branch.open = true;
            let project = (!saved.chat.project.is_empty()).then(|| saved.chat.project.clone());
            if let Some(project) = &project {
                branch.projects_open.insert(project.clone());
            }
            self.open_remote_selection(peer.clone(), project, Some(saved.chat), cx);
            self.rebuild_remote_tree();
            let project = self.remote_workspace.project.clone();
            self.load_remote_tree(&peer, None, 0, cx);
            if self.sidebar_split && project.is_some() {
                self.load_remote_tree(&peer, project, 0, cx);
            }
        }
    }

    fn start_remote_stream(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let (Some(peer), Some(chat), Some(worker), Some(helper)) = (
            state
                .selected
                .as_ref()
                .filter(|peer| peer.online && peer.allowed && peer.supports("workspace.stream"))
                .cloned(),
            state.chat.clone(),
            &state.stream_worker,
            state.helper.clone(),
        ) else {
            return;
        };
        if state.stream.is_some() {
            return;
        }
        let canceled = Arc::new(AtomicBool::new(false));
        state.stream = Some(StreamCancellation(canceled.clone()));
        let epoch = state.epoch;
        let (send, updates) = async_channel::bounded(16);
        let job = worker.submit(true, move || {
            let _ = stream_chat(&helper, &peer, &chat, &canceled, &send);
        });
        cx.spawn(async move |this, cx| {
            while let Ok(update) = updates.recv().await {
                let result = this.update(cx, |app, cx| app.with_remote_epoch(epoch, |app| {
                    let state = &mut app.remote_workspace;
                    match update {
                        StreamUpdate::Connected => {
                            state.stream_connected = true;
                            if state.transcript.is_none() && state.history_issue.take().is_some() {
                                app.fetch_remote_workspace(cx);
                            }
                        }
                        StreamUpdate::Workspace => {
                            app.refresh_open_remote_branches(cx);
                            app.fetch_remote_workspace(cx);
                            app.refresh_remote_overviews(cx);
                            return true;
                        }
                        StreamUpdate::Activity(items) => {
                            app.apply_remote_activity(items, cx);
                            return true;
                        }
                        StreamUpdate::Directory => {
                            if let Some(files) = &state.files {
                                files.update(cx, |browser, cx| browser.refresh_remote_root(cx));
                            }
                        }
                        StreamUpdate::Running(live) => {
                            if state.selected_chat_running() && state.controls.voice.is_none() {
                                if !state.project_live_when_prompt_is_saved(live) {
                                    app.fetch_remote_workspace(cx);
                                }
                            }
                            cx.notify();
                        }
                        StreamUpdate::Finished => {
                            if let Some(chat) = &state.chat {
                                state.active_chats.remove(&chat.id);
                            }
                            state.activity_revision = state.activity_revision.wrapping_add(1);
                            state.finish_poll_until =
                                Some(Instant::now() + Duration::from_secs(12));
                            app.fetch_remote_workspace(cx);
                            cx.notify();
                            return true;
                        }
                        StreamUpdate::Revoked => {
                            state.invalidate();
                            if state.files.take().is_some() {
                                app.files_visible = false;
                            }
                            state.active_chats.clear();
                            if let Some(peer) = &mut state.selected {
                                peer.allowed = false;
                            }
                            state.issue = Some("This Mac revoked your access".into());
                            cx.notify();
                            return false;
                        }
                    }
                    true
                }).unwrap_or(false));
                if !matches!(result, Ok(true)) {
                    break;
                }
            }
            let _ = job.recv().await;
            let _ = this.update(cx, |app, _| app.with_remote_epoch(epoch, |app| {
                app.remote_workspace.stream = None;
                app.remote_workspace.stream_connected = false;
            }));
        })
        .detach();
    }

    fn fetch_remote_workspace(&mut self, cx: &mut Context<Self>) {
        self.fetch_remote_page(false, cx);
    }

    fn fetch_remote_page(&mut self, earlier: bool, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        if !earlier && state.transcript.is_none() && state.history_issue.is_some() { return; }
        let Some(device) = state
            .selected
            .as_ref()
            .filter(|peer| peer.online && peer.allowed)
            .cloned()
        else {
            return;
        };
        let (Some(helper), Some(worker), Some(chat)) = (
            state.helper.clone(),
            &state.browse_worker,
            state.chat.clone(),
        ) else {
            return;
        };
        if state.loading {
            state.refresh_after_load |= !earlier;
            state.earlier_after_load |= earlier;
            return;
        }
        if earlier && state.pages.cursor.is_none() {
            return;
        }
        state.loading = true;
        state.loading_earlier = earlier;
        let epoch = state.epoch;
        let first = state.transcript.is_none();
        let previous = state.transcript.clone();
        let mut pages = state.pages.clone();
        let job = worker.submit(true, move || {
            let updated = load_remote_history(&mut pages, earlier, device.supports("workspace.canonical-history"), |cursor| {
                remote_history_page(&helper, &device, &chat, cursor)
            })?;
            let updated = (updated || first).then(|| {
                let rows = canonical_remote_rows(&chat, pages.newest.as_ref().clone());
                let prompt_end = rows
                    .iter()
                    .rposition(chat_flow::user)
                    .map_or(rows.len(), |index| index + 1);
                let flow = chat_flow::Index::for_transcript(&rows, true, None);
                let stable_prefix = previous.as_ref().map_or(0, |previous| previous.iter().zip(&rows)
                    .take_while(|(old, new)| old.role == new.role && old.block == new.block).count());
                (Arc::new(rows), prompt_end, flow, stable_prefix)
            });
            Ok::<_, String>((pages, updated))
        });
        cx.spawn(async move |this, cx| {
            let result = futures_lite::future::race(
                async { job.recv().await.unwrap_or_else(|_| Err("Remote request superseded".into())) },
                async { Timer::after(Duration::from_secs(12)).await; Err("The host did not return this conversation in time.".into()) },
            ).await;
            let _ = this.update(cx, |app, cx| app.with_remote_epoch(epoch, |app| {
                let state = &mut app.remote_workspace;
                state.loading = false;
                state.loading_earlier = false;
                let refresh = std::mem::take(&mut state.refresh_after_load);
                let load_earlier = std::mem::take(&mut state.earlier_after_load) && !earlier;
                match result {
                    Ok((pages, updated)) => {
                        let cursor_changed = state.pages.cursor != pages.cursor;
                        state.pages = pages;
                        let cleared = state.history_issue.take().is_some();
                        let changed = updated.is_some() || cleared || earlier || cursor_changed;
                        if let Some((rows, prompt_end, flow, stable_prefix)) = updated {
                            let first = state.transcript.is_none();
                            if earlier {
                                let added = rows.len().saturating_sub(state.history_len());
                                if std::mem::take(&mut state.native_earlier_requested) && added > 0 {
                                    state.native_history_end = Some(added);
                                } else if let Some(end) = &mut state.native_history_end {
                                    *end += added;
                                }
                            }
                            state.apply_history(rows, prompt_end, flow, earlier, stable_prefix);
                            if !earlier && state.submitted_preview.as_ref().is_some_and(|(_, previous_end, _)| {
                                state.latest_prompt_end > *previous_end
                            }) {
                                state.submitted_preview = None;
                                state.dirty_native_history();
                            }
                            if let Some(live) = state.pending_live_transcript.take() {
                                state.project_live_when_prompt_is_saved(live);
                            }
                            if app.motion && !native::reduced_motion() {
                                if !earlier && state.sent_from.is_some_and(|start| state.latest_prompt_end > start) {
                                    state.reveal = state.sent_from.take().map(ListReveal::appended);
                                    if let Some(list) = &state.transcript_list {
                                        list.scroll_to(ListOffset { item_ix: state.history_len(), offset_in_item: px(0.) });
                                    }
                                } else if first {
                                    state.reveal = Some(ListReveal::new(None));
                                }
                            }
                        }
                        let committed = state.pages.newest.last().is_some_and(|message| message.role != "user");
                        let expired = state.finish_poll_until.is_some_and(|until| until <= Instant::now());
                        let ended = (state.host_task_terminal && committed || expired && !state.selected_chat_running())
                            && state.clear_preview();
                        if changed || ended { cx.notify(); }
                    }
                    Err(error) => {
                        if earlier {
                            state.native_earlier_requested = false;
                        }
                        if state.history_issue.as_ref() != Some(&error) {
                            if let Some(owner) = &state.pane_id { unified::pane_feedback_failed(owner); }
                            unified::workspace_toast(&error, true);
                        }
                        state.history_issue = Some(error);
                        cx.notify();
                    }
                }
                if !earlier && state.transcript.is_none() && state.history_issue.is_some() { return; }
                if load_earlier {
                    app.fetch_remote_page(true, cx);
                    app.remote_workspace.refresh_after_load |= refresh && app.remote_workspace.loading;
                } else if refresh {
                    app.fetch_remote_workspace(cx);
                }
            }));
        })
        .detach();
        if earlier {
            cx.notify();
        }
    }

    fn fetch_remote_activity(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let (Some(device), Some(helper), Some(worker)) = (
            state
                .selected
                .as_ref()
                .filter(|p| p.online && p.allowed)
                .cloned(),
            state.helper.clone(),
            &state.activity_worker,
        ) else {
            return;
        };
        if state.refreshing_activity {
            return;
        }
        state.refreshing_activity = true;
        let epoch = state.epoch;
        let revision = state.activity_revision;
        let chat = state.chat.as_ref().map(|chat| chat.id.clone()).filter(|_| device.control);
        let job = worker.submit(true, move || {
            let controls = chat.and_then(|chat| {
                let mut req = request("run-controls", &device);
                req["chat"] = json!(&chat);
                let controls = call::<Value>(&helper, req).ok()?;
                let approvals = serde_json::from_value::<Vec<RemoteAgentApproval>>(controls["approvals"].clone()).ok()?;
                let task = controls["tasks"].as_array()?.iter()
                    .find(|task| task["chat_id"].as_str() == Some(chat.as_str()));
                let terminal = task.is_some_and(|task| task["terminal"] == true);
                let run_id = task.filter(|_| !terminal)
                    .and_then(|task| task["run_id"].as_str()).map(str::to_owned);
                let caption = task.and_then(|task| match task["status"].as_str()? {
                    "Queued" => Some(task["detail"].as_str().unwrap_or("Queued on host")),
                    "Connecting" => Some("Preparing workspace on host"),
                    "Needs attention" | "Held · resume explicitly" => task["detail"].as_str(),
                    status if !terminal => Some(status),
                    _ => None,
                }).map(str::to_owned);
                Some((approvals, caption, run_id, terminal))
            });
            (call_list::<Activity>(&helper, request("activity", &device)), controls)
        });
        cx.spawn(async move |this, cx| {
            let result = job.recv().await;
            let _ = this.update(cx, |app, cx| app.with_remote_epoch(epoch, |app| {
                let state = &mut app.remote_workspace;
                state.refreshing_activity = false;
                if let Ok((items, controls)) = result {
                    if let Some((mut approvals, caption, run_id, terminal)) = controls
                    {
                        let running = run_id.is_some();
                        let completed = terminal && !state.host_task_terminal;
                        if !(terminal && state.submitting())
                            && (state.queue_caption != caption || state.host_task_terminal != terminal || state.host_run_id != run_id)
                        {
                            state.queue_caption = caption;
                            state.host_task_running = running;
                            state.host_run_id = run_id;
                            state.host_task_terminal = terminal;
                            if completed && state.controls.voice.is_none() {
                                state.finish_poll_until = Some(Instant::now() + Duration::from_secs(12));
                            }
                            cx.notify();
                        }
                        if let Some(token) = &state.answering_approval {
                            approvals.retain(|item| &item.approval_request_id != token);
                        }
                        if state.agent_approvals != approvals {
                            state.agent_approvals = approvals;
                            cx.notify();
                        }
                    }
                    if revision != state.activity_revision {
                        return;
                    }
                    if let Ok(items) = items {
                        app.apply_remote_activity(items, cx);
                    } else if state.activity_confirmed_at.is_some_and(|at| at.elapsed() > Duration::from_secs(10)) {
                        app.apply_remote_activity(Vec::new(), cx);
                    }
                }
            }));
        })
        .detach();
    }

    fn apply_remote_activity(&mut self, items: Vec<Activity>, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        state.activity_revision = state.activity_revision.wrapping_add(1);
        state.activity_confirmed_at = Some(Instant::now());
        let active: HashSet<_> = items.iter().filter(|item| item.running()).map(|item| item.chat.clone()).collect();
        let owner = state.chat.as_ref().map(|chat| chat.id.as_str());
        let live = items
            .into_iter()
            .find(|item| Some(item.chat.as_str()) == owner && item.running());
        let changed = state.active_chats != active || state.live != live;
        state.active_chats = active;
        state.live = live;
        if changed {
            cx.notify();
        }
    }

    fn refresh_remote_overviews(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let (Some(worker), Some(helper)) = (&state.overview_worker, state.helper.clone()) else { return };
        let peers: Vec<_> = state.devices.iter().filter(|peer| peer.allowed && peer.online).cloned().collect();
        if state.overview_pending { return; }
        if peers.is_empty() {
            if !state.overviews.is_empty() {
                state.overviews.clear();
                self.publish_unified(cx);
                cx.notify();
            }
            return;
        }
        state.overview_pending = true;
        let job = worker.submit(false, move || std::thread::scope(|scope| {
            peers.into_iter().map(|peer| {
                let helper = &helper;
                scope.spawn(move || {
                    let value = if peer.supports("workspace-overview") {
                        call::<WorkspaceOverview>(helper, request("workspace-overview", &peer))
                    } else {
                        call_list::<Activity>(helper, request("activity", &peer))
                            .map(|activity| WorkspaceOverview { activity, ..Default::default() })
                    }.ok();
                    (peer.key(), value)
                })
            }).collect::<Vec<_>>().into_iter().filter_map(|thread| thread.join().ok()).collect::<Vec<_>>()
        }));
        cx.spawn(async move |this, cx| {
            let result = job.recv().await;
            let _ = this.update(cx, |app, cx| {
                let state = &mut app.remote_workspace;
                state.overview_pending = false;
                let Ok(result) = result else { return };
                let mut updated = HashMap::new();
                for (key, value) in result {
                    if state.devices.iter().any(|peer| peer.key() == key && peer.online && peer.allowed) {
                        if let Some(overview) = value {
                            updated.insert(key, overview);
                        }
                    }
                }
                if updated != state.overviews {
                    state.overviews = updated;
                    app.publish_unified(cx);
                    cx.notify();
                }
            });
        }).detach();
    }

    pub(super) fn paired_inference_machines(&self) -> Vec<ninja::Machine> {
        let state = &self.remote_workspace;
        state.devices.iter()
            .filter(|peer| peer.allowed && peer.control && peer.supports("ninja.inference"))
            .map(|peer| ninja::Machine {
                id: format!("kai:{}:{}", peer.node, peer.device),
                name: state.label(peer),
                host: format!("http://localhost/inference/{}/{}", peer.node, peer.device),
                online: peer.online,
                local: false,
                os: peer.platform.clone(),
            }).collect()
    }

    pub(super) fn remote_active_tasks(&self) -> Vec<Value> {
        let state = &self.remote_workspace;
        state.devices.iter().filter(|peer| peer.allowed && peer.online).flat_map(|peer| {
            state.overviews.get(&peer.key()).into_iter().flat_map(move |overview| {
                overview.activity.iter().filter(|item| item.running()).map(move |item| {
                    let chat = overview.running.iter().find(|chat| chat.id == item.chat);
                    json!({
                        "id": format!("remote:{}:{}", peer.node, item.chat),
                        "name": chat.map_or("Remote chat", |chat| &chat.title),
                        "scene": kai_icons::scene_for_status(&item.status),
                        "text": format!("{} · {}", state.label(peer), item.status),
                        "agent": chat.map_or("codex", |chat| &chat.agent),
                    })
                })
            })
        }).collect()
    }

    pub(super) fn open_remote_activity(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let state = &self.remote_workspace;
        let target = state.devices.iter().filter(|peer| peer.allowed && peer.online).find_map(|peer| {
            let overview = state.overviews.get(&peer.key())?;
            overview.running.iter().chain(&overview.tabs).find(|chat| id == format!("remote:{}:{}", peer.node, chat.id))
                .map(|chat| (peer.clone(), chat.clone()))
        });
        let Some((peer, chat)) = target else { return false };
        self.open_remote_chat(peer, Some(chat.project.clone()), Some(chat), window, cx);
        true
    }

    pub(super) fn remote_surface_token(&self) -> Option<String> {
        let state = &self.remote_workspace;
        let peer = state.selected.as_ref()?;
        Some(format!("remote:{}:{}:{}", peer.node, peer.device, state.epoch))
    }

    fn native_history_job(&self) -> impl FnOnce() -> Value + Send + use<> {
        let state = &self.remote_workspace;
        let provider = state.provider();
        let transcript = state.transcript.clone();
        let flow = state.flow.clone();
        let latest = state.native_history_end.is_none();
        let live = latest.then(|| state.live_transcript.clone()).flatten();
        let base = state.native_history_end.unwrap_or_else(|| state.visible_history_len());
        let expanded_work = state.expanded_work.clone();
        let expanded_tools = state.expanded_tools.clone();
        let prompt = state.submitted_preview.as_ref().filter(|_| latest)
            .map(|(prompt, _, _)| short(prompt, 4000));
        let generation = self.generation;
        move || {
            let mut history = transcript.as_ref().map_or_else(Vec::new, |rows| {
                Self::project_history_rows(rows, &flow, base, 0, &expanded_work, &expanded_tools, provider, generation, None)
            });
            if let Some(live) = live {
                let relative = |set: &HashSet<usize>| set.iter().filter_map(|id| id.checked_sub(base)).collect::<HashSet<_>>();
                history.extend(Self::project_history_rows(
                    &live.rows, &live.flow, live.rows.len(), base, &relative(&expanded_work),
                    &relative(&expanded_tools), provider, generation, None,
                ));
            }
            if let Some(text) = prompt {
                history.push(json!({"id":"remote-pending-prompt","kind":"user","lines":[{"text":text,"style":"text"}],"speechText":""}));
            }
            Value::Array(unified::budget_history(history))
        }
    }

    pub(super) fn remote_native_history(&self) -> Option<&Arc<[u8]>> {
        self.remote_workspace.native_history.as_ref().map(|(_, rows)| rows)
    }

    pub(super) fn prepare_remote_native_history(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let revision = state.native_history_revision;
        if !state.active() || state.native_history_pending
            || state.native_history.as_ref().is_some_and(|(rendered, _)| *rendered == revision)
        {
            return;
        }
        state.native_history_pending = true;
        let epoch = state.epoch;
        let previous = self.remote_native_history().cloned();
        let project = self.native_history_job();
        let job = cx.background_executor().spawn(async move {
            let _span = performance::Span::new("remote_native_history_worker");
            let history = project();
            let start = history.get(0).and_then(|row| row["id"].as_str()).and_then(|id| {
                id.strip_prefix("row-").or_else(|| id.strip_prefix("work-summary-"))
            }).and_then(|index| index.parse::<usize>().ok()).unwrap_or(0);
            let bytes = serde_json::to_vec(&history)
                .expect("a transcript made of JSON values serializes");
            let rows = match previous {
                Some(previous) if previous.as_ref() == bytes => previous,
                _ => Arc::<[u8]>::from(bytes),
            };
            (rows, start)
        });
        cx.spawn(async move |this, cx| {
            let (rows, start) = job.await;
            let _ = this.update(cx, |app, cx| {
                let state = &mut app.remote_workspace;
                if state.epoch != epoch {
                    return;
                }
                state.native_history_pending = false;
                state.native_history_start = start;
                // Keep the last complete frame while the next transcript
                // projection runs, even when stream packets arrive continuously.
                if state.native_history.as_ref().is_none_or(|(_, old)| !Arc::ptr_eq(old, &rows)) {
                    state.native_history = Some((revision, rows));
                    app.publish_unified(cx);
                } else if let Some((rendered, _)) = &mut state.native_history {
                    *rendered = revision;
                }
                app.prepare_remote_native_history(cx);
            });
        }).detach();
    }

    pub(super) fn remote_surface_snapshot(&self, value: &mut Value, cx: &App, project_history: bool) {
        let state = &self.remote_workspace;
        let Some(peer) = &state.selected else { return };
        let draft = state.composer.as_ref().map(|input| input.read(cx).text()).unwrap_or_default();
        if project_history {
            value["history"] = self.native_history_job()();
        }
        let status = self.remote_status_caption().unwrap_or(if peer.online { "Ready" } else { "Mac offline" });
        value["historyLoading"] = json!(state.loading_earlier || state.loading && state.transcript.is_none());
        value["historyHasMore"] = json!(state.native_history_start > 0 || state.pages.cursor.is_some());
        value["historyEarlier"] = json!(state.native_history_end.is_some());
        value["threadId"] = json!(state.chat.as_ref().map(|chat| format!("remote:{}:{}", peer.node, chat.id)));
        value["threadName"] = json!(state.chat.as_ref().map_or("New conversation", |chat| chat.title.as_str()));
        value["composerDraft"] = json!(draft);
        value["composerBusy"] = json!(state.submitting());
        value["readOnly"] = json!(!peer.allowed || !peer.online || !peer.control);
        value["sessionActivity"] = json!({"scene":if status=="Ready" {"off"} else {kai_icons::scene_for_status(status)}, "text":status});
        value["activeTurnId"] = json!(self.remote_running().then(|| state.chat.as_ref().map(|chat| format!("remote:{}",chat.id))).flatten());
        value["project"] = json!(state.project);
        value["projects"] = json!([]);
        value["chats"] = json!(state.overviews.get(&peer.key()).into_iter().flat_map(|overview| &overview.tabs)
            .map(|chat| json!({"id":format!("remote:{}:{}",peer.node,chat.id),"name":chat.title,"agent":chat.agent,
                "status":if state.active_chats.contains(&chat.id){"active"}else{"idle"}})).collect::<Vec<_>>());
        value["standaloneChats"] = json!([]);
        value["attachments"] = json!([]);
        self.remote_native_composer(value);
        value["files"] = json!(state.files.as_ref().map(|files| files.read(cx).unified_rows()).unwrap_or_default());
        value["nativeReview"] = Value::Null;
        value["approval"] = json!(false);
        value["approvalThreadId"] = Value::Null;
        value["approvalRequestId"] = Value::Null;
        value["approvalSummary"] = Value::Null;
    }

    pub(super) fn dispatch_remote_surface(&mut self, req: &Value, window: &mut Window, cx: &mut Context<Self>) -> Result<bool, String> {
        let action = req["action"].as_str().unwrap_or("");
        if action == "chat" && let Some(id) = req["id"].as_str().filter(|id| id.starts_with("remote:")) {
            return self.open_remote_activity(id, window, cx).then_some(true).ok_or("The remote chat is unavailable".into());
        }
        let Some(peer) = self.remote_workspace.selected.as_ref() else { return Ok(false) };
        let writable = peer.allowed && peer.online && peer.control;
        match action {
            "send" | "draft" | "composer_command" => {
                if !writable { return Err("This remote chat is read-only or offline.".into()); }
                if req["ownerToken"].as_str() != self.remote_surface_token().as_deref() {
                    return Err("The remote chat changed. Your draft is preserved.".into());
                }
                let input = self.remote_composer(window, cx);
                if req["draftBase"].as_str() != Some(input.read(cx).text().as_str()) {
                    return Err("The workspace draft changed. Your notch draft is preserved.".into());
                }
                let text = req[if action == "composer_command" { "draft" } else { "text" }].as_str()
                    .filter(|text| text.len() <= input::MAX_DRAFT_BYTES && !text.contains('\0'))
                    .ok_or("The remote draft is missing or too long")?;
                input.update(cx, |input, cx| input.set_text(text, cx));
                if action == "composer_command" {
                    if let Some(caret) = req["caret"].as_u64() {
                        input.update(cx, |input, cx| input.set_command_caret_utf16(caret as usize, cx));
                    }
                    self.accept_named_slash(req["name"].as_str().ok_or("Missing command")?, window, cx)?;
                } else if action == "send" {
                    self.submit_remote_composer(window, cx);
                }
            }
            "files" | "folder" => {
                let browser = self.toggle_remote_files(cx).ok_or("Remote files are unavailable.")?;
                if action == "folder" {
                    let path = PathBuf::from(req["path"].as_str().ok_or("Missing folder")?);
                    browser.update(cx, |files, cx| files.unified_folder(path, cx))?;
                }
            }
            "remote_file_open" => {
                if req["ownerToken"].as_str() != self.remote_surface_token().as_deref() {
                    return Err("The remote chat changed before the file could open.".into());
                }
                let path = req["path"].as_str().filter(|path| !path.is_empty()).ok_or("Missing file")?;
                let browser = self.toggle_remote_files(cx).ok_or("Remote files are unavailable.")?;
                browser.update(cx, |files, cx| files.open_citation(
                    citations::FileCitation { path: path.into(), line: None, end_line: None }, cx));
                cx.defer(|_| unified::reveal());
            }
            "history_more" => {
                let state = &mut self.remote_workspace;
                if state.native_history_start > 0 {
                    state.native_history_end = Some(state.native_history_start);
                    state.native_history_start = 0;
                    state.dirty_native_history();
                } else if state.pages.cursor.is_some() {
                    state.native_earlier_requested = true;
                    self.fetch_remote_page(true, cx);
                }
            }
            "history_latest" => {
                let state = &mut self.remote_workspace;
                state.native_earlier_requested = false;
                state.native_history_end = None;
                state.dirty_native_history();
                self.jump_remote_latest();
            }
            "interaction_details" => {
                let id = req["id"].as_str().ok_or("Missing interaction")?;
                let index = id.strip_prefix("work-").or_else(|| id.strip_prefix("tools-"))
                    .and_then(|value| value.split('-').next()).and_then(|value| value.parse::<usize>().ok()).ok_or("Invalid remote interaction")?;
                if self.remote_workspace.row(index).is_none() { return Err("This remote interaction has changed.".into()); }
                if id.starts_with("work-") { self.toggle_remote_work(index, cx); }
                else { self.toggle_remote_tool(index, cx); }
            }
            "interrupt" => self.remote_run_control("stop", None, cx),
            "settings" if writable => self.set_remote_native_model(req, cx)?,
            "agent" if writable => self.set_remote_native_agent(req["agent"].as_str().ok_or("Missing agent")?, cx)?,
            "fast_mode" if writable => self.remote_toggle_fast(cx),
            "workspace_permissions" if writable => {
                if req["approvals"].is_string() {
                    return Err("Approval policy is set by the host Mac.".into());
                }
                if let Some(access) = req["access"].as_str() {
                    let controls = &mut self.remote_workspace.controls;
                    match access {
                        "read-only" => controls.edits = Some(false),
                        "workspace" => { controls.edits = Some(true); controls.access = Some(runtime::CodexAccess::Workspace); }
                        "full" => { controls.edits = Some(true); controls.access = Some(runtime::CodexAccess::Full); }
                        _ => return Err("Unknown workspace access.".into()),
                    }
                }
            }
            "settings" | "agent" | "fast_mode" | "workspace_permissions" => return Err("This remote chat is read-only or offline.".into()),
            "workspace_voice" if self.remote_workspace.controls.voice.is_some() => self.stop_remote_voice(cx),
            "workspace_voice" | "voice_start" => self.start_remote_voice_prompt(None, window, cx)?,
            "workspace_voice_prompt" => {
                let audio = req["audio"].as_str().map(str::to_owned);
                self.start_remote_voice_prompt(audio, window, cx)?;
            }
            "workspace_voice_choice" => {
                let voice = req["voice"].as_str().ok_or("Missing voice")?;
                self.select_remote_voice(voice, cx);
            }
            "voice_stop" => self.stop_remote_voice(cx),
            _ => return Ok(false),
        }
        cx.notify();
        Ok(true)
    }

    pub(super) fn synced_remote_dock(&self) -> Option<terminal_dock::Dock> {
        let state = &self.remote_workspace;
        state.overviews.get(&state.selected.as_ref()?.key())?.dock
    }

    fn remote_workspace_action(&mut self, req: Value, cx: &mut Context<Self>) {
        let Some(helper) = self.remote_workspace.helper.clone() else {
            return;
        };
        let Some(worker) = &self.remote_workspace.action_worker else {
            return;
        };
        let job = worker.submit(false, move || call::<Value>(&helper, req));
        cx.spawn(async move |this, cx| {
            if let Err(error) = job
                .recv()
                .await
                .unwrap_or_else(|_| Err("Another remote action is pending.".into()))
            {
                let _ = this.update(cx, |app, cx| {
                    app.remote_workspace.issue = Some(error);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn answer_remote_approval(&mut self, token: String, choice: String, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let (Some(peer), Some(chat), Some(helper), Some(worker)) = (
            state.selected.as_ref().filter(|peer| peer.allowed && peer.online && peer.control),
            state.chat.as_ref(), state.helper.clone(), state.action_worker.as_ref(),
        ) else { return };
        if state.answering_approval.is_some() { return; }
        let mut req = request("command", peer);
        req["command"] = json!({"action":"answer","chat_id":chat.id,"request_id":uuid::Uuid::new_v4(),
            "answer":{"approvalRequestId":token,"choice":choice}});
        let epoch = state.epoch;
        state.answering_approval = Some(token.clone());
        let job = worker.submit(false, move || call::<Value>(&helper, req).and_then(composer::payload));
        cx.spawn(async move |this, cx| {
            let result = job.recv().await.unwrap_or_else(|_| Err("Could not send the decision to the host.".into()));
            let _ = this.update(cx, |app, cx| {
                let state = &mut app.remote_workspace;
                if state.epoch != epoch { return; }
                state.answering_approval = None;
                match result {
                    Ok(_) => state.agent_approvals.retain(|item| item.approval_request_id != token),
                    Err(error) => state.issue = Some(error),
                }
                app.fetch_remote_activity(cx);
                cx.notify();
            });
        }).detach();
        cx.notify();
    }

    pub(super) fn new_remote_conversation(&mut self, window: &mut Window, cx: &mut Context<Self>) -> bool {
        if window.window_handle().window_id() != self.primary_window { return false }
        let state = &self.remote_workspace;
        let Some(peer) = state.selected.clone() else { return false };
        if !peer.online || !peer.allowed || !peer.control {
            self.remote_workspace.issue = Some("Agent control on this Mac is required to create a conversation.".into());
            cx.notify();
            return true;
        }
        if let Some(project) = state.project.clone().or_else(|| state.chat.as_ref().map(|chat| chat.project.clone())) {
            self.open_remote_chat(peer, Some(project), None, window, cx);
        } else {
            self.open_remote_composer_panel(ComposerPanel::Projects, point(px(208.), px(88.)), window, cx);
        }
        true
    }

    pub(super) fn remote_project_destination(&mut self, window: &Window, cx: &mut Context<Self>)
        -> Option<Result<ProjectDestination, ()>>
    {
        if window.window_handle().window_id() != self.primary_window { return None }
        let state = &mut self.remote_workspace;
        let peer = state.selected.clone()?;
        if let Some(parent) = state.project.clone().or_else(|| state.chat.as_ref().map(|chat| chat.project.clone()))
            && peer.allowed && peer.control && peer.online
        {
            return Some(Ok(ProjectDestination { device: peer, parent, request: uuid::Uuid::new_v4() }));
        }
        state.issue = Some("Select a writable project on the connected Mac as the parent folder.".into());
        cx.notify();
        Some(Err(()))
    }

    pub(super) fn create_project_on_remote(
        &mut self, destination: ProjectDestination, name: String, operation: Instant,
        window: &mut Window, cx: &mut Context<Self>,
    ) {
        let Some(helper) = self.remote_workspace.helper.clone() else {
            self.remote_project_created(operation, Err("The remote service is unavailable.".into()), cx);
            return;
        };
        use sha2::{Digest, Sha256};
        let mut digest = Sha256::new();
        digest.update(destination.request.as_bytes());
        digest.update(name.as_bytes());
        let identity: [u8; 32] = digest.finalize().into();
        let request_id = uuid::Uuid::from_bytes(identity[..16].try_into().unwrap());
        let mut req = request("command", &destination.device);
        req["command"] = json!({"action":"create_project","request_id":request_id,
            "project":destination.parent,"name":name});
        let job = cx.background_executor().spawn(async move { call::<Value>(&helper, req) });
        cx.spawn_in(window, async move |app, cx| {
            let result = job.await.and_then(|value| {
                let result = value.get("result").filter(|item| !item.is_null()).unwrap_or(&value);
                if let Some(error) = result["error"].as_str() { return Err(error.into()) }
                result["project"].as_str().map(str::to_owned)
                    .ok_or_else(|| "Waiting for the host to confirm project creation. Retry without changing the name.".into())
            });
            let _ = app.update_in(cx, |app, window, cx| {
                let focus = app.remote_project_created(operation, result.as_ref().map(|_| ()).map_err(Clone::clone), cx);
                if let Ok(project) = result {
                    let same_device = app.remote_workspace.selected.as_ref()
                        .is_some_and(|peer| peer.key() == destination.device.key());
                    if focus && same_device {
                        app.open_remote_chat(destination.device.clone(), Some(project), None, window, cx);
                    }
                    app.load_remote_tree(&destination.device, None, 0, cx);
                    app.refresh_remote_overviews(cx);
                }
            });
        }).detach();
    }

    pub(super) fn split_remote_chat(&mut self, source: &str, drag: &RemoteChatDrag, side: terminal_dock::Side, window: &mut Window, cx: &mut Context<Self>) {
        let destination = Self::remote_pane_key(&drag.peer, Some(&drag.chat));
        if !drag.peer.allowed || !self.add_chat_split_pane(source, &destination, side, window, cx) {
            return;
        }
        if !self.focus_remote_pane(&destination, cx) {
            self.open_remote_selection_for_pane(drag.peer.clone(), Some(drag.chat.project.clone()), Some(drag.chat.clone()), destination, cx);
        }
        self.focus.focus(window);
    }

    pub(super) fn retain_remote_split_panes(&mut self) {
        let split = self.chat_split.as_ref();
        self.remote_workspace.parked.retain(|id, _| split.is_some_and(|split| split.sessions.contains(id)));
    }

    pub(super) fn remote_feedback_pending(&self, cx: &App) -> bool {
        let state = &self.remote_workspace;
        (state.loading && state.transcript.is_none()) || state.pending_cli.is_some() ||
            self.remote_terminal_owner().and_then(|owner| state.remote_clis.get(&owner))
                .is_some_and(|cli| cli.selected && cli.panel.read(cx).launching())
    }

    pub(super) fn painting_remote_companion(&self) -> bool {
        self.remote_workspace.rendering_remote_peer
    }

    pub(super) fn render_parked_remote_pane(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.with_remote_pane(id, |app| {
            app.remote_workspace.rendering_remote_peer = true;
            let pending = app.remote_feedback_pending(cx);
            let content = app.remote_workspace_content(window, cx);
            let content = app.docked_remote_peer_conversation(content, window, cx);
            app.remote_workspace.rendering_remote_peer = false;
            app.pane_launch_feedback(id.to_owned(), pending, content)
        })
    }

    pub(super) fn focus_remote_terminal_pane(&mut self, owner: &workspace_ui::TerminalOwner, cx: &mut Context<Self>) -> bool {
        if self.remote_terminal_owner().as_ref() == Some(owner) { return true; }
        let id = self.remote_workspace.parked.keys()
            .find(|id| self.parked_remote_terminal_owner(id).as_ref() == Some(owner))
            .cloned();
        id.is_some_and(|id| self.focus_remote_pane(&id, cx))
    }

    pub(super) fn focus_remote_cli_by_panel(&mut self, panel: EntityId, cx: &mut Context<Self>) {
        let owner = self.remote_workspace.remote_clis.iter()
            .find(|(_, cli)| cli.panel.entity_id() == panel)
            .map(|(owner, _)| owner.clone());
        if let Some(owner) = owner {
            self.focus_remote_terminal_pane(&owner, cx);
        }
    }

    pub(super) fn remote_cli_by_metal(&self, id: uuid::Uuid, cx: &App) -> Option<Entity<terminal_ui::TerminalPanel>> {
        self.remote_workspace.remote_clis.values().find(|cli| cli.panel.read(cx).metal_id() == Some(id))
            .map(|cli| cli.panel.clone())
    }

    pub(super) fn remote_pane_id(&self) -> Option<&str> {
        let state = &self.remote_workspace;
        state.selected.as_ref()?;
        state.pane_id.as_deref()
    }

    fn remote_pane_key(peer: &Device, chat: Option<&RemoteChat>) -> String {
        chat.map_or_else(
            || format!("remote-pane:{}", uuid::Uuid::new_v4()),
            |chat| format!("remote-pane:{}:{}:{}", peer.node, peer.device, chat.id),
        )
    }

    fn park_remote_pane(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(id) = self.remote_pane_id().filter(|id| self.split_tab_for(id).is_some()).map(str::to_owned) else {
            return false;
        };
        self.remote_workspace.remember_draft(cx);
        let mut pane = RemotePane::default();
        self.remote_workspace.swap_pane(&mut pane);
        self.remote_workspace.parked.insert(id, pane);
        true
    }

    pub(super) fn with_remote_pane<T>(&mut self, id: &str, f: impl FnOnce(&mut Self) -> T) -> Option<T> {
        let mut pane = self.remote_workspace.parked.remove(id)?;
        self.remote_workspace.swap_pane(&mut pane);
        let result = f(self);
        self.remote_workspace.swap_pane(&mut pane);
        self.remote_workspace.parked.insert(id.to_owned(), pane);
        Some(result)
    }

    pub(in crate) fn with_remote_epoch<T>(&mut self, epoch: u64, f: impl FnOnce(&mut Self) -> T) -> Option<T> {
        if self.remote_workspace.epoch == epoch { return Some(f(self)); }
        let id = self.remote_workspace.parked.iter().find(|(_, pane)| pane.epoch == epoch)?.0.clone();
        self.with_remote_pane(&id, f)
    }

    pub(super) fn focus_remote_pane(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if self.remote_pane_id() == Some(id) { return true; }
        if !self.remote_workspace.parked.contains_key(id) { return false; }
        if !self.park_remote_pane(cx) { self.leave_remote_workspace(cx); }
        let Some(mut pane) = self.remote_workspace.parked.remove(id) else { return false; };
        self.remote_workspace.swap_pane(&mut pane);
        self.files_visible = self.remote_workspace.files.is_some();
        self.update_split_focus(id);
        self.remote_workspace.persist_selection();
        self.sync_chat_terminal_owner(cx);
        cx.notify();
        true
    }

    pub(super) fn remote_split_provider(&self) -> AgentKind {
        self.remote_workspace.chat.as_ref().map_or(self.remote_workspace.controls.draft_agent, |chat| agent(&chat.agent))
    }

    pub(super) fn remote_split_projects(&self) -> Vec<(String, PathBuf)> {
        let state = &self.remote_workspace;
        let Some(peer) = &state.selected else { return Vec::new(); };
        let mut projects = state.tree_branches.get(&peer.key()).and_then(|branch| branch.directory.as_ref())
            .map(|directory| directory.projects.iter().map(|p| (p.name.clone(), PathBuf::from(&p.path))).collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(project) = &state.project {
            let path = PathBuf::from(project);
            if !projects.iter().any(|(_, p)| *p == path) {
                projects.insert(0, (path.file_name().unwrap_or_default().to_string_lossy().into_owned(), path));
            }
        }
        projects
    }

    pub(super) fn remote_split_project_label(&self) -> String {
        let current = self.remote_workspace.project.as_deref().map(Path::new);
        self.remote_split_projects().into_iter()
            .find_map(|(label, path)| (Some(path.as_path()) == current).then_some(label))
            .unwrap_or_else(|| "this project".into())
    }

    pub(super) fn split_remote_view(&mut self, side: terminal_dock::Side, window: &mut Window, cx: &mut Context<Self>) {
        self.split_remote_choice(Some(side), None, None, false, false, window, cx);
    }

    pub(super) fn split_remote_choice(&mut self, direction: Option<terminal_dock::Side>, agent: Option<AgentKind>,
        project: Option<PathBuf>, cli: bool, shell: bool, window: &mut Window, cx: &mut Context<Self>) {
        let state = &self.remote_workspace;
        let provider = agent.unwrap_or_else(|| self.remote_split_provider());
        let project = project.map(|project| project.to_string_lossy().into_owned()).or_else(|| state.project.clone());
        let (Some(id), Some(peer), Some(project)) = (self.remote_pane_id(), state.selected.clone(), project) else { return; };
        let source = id.to_owned();
        let destination = Self::remote_pane_key(&peer, None);
        if self.add_workspace_split_pane(&source, &destination, direction, window, cx) {
            self.open_remote_selection_for_pane(peer, Some(project), None, destination, cx);
            self.remote_workspace.controls.draft_agent = provider;
            if cli {
                self.remote_workspace.terminal_only = shell;
                self.toggle_remote_cli(window, cx);
            } else {
                let composer = self.remote_composer(window, cx);
                composer.read(cx).focus_handle.focus(window);
            }
        }
    }

    fn open_remote_chat(
        &mut self,
        device: Device,
        project: Option<String>,
        chat: Option<RemoteChat>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_remote_selection(device, project, chat, cx);
        self.focus.focus(window);
    }

    fn open_remote_selection(
        &mut self,
        device: Device,
        project: Option<String>,
        chat: Option<RemoteChat>,
        cx: &mut Context<Self>,
    ) {
        let state = &self.remote_workspace;
        let matches = |peer: Option<&Device>, candidate: Option<&RemoteChat>| {
            peer.is_some_and(|peer| peer.key() == device.key())
                && candidate.map(|chat| &chat.id) == chat.as_ref().map(|chat| &chat.id)
        };
        if state.project == project && matches(state.selected.as_ref(), state.chat.as_ref()) { return; }
        let id = chat.as_ref().and_then(|_| state.parked.iter()
            .find(|(_, pane)| matches(pane.selected.as_ref(), pane.chat.as_ref())).map(|(id, _)| id.clone()))
            .unwrap_or_else(|| Self::remote_pane_key(&device, chat.as_ref()));
        if self.focus_remote_pane(&id, cx) { return; }
        self.open_remote_selection_for_pane(device, project, chat, id, cx);
    }

    fn open_remote_selection_for_pane(
        &mut self, device: Device, project: Option<String>, chat: Option<RemoteChat>,
        id: String, cx: &mut Context<Self>,
    ) {
        self.stop_remote_voice(cx);
        self.stop_dictation(cx);
        self.dismiss_slash(false, cx);
        self.park_remote_pane(cx);
        let state = &mut self.remote_workspace;
        state.remember_draft(cx);
        state.controls.reset_selection();
        if state.files.is_some()
            && (state.project != project
                || state
                    .selected
                    .as_ref()
                    .is_none_or(|peer| peer.key() != device.key()))
        {
            state.files = None;
            self.files_visible = false;
        }
        state.active_chats.clear();
        if let Some(chat) = &chat {
            state.remember_tab(device.key(), chat.clone());
        }
        state.split_draft = chat.is_none() && self.chat_split.as_ref().is_some_and(|split| split.sessions.contains(&id));
        state.pane_id = Some(id.clone());
        state.selected = Some(device);
        state.project = project;
        state.chat = chat;
        state.composer = None;
        state.invalidate();
        state.browse_worker = Worker::start("Kai remote history");
        state.composer_worker = Worker::start("Kai remote composer");
        state.voice_worker = Worker::start("Kai remote voice");
        state.activity_worker = Worker::start("Kai remote activity");
        state.stream_worker = Worker::start("Kai remote stream");

        state.persist_selection();
        self.update_split_focus(&id);
        self.sync_chat_terminal_owner(cx);
        self.queue_workspace_save(cx);
        self.fetch_remote_workspace(cx);
        self.start_remote_stream(cx);
        self.refresh_remote_composer(cx);
        if self.remote_workspace.stream.is_none() {
            self.fetch_remote_activity(cx);
        }
        cx.notify();
    }

    pub(super) fn leave_remote_workspace(&mut self, cx: &mut Context<Self>) {
        if self.remote_workspace.active() {
            self.stop_remote_voice(cx);
            self.stop_dictation(cx);
        }
        if self.park_remote_pane(cx) {
            self.files_visible = false;
            self.sync_chat_terminal_owner(cx);
            return;
        }
        if self.remote_workspace.files.take().is_some() {
            self.files_visible = false;
        }
        self.remote_workspace.remember_draft(cx);
        self.remote_workspace.composer = None;
        self.remote_workspace.close_chat();
        self.sync_chat_terminal_owner(cx);
    }

    pub(super) fn remote_terminal_owner(&self) -> Option<workspace_ui::TerminalOwner> {
        let (device, target) = self.remote_workspace.draft_key()?;
        Some(workspace_ui::TerminalOwner::Remote(device.0, device.1, target))
    }

    pub(super) fn parked_remote_terminal_owner(&self, id: &str) -> Option<workspace_ui::TerminalOwner> {
        let pane = self.remote_workspace.parked.get(id)?;
        let peer = pane.selected.as_ref()?.key();
        let target = pane.chat.as_ref().map_or_else(
            || {
                let project = pane.project.as_deref().unwrap_or_default();
                if pane.split_draft {
                    format!("split-draft:{}:{project}", id.trim_start_matches("remote-pane:"))
                } else {
                    format!("project:{project}")
                }
            },
            |chat| format!("chat:{}", chat.id),
        );
        Some(workspace_ui::TerminalOwner::Remote(peer.0, peer.1, target))
    }

    fn remote_project_access(&mut self, terminal: bool, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let Some(peer) = state
            .selected
            .clone()
            .filter(|peer| peer.online && peer.allowed)
        else {
            return;
        };
        let Some(project) = state
            .project
            .as_ref()
            .filter(|path| !standalone_project(path))
        else {
            state.issue = Some("Select a remote project to open its files or terminal.".into());
            cx.notify();
            return;
        };
        let mut req = request("request-project", &peer);
        req["project"] = json!(project);
        req["terminal"] = json!(terminal);
        let Some(helper) = state.helper.clone() else {
            return;
        };
        let Some(worker) = state.action_worker.as_ref() else {
            return;
        };
        let epoch = state.epoch;
        let job = worker.submit(false, move || call::<Value>(&helper, req));
        cx.spawn(async move |this, cx| {
            let result = job
                .recv()
                .await
                .unwrap_or_else(|_| Err("The access request was interrupted.".into()));
            let _ = this.update(cx, |app, cx| {
                if app.remote_workspace.epoch != epoch {
                    return;
                }
                app.notice = Some(match result {
                    Ok(_) if terminal => {
                        "Approve Files and Terminal on the host Mac, then open Terminal again."
                            .into()
                    }
                    Ok(_) => "Approve Files on the host Mac, then open Files again.".into(),
                    Err(error) => error,
                });
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn sync_remote_cli_visibility(&mut self, cx: &mut Context<Self>) {
        let active = self.remote_terminal_owner();
        for (owner, cli) in &self.remote_workspace.remote_clis {
            let mounted = active.as_ref() == Some(owner) || self.split_terminal_owners.contains(owner);
            let visible = self.terminal_workspace_visible && mounted && cli.selected;
            if cli.panel.read(cx).is_visible() != visible {
                cli.panel.update(cx, |panel, _| panel.set_visible(visible));
            }
        }
    }

    pub(super) fn remote_cli_selected(&self) -> bool {
        self.remote_workspace.terminal_only || self.remote_terminal_owner()
            .and_then(|owner| self.remote_workspace.remote_clis.get(&owner))
            .is_some_and(|cli| cli.selected)
    }

    pub(super) fn restore_remote_cli_focus(&self, window: &mut Window, cx: &Context<Self>) -> bool {
        let Some(cli) = self.remote_terminal_owner().and_then(|owner| self.remote_workspace.remote_clis.get(&owner))
            .filter(|cli| cli.selected) else { return false; };
        cli.panel.read(cx).focus_handle(cx).focus(window);
        true
    }

    pub(super) fn render_remote_cli(&self) -> Option<AnyElement> {
        let owner = self.remote_terminal_owner()?;
        let cli = self.remote_workspace.remote_clis.get(&owner).filter(|cli| cli.selected)?;
        Some(cli.panel.clone().into_any_element())
    }

    pub(super) fn toggle_remote_cli(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(owner) = self.remote_terminal_owner() else { return };
        let state = &mut self.remote_workspace;
        if let Some(cli) = state.remote_clis.get_mut(&owner) {
            if state.terminal_only {
                cli.panel.read(cx).focus_handle(cx).focus(window);
                return;
            }
            cli.selected = !cli.selected;
            cli.panel.update(cx, |panel, _| panel.set_visible(cli.selected));
            if cli.selected {
                cli.panel.read(cx).focus_handle(cx).focus(window);
            } else {
                self.focus.focus(window);
                self.fetch_remote_workspace(cx);
            }
            cx.notify();
            return;
        }
        let (Some(peer), Some(project), Some(helper), Some(worker)) = (
            state.selected.clone(), state.project.clone(), state.helper.clone(), &state.action_worker,
        ) else { unified::workspace_toast("Choose a host project to open a CLI.", true); return; };
        if !peer.online || !peer.allowed || !peer.control || !peer.supports("workspace.chat-cli") {
            unified::workspace_toast("This host is offline or does not support remote CLI sessions.", true);
            cx.notify();
            return;
        }
        if state.pending_cli.is_some() {
            return;
        }
        let ticket = uuid::Uuid::new_v4();
        state.pending_cli = Some(ticket);
        let epoch = state.epoch;
        let pane = state.pane_id.clone();
        let mut req = request("terminal-start", &peer);
        req["project"] = json!(project);
        req["command"] = if state.terminal_only {
            json!({"request_id":ticket,"program":"shell"})
        } else {
            state.chat.as_ref().map_or_else(
                || json!({"request_id":ticket,"program":state.controls.draft_agent.command()}),
                |chat| json!({"request_id":ticket,"cli_chat":chat.id}),
            )
        };
        let connector = helper.clone();
        let job = worker.submit(false, move || call::<RemoteTerminal>(&helper, req));
        cx.spawn_in(window, async move |this, cx| {
            let result = futures_lite::future::race(
                async { job.recv().await.unwrap_or_else(|_| Err("The CLI request was interrupted.".into())) },
                async { Timer::after(Duration::from_secs(12)).await; Err("The host did not open the CLI in time.".into()) },
            ).await;
            let _ = this.update_in(cx, |app, window, cx| {
                let focused = app.remote_workspace.epoch == epoch;
                app.with_remote_epoch(epoch, |app| {
                let state = &mut app.remote_workspace;
                if state.epoch != epoch || state.pending_cli != Some(ticket) { return; }
                state.pending_cli = None;
                match result {
                    Ok(record) => {
                        let panel = cx.new(|cx| terminal_ui::TerminalPanel::new_remote(
                            connector, &peer.node, &peer.device, &record.id, app.terminal_backend, cx,
                        ));
                        panel.update(cx, |panel, _| panel.return_to_chat_on_exit());
                        let key = owner.clone();
                        let shell_pane = state.terminal_only.then(|| state.pane_id.clone()).flatten();
                        let subscription = cx.subscribe_in(&panel, window, move |app, panel, event, window, cx| {
                            if !matches!(event, terminal_ui::TerminalEvent::Close) { return; }
                            app.remote_workspace.remote_clis.remove(&key);
                            if let Some(id) = &shell_pane {
                                if let Some(error) = panel.read(cx).error() {
                                    unified::pane_feedback_failed(id);
                                    unified::workspace_toast(error, true);
                                }
                                if app.split_tab_for(id).is_some() {
                                    app.close_remote_split_pane(id, window, cx);
                                }
                                return;
                            }
                            if app.remote_terminal_owner().as_ref() == Some(&key) {
                                if let Some(error) = panel.read(cx).error() {
                                    if let Some(owner) = app.remote_pane_id() { unified::pane_feedback_failed(owner); }
                                    unified::workspace_toast(error, true);
                                }
                                app.focus.focus(window);
                                app.fetch_remote_workspace(cx);
                            }
                            cx.notify();
                        });
                        if focused { panel.read(cx).focus_handle(cx).focus(window); }
                        state.remote_clis.insert(owner, RemoteCli { panel, selected: true, _subscription: subscription });
                    }
                    Err(error) => {
                        if let Some(id) = &pane { unified::pane_feedback_failed(id); }
                        unified::workspace_toast(&error, true);
                    }
                }
                cx.notify();
                });
            });
        }).detach();
    }

    pub(super) fn remote_action_issue(&self) -> Option<&str> {
        self.remote_workspace.issue.as_deref()
    }

    pub(super) fn toggle_pending_remote_terminal(&mut self, hide: bool) -> bool {
        let Some((_, visible)) = &mut self.remote_workspace.pending_terminal else { return false; };
        *visible = !hide && !*visible;
        true
    }

    pub(super) fn open_remote_terminal(
        &mut self,
        new: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_remote_terminal_with_restore(new, None, window, cx);
    }

    fn open_remote_terminal_with_restore(
        &mut self,
        new: bool,
        restore_chat: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(owner) = self.remote_terminal_owner() else { return; };
        let state = &mut self.remote_workspace;
        let (Some(peer), Some(helper), Some(worker)) = (
            state.selected.clone(),
            state.helper.clone(),
            state.action_worker.as_ref(),
        ) else {
            return;
        };
        if !peer.online || !peer.allowed {
            state.issue = Some("Reconnect to the host Mac before opening a terminal.".into());
            cx.notify();
            return;
        }
        let Some(project) = state.project.clone() else { return };
        if !peer.has_paired_access() && (!peer.control || !peer.terminal || peer.file_project != project) {
            self.remote_project_access(true, cx);
            return;
        }
        if state.pending_terminal.is_some() {
            return;
        }
        let ticket = uuid::Uuid::new_v4();
        state.pending_terminal = Some((ticket, true));
        let epoch = state.epoch;
        let mut req = request("terminal-start", &peer);
        req["project"] = json!(&project);
        req["command"] = json!({"request_id":ticket,"program":"shell"});
        if let Some(chat) = &restore_chat {
            req["command"]["restore_chat"] = json!(chat);
        }
        let mut list = request("terminals", &peer);
        list["project"] = json!(project);
        let connector = helper.clone();
        let job = worker.submit(false, move || {
            if !new && restore_chat.is_none()
                && let Some(terminal) =
                    call_list::<RemoteTerminal>(&helper, list)?
                        .into_iter()
                        .find(|terminal| terminal.program == "shell")
            {
                return Ok(terminal);
            }
            call::<RemoteTerminal>(&helper, req)
        });
        let peer = state.selected.clone().unwrap();
        cx.spawn_in(window, async move |this, cx| {
            let result = job
                .recv()
                .await
                .unwrap_or_else(|_| Err("The terminal request was interrupted.".into()));
            let _ = this.update_in(cx, |app, window, cx| {
                let focused = app.remote_workspace.epoch == epoch;
                let Some(Some(show)) = app.with_remote_epoch(epoch, |app| {
                    let state = &mut app.remote_workspace;
                    let (pending, show) = state.pending_terminal?;
                    if pending != ticket { return None; }
                    state.pending_terminal = None;
                    Some(show)
                }) else { return; };
                match result {
                    Ok(record)
                        if record.id.len() == 32
                            && record.id.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
                    {
                        let backend = app.terminal_backend;
                        let panel = cx.new(|cx| {
                            terminal_ui::TerminalPanel::new_remote(
                                connector,
                                &peer.node,
                                &peer.device,
                                &record.id,
                                backend,
                                cx,
                            )
                        });
                        if focused {
                            let id = app.register_terminal(panel, window, cx);
                            if show {
                                app.select_terminal(id, window, cx);
                            } else {
                                app.active_terminal = Some(id);
                                app.sync_terminal_visibility(cx);
                            }
                        } else {
                            app.attach_parked_terminal(owner, panel, window, cx);
                        }
                    }
                    Ok(_) => { app.with_remote_epoch(epoch, |app| {
                        app.remote_workspace.issue = Some("The host returned an invalid terminal ID.".into());
                    }); }
                    Err(error) => { app.with_remote_epoch(epoch, |app| app.remote_workspace.issue = Some(error)); }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn toggle_remote_files(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Entity<files::FileBrowser>> {
        let state = &mut self.remote_workspace;
        let (Some(peer), Some(helper), Some(project)) = (
            state.selected.clone(),
            state.helper.clone(),
            state.project.clone(),
        ) else {
            self.remote_project_access(false, cx);
            return None;
        };
        if !peer.online || !peer.allowed {
            state.issue = Some("Reconnect to the host Mac before browsing files.".into());
            cx.notify();
            return None;
        }
        if !peer.has_project_files(&project) {
            self.remote_project_access(false, cx);
            return None;
        }
        self.prepare_inspector_switch(pane_layout::Pane::Files);
        self.broker_panel = None;
        self.diff_view = None;
        self.diff_observer = None;
        self.diff_started = None;
        let browser = self
            .remote_workspace
            .files
            .get_or_insert_with(|| {
                let source = RemoteFiles {
                    helper,
                    peer,
                    root: PathBuf::from(project),
                };
                cx.new(|cx| files::FileBrowser::new_remote(source, cx))
            })
            .clone();
        let appearance = self.appearance;
        browser.update(cx, |files, cx| {
            files.set_appearance(appearance, cx);
            files.set_visible(true, cx);
        });
        self.files_visible = true;
        cx.notify();
        Some(browser)
    }

    pub(super) fn open_remote_citation(
        &mut self,
        url: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match citations::classify(url) {
            Ok(citations::Citation::Web(url)) => cx.open_url(&url),
            Ok(citations::Citation::File(citation)) => {
                if let Some(browser) = self.toggle_remote_files(cx) {
                    browser.update(cx, |files, cx| files.open_citation(citation, cx));
                }
            }
            Err(error) => {
                self.remote_workspace.issue = Some(error);
                cx.notify();
            }
        }
    }

    pub(super) fn remote_attachment(
        &self,
        ix: usize,
        name: &str,
        target: &str,
        cx: &Context<Self>,
    ) -> AnyElement {
        self.transcript_media_card(ix, name, target, None, true, cx)
            .unwrap_or_else(|| self.attachment_history_card_for(ix, name, target, true, cx))
    }

    pub(super) fn close_remote_split_pane(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let terminal_owner = if self.remote_pane_id() == Some(id) {
            self.remote_workspace.terminal_only.then(|| self.remote_terminal_owner()).flatten()
        } else {
            self.remote_workspace.parked.get(id).is_some_and(|pane| pane.terminal_only)
                .then(|| self.parked_remote_terminal_owner(id)).flatten()
        };
        if let Some(owner) = terminal_owner
            && let Some(cli) = self.remote_workspace.remote_clis.remove(&owner)
        {
            drop(cli._subscription);
            cli.panel.update(cx, |panel, _| panel.stop());
        }
        let neighbor = self.remove_split_pane(id);
        if self.remote_pane_id() != Some(id) {
            self.remote_workspace.parked.remove(id);
        } else {
            self.leave_remote_workspace(cx);
            if let Some(next) = neighbor {
                self.focus_split_chat(&next, window, cx);
            }
        }
        self.sync_chat_terminal_owner(cx);
        self.queue_workspace_save(cx);
        cx.notify();
    }

    pub(super) fn close_remote_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.remote_pane_id().filter(|id| self.split_tab_for(id).is_some()).map(str::to_owned) {
            self.close_remote_split_pane(&id, window, cx);
            return;
        }
        if self.motion && !native::reduced_motion() && !self.remote_tab_is_grouped() {
            self.remote_workspace.closing_tab = self.remote_tab().map(|(_, title)| (title, Instant::now()));
        }
        self.leave_remote_workspace(cx);
        self.focus.focus(window);
        cx.notify();
    }

    fn refresh_open_remote_branches(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let queries: Vec<_> = state
            .devices
            .iter()
            .filter(|device| device.online && device.allowed)
            .map(Device::key)
            .filter(|key| {
                state
                    .tree_branches
                    .get(key)
                    .is_some_and(|branch| branch.open)
            })
            .map(|device| TreeQuery {
                device,
                project: None,
                offset: 0,
            })
            .collect();
        for query in queries {
            state.queue_tree(query);
        }
        self.dispatch_remote_tree(cx);
    }

    fn refresh_remote_tree(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        state.tree_branches.retain(|key, _| {
            state
                .devices
                .iter()
                .any(|peer| peer.key() == *key && (!peer.online || peer.allowed))
        });
        self.rebuild_remote_tree();
        self.refresh_open_remote_branches(cx);
    }

    fn load_remote_tree(
        &mut self,
        device: &Device,
        project: Option<String>,
        offset: usize,
        cx: &mut Context<Self>,
    ) {
        self.remote_workspace.queue_tree(TreeQuery {
            device: device.key(),
            project,
            offset,
        });
        self.dispatch_remote_tree(cx);
    }

    fn dispatch_remote_tree(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        if state.tree_busy {
            return;
        }
        let (Some(worker), Some(helper)) = (&state.tree_worker, state.helper.clone()) else {
            return;
        };
        let mut request_and_peer = None;
        while let Some(query) = state.tree_queue.pop_front() {
            if let Some(peer) = state
                .devices
                .iter()
                .find(|peer| peer.key() == query.device && peer.online && peer.allowed)
                .cloned()
                && state
                    .tree_branches
                    .get(&query.device)
                    .is_some_and(|branch| {
                        branch.open
                            && query
                                .project
                                .as_ref()
                                .is_none_or(|p| branch.projects_open.contains(p))
                    })
            {
                request_and_peer = Some((query, peer));
                break;
            }
        }
        let Some((query, peer)) = request_and_peer else {
            return;
        };
        state.tree_busy = true;
        let mut params = request("workspace", &peer);
        if let Some(project) = &query.project {
            params["project"] = json!(project);
        }
        params["cursor"] = json!(query.offset.to_string());
        let job = worker.submit(false, move || call::<Directory>(&helper, params));
        cx.spawn(async move |this, cx| {
            let result = job.recv().await;
            let _ = this.update(cx, |app, cx| {
                let state = &mut app.remote_workspace;
                state.tree_busy = false;
                if state
                    .devices
                    .iter()
                    .any(|peer| peer.key() == query.device && peer.allowed)
                    && let Some(branch) = state.tree_branches.get_mut(&query.device)
                {
                    match result {
                        Ok(Ok(directory)) => {
                            let unscoped = query.project.is_none();
                            let issue_changed = branch.issue.take().is_some();
                            let changed = branch.receive(query.project, query.offset, directory);
                            if unscoped
                                && branch
                                    .directory
                                    .as_ref()
                                    .is_some_and(|dir| dir.next_offset > 0)
                            {
                                let missing: Vec<_> = branch
                                    .projects_open
                                    .iter()
                                    .filter(|path| !branch.project_pages.contains_key(*path))
                                    .cloned()
                                    .collect();
                                for project in missing {
                                    state.queue_tree(TreeQuery {
                                        device: query.device.clone(),
                                        project: Some(project),
                                        offset: 0,
                                    });
                                }
                            }
                            if changed || issue_changed {
                                app.rebuild_remote_tree();
                                cx.notify();
                            }
                        }
                        Ok(Err(error)) if branch.issue.as_ref() != Some(&error) => {
                            branch.issue = Some(error);
                            app.rebuild_remote_tree();
                            cx.notify();
                        }
                        _ => {}
                    }
                }
                app.dispatch_remote_tree(cx);
            });
        })
        .detach();
    }

    fn toggle_remote_device(&mut self, device: Device, cx: &mut Context<Self>) {
        let branch = self
            .remote_workspace
            .tree_branches
            .entry(device.key())
            .or_default();
        branch.open = !branch.open;
        let open = branch.open;
        let should_load = open && device.online && device.allowed;
        self.transition_remote_fold(Fold::Device(device.key()), open);
        self.rebuild_remote_tree();
        if should_load {
            self.load_remote_tree(&device, None, 0, cx);
        }
        cx.notify();
    }

    fn toggle_remote_project(&mut self, device: Device, project: String, cx: &mut Context<Self>) {
        if self.sidebar_split {
            self.show_remote_conversations(device, Some(project), cx);
            return;
        }
        let Some(branch) = self.remote_workspace.tree_branches.get_mut(&device.key()) else {
            return;
        };
        let open = if branch.projects_open.remove(&project) {
            false
        } else {
            branch.projects_open.insert(project.clone());
            true
        };
        let should_load = open
            && device.online
            && device.allowed
            && branch
                .directory
                .as_ref()
                .is_some_and(|dir| dir.next_offset > 0)
            && !branch.project_pages.contains_key(&project);
        self.transition_remote_fold(Fold::Project(device.key(), project.clone()), open);
        self.rebuild_remote_tree();
        if should_load {
            self.load_remote_tree(&device, Some(project), 0, cx);
        }
        cx.notify();
    }

    fn show_remote_conversations(
        &mut self,
        device: Device,
        project: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let same_scope = self
            .remote_workspace
            .selected
            .as_ref()
            .is_some_and(|peer| peer.key() == device.key())
            && self.remote_workspace.project == project;
        if !same_scope {
            self.open_remote_selection(device.clone(), project.clone(), None, cx);
        }
        if self.project_tree.chats_collapsed {
            self.toggle_sidebar_section(project_tree::SidebarSection::Chats, cx);
        }
        if let Some(branch) = self.remote_workspace.tree_branches.get_mut(&device.key()) {
            if let Some(path) = &project {
                branch.projects_open.insert(path.clone());
            } else {
                branch.chats_open = true;
            }
        }
        if device.online && device.allowed {
            self.load_remote_tree(&device, project, 0, cx);
        }
        self.rebuild_remote_tree();
        cx.notify();
    }

    pub(super) fn split_remote_scope(
        &self,
    ) -> Option<(String, Vec<SessionSummary>, Option<usize>)> {
        let state = &self.remote_workspace;
        let peer = state.selected.as_ref()?;
        let branch = state.tree_branches.get(&peer.key());
        let project = state.project.as_deref();
        let directory = branch.and_then(|branch| branch.directory.as_ref());
        let name = project.map_or_else(
            || state.label(peer),
            |path| {
                directory
                    .and_then(|directory| directory.projects.iter().find(|p| p.path == path))
                    .map_or_else(
                        || self.project_path_label(Path::new(path)),
                        |p| p.name.clone(),
                    )
            },
        );
        let page = match project {
            Some(path) => branch.and_then(|branch| branch.project_pages.get(path)),
            None => directory,
        };
        let mut seen = HashSet::new();
        let chats = page
            .into_iter()
            .chain(directory)
            .flat_map(|page| &page.chats)
            .filter(|chat| match project {
                Some(path) => Path::new(&chat.project).starts_with(path),
                None => chat.project.is_empty() || standalone_project(&chat.project),
            })
            .filter(|chat| seen.insert(chat.id.as_str()))
            .map(|chat| SessionSummary {
                id: chat.id.clone(),
                title: chat.title.clone(),
                agent: agent(&chat.agent),
                project_path: PathBuf::from(&chat.project),
                execution_path: None,
                updated_at: chat.updated_at,
                external_id: None,
                transcript_path: None,
            })
            .collect();
        Some((
            name,
            chats,
            page.map(|page| page.next_offset)
                .filter(|offset| *offset > 0),
        ))
    }

    pub(super) fn load_remote_split_page(&mut self, offset: usize, cx: &mut Context<Self>) {
        if let Some(device) = self.remote_workspace.selected.clone() {
            self.load_remote_tree(&device, self.remote_workspace.project.clone(), offset, cx);
        }
    }

    pub(super) fn open_remote_split_session(
        &mut self,
        session: &SessionSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(device) = self.remote_workspace.selected.clone() else {
            return;
        };
        let project = self.remote_workspace.project.clone();
        self.open_remote_chat(
            device,
            project,
            Some(RemoteChat {
                id: session.id.clone(),
                title: session.title.clone(),
                agent: session.agent.command().into(),
                project: session.project_path.to_string_lossy().into_owned(),
                updated_at: session.updated_at,
                terminal: false,
            }),
            window,
            cx,
        );
    }

    pub(super) fn remote_session_status(&self, id: &str) -> (bool, bool) {
        let state = &self.remote_workspace;
        (
            state.chat.as_ref().is_some_and(|chat| chat.id == id),
            state.active_chats.contains(id),
        )
    }

    fn begin_remote_rename(&mut self, device: Device, window: &mut Window, cx: &mut Context<Self>) {
        let label = self.remote_workspace.label(&device);
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_text(&label, cx);
            input
        });
        cx.subscribe(&input, |app, _, event, cx| {
            if matches!(event, input::InputEvent::Submit) {
                app.commit_remote_rename(cx);
            }
        })
        .detach();
        input.read(cx).focus_handle.focus(window);
        self.remote_workspace.rename = Some((device.node, input));
        cx.notify();
    }

    fn commit_remote_rename(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let Some((node, input)) = state.rename.take() else {
            return;
        };
        let text: String = input.read(cx).text().trim().chars().take(80).collect();
        if text.is_empty() {
            state.aliases.remove(&node);
        } else {
            state.aliases.insert(node, text);
        }
        if let Some(path) = state.alias_file.clone() {
            let aliases = state.aliases.clone();
            state.alias_revision += 1;
            let revision = state.alias_revision;
            let saved = state.aliases_saved.clone();
            cx.background_executor()
                .spawn(async move {
                    let _ = save_aliases(&path, &aliases, &saved, revision);
                })
                .detach();
        }
        cx.notify();
    }

    fn transition_remote_fold(&mut self, key: Fold, open: bool) {
        let state = &mut self.remote_workspace;
        if self.motion && !native::reduced_motion() {
            let from = state
                .tree_motion
                .get(&key)
                .map_or(if open { 0. } else { 1. }, |motion| motion.progress());
            state
                .tree_motion
                .insert(key, interaction_ui::WorkMotion::new(from, open));
        } else {
            state.tree_motion.remove(&key);
        }
    }

    pub(super) fn tick_remote_tree(&mut self, window: &Window, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        if state.tree_motion.is_empty() {
            return;
        }
        let mut start = None;
        let mut end = 0;
        for (index, folds) in state.tree_folds.iter().enumerate() {
            if folds
                .iter()
                .flatten()
                .any(|key| state.tree_motion.contains_key(key))
            {
                start.get_or_insert(index);
                end = index + 1;
            }
        }
        if let Some(start) = start {
            if let Some(list) = &state.tree_scroll {
                let anchor = list.logical_scroll_top();
                list.splice(start..end, end - start);
                list.scroll_to(anchor);
            }
            self.remeasure_remote_project_tree(start..end);
        }
        let state = &mut self.remote_workspace;
        let count = state.tree_motion.len();
        state
            .tree_motion
            .retain(|_, motion| self.motion && !native::reduced_motion() && motion.active());
        if count != state.tree_motion.len() {
            self.rebuild_remote_tree();
        }
        if !self.remote_workspace.tree_motion.is_empty() && window.is_window_active() {
            request_motion_frame(window, cx);
        }
    }

    pub(super) fn rebuild_remote_tree(&mut self) {
        let state = &mut self.remote_workspace;
        let mut rows = Vec::new();
        if state.status.is_some() || !state.devices.is_empty() {
            if state.status.as_ref().is_some_and(|status| {
                    !status.enabled || !status.requests.is_empty() || status.auth_url.is_some()
                })
            {
                rows.push(TreeRow::Setup);
            }
            for device in &state.devices {
                rows.push(TreeRow::Device(device.clone()));
                if let Some(branch) = state
                    .tree_branches
                    .get(&device.key())
                    .filter(|branch| branch.open || state.tree_motion.contains_key(&Fold::Device(device.key())))
                {
                    if device.online && !device.compatible {
                        rows.push(TreeRow::Note(
                            1,
                            "Update Kai on one or both Macs to connect.".into(),
                        ));
                    } else if device.online && !device.allowed {
                        rows.push(TreeRow::Connect(device.clone()));
                    } else {
                        if !device.online {
                            rows.push(TreeRow::Note(1, "Mac offline".into()));
                        } else if device.supports("pairing.mutual") && state.status.as_ref().is_some_and(|status| {
                            !status.trusted.iter().any(|trusted| trusted.key() == device.key())
                        }) {
                            rows.push(TreeRow::Connect(device.clone()));
                        }
                        branch.rows(device, &mut rows, self.sidebar_split, &state.tree_motion);
                    }
                }
            }
            if state.devices.is_empty()
                && state.status.as_ref().is_some_and(|status| status.enabled)
            {
                rows.push(TreeRow::Note(0, "No Kai devices found".into()));
            }
        }
        let prefix = state
            .tree_rows
            .iter()
            .zip(&rows)
            .take_while(|(a, b)| a == b)
            .count();
        let suffix = state.tree_rows[prefix..]
            .iter()
            .rev()
            .zip(rows[prefix..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let old_range = prefix..state.tree_rows.len() - suffix;
        let new_count = rows.len() - prefix - suffix;
        if let Some(scroll) = &state.tree_scroll {
            if !old_range.is_empty() || new_count > 0 {
                scroll.splice(old_range.clone(), new_count);
            }
        } else if !rows.is_empty() {
            state.tree_scroll = Some(ListState::new(rows.len(), ListAlignment::Top, px(160.)));
        }
        let mut device = None;
        let mut branch = None;
        state.tree_folds = rows
            .iter()
            .map(|row| match row {
                TreeRow::Device(peer) => {
                    device = Some(Fold::Device(peer.key()));
                    branch = None;
                    [None, None]
                }
                TreeRow::Project(peer, project, _) => {
                    branch = Some(Fold::Project(peer.key(), project.path.clone()));
                    [device.clone(), None]
                }
                TreeRow::Chats(peer, _) => {
                    branch = Some(Fold::Chats(peer.key()));
                    [device.clone(), None]
                }
                TreeRow::Chat(..) | TreeRow::More(..) | TreeRow::Note(2.., _) => {
                    [device.clone(), branch.clone()]
                }
                TreeRow::Connect(..) | TreeRow::Note(1, _) => [device.clone(), None],
                _ => [None, None],
            })
            .collect();
        state.tree_rows = rows;
        self.sync_remote_project_tree(old_range, new_count);
    }

    pub(super) fn remote_tree_count(&self) -> usize {
        self.remote_workspace.tree_rows.len()
    }

    pub(super) fn split_remote_tree(&self, cx: &Context<Self>) -> AnyElement {
        let Some(scroll) = &self.remote_workspace.tree_scroll else {
            return div().into_any_element();
        };
        let height = self
            .remote_workspace
            .tree_folds
            .iter()
            .map(|folds| {
                32. * folds.iter().flatten().fold(1_f32, |fraction, key| {
                    fraction
                        * self
                            .remote_workspace
                            .tree_motion
                            .get(key)
                            .map_or(1., |motion| motion.progress())
                })
            })
            .sum::<f32>()
            .min(224.);
        list(
            scroll.clone(),
            cx.processor(|app, index, _, cx| app.remote_tree_row(index, cx)),
        )
        .h(px(height))
        .max_h(relative(0.4))
        .flex_none()
        .px_2()
        .into_any_element()
    }

    pub(super) fn remote_tab(&self) -> Option<(AgentKind, String)> {
        let state = &self.remote_workspace;
        let device = state.selected.as_ref()?;
        Some(state.chat.as_ref().map_or_else(
            || (state.provider(), if state.terminal_only { "Terminal" } else { "New conversation" }.into()),
            |chat| (agent(&chat.agent), chat.title.clone()),
        ))
        .map(|(provider, title)| (provider, format!("{title} · {}", state.label(device))))
    }

    pub(super) fn closing_remote_tab(&self) -> Option<(&str, Instant)> {
        self.remote_workspace.closing_tab.as_ref()
            .filter(|(_, started)| started.elapsed() < workspace_motion::pane_duration())
            .map(|(title, started)| (title.as_str(), *started))
    }

    pub(super) fn remote_tab_is_grouped(&self) -> bool {
        let state = &self.remote_workspace;
        let (Some(peer), Some(chat)) = (&state.selected, &state.chat) else { return false };
        let key = peer.key();
        state.devices.iter().any(|device| device.allowed && device.online && device.key() == key)
            && (state.expanded_host_tabs.as_ref() == Some(&key)
                || state.tab_group_motion.get(&key).is_some_and(|motion| motion.active()))
            && state.tabs_for_device(&key).iter().any(|tab| tab.id == chat.id)
    }

    pub(super) fn remote_tab_close(&self, cx: &Context<Self>) -> impl IntoElement {
        div().id("remote-tab-close").debug_selector(|| "remote-tab-close".into())
            .p_1().rounded_sm().cursor_pointer().hover(|row| row.bg(rgba(GLASS_HOVER)))
            .child(icon("close", 11., MUTED))
            .on_click(cx.listener(|app, _, window, cx| {
                cx.stop_propagation();
                let state = &mut app.remote_workspace;
                if let (Some(peer), Some(chat)) = (&state.selected, &state.chat)
                    && let Some(tabs) = state.client_tabs.get_mut(&peer.key())
                {
                    tabs.retain(|item| item.id != chat.id);
                }
                app.close_remote_workspace(window, cx);
                app.queue_workspace_save(cx);
            }))
    }

    fn toggle_remote_tab_group(&mut self, key: DeviceKey, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let previous = state.expanded_host_tabs.take();
        let open = previous.as_ref() != Some(&key);
        if self.motion && !native::reduced_motion() {
            if let Some(previous) = previous {
                let from = state.tab_group_motion.get(&previous).map_or(1., |motion| motion.progress());
                state.tab_group_motion.insert(previous, interaction_ui::WorkMotion::new(from, false));
            }
            if open {
                let from = state.tab_group_motion.get(&key).map_or(0., |motion| motion.progress());
                state.tab_group_motion.insert(key.clone(), interaction_ui::WorkMotion::new(from, true));
            }
        } else {
            state.tab_group_motion.clear();
        }
        state.expanded_host_tabs = open.then_some(key);
        cx.notify();
    }

    pub(super) fn remote_tab_groups(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement {
        let state = &self.remote_workspace;
        div().flex().h_full().items_center().gap_1().children(
            state.devices.iter().filter(|peer| peer.allowed && peer.online).filter_map(|peer| {
                let key = peer.key();
                let tabs = state.tabs_for_device(&key);
                if tabs.is_empty() { return None; }
                let host_active = state.overviews.get(&key).map(|overview| overview.active.as_str());
                let open = state.expanded_host_tabs.as_ref() == Some(&key);
                let fraction = state.tab_group_motion.get(&key).filter(|motion| motion.active())
                    .map_or(if open { 1. } else { 0. }, |motion| {
                        if window.is_window_active() { request_motion_frame(window, cx); }
                        motion.progress()
                    });
                let group_width = (tabs.len() as f32 * 174.).min((f32::from(window.viewport_size().width) * 0.45).clamp(174., 550.));
                let group_id = SharedString::from(format!("host-tabs-{}", peer.node));
                Some(div().flex().items_center().flex_none()
                    .child(div().id(group_id.clone()).h(px(30.)).px_2().flex().items_center().gap_1()
                        .rounded_md().cursor_pointer().hover(|row| row.bg(rgba(GLASS_HOVER)))
                        .child(icon(peer.glyph(), 13., MUTED))
                        .child(div().max_w(px(124.)).truncate().text_size(theme::ui_px(12.)).child(state.label(peer)))
                        .child(icon(if open { "chevron-left" } else { "chevron-right" }, 11., MUTED))
                        .on_click(cx.listener(move |app, _, _, cx| app.toggle_remote_tab_group(key.clone(), cx))))
                    .when(fraction > 0., |row| row.child(div().w(px(group_width * fraction)).flex_none()
                        .h(px(30.)).overflow_hidden().child(div().id(SharedString::from(format!("{group_id}-scroll")))
                            .w(px(group_width)).h_full().flex().items_center().gap(px(6.))
                            .overflow_x_scroll().opacity(fraction)
                            .children(tabs.iter().map(|chat| {
                                let active = state.selected.as_ref().is_some_and(|current| current.key() == peer.key())
                                    && state.chat.as_ref().is_some_and(|current| current.id == chat.id);
                                let host_active = host_active == Some(chat.id.as_str());
                                let provider = agent(&chat.agent);
                                let peer = peer.clone();
                                let chat = chat.clone();
                                div().id(SharedString::from(format!("host-tab-{}-{}", peer.node, chat.id)))
                                    .w(px(168.)).h_full().flex_none().flex().items_center().gap_2().px_2()
                                    .rounded_md().cursor_pointer().hover(|tab| tab.bg(rgba(GLASS_HOVER)))
                                    .when(active, |tab| tab.bg(rgba(0xffffff20)))
                                    .child(provider_icon(agent_icon(provider), 14., agent_color(provider)))
                                    .child(div().flex_1().min_w(px(0.)).truncate().text_size(theme::ui_px(12.)).child(chat.title.clone()))
                                    .when(host_active, |tab| tab.child(div().size(px(4.)).rounded_full().bg(rgb(ACCENT))))
                                    .when(active, |tab| tab.child(self.remote_tab_close(cx)))
                                    .on_drag(RemoteChatDrag { peer: peer.clone(), chat: chat.clone() },
                                        |drag, _, _, cx| cx.new(|_| drag.clone()))
                                    .on_click(cx.listener(move |app, _, window, cx| {
                                        app.open_remote_chat(peer.clone(), Some(chat.project.clone()), Some(chat.clone()), window, cx);
                                        if chat.terminal && peer.has_paired_access() && app.active_terminal.is_none() {
                                            app.open_remote_terminal_with_restore(false, Some(chat.id.clone()), window, cx);
                                        }
                                    }))
                            }))))))
            }),
        )
    }

    pub(super) fn remote_tree_row(&self, index: usize, cx: &Context<Self>) -> AnyElement {
        use project_tree::{navigation_row, tree_chevron};
        let state = &self.remote_workspace;
        let Some(row) = state.tree_rows.get(index).cloned() else {
            return div().into_any_element();
        };
        let reveal = state.tree_folds[index]
            .iter()
            .flatten()
            .fold(1_f32, |fraction, key| {
                fraction
                    * state
                        .tree_motion
                        .get(key)
                        .map_or(1., |motion| motion.progress())
            });
        let chevron = |key: Fold, open| {
            state
                .tree_motion
                .get(&key)
                .map_or(if open { 1. } else { 0. }, |motion| motion.progress())
        };
        let row = match row {
            TreeRow::Setup => {
                return div()
                    .px_3()
                    .py_2()
                    .child(self.remote_workspace_setup(cx))
                    .when_some(
                        state.issue.as_ref().filter(|_| !state.active()).cloned(),
                        |row, error| {
                            row.child(
                                div()
                                    .text_color(rgb(MUTED))
                                    .text_size(theme::ui_px(11.))
                                    .child(error),
                            )
                        },
                    )
                    .into_any_element();
            }
            TreeRow::Device(device) => {
                if let Some((node, input)) = &state.rename
                    && *node == device.node
                {
                    return navigation_row("remote-device-rename-input", 14.)
                        .child(div().flex_1().min_w(px(0.)).child(input.clone()))
                        .child(
                            div()
                                .id("remote-label-save")
                                .p_1()
                                .child(icon("check", 14., ACCENT))
                                .on_click(
                                    cx.listener(|app, _, _, cx| app.commit_remote_rename(cx)),
                                ),
                        )
                        .into_any_element();
                }
                let id = SharedString::from(format!("remote-device-{}", device.node));
                let open = state
                    .tree_branches
                    .get(&device.key())
                    .is_some_and(|branch| branch.open);
                let renamed = device.clone();
                let label = state.label(&device);
                navigation_row(id.clone(), 8.)
                    .debug_selector(move || id.to_string())
                    .group("remote-device")
                    .tooltip(move |_, cx| {
                        let label = if device.online { "Online" } else { "Offline" };
                        cx.new(|_| tui_badges::Label(label.into())).into()
                    })
                    .child(tree_chevron(chevron(Fold::Device(device.key()), open)))
                    .child(icon(device.glyph(), 15., MUTED))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(label))
                    .child(
                        div()
                            .size(px(6.))
                            .flex_none()
                            .rounded_full()
                            .bg(rgb(if device.online { ACCENT } else { MUTED })),
                    )
                    .child(
                        div()
                            .id("remote-rename")
                            .p_1()
                            .rounded_sm()
                            .hover(|s| s.bg(rgba(GLASS_HOVER)))
                            .child(icon("more", 12., MUTED))
                            .on_click(cx.listener(move |app, _, window, cx| {
                                cx.stop_propagation();
                                app.begin_remote_rename(renamed.clone(), window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.toggle_remote_device(device.clone(), cx);
                    }))
            }
            TreeRow::Project(device, project, open) => {
                let id =
                    SharedString::from(format!("remote-project-{}-{}", device.node, project.path));
                let path = project.path.clone();
                let new = device.clone();
                let new_project = path.clone();
                navigation_row(id.clone(), 27.)
                    .debug_selector(move || id.to_string())
                    .group("remote-tree-project")
                    .child(tree_chevron(chevron(
                        Fold::Project(device.key(), path.clone()),
                        open,
                    )))
                    .child(project_tree::project_badge(
                        &project.name,
                        Path::new(&project.path),
                        22.,
                    ))
                    .child(div().flex_1().min_w(px(0.)).truncate().child(project.name))
                    .when(device.online && device.allowed && device.control, |row| {
                        row.child(
                            div()
                                .id("remote-project-new")
                                .size(px(20.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_sm()
                                .opacity(0.)
                                .group_hover("remote-tree-project", |s| s.opacity(1.))
                                .hover(|s| s.bg(rgba(GLASS_HOVER)))
                                .child(icon("plus", 12., MUTED))
                                .on_click(cx.listener(move |app, _, window, cx| {
                                    cx.stop_propagation();
                                    app.open_remote_chat(
                                        new.clone(),
                                        Some(new_project.clone()),
                                        None,
                                        window,
                                        cx,
                                    );
                                })),
                        )
                    })
                    .on_click(cx.listener(move |app, _, _, cx| {
                        cx.stop_propagation();
                        app.toggle_remote_project(device.clone(), path.clone(), cx);
                    }))
            }
            TreeRow::Chats(device, open) => navigation_row(
                SharedString::from(format!("remote-chats-{}", device.node)),
                27.,
            )
            .text_size(theme::ui_px(11.))
            .text_color(rgb(MUTED))
            .child(tree_chevron(chevron(Fold::Chats(device.key()), open)))
            .child("Chats")
            .on_click(cx.listener(move |app, _, _, cx| {
                cx.stop_propagation();
                if app.sidebar_split {
                    app.show_remote_conversations(device.clone(), None, cx);
                    return;
                }
                if let Some(branch) = app.remote_workspace.tree_branches.get_mut(&device.key()) {
                    branch.chats_open = !branch.chats_open;
                    let open = branch.chats_open;
                    app.transition_remote_fold(Fold::Chats(device.key()), open);
                    app.rebuild_remote_tree();
                    cx.notify();
                }
            })),
            TreeRow::Chat(device, chat, nested) => {
                let selected = state
                    .selected
                    .as_ref()
                    .is_some_and(|peer| peer.key() == device.key())
                    && state
                        .chat
                        .as_ref()
                        .is_some_and(|current| current.id == chat.id);
                let running = selected && state.active_chats.contains(&chat.id);
                let id = SharedString::from(format!("remote-chat-{}-{}", device.node, chat.id));
                let provider = agent(&chat.agent);
                navigation_row(id.clone(), if nested { 49. } else { 38. })
                    .debug_selector(move || id.to_string())
                    .when(selected, |row| row.bg(rgba(0xffffff14)))
                    .child(
                        provider_icon(agent_icon(provider), 12., agent_color(provider)).flex_none(),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .truncate()
                            .text_size(theme::ui_px(12.))
                            .child(chat.title.clone()),
                    )
                    .when(running, |row| {
                        row.child(div().size(px(6.)).rounded_full().bg(rgb(ACCENT)))
                    })
                    .on_drag(RemoteChatDrag { peer: device.clone(), chat: chat.clone() },
                        |drag, _, _, cx| cx.new(|_| drag.clone()))
                    .on_click(cx.listener(move |app, _, window, cx| {
                        cx.stop_propagation();
                        let project = (!chat.project.is_empty()).then(|| chat.project.clone());
                        app.open_remote_chat(
                            device.clone(),
                            project,
                            Some(chat.clone()),
                            window,
                            cx,
                        );
                    }))
            }
            TreeRow::More(device, project, offset) => navigation_row("remote-more", 43.)
                .text_size(theme::ui_px(11.))
                .text_color(rgb(MUTED))
                .child("Load more chats")
                .on_click(cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.load_remote_tree(&device, project.clone(), offset, cx);
                })),
            TreeRow::Note(depth, text) => {
                return project_tree::reveal_tree_row(
                    div().h(px(32.)).pl(px(14. + 13. * f32::from(depth)))
                        .flex().items_center().text_size(theme::ui_px(11.)).text_color(rgb(MUTED))
                        .child(div().truncate().child(text)),
                    32., reveal,
                );
            }
            TreeRow::Connect(device) => navigation_row("remote-ask-connect", 27.)
                .text_size(theme::ui_px(11.))
                .child(if device.supports("pairing.mutual") { "Connect Both Macs" } else { "Connect" })
                .on_click(cx.listener(move |app, _, _, cx| {
                    cx.stop_propagation();
                    app.remote_workspace_action(request("pair", &device), cx);
                    cx.notify();
                })),
        };
        project_tree::reveal_tree_row(row, 32., reveal)
    }

    fn remote_workspace_setup(&self, cx: &Context<Self>) -> AnyElement {
        let state = &self.remote_workspace;
        let Some(status) = &state.status else {
            return div()
                .text_color(rgb(MUTED))
                .text_size(theme::ui_px(11.))
                .child("Looking for Kai devices…")
                .into_any_element();
        };
        div()
            .flex()
            .flex_col()
            .gap_1()
            .text_size(theme::ui_px(11.))
            .when(!status.enabled, |v| {
                v.child(button(
                    "remote-enable",
                    "Enable Remote Access",
                    None,
                    true,
                    cx,
                    |app, _, cx| app.remote_workspace_action(json!({"action":"enable"}), cx),
                ))
            })
            .when_some(
                status
                    .auth_url
                    .as_ref()
                    .filter(|url| url.starts_with("https://login.tailscale.com/"))
                    .cloned(),
                |v, url| {
                    v.child(button(
                        "remote-login",
                        "Sign in to Tailscale",
                        None,
                        true,
                        cx,
                        move |_, _, cx| cx.open_url(&url),
                    ))
                },
            )
            .children(status.requests.iter().take(3).cloned().map(|approval| {
                let grant = approval.peer.clone();
                let grant_control = approval.peer.clone();
                let deny = approval.peer.clone();
                let project_request = !approval.project_requested.is_empty();
                let request_terminal = approval.terminal_requested;
                let project = approval.project_requested.clone();
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(div().text_color(rgb(MUTED)).child(if project_request {
                        let access = if request_terminal {
                            "files and terminal"
                        } else {
                            "files"
                        };
                        format!(
                            "{} requests {access} for {}",
                            approval.peer.name, approval.project_requested
                        )
                    } else if approval.control_requested {
                        format!("{} requests agent control", approval.peer.name)
                    } else {
                        format!("{} wants to pair. Pairing enables agent control, files, and terminals.", approval.peer.name)
                    }))
                    .when(!approval.peer.account.is_empty(), |request| {
                        request.child(
                            div()
                                .truncate()
                                .text_color(rgb(MUTED))
                                .child(approval.peer.account),
                        )
                    })
                    .child(button(
                        "remote-allow-control",
                        if request_terminal {
                            "Allow Files and Terminal"
                        } else if project_request {
                            "Allow Files"
                        } else if approval.control_requested {
                            "Enable Agent Control"
                        } else {
                            "Pair Mac"
                        },
                        None,
                        true,
                        cx,
                        move |app, _, cx| {
                            let grant = if project_request {
                                let mut grant = request("allow", &grant_control);
                                grant["control"] = json!(request_terminal);
                                grant["project"] = json!(project);
                                grant["terminal"] = json!(request_terminal);
                                grant
                            } else {
                                request("allow-workspace", &grant_control)
                            };
                            app.remote_workspace_action(grant, cx);
                        },
                    ).bg(rgba(0x728cff30)))
                    .child(
                        div().flex().gap_2()
                            .when(!approval.control_requested && !project_request, |row| {
                                row.child(button(
                                    "remote-allow",
                                    "Pair Read Only",
                                    None,
                                    true,
                                    cx,
                                    move |app, _, cx| {
                                        app.remote_workspace_action(request("allow", &grant), cx)
                                    },
                                ))
                            })
                            .child(button(
                                "remote-deny",
                                "Deny",
                                None,
                                true,
                                cx,
                                move |app, _, cx| {
                                    app.remote_workspace_action(request("deny", &deny), cx)
                                },
                            )),
                    )
            }))
            .into_any_element()
    }
}

impl Orbit {
    fn remote_composer(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextInput> {
        if let Some(input) = &self.remote_workspace.composer {
            return input.clone();
        }
        let draft = self.remote_workspace.draft_key()
            .and_then(|key| self.remote_workspace.drafts.get(&key)).cloned().unwrap_or_default();
        let input = cx.new(|cx| {
            let mut input = TextInput::new(window, cx);
            input.set_placeholder("Message Kai", cx);
            input.set_text(&draft, cx);
            input
        });
        cx.subscribe_in(&input, window, |app, _, event, window, cx| match event {
            input::InputEvent::Submit => app.submit_remote_composer(window, cx),
            input::InputEvent::Changed | input::InputEvent::CursorChanged => app.update_slash(window, cx),
            input::InputEvent::CommandPrevious => app.move_slash(-1, cx),
            input::InputEvent::CommandNext => app.move_slash(1, cx),
            input::InputEvent::CommandAccept => app.accept_slash(false, window, cx),
            input::InputEvent::CommandComplete => app.accept_slash(true, window, cx),
            input::InputEvent::FilesPasted(paths) => {
                app.upload_remote_attachments(paths.clone(), cx)
            }
            input::InputEvent::DictationCancelled => app.stop_dictation(cx),
            _ => {}
        })
        .detach();
        self.remote_workspace.composer = Some(input.clone());
        input
    }

    pub(super) fn remote_slash_agent(&self) -> Option<AgentKind> {
        self.remote_workspace.selected.as_ref()?;
        Some(self.remote_workspace.chat.as_ref().map_or(self.remote_workspace.controls.draft_agent, |chat| agent(&chat.agent)))
    }

    pub(super) fn supports_remote_slash(action: commands::Action) -> bool {
        use commands::Action::*;
        matches!(action, New | Files | Shell | Terminal | Latest | Sidebar | Conversations |
            Dictate | Attach | Models | Agents | Permissions | Stop | Interrupt | Steer |
            ReadOnly | Edit | Delegate | Motion | Projects)
    }

    pub(super) fn submit_remote_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.remote_prompt_input() else { return };
        let text = input.read(cx).text();
        if let Some(command) = commands::name(text.trim()).and_then(|name| commands::lookup(self.remote_slash_agent().unwrap_or(self.agent), name)) {
            if Self::supports_remote_slash(command.action) {
                self.apply_remote_slash(command, window, cx);
            } else {
                self.remote_workspace.issue = Some(format!("/{} is currently available in local chats.", command.name));
                cx.notify();
            }
        } else {
            self.send_remote_workspace(cx);
        }
    }

    pub(super) fn apply_remote_slash(&mut self, command: commands::Command, window: &mut Window, cx: &mut Context<Self>) {
        use commands::Action::*;
        let Some(input) = self.remote_prompt_input() else { return };
        let text = input.read(cx).text();
        let args = text.trim().split_once(char::is_whitespace).map_or("", |(_, args)| args.trim()).to_owned();
        let mut remaining = text.to_string();
        if let Some(span) = input.read(cx).command_span().filter(|span| span.start > 0) {
            remaining.replace_range(span, "");
        } else {
            remaining.clear();
        }
        let anchor = input.read(cx).bounds().map_or(point(px(32.), px(40.)), |bounds| bounds.origin);
        self.dismiss_slash(true, cx);
        if command.action == Delegate {
            input.update(cx, |input, cx| input.set_text(&format!("/delegate {args}"), cx));
            if !args.is_empty() { self.send_remote_workspace(cx); }
            return;
        }
        input.update(cx, |input, cx| input.set_text(&remaining, cx));
        match command.action {
            New => if let Some(peer) = self.remote_workspace.selected.clone() {
                self.open_remote_chat(peer, self.remote_workspace.project.clone(), None, window, cx);
            },
            Files => self.toggle_files(window, cx),
            Shell | Terminal => self.toggle_terminal(window, cx),
            Latest => self.jump_remote_latest(),
            Sidebar => self.toggle_sidebar(),
            Conversations => self.toggle_conversations(cx),
            Dictate => self.toggle_dictation(window, cx),
            Attach => self.choose_remote_attachments(cx),
            Models | Agents => self.open_remote_composer_panel(ComposerPanel::Model, anchor, window, cx),
            Projects => self.open_remote_composer_panel(ComposerPanel::Projects, anchor, window, cx),
            Permissions => self.open_remote_composer_panel(ComposerPanel::Permissions, anchor, window, cx),
            Stop | Interrupt => self.remote_run_control("stop", None, cx),
            Steer => if !args.is_empty() { self.remote_run_control("steer", Some(args), cx); },
            ReadOnly | Edit => {
                self.remote_workspace.controls.edits = Some(command.action == Edit);
                self.remote_workspace.controls.access = Some(runtime::CodexAccess::Workspace);
            },
            Motion => { self.motion = !self.motion; self.queue_workspace_save(cx); }
            _ => {}
        }
        cx.notify();
    }

    fn jump_remote_latest(&self) {
        let state = &self.remote_workspace;
        if let Some(list) = &state.transcript_list {
            list.scroll_to(ListOffset { item_ix: state.visible_history_len() +
                state.live_transcript.as_ref().map_or(0, |live| live.rows.len()), offset_in_item: px(0.) });
        }
    }

    pub(super) fn start_remote_frame_probe(owner: WindowHandle<Orbit>, cx: &mut App) {
        cx.spawn(async move |cx| {
            Timer::after(Duration::from_secs(2)).await;
            let chat: RemoteChat = serde_json::from_value(json!({
                "id":"frame-probe","agent":"codex","title":"Remote streaming",
                "project":"/remote/performance"
            })).unwrap();
            let messages = (0..72).flat_map(|index| [
                runtime::TranscriptMessage {
                    role: "user".into(), text: format!("Review component {index}."), code_language: None,
                },
                runtime::TranscriptMessage {
                    role: "assistant".into(), code_language: None,
                    text: format!("## Component {index}\n\n{}\n\n```rust\nlet status = ready();\n```",
                        "The measured response preserves layout while the host continues streaming. ".repeat(5)),
                },
            ]).chain([runtime::TranscriptMessage {
                role: "user".into(), text: "Follow up with live progress.".into(), code_language: None,
            }]).collect();
            let rows = Arc::new(canonical_remote_rows(&chat, messages));
            let flow = chat_flow::Index::for_transcript(&rows, true, None);
            owner.update(cx, |app, window, cx| {
                app.motion = true;
                app.open_remote_chat(Device {
                    node: "frame-probe-host".into(), device: "frame-probe-host".into(), name: "Fixture Mac".into(),
                    allowed: true, online: true, ..Default::default()
                }, Some(chat.project.clone()), Some(chat.clone()), window, cx);
                app.remote_workspace.apply_history(rows.clone(), rows.len(), flow, false, 0);
                app.remote_workspace.live = serde_json::from_value(json!({
                    "chat": chat.id, "status": "Thinking"
                })).ok();
                window.activate_window();
                cx.notify();
            }).unwrap();
            Timer::after(Duration::from_millis(500)).await;

            for streaming in [false, true] {
                let producer = async_channel::bounded(1);
                if streaming {
                    let (send, _) = producer.clone();
                    let chat = chat.clone();
                    cx.background_executor().spawn(async move {
                        for revision in 0..160 {
                            let text = format!("{}\n\nChunk {revision}", "### Tool output\n\nInspecting the workspace while the transcript stays scrollable.\n\n".repeat(20));
                            let rows = canonical_remote_rows(&chat, vec![runtime::TranscriptMessage {
                                role: "assistant".into(), text, code_language: None,
                            }]);
                            let live = LiveTranscript {
                                flow: Arc::new(chat_flow::Index::new(&rows)), rows: Arc::new(rows),
                                status: "Running terminal command".into(),
                            };
                            let _ = send.try_send(live);
                            Timer::after(Duration::from_millis(32)).await;
                        }
                    }).detach();
                }
                let mut samples = Vec::new();
                let mut last: Option<Instant> = None;
                let mut updates = 0;
                for index in 0..150 {
                    let (sent, received) = async_channel::bounded::<Instant>(1);
                    owner.update(cx, |app, window, cx| {
                        let state = &mut app.remote_workspace;
                        if let Ok(live) = producer.1.try_recv() {
                            state.receive_live(live);
                            updates += 1;
                        }
                        let list = state.transcript_list.as_ref().unwrap();
                        let at = index % 100;
                        list.scroll_to(ListOffset {
                            item_ix: 10 + at.min(100 - at), offset_in_item: px((index % 7) as f32),
                        });
                        window.on_frame_submitted(move |_, _| { let _ = sent.try_send(Instant::now()); });
                        cx.notify();
                    }).unwrap();
                    let now = futures_lite::future::race(
                        async { received.recv().await.ok() },
                        async { Timer::after(Duration::from_secs(4)).await; None },
                    ).await.expect("No rendered remote workspace frame");
                    if index >= 10 && let Some(previous) = last {
                        samples.push(now.duration_since(previous).as_secs_f64() * 1000.);
                    }
                    last = Some(now);
                }
                samples.sort_by(f64::total_cmp);
                assert!(!streaming || updates > 10, "The workload did not exercise live transcript updates");
                eprintln!("KAI_REMOTE_FRAMES streaming={streaming} samples={} updates={updates} p50_ms={:.2} p95_ms={:.2} max_ms={:.2}",
                    samples.len(), samples[samples.len() / 2], samples[samples.len() * 95 / 100], samples.last().unwrap());
            }
            let _ = cx.update(|cx| cx.quit());
        }).detach();
    }

    async fn probe_remote_split(window: WindowHandle<Orbit>, cx: &mut AsyncApp) {
        let wait = |millis| Timer::after(Duration::from_millis(millis));
        let deadline = Instant::now() + Duration::from_secs(25);
        let original = loop {
            if let Some(id) = window.update(cx, |app, window, _| {
                window.activate_window();
                app.remote_workspace.transcript.as_ref().filter(|rows| rows.len() > 1)?;
                app.remote_pane_id().map(str::to_owned)
            }).unwrap() { break id; }
            assert!(Instant::now() < deadline, "Established chat did not load");
            wait(120).await;
        };
        if std::env::var_os("KAI_REMOTE_TERMINAL_SPLIT_PROBE").is_some() {
            let terminal = window.update(cx, |app, window, cx| {
                app.split_remote_choice(None, None, None, true, true, window, cx);
                assert!(app.remote_workspace.terminal_only);
                assert!(app.remote_workspace.chat.is_none());
                assert!(app.remote_prompt_input().is_none());
                assert!(app.remote_workspace.parked[&original].transcript.is_some());
                app.remote_pane_id().unwrap().to_owned()
            }).unwrap();
            let deadline = Instant::now() + Duration::from_secs(20);
            let panel = loop {
                let panel = window.update(cx, |app, _, cx| {
                    let owner = app.remote_terminal_owner()?;
                    let cli = app.remote_workspace.remote_clis.get(&owner)?;
                    cli.panel.read(cx).probe_has_text("%").then(|| cli.panel.clone())
                }).unwrap();
                if let Some(panel) = panel { break panel; }
                assert!(Instant::now() < deadline, "Terminal-only split did not open its host shell");
                wait(120).await;
            };
            window.update(cx, |app, _, cx| {
                assert!(app.focus_remote_pane(&original, cx));
                assert!(app.remote_workspace.parked[&terminal].terminal_only);
                assert!(panel.read(cx).is_visible());
                assert!(app.focus_remote_pane(&terminal, cx));
                assert!(app.remote_prompt_input().is_none());
                assert!(panel.read(cx).probe_command("exit\r"));
            }).unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let closed = window.update(cx, |app, _, _| app.remote_pane_id() == Some(&original)
                    && app.split_tab_for(&terminal).is_none()).unwrap();
                if closed { break; }
                assert!(Instant::now() < deadline, "Closing the terminal did not restore the original chat");
                wait(120).await;
            }
            eprintln!("KAI_REMOTE_TERMINAL_ONLY_SPLIT_OK");
            return;
        }
        let (other, original_epoch) = window.update(cx, |app, window, cx| {
            let epoch = app.remote_workspace.epoch;
            app.split_remote_view(terminal_dock::Side::Right, window, cx);
            let other = app.remote_pane_id().unwrap().to_owned();
            assert_ne!(other, original);
            assert_eq!(app.chat_split.as_ref().unwrap().sessions.len(), 2);
            assert!(app.remote_workspace.parked[&original].transcript_list.is_some());
            (other, epoch)
        }).unwrap();
        let marker = format!("KAI_SPLIT_RESULT_{}", uuid::Uuid::new_v4().simple());
        window.update(cx, |app, window, cx| {
            let input = app.remote_composer(window, cx);
            input.update(cx, |input, cx| input.set_text(
                &format!("Use /bin/sleep 3 and then reply exactly {marker}. Do not make any file changes."), cx));
            app.submit_remote_composer(window, cx);
            assert!(app.remote_workspace.submitting());
            assert!(app.focus_remote_pane(&original, cx));
            assert_eq!(app.remote_workspace.epoch, original_epoch);
            assert!(app.remote_workspace.transcript.is_some());
        }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(75);
        let mut running = false;
        loop {
            let (busy, done) = window.update(cx, |app, _, _| {
                let pane = &app.remote_workspace.parked[&other];
                let busy = pane.host_task_running || pane.live_transcript.is_some();
                let complete = pane.transcript.as_ref().is_some_and(|rows| rows.iter().any(|row|
                    !chat_flow::user(row) && match &row.block {
                        MessageBlock::Paragraph(text) | MessageBlock::RichParagraph { text, .. } => text.trim().contains(&marker),
                        _ => false,
                    }));
                assert_eq!(app.remote_pane_id(), Some(original.as_str()));
                (busy, complete)
            }).unwrap();
            running |= busy;
            if done { break; }
            assert!(Instant::now() < deadline, "The background pane did not receive the saved reply");
            wait(150).await;
        }
        assert!(running, "The background pane showed no live progress");
        eprintln!("KAI_REMOTE_SPLIT_BACKGROUND_PROGRESS_OK");

        window.update(cx, |app, window, cx| {
            app.motion = false;
            app.terminal_backend = terminal::Backend::Ghostty;
            app.open_remote_terminal(true, window, cx);
        }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
        let panel = loop {
            let ready = window.update(cx, |app, _, cx| app.active_terminal_panel().cloned()
                .filter(|panel| panel.read(cx).probe_has_text("%"))).unwrap();
            if let Some(panel) = ready { break panel; }
            assert!(Instant::now() < deadline, "Host shell did not start");
            wait(150).await;
        };
        for height in [210., 350.] {
            window.update(cx, |app, _, cx| {
                app.dock_terminal(terminal_dock::Side::Bottom, cx);
                app.resize_pane(pane_layout::Pane::Terminal, height, cx);
                assert_eq!(app.effective_terminal_dock().height, height);
            }).unwrap();
            wait(850).await;
            let (columns, rows) = cx.update(|cx| panel.read(cx).probe_grid()).unwrap();
            let marker = format!("KAI_SIZE_{height}");
            let command = format!("/usr/bin/printf 'KAI_SIZE_%s=' {height}; /bin/stty size\r");
            assert!(cx.update(|cx| panel.read(cx).probe_command(&command)).unwrap());
            let deadline = Instant::now() + Duration::from_secs(10);
            let expected = format!("{marker}={rows} {columns}");
            loop {
                if cx.update(|cx| panel.read(cx).probe_has_text(&expected)).unwrap() { break; }
                assert!(Instant::now() < deadline, "Host PTY did not report {expected}");
                wait(120).await;
            }
            eprintln!("KAI_REMOTE_PTY_RESIZE_OK {columns}x{rows}");
        }
        assert!(cx.update(|cx| panel.read(cx).probe_command("exit\r")).unwrap());

        window.update(cx, |app, window, cx| app.toggle_remote_cli(window, cx)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let ready = window.update(cx, |app, _, cx| {
                let key = app.remote_terminal_owner().unwrap();
                app.remote_workspace.remote_clis.get(&key).filter(|cli| cli.selected)
                    .is_some_and(|cli| cli.panel.read(cx).probe_has_text("Codex"))
            }).unwrap();
            if ready { break; }
            assert!(Instant::now() < deadline, "Host Codex TUI did not render");
            wait(150).await;
        }
        window.update(cx, |app, window, cx| {
            let key = app.remote_terminal_owner().unwrap();
            let cli = app.remote_workspace.remote_clis[&key].panel.clone();
            assert!(app.focus_remote_pane(&other, cx));
            assert!(cli.read(cx).is_visible());
            assert!(app.focus_remote_pane(&original, cx));
            assert_eq!(app.remote_workspace.remote_clis[&key].panel.entity_id(), cli.entity_id());
            cli.read(cx).probe_command("/exit\r");
        }).unwrap();
        eprintln!("KAI_REMOTE_TUI_FOCUS_OK");
    }

    pub(super) fn start_remote_send_probe(window: WindowHandle<Orbit>, cx: &mut App) -> anyhow::Result<()> {
        let project = std::env::var("KAI_REMOTE_SEND_PROBE_PROJECT")?;
        let node = std::env::var("KAI_REMOTE_SEND_PROBE_NODE")?;
        let established = std::env::var("KAI_REMOTE_SEND_PROBE_CHAT").ok();
        anyhow::ensure!(Path::new(&project).is_absolute(), "Use a fixture project on the paired host");
        cx.spawn(async move |cx| {
            let start = Instant::now();
            loop {
                let ready = window.update(cx, |app, window, cx| {
                    let peer = app.remote_workspace.devices.iter()
                        .find(|peer| peer.node == node && peer.allowed && peer.control && peer.online).cloned();
                    if let Some(peer) = peer {
                        let chat = established.as_ref().map(|id| RemoteChat {
                            id: id.clone(), title: "Transcript fixture".into(),
                            agent: "codex".into(), project: project.clone(),
                            updated_at: 0, terminal: false,
                        });
                        app.open_remote_chat(peer, Some(project.clone()), chat, window, cx);
                        true
                    } else { false }
                }).expect("Probe workspace closed");
                if ready { break; }
                assert!(start.elapsed() < Duration::from_secs(40), "The paired fixture host did not become reachable");
                Timer::after(Duration::from_millis(200)).await;
            }

            if std::env::var_os("KAI_REMOTE_SPLIT_PROBE").is_some() {
                Self::probe_remote_split(window, cx).await;
                let _ = cx.update(|cx| cx.quit());
                return;
            }
            let tag = format!("KAI-UI-{}", uuid::Uuid::new_v4().simple());
            if std::env::var_os("KAI_REMOTE_STOP_PROBE").is_some() {
                let followup = format!("{tag}-AFTER-STOP");
                window.update(cx, |app, window, cx| {
                    let input = app.remote_composer(window, cx);
                    input.update(cx, |input, cx| input.set_text(
                        &format!("Run /bin/sleep 40 using the shell. Make no file changes. Reply exactly {tag}-FIRST-FINISHED after it exits."), cx));
                    app.submit_remote_composer(window, cx);
                }).expect("Probe workspace closed");
                let started = Instant::now();
                let run = loop {
                    if let Some(run) = window.update(cx, |app, _, _| app.remote_workspace.host_run_id.clone())
                        .expect("Probe workspace closed")
                    { break run; }
                    assert!(started.elapsed() < Duration::from_secs(60), "The Studio did not start the first turn");
                    Timer::after(Duration::from_millis(150)).await;
                };
                Timer::after(Duration::from_secs(3)).await;
                window.update(cx, |app, window, cx| {
                    assert_eq!(app.remote_workspace.host_run_id.as_deref(), Some(run.as_str()));
                    app.remote_run_control("stop", None, cx);
                    let key = app.remote_workspace.draft_key().unwrap();
                    assert!(app.remote_workspace.stopping.contains_key(&key));
                    let input = app.remote_composer(window, cx);
                    input.update(cx, |input, cx| input.set_text(
                        &format!("Reply exactly {followup}. Do not use tools or change files."), cx));
                    app.submit_remote_composer(window, cx);
                    assert!(app.remote_workspace.sending.contains(&key));
                }).expect("Probe workspace closed");
                let stopped = Instant::now();
                loop {
                    let rendered = window.update(cx, |app, _, _| {
                        app.remote_workspace.transcript.as_ref().is_some_and(|rows| rows.iter().any(|row|
                            !chat_flow::user(row) && match &row.block {
                                MessageBlock::Paragraph(text) | MessageBlock::RichParagraph { text, .. } => text.trim() == followup,
                                _ => false,
                            }))
                    }).expect("Probe workspace closed");
                    if rendered { break; }
                    assert!(stopped.elapsed() < Duration::from_secs(34), "The follow-up did not run promptly after Stop");
                    Timer::after(Duration::from_millis(150)).await;
                }
                eprintln!("KAI_REMOTE_STOP_IMMEDIATE_FOLLOWUP_RENDERED");
                let _ = cx.update(|cx| cx.quit());
                return;
            }
            let mut protected = None;
            if established.is_some() {
                let deadline = Instant::now() + Duration::from_secs(20);
                loop {
                    protected = window.update(cx, |app, _, _| {
                        app.remote_workspace.transcript.clone().filter(|rows| rows.len() >= 2)
                    }).expect("Probe workspace closed");
                    if protected.is_some() { break; }
                    assert!(Instant::now() < deadline, "The established remote chat loaded only one message");
                    Timer::after(Duration::from_millis(100)).await;
                }
            }
            let assert_history = |state: &State, previous: &[Row]| {
                let visible = state.visible_history_len();
                assert!(visible >= previous.len(), "Sending hid saved rows from the transcript");
                assert!(state.transcript_list.as_ref().is_some_and(|list| list.item_count() >= visible));
                assert!(state.transcript.as_ref().is_some_and(|rows| rows.windows(previous.len()).any(|window| {
                    window.iter().zip(previous).all(|(after, before)| after.role == before.role && after.block == before.block)
                })), "Sending replaced previously displayed transcript rows");
            };
            for turn in 1..=2 {
                let marker = format!("{tag}-{turn}");
                let start = Instant::now();
                loop {
                    let posted = window.update(cx, |app, window, cx| {
                        let Some(input) = app.remote_prompt_input() else { return false };
                        input.update(cx, |input, cx| input.set_text(&format!("Reply exactly {marker}. No tools or file changes."), cx));
                        app.submit_remote_composer(window, cx);
                        assert!(input.read(cx).text().is_empty(), "Send did not clear the composer");
                        assert!(app.remote_workspace.submitting(), "The composer did not dispatch");
                        if let Some(previous) = &protected { assert_history(&app.remote_workspace, previous); }
                        true
                    }).expect("Probe workspace closed");
                    if posted { break; }
                    assert!(start.elapsed() < Duration::from_secs(12), "The live remote composer did not appear");
                    Timer::after(Duration::from_millis(100)).await;
                }
                eprintln!("KAI_REMOTE_COMPOSER_SUBMITTED {turn}");
                loop {
                    let done = window.update(cx, |app, _, _| {
                        if let Some(previous) = &protected { assert_history(&app.remote_workspace, previous); }
                        app.remote_workspace.transcript.as_ref().is_some_and(|rows|
                            rows.iter().any(|row| !chat_flow::user(row) && match &row.block {
                                MessageBlock::Paragraph(text) | MessageBlock::RichParagraph { text, .. } => text.trim() == marker,
                                _ => false,
                            }))
                    }).expect("Probe workspace closed");
                    if done { break; }
                    assert!(start.elapsed() < Duration::from_secs(95), "The remote conversation did not render its saved reply");
                    Timer::after(Duration::from_millis(200)).await;
                }
                protected = window.update(cx, |app, _, _| app.remote_workspace.transcript.clone())
                    .expect("Probe workspace closed");
                eprintln!("KAI_REMOTE_COMPOSER_RENDERED_REPLY {turn}");
            }
            window.update(cx, |app, window, cx| {
                let state = &app.remote_workspace;
                let peer = state.selected.clone().expect("The selected peer disappeared");
                let original = state.chat.clone().expect("The saved chat disappeared");
                app.new_conversation(window, cx);
                assert!(app.remote_workspace.active() && app.remote_workspace.chat.is_none());
                assert!(app.remote_workspace.tabs_for_device(&peer.key()).iter().any(|tab| tab.id == original.id));
                app.open_remote_chat(peer, Some(original.project.clone()), Some(original), window, cx);
            }).expect("Probe workspace closed");
            let reopening = Instant::now();
            loop {
                let restored = window.update(cx, |app, _, _| {
                    app.remote_workspace.transcript.as_ref().is_some_and(|rows| (1..=2).all(|turn| {
                        let marker = format!("{tag}-{turn}");
                        rows.iter().any(|row| !chat_flow::user(row) && match &row.block {
                            MessageBlock::Paragraph(text) | MessageBlock::RichParagraph { text, .. } => text.trim() == marker,
                            _ => false,
                        })
                    }))
                }).expect("Probe workspace closed");
                if restored { break }
                assert!(reopening.elapsed() < Duration::from_secs(25), "Switching to a new chat lost the previous transcript");
                Timer::after(Duration::from_millis(150)).await;
            }
            eprintln!("KAI_REMOTE_NEW_CHAT_RETAINS_PRIOR_TAB_AND_HISTORY");
            if let Ok(parent) = std::env::var("KAI_REMOTE_PROJECT_PROBE_PARENT") {
                assert!(Path::new(&parent).is_absolute());
                let name = format!("Kai UI test {}", &tag[tag.len() - 8..]);
                let path = Path::new(&parent).join(&name);
                window.update(cx, |app, window, cx| {
                    let peer = app.remote_workspace.selected.clone().unwrap();
                    app.open_remote_chat(peer, Some(parent), None, window, cx);
                    app.new_project_folder(window, cx);
                    assert!(app.submit_remote_project_probe(&name, window, cx), "The shared project editor did not open on the host");
                }).expect("Probe workspace closed");
                let creating = Instant::now();
                loop {
                    let created = window.update(cx, |app, _, _| {
                        app.remote_workspace.project.as_deref().is_some_and(|project| Path::new(project) == path)
                            && app.lifecycle_editor.is_none()
                    }).expect("Probe workspace closed");
                    if created { break; }
                    assert!(creating.elapsed() < Duration::from_secs(25), "The project editor did not open the newly created remote folder");
                    Timer::after(Duration::from_millis(150)).await;
                }
                eprintln!("KAI_REMOTE_PROJECT_CREATED_VIA_SHARED_EDITOR");
            }
            eprintln!("KAI_REMOTE_SEND_PROBE_OK");
            let _ = cx.update(|cx| cx.quit());
        }).detach();
        Ok(())
    }

    fn recover_remote_send(&mut self, cx: &mut Context<Self>) {
        let state = &mut self.remote_workspace;
        let Some(device) = state.selected.as_ref().filter(|peer| peer.online && peer.allowed && peer.control).cloned() else { return };
        let (Some(key), Some(helper)) = (state.draft_key(), state.helper.clone()) else { return };
        if state.sending.contains(&key) || state.sending.len() >= 4 || state.receipt_checked.contains(&key) { return; }
        let Some((prompt, command)) = state.pending_sends.get(&key).cloned() else { return };
        self.dispatch_remote_send(key, device, helper, prompt, command, false, cx);
    }

    pub(super) fn send_remote_workspace(&mut self, cx: &mut Context<Self>) {
        let running = self.remote_running();
        let state = &mut self.remote_workspace;
        let Some(device) = state
            .selected
            .as_ref()
            .filter(|p| p.allowed && p.online && p.control)
            .cloned()
        else {
            return;
        };
        let Some(input) = state.composer.clone() else {
            return;
        };
        let Some(helper) = state.helper.clone() else {
            return;
        };
        let Some(key) = state.draft_key() else { return };
        if state.sending.contains(&key) {
            return;
        }
        let pending = state.pending_sends.get(&key);
        if state.sending.len() >= 4 || pending.is_none() && state.pending_sends.len() >= 64 {
            state.issue = Some("Finish checking other pending sends before sending another message.".into());
            cx.notify();
            return;
        }
        let draft = input.read(cx).text();
        let prompt = if draft.trim().is_empty() {
            pending.map_or_else(String::new, |(text, _)| text.clone())
        } else {
            draft.trim().to_owned()
        };
        if prompt.is_empty() && running {
            self.remote_run_control("stop", None, cx);
            return;
        }
        if running
            && commands::name(&prompt) == Some("steer")
            && let Some((_, text)) = prompt.split_once(char::is_whitespace)
            && !text.trim().is_empty()
        {
            self.remote_run_control("steer", Some(text.trim().to_owned()), cx);
            return;
        }
        if state.controls.attachment_pending
            || (prompt.is_empty() && state.controls.attachments.is_empty())
        {
            return;
        }
        let task = if let Some((_, remainder)) = prompt.split_once(char::is_whitespace)
            && commands::name(&prompt) == Some("delegate")
        {
            format!(
                "Delegate this task to the appropriate coding agent and summarize the result: {}",
                remainder.trim()
            )
        } else {
            prompt.clone()
        };
        let mut task = task;
        if !state.controls.attachments.is_empty() {
            task.push_str("\n\nFiles attached in the host project:\n");
            for (_, path) in &state.controls.attachments {
                task.push_str(&format!("- {path}\n"));
            }
        }
        let limit = if device.supports("workspace.long-prompts") { teams::MAX_REMOTE_PROMPT_BYTES } else { teams::MAX_TASK_BYTES };
        if task.len() > limit {
            state.issue = Some(format!("Keep the prompt and file references under {} KB.", limit / 1024));
            cx.notify();
            return;
        }
        let command = if let Some((text, prior)) = pending {
            if text != &prompt {
                state.issue =
                    Some("Check the host’s receipt before changing a pending message.".into());
                cx.notify();
                return;
            }
            prior.clone()
        } else {
            let id = uuid::Uuid::new_v4().to_string();
            let mut command = if let Some(chat) = &state.chat {
                json!({"action":"prompt_chat","request_id":id,"chat_id":chat.id,"prompt":task})
            } else if let Some(project) = &state.project {
                let title: String = if prompt.is_empty() {
                    "Attached files".into()
                } else {
                    prompt.chars().take(60).collect()
                };
                json!({"action":"create_chat","request_id":id,"project":project,"prompt":task,
                    "agent":state.controls.draft_agent.command(),"name":title})
            } else {
                return;
            };
            if device.supports("workspace.composer") {
                state.controls.apply_turn(&mut command);
                if commands::name(&prompt) == Some("delegate") {
                    command["orchestrator"] = json!(true);
                }
            }
            command
        };
        let fresh = pending.is_none();
        if !running && fresh && self.motion && !native::reduced_motion() {
            state.sent_from = Some(state.history_len());
        }
        if !running && fresh {
            state.submitted_preview = Some((prompt.clone(), state.latest_prompt_end, Instant::now()));
            state.dirty_native_history();
        }
        if fresh && state.pending_sends.values().map(|(_, command)| command.to_string().len()).sum::<usize>() + command.to_string().len() > 1_048_576 {
            state.issue = Some("Kai still has messages awaiting host confirmation. Try again after they are delivered.".into());
            cx.notify();
            return;
        }
        state.pending_sends.insert(key.clone(), (prompt.clone(), command.clone()));
        input.update(cx, |input, cx| input.set_text("", cx));
        self.dispatch_remote_send(key, device, helper, prompt, command, fresh, cx);
    }

    fn dispatch_remote_send(
        &mut self, key: (DeviceKey, String), device: Device, helper: PathBuf,
        prompt: String, command: Value, fresh: bool, cx: &mut Context<Self>,
    ) {
        let epoch = self.remote_workspace.epoch;
        self.remote_workspace.sending.insert(key.clone());
        self.remote_workspace.receipt_checked.insert(key.clone());
        let prior_stop = self.remote_workspace.stopping.get(&key).cloned();
        let mut request = request("command", &device);
        request["command"] = command.clone();
        let durable = if fresh {
            self.save_workspace_now(cx);
            self.workspace_store.as_ref().map(|store| store.flush())
        } else { None };
        let executor = cx.background_executor().clone();
        cx.spawn(async move |this, cx| {
            if let Some(stop) = prior_stop {
                // The host must decide Stop against the earlier run before
                // this follow-up can enter that chat's execution queue.
                let _ = stop.recv().await;
            }
            let saved = if let Some(durable) = durable {
                executor.spawn(async move {
                    matches!(durable.recv_timeout(Duration::from_secs(10)), Ok(Ok(())))
                }).await
            } else { true };
            let result = if saved {
                let mut result = Err("Could not confirm the host’s receipt".to_owned());
                for delay in [0, 1, 3] {
                    if delay > 0 {
                        executor.timer(Duration::from_secs(delay)).await;
                    }
                    let (helper, request) = (helper.clone(), request.clone());
                    result = executor.spawn(async move { call::<Value>(&helper, request) }).await;
                    if result.as_ref().is_ok_and(|value| !matches!(value["state"].as_str(), Some("uncertain" | "pending"))) {
                        break;
                    }
                }
                result
            } else {
                Ok(json!({"error":"Kai couldn’t save this message locally. Try sending it again."}))
            };
            let _ = this.update(cx, |app, cx| {
                app.remote_workspace.sending.remove(&key);
                let handled = app.with_remote_epoch(epoch, |app| {
                let state = &mut app.remote_workspace;
                if state.draft_key().as_ref() != Some(&key) { return false; }
                match result.clone() {
                    Ok(value)
                        if matches!(value["state"].as_str(), Some("uncertain" | "pending")) =>
                    {
                        state.issue = Some(
                            "Waiting for the host’s receipt. Retry the same message to check it."
                                .into(),
                        );
                    }
                    Ok(value) => {
                        let issue = value["error"]
                            .as_str()
                            .or_else(|| value["result"]["error"].as_str())
                            .map(str::to_owned);
                        if issue.is_none() {
                            let status = value["result"]["status"].as_str().or_else(|| value["status"].as_str());
                            if status == Some("Queued") {
                                state.host_task_terminal = false;
                                state.queue_caption = Some("Queued on host".into());
                            }
                            state.pending_sends.remove(&key);
                            state.receipt_checked.remove(&key);
                            state.controls.attachments.clear();
                            if state.chat.is_none()
                                && let Some(id) = value["result"]["chat_id"]
                                    .as_str()
                                    .or_else(|| value["chat_id"].as_str())
                            {
                                let chat = RemoteChat {
                                    id: id.to_owned(),
                                    title: command["name"]
                                        .as_str()
                                        .unwrap_or("New conversation")
                                        .into(),
                                    agent: command["agent"].as_str().unwrap_or("codex").into(),
                                    project: state.project.clone().unwrap_or_default(),
                                    updated_at: chrono::Utc::now().timestamp(),
                                    terminal: false,
                                };
                                if let Some(device) = state.selected.as_ref().map(Device::key) {
                                    state.remember_tab(device, chat.clone());
                                }
                                state.chat = Some(chat);
                                // Only the focused pane owns the workspace selection file.
                            }
                        } else {
                            state.pending_sends.remove(&key);
                            state.receipt_checked.remove(&key);
                            state.sent_from = None;
                            state.submitted_preview = None;
                            state.dirty_native_history();
                            if let Some(current_input) = &state.composer
                                && current_input.read(cx).text().is_empty()
                            {
                                current_input.update(cx, |input, cx| input.set_text(&prompt, cx));
                            } else if state.composer.is_none() {
                                state.drafts.entry(key.clone()).or_insert(prompt.clone());
                            }
                        }
                        state.issue = issue;
                    }
                    Err(error) => state.issue = Some(error),
                }
                app.fetch_remote_workspace(cx);
                app.start_remote_stream(cx);
                app.fetch_remote_activity(cx);
                app.refresh_open_remote_branches(cx);
                true
                }).unwrap_or(false);
                if !handled && let Ok(value) = &result
                    && !matches!(value["state"].as_str(), Some("uncertain" | "pending"))
                {
                    let state = &mut app.remote_workspace;
                    state.pending_sends.remove(&key);
                    state.receipt_checked.remove(&key);
                    if (value["error"].is_string() || value["result"]["error"].is_string()) && !prompt.is_empty() {
                        state.drafts.entry(key).or_insert(prompt);
                    }
                }
                if app.remote_workspace.epoch == epoch {
                    app.remote_workspace.persist_selection();
                    app.sync_chat_terminal_owner(cx);
                }
                app.queue_workspace_save(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn remote_status_caption(&self) -> Option<&str> {
        let state = &self.remote_workspace;
        state.selected.as_ref().filter(|peer| peer.allowed && peer.online)?;
        if let Some(status) = self.dictation_status.filter(|_| self.speech.is_some()) {
            Some(status)
        } else if !state.agent_approvals.is_empty() {
            Some("Waiting for your decision")
        } else if state.submitting() {
            Some("Sending")
        } else if state.selected_chat_running() {
            state.live_transcript.as_ref().map(|live| live.status.as_str())
                .or_else(|| state.live.as_ref().map(|live| live.status.as_str()))
                .or(state.queue_caption.as_deref()).or(Some("Working on host"))
        } else if state.has_pending_send() {
            Some("Waiting for host confirmation")
        } else {
            state.queue_caption.as_deref()
        }
    }

    fn remote_approval_tray(&self, cx: &Context<Self>) -> impl IntoElement {
        let state = &self.remote_workspace;
        div().w_full().max_w(px(800.)).flex().flex_col().gap_2()
            .children(state.agent_approvals.iter().filter_map(|item| {
                let runtime::NativeRequestKind::Approval { choices } = &item.request.kind else {
                    return None;
                };
                Some(div().px_3().py_2().rounded_md().border_1().border_color(rgba(0xebebeb1a))
                    .bg(rgba(GLASS_HOVER)).flex().flex_col().gap_2()
                    .debug_selector(|| "remote-agent-approval".into())
                    .child(item.request.title.clone())
                    .child(div().text_size(theme::ui_px(11.)).text_color(rgb(MUTED))
                        .child(item.request.detail.clone()))
                    .child(div().flex().flex_wrap().gap_2().children(choices.iter().enumerate().map(|(index, choice)| {
                        let id = item.approval_request_id.clone();
                        let choice_id = choice.id.clone();
                        div().id(SharedString::from(format!("remote-answer-{id}-{index}")))
                            .px_3().py_1().rounded_md().bg(rgba(GLASS_HOVER))
                            .when(state.answering_approval.is_none(), |row| row.cursor_pointer()
                                .hover(|row| row.bg(rgba(0xffffff24)))
                                .on_click(cx.listener(move |app, _, _, cx| {
                                    app.answer_remote_approval(id.clone(), choice_id.clone(), cx)
                                })))
                            .child(choice.label.clone())
                    }))))
            }))
    }

    pub(super) fn remote_workspace_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if self.remote_workspace.terminal_only {
            let content = self.render_remote_cli().unwrap_or_else(|| {
                let mut body = div().flex_1().flex().items_center().justify_center();
                if self.remote_workspace.pending_cli.is_none() {
                    body = body.child(context_ui::header_control("retry-host-terminal", false)
                        .px_3().child("Retry terminal")
                        .on_click(cx.listener(|app, _, window, cx| app.toggle_remote_cli(window, cx))));
                }
                body.into_any_element()
            });
            return self.conversation_pane()
                .child(self.conversation_header_for("Terminal".into(), None, false, false,
                    self.files_visible, None, window, cx))
                .child(div().flex_1().min_h(px(0.)).child(content)).into_any_element();
        }
        if let Some(cli) = self.render_remote_cli() {
            let title = self.remote_workspace.chat.as_ref().map_or_else(|| "Host CLI".into(), |chat| chat.title.clone());
            return self.conversation_pane()
                .child(self.conversation_header_for(title, None, false, false,
                    self.files_visible, Some(self.remote_workspace.provider()), window, cx))
                .child(div().flex_1().min_h(px(0.)).child(cli)).into_any_element();
        }
        self.tick_remote_transcript_motion(window, cx);
        use workspace_motion::Part;
        let frame = unified::workspace_frame(self.motion);
        if frame.animating() && window.is_window_active() {
            request_motion_frame(window, cx);
        }
        let device = self
            .remote_workspace
            .selected
            .clone()
            .expect("selected remote workspace");
        let title = self
            .remote_workspace
            .chat
            .as_ref()
            .map(|chat| chat.title.clone())
            .or_else(|| {
                self.remote_workspace
                    .tree_branches
                    .get(&device.key())?
                    .directory
                    .as_ref()?
                    .projects
                    .iter()
                    .find(|p| Some(&p.path) == self.remote_workspace.project.as_ref())
                    .map(|project| project.name.clone())
            })
            .unwrap_or_else(|| self.remote_workspace.label(&device));
        let provider = self
            .remote_workspace
            .chat
            .as_ref()
            .map(|chat| agent(&chat.agent));
        let input = (device.allowed
            && device.online
            && device.control
            && (self.remote_workspace.chat.is_some() || self.remote_workspace.project.is_some()))
        .then(|| self.remote_composer(window, cx));
        let content = self
            .conversation_pane()
            .child(self.conversation_header_for(
                title,
                None,
                self.remote_workspace.live.is_some(),
                false,
                self.files_visible && self.remote_workspace.files.is_some(),
                Some(provider.unwrap_or(AgentKind::Codex)),
                window,
                cx,
            ))
            .when(!device.online, |s| {
                s.child(
                    div()
                        .px_4()
                        .py_2()
                        .text_color(rgb(MUTED))
                        .child("Mac offline"),
                )
            })
            .when_some(self.remote_workspace.issue.clone().or_else(|| self.remote_workspace.history_issue.clone()), |s, error| {
                s.child(div().px_4().py_2().text_color(rgb(MUTED)).child(error))
            })
            .when(
                self.remote_workspace
                    .transcript
                    .as_ref()
                    .is_some_and(|rows| rows.is_empty())
                    && self.remote_workspace.live_transcript.is_none()
                    && self.remote_workspace.chat.is_some(),
                |s| {
                    let message = if device.supports("workspace.canonical-history") {
                        "This conversation has no messages yet."
                    } else {
                        "Update Kai on the host Mac to load its original transcript."
                    };
                    s.child(div().px_4().py_2().text_color(rgb(MUTED)).child(message))
                },
            )
            .when(
                self.remote_workspace.pages.cursor.is_some() && device.online && device.allowed,
                |s| {
                    let loading = self.remote_workspace.loading_earlier;
                    s.child(div().h(px(28.)).flex_none().flex().items_center().justify_center().gap_2()
                        .when(loading && self.motion && !native::reduced_motion(), |row|
                            row.child(Self::history_loading_indicator(("remote-history-loading", self.remote_workspace.epoch))))
                        .child(button(
                            "remote-older",
                            if loading { "Loading history…" } else { "Load earlier messages" },
                            None,
                            !loading && !self.remote_workspace.earlier_after_load,
                            cx,
                            |app, _, cx| app.fetch_remote_page(true, cx),
                        )))
                },
            )
            .child(
                frame
                    .container(Part::Content)
                    .min_h(px(0.))
                    .flex_1()
                    .flex()
                    .flex_col()
                    .when_some(
                        self.remote_workspace
                            .transcript_list
                            .clone()
                            .filter(|_| device.allowed),
                        |s, scroll| {
                            s.child(
                                list(
                                    scroll,
                                    cx.processor({
                                        let epoch = self.remote_workspace.epoch;
                                        move |app, index, window, cx| app.with_remote_epoch(epoch, |app| {
                                            app.remote_workspace_row(index, window, cx)
                                        }).unwrap_or_else(|| div().into_any_element())
                                    }),
                                )
                                .flex_1()
                                .min_h(px(0.)),
                            )
                        },
                    )
                    .when(
                        self.remote_workspace.transcript_list.is_none()
                            && device.online
                            && device.allowed,
                        |s| {
                            if self.remote_workspace.history_issue.is_some() {
                                s.justify_center().items_center().child(button("retry-remote-history",
                                    "Retry loading conversation", None, !self.remote_workspace.loading, cx,
                                    |app, _, cx| {
                                        app.remote_workspace.history_issue = None;
                                        app.fetch_remote_workspace(cx);
                                    }))
                            } else if self.remote_workspace.chat.is_some() {
                                s
                            } else {
                                s.justify_center().items_center()
                                    .child(div().text_color(rgb(MUTED)).child("Select a chat"))
                            }
                        },
                    ),
            )
            .when_some(self.remote_status_caption(), |pane, status| pane.child(
                Self::conversation_status(self.live_indicator.clone().into_any_element(),
                    activity_caption(status), None)))
            .when_some(self.remote_workspace.submitted_preview.as_ref(), |pane, (prompt, _, started)| {
                let motion = self.motion && !native::reduced_motion();
                let duration = workspace_motion::pane_duration().as_secs_f32();
                let progress = (started.elapsed().as_secs_f32() / duration).clamp(0., 1.);
                if motion && progress < 1. && window.is_window_active() {
                    request_motion_frame(window, cx);
                }
                let fraction = if motion { workspace_motion::ease_out(progress) } else { 1. };
                pane.child(div().flex_none().w_full().px_4().flex().justify_center().child(
                    div().w_full().max_w(px(800.)).relative().top(px(8. * (1. - fraction))).opacity(fraction)
                        .child(Self::conversation_message_alignment(true,
                            self.conversation_message_body(0, true, false, provider.unwrap_or(AgentKind::Codex))
                                .child(if prompt.is_empty() { "Attached files".to_owned() } else { prompt.clone() })))))
            })
            .when(!device.control && device.allowed && device.online, |pane| {
                let peer = device.clone();
                pane.child(frame.container(Part::Composer).flex_none().px_4().pb_3().child(
                    div().w_full().flex().items_center().gap_2().flex_wrap()
                        .child(div().text_size(theme::ui_px(11.)).text_color(rgb(MUTED))
                            .child("This Mac is paired read-only."))
                        .child(button("remote-request-control", "Request Agent Control", None, true, cx,
                            move |app, _, cx| app.remote_workspace_action(request("request-control", &peer), cx))),
                ))
            })
            .when_some(input, |s, input| {
                let label = self.remote_workspace.label(&device);
                s.child(frame.container(Part::Composer).flex_none().child(
                    div().w_full().flex().flex_col().items_center()
                    .child(self.remote_approval_tray(cx)).child(
                    self.conversation_composer_for(
                        None,
                        input,
                        Some((
                            provider.unwrap_or(self.remote_workspace.controls.draft_agent),
                            label,
                        )),
                        window,
                        cx,
                    )),
                ))
            });
        content.into_any_element()
    }

    fn remote_workspace_row(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.transcript_row(
            ix,
            window.viewport_size().width - px(self.sidebar_width),
            window,
            cx,
            true,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Branch, Device, Directory, Message, RemoteChat, State, StreamUpdate, TreeRow, Worker,
        call_list, save_aliases, stream_chat,
    };
    use gpui::{AppContext, TestAppContext};
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::prelude::v1::test;
    use std::{
        collections::HashMap,
        sync::{Mutex, atomic::AtomicBool, mpsc},
        time::Duration,
    };

    #[gpui::test]
    fn a_remote_tab_splits_beside_a_local_chat_and_focus_keeps_both_views(cx: &mut TestAppContext) {
        use super::*;
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            app.materialize_blank_tab(true, cx);
            let local = app.active.clone().unwrap();
            let peer = Device { node: "split-host".into(), device: "one".into(), online: true, allowed: true, ..Default::default() };
            let chat = serde_json::from_value(json!({
                "id": "remote-target", "title": "Remote target", "agent": "codex", "project": "/remote"
            })).unwrap();
            app.split_remote_chat(&local, &RemoteChatDrag { peer, chat }, terminal_dock::Side::Right, window, cx);
            let remote = app.remote_pane_id().unwrap().to_owned();
            let epoch = app.remote_workspace.epoch;
            assert_eq!(app.chat_split.as_ref().unwrap().sessions, [local.clone(), remote.clone()]);
            assert!(app.focus_split_chat(&local, window, cx));
            assert_eq!(app.active.as_deref(), Some(local.as_str()));
            assert!(app.remote_pane_id().is_none());
            assert!(app.focus_split_chat(&remote, window, cx));
            assert_eq!(app.remote_workspace.epoch, epoch);
            assert_eq!(app.chat_split.as_ref().unwrap().sessions, [local, remote]);
        });
    }

    #[gpui::test]
    fn a_second_terminal_toggle_hides_a_pending_remote_launch(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let peer = Device { node: "host".into(), device: "one".into(),
                online: true, allowed: true, control: true, ..Default::default() };
            app.open_remote_chat(peer, Some("/remote".into()), None, window, cx);
            let ticket = uuid::Uuid::new_v4();
            app.remote_workspace.pending_terminal = Some((ticket, true));
            app.toggle_terminal(window, cx);
            assert_eq!(app.remote_workspace.pending_terminal, Some((ticket, false)));
            app.toggle_terminal(window, cx);
            assert_eq!(app.remote_workspace.pending_terminal, Some((ticket, true)));
        });
    }

    #[gpui::test]
    fn remote_shell_split_stays_terminal_only_and_does_not_change_the_chat(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let peer = Device {
                node: "split-host".into(), device: "one".into(), online: true,
                allowed: true, control: true, ..Default::default()
            };
            let chat: RemoteChat = serde_json::from_value(json!({
                "id":"first","title":"Existing chat","agent":"codex","project":"/remote"
            })).unwrap();
            app.open_remote_chat(peer, Some(chat.project.clone()), Some(chat), window, cx);
            let original = app.remote_pane_id().unwrap().to_owned();
            app.split_remote_choice(None, None, None, true, true, window, cx);
            let terminal = app.remote_pane_id().unwrap().to_owned();
            assert_ne!(original, terminal);
            assert!(app.remote_workspace.terminal_only);
            assert!(app.remote_workspace.chat.is_none());
            assert!(app.remote_prompt_input().is_none());
            assert!(app.terminals.is_empty());
            assert_eq!(app.remote_workspace.parked[&original].chat.as_ref().unwrap().id, "first");
            app.close_remote_split_pane(&terminal, window, cx);
            assert_eq!(app.remote_pane_id(), Some(original.as_str()));
            assert!(!app.remote_workspace.terminal_only);
        });
    }

    #[gpui::test]
    fn split_remote_focus_preserves_each_transcript_and_geometry(cx: &mut TestAppContext) {
        use super::*;
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let peer = Device { node: "split-host".into(), device: "one".into(), online: true, allowed: true, control: true, ..Default::default() };
            let chats = |id: &str| serde_json::from_value::<RemoteChat>(json!({
                "id": id, "title": id, "agent": "codex", "project": "/remote"
            })).unwrap();
            let first = chats("First");
            app.open_remote_chat(peer.clone(), Some(first.project.clone()), Some(first), window, cx);
            let original = app.remote_pane_id().unwrap().to_owned();
            let original_epoch = app.remote_workspace.epoch;
            let rows = Arc::new(vec![Row {
                role: Some("user".into()), block: MessageBlock::Paragraph("Existing".into()),
            }]);
            app.remote_workspace.apply_history(rows.clone(), 1, chat_flow::Index::new(&rows), false, 0);
            app.remote_workspace.overviews.insert(peer.key(), WorkspaceOverview {
                dock: Some(terminal_dock::Dock { side: terminal_dock::Side::Left, width: 270., height: 210. }),
                ..Default::default()
            });
            app.resize_pane(pane_layout::Pane::Terminal, 375., cx);
            assert_eq!(app.effective_terminal_dock().width, 375.);
            app.split_remote_chat(&original, &RemoteChatDrag { peer: peer.clone(), chat: chats("Second") },
                terminal_dock::Side::Right, window, cx);
            let second = app.remote_pane_id().unwrap().to_owned();
            assert_ne!(app.remote_workspace.epoch, original_epoch);
            assert_eq!(app.effective_terminal_dock().width, 270.);
            assert_eq!(app.remote_workspace.parked[&original].transcript.as_ref().unwrap()[0].block, rows[0].block);
            app.with_remote_epoch(original_epoch, |app| app.remote_workspace.issue = Some("background update".into())).unwrap();
            assert_ne!(app.remote_workspace.issue.as_deref(), Some("background update"));
            assert!(app.focus_remote_pane(&original, cx));
            assert_eq!(app.remote_workspace.epoch, original_epoch);
            assert_eq!(app.remote_workspace.issue.as_deref(), Some("background update"));
            assert_eq!(app.effective_terminal_dock().width, 375.);
            assert!(app.focus_remote_pane(&second, cx));
            assert_eq!(app.chat_split.as_ref().unwrap().sessions.len(), 2);
        });
    }

    #[gpui::test]
    fn remote_device_keeps_project_and_chat_rows_until_its_close_finishes(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let peer = Device {
                node: "remote-host".into(), device: "device-id".into(),
                online: true, allowed: true, compatible: true, ..Device::default()
            };
            let key = peer.key();
            let path = "/remote-project";
            let directory: Directory = serde_json::from_value(json!({
                "projects":[{"path":path,"name":"Remote project"}],
                "chats":[{"id":"work-chat","title":"Work chat","agent":"codex","project":path}]
            })).unwrap();
            app.remote_workspace.devices = vec![peer.clone()];
            app.remote_workspace.tree_branches.insert(key.clone(), Branch {
                open: true, project_pages: HashMap::new(),
                projects_open: [path.into()].into_iter().collect(), directory: Some(directory),
                ..Branch::default()
            });
            app.motion = true;
            app.sidebar_split = false;
            app.rebuild_remote_tree();
            let count = app.remote_tree_count();
            assert!(app.remote_workspace.tree_rows.iter().any(|row| matches!(row, TreeRow::Chat(..))));
            app.toggle_remote_device(peer, cx);
            assert_eq!(app.remote_tree_count(), count);
            assert!(app.remote_workspace.tree_folds.iter().any(|folds| folds.contains(&Some(super::Fold::Device(key.clone())))));
            app.motion = false;
            app.tick_remote_tree(window, cx);
            assert_eq!(app.remote_tree_count(), 1);
        });
    }

    #[gpui::test]
    fn selected_remote_tab_uses_one_slot_when_its_host_group_expands(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.motion = false;
            let peer = Device {
                node: "studio".into(), device: "studio-device".into(),
                online: true, allowed: true, ..Device::default()
            };
            let chat: RemoteChat = serde_json::from_value(json!({
                "id":"open-chat", "title":"Open chat", "agent":"codex"
            })).unwrap();
            let key = peer.key();
            let state = &mut app.remote_workspace;
            state.selected = Some(peer.clone());
            state.devices = vec![peer];
            state.chat = Some(chat.clone());
            state.overviews.insert(key.clone(), super::WorkspaceOverview {
                tabs: vec![chat], ..Default::default()
            });
            assert!(!app.remote_tab_is_grouped());
            app.toggle_remote_tab_group(key.clone(), cx);
            assert!(app.remote_tab_is_grouped());
            app.remote_workspace.devices[0].online = false;
            assert!(!app.remote_tab_is_grouped());
            app.remote_workspace.devices[0].online = true;
            app.toggle_remote_tab_group(key, cx);
            assert!(!app.remote_tab_is_grouped());
        });
    }

    #[gpui::test]
    fn notch_opens_remote_chat_and_holds_live_output_until_its_prompt_is_saved(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let peer = Device {
                node: "paired-host".into(), device: "host-id".into(), online: true, allowed: true,
                control: true, ..Device::default()
            };
            let chat: RemoteChat = serde_json::from_value(json!({
                "id":"remote-chat", "title":"Synced conversation", "agent":"codex", "project":"/remote"
            })).unwrap();
            let key = peer.key();
            app.remote_workspace.devices = vec![peer];
            app.remote_workspace.overviews.insert(key, super::WorkspaceOverview {
                tabs: vec![chat.clone()], ..Default::default()
            });
            app.unified_command(&json!({"action":"chat","id":"remote:paired-host:remote-chat"}), window, cx).unwrap();
            let msg = |role: &str, text: &str| super::runtime::TranscriptMessage {
                role: role.into(), text: text.into(), code_language: None,
            };
            let live_rows = super::canonical_remote_rows(&chat, vec![msg("assistant","Live reply")]);
            let live = super::LiveTranscript {
                flow: std::sync::Arc::new(super::chat_flow::Index::new(&live_rows)), rows: live_rows.into(), status: "Working".into(),
            };
            let state = &mut app.remote_workspace;
            state.pages.newest = vec![msg("assistant","Previous reply")].into();
            assert!(!state.project_live_when_prompt_is_saved(live.clone()));
            assert!(state.live_transcript.is_none());
            let messages = vec![msg("assistant","Previous reply"),msg("user","Current prompt")];
            let rows = super::canonical_remote_rows(&chat,messages.clone());
            let flow = super::chat_flow::Index::new(&rows);
            let len = rows.len();
            state.pages.newest = messages.into();
            state.apply_history(rows.into(), len, flow, false, 0);
            assert!(state.project_live_when_prompt_is_saved(live));
            let snapshot = app.unified_snapshot(cx);
            assert_eq!(snapshot["threadId"], "remote:paired-host:remote-chat");
            let history = snapshot["history"].to_string();
            assert!(history.find("Current prompt").unwrap() < history.find("Live reply").unwrap());
        });
    }

    #[test]
    fn refreshing_a_remote_head_retains_displaced_and_previously_loaded_messages() {
        use super::*;
        let page = |lines: &[&str], older, snapshot: &str| CanonicalHistory {
            messages: lines.iter().map(|line| runtime::TranscriptMessage {
                role: "assistant".into(), text: (*line).into(), code_language: None,
            }).collect(),
            older,
            snapshot: Some(snapshot.into()),
        };
        let mut history = HistoryPages::default();
        assert!(history.receive_head(page(&["First", "Second"], Some(1), "before")));
        assert!(!history.receive_head(page(&[], None, "empty")));
        assert!(!history.receive_head(page(&["First"], None, "short")));
        assert!(history.receive_head(page(&["Second", "Third"], Some(1), "active")));
        let snapshot = |cursor| Ok(match cursor {
            0 => page(&["Fourth", "Fifth"], Some(1), "after"),
            1 => page(&["Second", "Third"], Some(2), "after"),
            2 => page(&["Oldest", "First"], None, "after"),
            _ => unreachable!(),
        });
        assert!(load_remote_history(&mut history, false, true, snapshot).unwrap());
        assert_eq!(history.combined().into_iter().map(|message| message.text).collect::<Vec<_>>(),
            ["First", "Second", "Third", "Fourth", "Fifth"]);
        assert!(load_remote_history(&mut history, true, true, snapshot).unwrap());
        assert_eq!(history.combined().into_iter().map(|message| message.text).collect::<Vec<_>>(),
            ["Oldest", "First", "Second", "Third", "Fourth", "Fifth"]);
        assert!(history.cursor.is_none());
    }

    #[gpui::test]
    fn shared_new_command_keeps_remote_chats_and_preserves_their_tab(cx: &mut TestAppContext) {
        use super::*;
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let peer = Device {
                node: "paired-host".into(), device: "host-id".into(), online: true,
                allowed: true, control: true, ..Device::default()
            };
            let chat: RemoteChat = serde_json::from_value(json!({
                "id":"saved-chat", "title":"Existing chat", "agent":"codex", "project":"/remote"
            })).unwrap();
            app.open_remote_chat(peer.clone(), Some(chat.project.clone()), Some(chat), window, cx);
            app.new_conversation(window, cx);
            assert!(app.remote_workspace.chat.is_none());
            assert_eq!(app.remote_workspace.project.as_deref(), Some("/remote"));
            assert_eq!(app.remote_workspace.tabs_for_device(&peer.key())[0].id, "saved-chat");
        });
    }

    #[gpui::test]
    fn an_empty_activity_poll_leaves_confirmed_remote_status_available(cx: &mut TestAppContext) {
        use super::*;
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            let state = &mut app.remote_workspace;
            state.selected = Some(Device { online: true, allowed: true, control: true, ..Device::default() });
            state.chat = serde_json::from_value(json!({
                "id":"saved-chat", "title":"Existing chat", "agent":"codex", "project":"/remote"
            })).ok();
            state.queue_caption = Some("Thinking".into());
            state.host_task_running = true;
            app.apply_remote_activity(Vec::new(), cx);
            assert_eq!(app.remote_status_caption(), Some("Thinking"));
        });
    }

    #[test]
    fn persisted_prompt_receipt_is_scoped_to_its_original_host_and_chat() {
        let original = super::SavedSend {
            device: super::DeviceKey("host-node".into(), "host-device".into()),
            destination: "chat:owner".into(),
            prompt: "x".repeat(16 * 1024),
            command: json!({"action":"prompt_chat","chat_id":"owner","request_id":uuid::Uuid::new_v4(),
                "prompt":"x".repeat(16 * 1024)}),
        };
        let mut restored = State::default();
        let snapshot = serde_json::to_vec(&vec![original.clone()]).unwrap();
        restored.restore_pending_sends(serde_json::from_slice(&snapshot).unwrap());
        assert_eq!(restored.pending_snapshot()[0].command, original.command);
        let mut moved = original;
        moved.destination = "chat:other".into();
        restored.restore_pending_sends(vec![moved]);
        assert_eq!(restored.pending_snapshot().len(), 1);
    }

    #[gpui::test]
    fn remote_send_clears_the_composer_before_the_host_receipt(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let state = &mut app.remote_workspace;
            state.selected = Some(Device { online: true, allowed: true, control: true, ..Device::default() });
            state.project = Some("/fixture".into());
            state.helper = Some("/unavailable-test-helper".into());
            let input = cx.new(|cx| super::TextInput::new(window, cx));
            input.update(cx, |input, cx| input.set_text("Show the message immediately", cx));
            state.composer = Some(input.clone());
            app.send_remote_workspace(cx);
            assert!(input.read(cx).text().is_empty());
            let state = &app.remote_workspace;
            assert!(state.submitting());
            assert_eq!(state.submitted_preview.as_ref().unwrap().0, "Show the message immediately");
            assert_eq!(state.pending_sends[&state.draft_key().unwrap()].1["action"], "create_chat");
        });
    }

    #[test]
    fn slow_history_coalesces_navigation_without_blocking_device_discovery() {
        let history = Worker::start("test remote history").unwrap();
        let discovery = Worker::start("test remote discovery").unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first = history.submit(true, move || {
            started_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            1
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let stale = history.submit(true, || 2);
        let latest = history.submit(true, || 3);
        assert!(stale.is_closed());
        let refresh = discovery.submit(false, || "Studio reachable");
        assert_eq!(refresh.recv_blocking().unwrap(), "Studio reachable");
        release_tx.send(()).unwrap();
        assert_eq!(first.recv_blocking().unwrap(), 1);
        assert_eq!(latest.recv_blocking().unwrap(), 3);
    }

    #[test]
    fn empty_remote_pages_and_nodes_remain_distinct() {
        let page: Directory =
            serde_json::from_value(json!({"projects":null,"chats":null})).unwrap();
        assert!(page.projects.is_empty() && page.chats.is_empty());
        let mut state = State::default();
        let node = |id: &str| Device {
            node: id.into(),
            device: "id".into(),
            name: "Mac".into(),
            account: String::new(),
            online: true,
            allowed: true,
            compatible: true,
            control: false,
            ..Device::default()
        };
        state.aliases.insert("studio".into(), "Workstation".into());
        assert_eq!(state.label(&node("studio")), "Workstation");
        assert_eq!(state.label(&node("air")), "Mac");
        let previous = state.epoch;
        state.invalidate();
        assert_ne!(state.epoch, previous);
    }

    #[test]
    fn empty_transcripts_are_valid_and_older_alias_writes_are_discarded() {
        let root = std::env::temp_dir().join(format!("kai-remote-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        let executable = root.join("empty-helper");
        std::fs::write(&executable, "#!/bin/sh\ncat > /dev/null\nprintf null\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let rows = call_list::<Message>(&executable, json!({"action":"messages"})).unwrap();
        assert!(rows.is_empty());
        let file = root.join("aliases.json");
        let revision = Mutex::new(0);
        let newest = HashMap::from([("studio".to_owned(), "New name".to_owned())]);
        let older = HashMap::from([("studio".to_owned(), "Old name".to_owned())]);
        save_aliases(&file, &newest, &revision, 2).unwrap();
        save_aliases(&file, &older, &revision, 1).unwrap();
        assert_eq!(
            serde_json::from_slice::<HashMap<String, String>>(&std::fs::read(file).unwrap())
                .unwrap(),
            newest
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stream_formats_incremental_journal_text_as_native_markdown() {
        let scratch =
            std::env::temp_dir().join(format!("kai-remote-stream-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&scratch).unwrap();
        let executable = scratch.join("stream-helper");
        let active =
            json!({"type":"activity","activity":[{"chat":"stream-chat","status":"Running"}]});
        let inactive = json!({"type":"activity","activity":[]});
        let initial = json!({"type":"reset","chat":"stream-chat"});
        let output = json!({"type":"events","chat":"stream-chat","events":[{"Status":"Running"},{"Text":"## Progress"}]});
        let finish = json!({"type":"events","chat":"stream-chat","events":["Finished", {"Status":"Late status"}]});
        std::fs::write(&executable, format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{active}' '{initial}' '{output}'\nsleep 0.22\nprintf '%s\\n' '{finish}'\nsleep 0.22\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        let chat = RemoteChat {
            id: "stream-chat".into(),
            title: "Streaming".into(),
            agent: "codex".into(),
            project: String::new(),
            updated_at: 0,
            terminal: false,
        };
        let (send, updates) = async_channel::bounded(8);
        stream_chat(
            &executable,
            &Device::default(),
            &chat,
            &AtomicBool::new(false),
            &send,
        )
        .unwrap();
        drop(send);
        let updates: Vec<_> = std::iter::from_fn(|| updates.try_recv().ok()).collect();
        assert!(updates.iter().any(|update| matches!(update, StreamUpdate::Running(live)
            if live.status == "Running" && live.rows.iter().any(|row| matches!(&row.block, super::MessageBlock::Heading(text) if text == "Progress")))));
        assert!(
            updates
                .iter()
                .any(|update| matches!(update, StreamUpdate::Finished))
        );
        assert!(
            !updates
                .iter()
                .skip_while(|item| !matches!(item, StreamUpdate::Finished))
                .any(|item| matches!(item, StreamUpdate::Running(_)))
        );
        std::fs::write(&executable, format!("#!/bin/sh\ncat >/dev/null\nprintf '%s\\n' '{inactive}' '{initial}' '{output}'\nsleep 0.22\n")).unwrap();
        let (send, updates) = async_channel::bounded(8);
        stream_chat(
            &executable,
            &Device::default(),
            &chat,
            &AtomicBool::new(false),
            &send,
        )
        .unwrap();
        drop(send);
        assert!(
            !std::iter::from_fn(|| updates.try_recv().ok())
                .any(|item| matches!(item, StreamUpdate::Running(_)))
        );
        std::fs::remove_dir_all(scratch).unwrap();
    }

    #[gpui::test]
    fn notch_remote_history_keeps_an_older_window_and_can_return_to_latest(cx: &mut TestAppContext) {
        use super::*;
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update_in(cx, |app, window, cx| {
            let peer = Device { node: "paired".into(), device: "host".into(), online: true, allowed: true, ..Default::default() };
            let chat: RemoteChat = serde_json::from_value(json!({"id":"paged","agent":"codex","title":"Paged"})).unwrap();
            app.open_remote_selection(peer, Some(chat.project.clone()), Some(chat.clone()), cx);
            let rows = Arc::new(canonical_remote_rows(&chat, (0..220).map(|index| runtime::TranscriptMessage {
                role: "user".into(), text: format!("Prompt {index}."), code_language: None,
            }).collect()));
            let flow = chat_flow::Index::for_transcript(&rows, true, None);
            app.remote_workspace.apply_history(rows.clone(), rows.len(), flow, false, 0);
            let latest = app.native_history_job()();
            assert!(latest.to_string().contains("Prompt 219."));
            let start = latest[0]["id"].as_str().unwrap().strip_prefix("row-").unwrap().parse::<usize>().unwrap();
            assert!(start > 0);
            app.remote_workspace.native_history_start = start;
            assert!(app.dispatch_remote_surface(&json!({"action":"history_more"}), window, cx).unwrap());
            let earlier = app.native_history_job()();
            assert_eq!(earlier.as_array().unwrap().last().unwrap()["id"], format!("row-{}", start - 1));
            assert_eq!(app.remote_workspace.history_len(), rows.len());
            assert!(app.dispatch_remote_surface(&json!({"action":"history_latest"}), window, cx).unwrap());
            assert_eq!(app.native_history_job()(), latest);
        });
    }

    #[test]
    fn early_live_events_cannot_replace_a_previous_saved_answer() {
        use super::*;
        let chat: RemoteChat = serde_json::from_value(json!({"id":"one","agent":"codex","title":"Chat"})).unwrap();
        let msg = |role: &str, text: &str| runtime::TranscriptMessage {
            role: role.into(), text: text.into(), code_language: None,
        };
        let saved = Arc::new(canonical_remote_rows(&chat, vec![
            msg("user", "Previous question"),
            msg("assistant", "Previous answer"),
        ]));
        let live = Arc::new(canonical_remote_rows(&chat, vec![msg("assistant", "Streaming next answer")]));
        let mut state = State::default();
        state.apply_history(saved.clone(), 1, chat_flow::Index::new(&saved), false, 0);
        state.receive_live(LiveTranscript {
            flow: Arc::new(chat_flow::Index::new(&live)), rows: live, status: "Working".into(),
        });
        assert_eq!(state.visible_history_len(), 2);
        assert_eq!(state.transcript_list.as_ref().unwrap().item_count(), 3);
        assert_eq!(state.row(1).unwrap().block, saved[1].block);
        assert!(state.clear_preview());
        assert_eq!(state.transcript_list.as_ref().unwrap().item_count(), 2);
        assert_eq!(state.row(1).unwrap().block, saved[1].block);
    }

    #[test]
    fn status_frames_keep_the_remote_transcript_projection_and_streamed_rows() {
        use super::*;
        let chat: RemoteChat = serde_json::from_value(
            json!({"id":"chat","agent":"codex","title":"Progress"})
        ).unwrap();
        let live = |text: &str, status: &str| {
            let rows = canonical_remote_rows(&chat, vec![runtime::TranscriptMessage {
                role: "assistant".into(), text: text.into(), code_language: None,
            }]);
            LiveTranscript {
                flow: Arc::new(chat_flow::Index::new(&rows)),
                rows: Arc::new(rows),
                status: status.into(),
            }
        };
        let mut state = State::default();
        let first = live("First paragraph.\n\nStreaming output", "Thinking");
        state.receive_live(first.clone());
        let revision = state.native_history_revision;
        let list = state.transcript_list.clone().unwrap();
        list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(4.) });
        state.receive_live(live("First paragraph.\n\nStreaming output", "Running a command"));
        assert_eq!(state.native_history_revision, revision);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(4.));
        let updated = live("First paragraph.\n\nStreaming output continues", "Working");
        assert_eq!(first.stable_prefix(&updated), 1);
        state.receive_live(updated);
        assert_ne!(state.native_history_revision, revision);
        assert_eq!(list.item_count(), 2);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(4.));
    }

    #[test]
    fn prepending_history_keeps_current_rows_and_the_live_scroll_anchor() {
        use super::*;
        let chat: RemoteChat =
            serde_json::from_value(json!({"id":"id","agent":"codex","title":"Chat"})).unwrap();
        let messages = |rows: &[(&str, &str)], older| CanonicalHistory {
            messages: rows
                .iter()
                .map(|(role, text)| runtime::TranscriptMessage {
                    role: (*role).into(),
                    text: (*text).into(),
                    code_language: None,
                })
                .collect(),
            older,
            snapshot: None,
        };
        let mut state = State::default();
        let apply = |state: &mut State, earlier| {
            let rows = canonical_remote_rows(&chat, state.pages.combined());
            let prompt_end = rows
                .iter()
                .rposition(chat_flow::user)
                .map_or(rows.len(), |index| index + 1);
            let flow = chat_flow::Index::for_transcript(&rows, true, None);
            state.apply_history(Arc::new(rows), prompt_end, flow, earlier, 0);
        };
        let latest = [
            ("user", "Current prompt"),
            ("assistant", "Current answer"),
            ("user", "Active prompt"),
        ];
        state.pages.receive_head(messages(&latest, Some(1)));
        apply(&mut state, false);
        let last_count = state.history_len();
        let stream = canonical_remote_rows(
            &chat,
            messages(&[("assistant", "Live output")], None).messages,
        );
        let flow = chat_flow::Index::new(&stream);
        let list = state.transcript_list.clone().unwrap();
        list.splice(last_count..last_count, stream.len());
        state.live_transcript = Some(LiveTranscript {
            rows: Arc::new(stream),
            flow: Arc::new(flow),
            status: "Working".into(),
        });
        list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(9.),
        });
        let earlier = [
            ("user", "Old prompt"),
            ("assistant", "Old answer"),
        ];
        state.pages.receive_older(messages(&earlier, None), true);
        apply(&mut state, true);
        let prefix = state.history_len() - last_count;
        assert_eq!(prefix, 2);
        assert_eq!(list.logical_scroll_top().item_ix, prefix);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(9.));
        assert!(
            matches!(state.row(state.visible_history_len()).map(|row| &row.block), Some(MessageBlock::Paragraph(text)) if text == "Live output")
        );
        state.pages.receive_head(messages(&latest[1..], Some(1)));
        apply(&mut state, false);
        assert_eq!(state.pages.combined().len(), 5);
        assert!(state.pages.cursor.is_none());
        list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(0.),
        });
        assert!(
            matches!(state.row(0).map(|row| &row.block), Some(MessageBlock::Paragraph(text)) if text == "Old prompt")
        );
        list.scroll_to(ListOffset {
            item_ix: prefix,
            offset_in_item: px(0.),
        });
        assert!(
            matches!(state.row(list.logical_scroll_top().item_ix).map(|row| &row.block), Some(MessageBlock::Paragraph(text)) if text == "Current prompt")
        );
    }

    #[gpui::test]
    fn remote_header_pet_follows_sidebar_and_window_geometry(cx: &mut TestAppContext) {
        use super::*;
        let (view, cx) = crate::ui_tests::fixture(cx);
        unified::with_test_workspace_pet_host(|| {
            view.update(cx, |app, cx| {
                app.sidebar_split = false;
                app.sidebar = true;
                app.sidebar_width = app.sidebar_target_width();
                app.conversations_visible = true;
                app.project_tree.chats_collapsed = false;
                app.remote_workspace.started = true;
                let peer = Device {
                    node: "studio".into(),
                    device: "id".into(),
                    name: "Studio".into(),
                    online: true,
                    allowed: true,
                    compatible: true,
                    ..Device::default()
                };
                app.open_remote_selection(peer, Some("/shared/project".into()), None, cx);
            });
            crate::ui_tests::draw_frame(&view, cx);
            let single = unified::test_workspace_pet_rect()
                .expect("Remote header must publish a workspace pet anchor");
            view.update(cx, |app, cx| app.toggle_sidebar_layout(cx));
            crate::ui_tests::draw_frame(&view, cx);
            let double = unified::test_workspace_pet_rect().unwrap();
            assert!(
                double[0] > single[0],
                "Pet must move with the remote header"
            );
            cx.simulate_resize(size(px(1040.), px(690.)));
            cx.draw(point(px(0.), px(0.)), size(px(1040.), px(690.)), |_, _| {
                view.clone().into_any_element()
            });
            let resized = unified::test_workspace_pet_rect().unwrap();
            assert_eq!(
                resized[0], double[0],
                "Height and width changes must retain the same sidebar-relative anchor"
            );
            view.update(cx, |app, cx| {
                app.resize_sidebar(app.sidebar_width + 44., cx)
            });
            cx.draw(point(px(0.), px(0.)), size(px(1040.), px(690.)), |_, _| {
                view.clone().into_any_element()
            });
            assert_eq!(
                unified::test_workspace_pet_rect().unwrap()[0],
                double[0] + 44.
            );
            view.update(cx, |app, cx| app.toggle_sidebar_layout(cx));
            cx.draw(point(px(0.), px(0.)), size(px(1040.), px(690.)), |_, _| {
                view.clone().into_any_element()
            });
            assert_eq!(unified::test_workspace_pet_rect().unwrap()[0], single[0]);
        });
    }

    #[gpui::test]
    fn remote_project_uses_the_standard_split_sidebar_session_rows(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        view.update(cx, |app, cx| {
            app.motion = false;
            app.sidebar_split = true;
            app.sidebar = true;
            app.conversations_visible = true;
            app.remote_workspace.started = true;
            let peer = Device { node: "studio".into(), device: "id".into(), name: "Studio".into(),
                online: true, allowed: true, compatible: true, ..Device::default() };
            let branch = app.remote_workspace.tree_branches.entry(peer.key()).or_default();
            branch.open = true;
            branch.directory = Some(serde_json::from_value(json!({
                "projects":[{"path":"/remote/project","name":"Remote Project"}],
                "chats":[{"id":"chat","title":"Host Chat","agent":"codex","project":"/remote/project","updated_at":42}]
            })).unwrap());
            app.remote_workspace.devices = vec![peer.clone()];
            app.rebuild_remote_tree();
            app.show_remote_conversations(peer, Some("/remote/project".into()), cx);
        });
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("workspace-pane").is_some());
        assert!(cx.debug_bounds("session-content-0").is_some());
        assert!(cx.debug_bounds("remote-chat-studio-chat").is_none());
        crate::ui_tests::click_selector(cx, "session-content-0");
        view.update(cx, |app, _| {
            assert_eq!(app.remote_workspace.chat.as_ref().unwrap().id, "chat")
        });
    }

    #[test]
    fn loading_scoped_chats_preserves_all_projects_and_deduplicates_pages() {
        let result = |chats, next| {
            serde_json::from_value::<Directory>(json!({
                "projects":[{"path":"/remote/project","name":"Remote Project"},
                             {"path":"/remote/empty","name":"Empty Project"}],
                "chats":chats, "next_offset":next
            }))
            .unwrap()
        };
        let first =
            json!({"id":"one","title":"First Chat","project":"/remote/project","agent":"codex"});
        let second =
            json!({"id":"two","title":"Second Chat","project":"/remote/project","agent":"codex"});
        let mut branch = Branch::default();
        assert!(branch.receive(None, 0, result(json!([&first]), 100)));
        assert!(branch.receive(
            Some("/remote/project".into()),
            0,
            result(json!([&first]), 100)
        ));
        assert!(branch.receive(
            Some("/remote/project".into()),
            100,
            result(json!([&first, &second]), 0)
        ));
        assert_eq!(branch.directory.as_ref().unwrap().projects.len(), 2);
        assert_eq!(branch.project_pages["/remote/project"].chats.len(), 2);
        assert_eq!(branch.project_pages["/remote/project"].next_offset, 0);
    }

    #[gpui::test]
    fn studio_projects_expand_in_the_local_tree_before_any_chat_opens(cx: &mut TestAppContext) {
        let (view, cx) = crate::ui_tests::fixture(cx);
        let worker = Worker::start("test blocked workspace fetch").unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (unblock, blocked) = mpsc::channel();
        let fetch = worker.submit(false, move || {
            started_tx.send(()).unwrap();
            blocked.recv_timeout(Duration::from_secs(3)).unwrap();
        });
        started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        view.update(cx, |app, cx| {
            app.sidebar_split = false;
            app.motion = false;
            app.active = Some("fixture-session".into());
            app.composer.update(cx, |input, cx| input.set_text("Keep local draft", cx));
            let device = Device {
                node: "fixture-studio".into(),
                device: "fixture-remote-device".into(),
                name: "Studio".into(),
                account: String::new(),
                online: true,
                allowed: true,
                compatible: true,
                control: false,
                ..Device::default()
            };
            let state = &mut app.remote_workspace;
            state.started = true;
            state.browse_worker = Some(worker);
            let branch = state.tree_branches.entry(device.key()).or_default();
            branch.directory = Some(serde_json::from_value(json!({
                "projects":[{"path":"/remote/project","name":"Remote Project"},
                             {"path":"/remote/empty","name":"Empty Project"}],
                "chats":[{"id":"remote-chat","title":"Remote Chat","agent":"codex","project":"/remote/project"}]
            })).unwrap());
            state.devices = vec![device];
            app.rebuild_remote_tree();
            app.rebuild_project_tree();
            cx.notify();
        });
        const MAC: &str = "remote-device-fixture-studio";
        const PROJECT: &str = "remote-project-fixture-studio-/remote/project";
        const EMPTY: &str = "remote-project-fixture-studio-/remote/empty";
        const CHAT: &str = "remote-chat-fixture-studio-remote-chat";
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("project-chat-tree").is_some());
        assert!(cx.debug_bounds("project-0").is_some());
        assert!(cx.debug_bounds(PROJECT).is_none());
        assert!(
            cx.debug_bounds(MAC).unwrap().top()
                < cx.debug_bounds("sidebar-layout-header").unwrap().top()
        );
        crate::ui_tests::click_selector(cx, MAC);
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("project-0").is_some());
        assert!(cx.debug_bounds(EMPTY).is_some());
        assert!(cx.debug_bounds(CHAT).is_none());
        crate::ui_tests::click_selector(cx, PROJECT);
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds(CHAT).is_some());
        view.update(cx, |app, _| assert!(!app.remote_workspace.active()));
        crate::ui_tests::click_selector(cx, CHAT);
        crate::ui_tests::draw_frame(&view, cx);
        assert!(cx.debug_bounds("workspace-tab-strip").is_some());
        assert!(cx.debug_bounds("project-chat-tree").is_some());
        assert!(cx.debug_bounds("chat-pane").is_some());
        assert!(cx.debug_bounds("remote-native-send").is_none());
        crate::ui_tests::click_selector(cx, MAC);
        crate::ui_tests::draw_frame(&view, cx);
        view.update(cx, |app, _| {
            assert!(
                !app.remote_workspace
                    .tree_branches
                    .values()
                    .next()
                    .unwrap()
                    .open
            );
            assert_eq!(app.remote_tree_count(), 1);
        });
        assert!(cx.debug_bounds("remote-conversation-tab").is_some());
        crate::ui_tests::click_selector(cx, "remote-tab-close");
        view.update(cx, |app, cx| {
            assert!(!app.remote_workspace.active());
            assert_eq!(app.active.as_deref(), Some("fixture-session"));
            assert_eq!(app.composer.read(cx).text(), "Keep local draft");
            let peer = &mut app.remote_workspace.devices[0];
            peer.online = false;
            let key = peer.key();
            app.remote_workspace
                .tree_branches
                .get_mut(&key)
                .unwrap()
                .open = true;
            app.refresh_remote_tree(cx);
            assert!(
                app.remote_workspace
                    .tree_rows
                    .iter()
                    .any(|row| { matches!(row, TreeRow::Note(_, note) if note == "Mac offline") })
            );
        });
        unblock.send(()).unwrap();
        fetch.recv_blocking().unwrap();
    }
}
