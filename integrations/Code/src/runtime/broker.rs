//! Bounded ownership registry for native provider services. No service is
//! attached to another app; prompts are never replayed after an ambiguous loss.
use super::*;
use crate::chat_interactions::InteractionStatus;
use std::{collections::HashMap, os::unix::net::UnixStream, path::Path, sync::OnceLock};

pub const IDLE_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_SERVICES: usize = 16;
const MAX_IDLE: usize = 2;
const MAX_CHILDREN: usize = 128;
const PAGE_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct ChildSnapshot {
    pub name:Option<String>,
    pub scope:Option<String>,
    pub task:Option<String>,
    pub model:Option<String>,
    pub effort:Option<String>,
    pub workspace:Option<PathBuf>,
    pub provider: AgentKind,
    pub project: PathBuf,
    pub parent_id: String,
    pub id: String,
    pub turn_id: Option<String>,
    pub status: InteractionStatus,
    pub preview: String,
    pub requests: Vec<NativeRequest>,
    pub transcript_bytes: usize,
    pub transcript_revision: u64,
    pub connected: bool,
}
#[derive(Clone, Debug)]
pub enum ChildAction {
    Stop,
    Steer(String),
    Respond { id: String, answer: NativeAnswer },
}
#[derive(Debug)]
pub struct ChildPage {
    pub text: String,
    pub offset: usize,
    pub revision: u64,
    pub next_offset: usize,
    pub total_bytes: usize,
}
#[derive(Clone, Debug)]
pub struct ServiceSnapshot {
    pub provider: AgentKind,
    pub project: PathBuf,
    pub parent_id: String,
    pub process_id: u32,
    pub active: bool,
    pub connected: bool,
    pub usage: Option<crate::usage::Usage>,
}
pub(super) type TurnJob = (RunRequest, NativeContext, Option<(String, Value)>);
pub(super) enum Command {
    Turn(Box<TurnJob>),
    Child(String, ChildAction),
}
struct ChildRecord {
    snapshot: ChildSnapshot,
    text: ResponseBuffer,
    item: String,
    item_start: usize,
    request_errors: HashMap<String, String>,
    updated:Instant,
}
struct ActiveTurn {
    cancelled: Arc<AtomicBool>,
    budget: Arc<QueueBudget>,
    group: Arc<Mutex<Option<i32>>>,
}
pub(super) struct Service {
    pub agent: AgentKind,
    pub executable: PathBuf,
    pub project: PathBuf,
    pub pid: i32,
    pub busy: AtomicBool,
    pub alive: AtomicBool,
    pub shutdown: AtomicBool,
    pub idle_cancelled: Arc<AtomicBool>,
    pub parent: Mutex<String>,
    pub commands: SyncSender<Command>,
    wake: Mutex<UnixStream>,
    children: Mutex<HashMap<String, ChildRecord>>,
    idle_since: Mutex<Instant>,
    settlers: Mutex<Vec<async_channel::Sender<()>>>,
    pub finished: async_channel::Receiver<()>,
    cleanup: Mutex<Option<async_channel::Sender<()>>>,
    active_turn: Mutex<Option<ActiveTurn>>,
    usage: Mutex<Option<crate::usage::Usage>>,
}
fn registry() -> &'static Mutex<Vec<Arc<Service>>> {
    static REGISTRY: OnceLock<Mutex<Vec<Arc<Service>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Vec::new()))
}
fn signal() -> &'static (async_channel::Sender<()>, async_channel::Receiver<()>) {
    static SIGNAL: OnceLock<(async_channel::Sender<()>, async_channel::Receiver<()>)> =
        OnceLock::new();
    SIGNAL.get_or_init(|| async_channel::bounded(1))
}
/// Coalesced edge notification; read snapshots after each wake. No polling or
/// unbounded event backlog is required when the app is idle or hidden.
#[cfg_attr(test, allow(dead_code))] // Native UI readiness task is omitted by GPUI tests.
pub fn wake() -> async_channel::Receiver<()> {
    signal().1.clone()
}
fn changed() {
    let _ = signal().0.try_send(());
}
fn transcript_watch() -> &'static AtomicBool {
    static WATCH: AtomicBool = AtomicBool::new(false);
    &WATCH
}
pub fn watch_transcripts(watch: bool) {
    transcript_watch().store(watch, Ordering::Release);
    if watch {
        changed();
    }
}
impl Service {
    pub fn report_usage(&self, next: crate::usage::Usage) {
        let mut usage = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        usage.get_or_insert_with(Default::default).merge(next);
        drop(usage);
        changed();
    }
    pub fn attach(&self, context: &NativeContext) -> Result<()> {
        let mut active = self.active_turn.lock().unwrap_or_else(|e| e.into_inner());
        if !self.alive.load(Ordering::Acquire) || self.shutdown.load(Ordering::Acquire) {
            *context.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
            bail!("This native service stopped before dispatch; retry explicitly");
        }
        *active = Some(ActiveTurn {
            cancelled: context.cancelled.clone(),
            budget: context.tx.budget.clone(),
            group: context.group.clone(),
        });
        Ok(())
    }
    pub fn stop(&self) {
        let active = self.active_turn.lock().unwrap_or_else(|e| e.into_inner());
        self.shutdown.store(true, Ordering::Release);
        self.idle_cancelled.store(true, Ordering::Release);
        if let Some(active) = active.as_ref() {
            active.cancelled.store(true, Ordering::Release);
            drop(
                active
                    .budget
                    .bytes
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()),
            );
            active.budget.changed.notify_all();
            let group = active.group.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_secs(2));
                if let Some(pid) = *group.lock().unwrap_or_else(|e| e.into_inner()) {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            });
        }
        self.notify();
    }
    pub fn begin_cleanup(&self) {
        let _active = self.active_turn.lock().unwrap_or_else(|e| e.into_inner());
        self.alive.store(false, Ordering::Release);
        self.shutdown.store(true, Ordering::Release);
    }
    pub fn notify(&self) {
        if let Ok(mut wake) = self.wake.lock() {
            let _ = wake.write(&[1]);
        }
    }
    pub fn set_parent(&self, id: &str) {
        *self.parent.lock().unwrap_or_else(|e| e.into_inner()) = id.into();
        changed();
    }
    pub fn release(&self) {
        self.active_turn
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        *self.idle_since.lock().unwrap_or_else(|e| e.into_inner()) = Instant::now();
        self.busy.store(false, Ordering::Release);
        let entries = registry().lock().unwrap_or_else(|e| e.into_inner());
        let mut idle: Vec<_> = entries
            .iter()
            .filter(|service| {
                service.alive.load(Ordering::Acquire)
                    && !service.busy.load(Ordering::Acquire)
                    && !service.has_running_children()
            })
            .collect();
        idle.sort_by_key(|service| *service.idle_since.lock().unwrap_or_else(|e| e.into_inner()));
        for service in idle.iter().take(idle.len().saturating_sub(MAX_IDLE)) {
            service.stop();
        }
        changed();
    }
    pub fn has_running_children(&self) -> bool {
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .any(|child| {
                matches!(
                    child.snapshot.status,
                    InteractionStatus::Started | InteractionStatus::Working
                ) || !child.snapshot.requests.is_empty()
            })
    }
    pub fn idle_expired(&self) -> bool {
        !self.has_running_children()
            && self
                .idle_since
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .elapsed()
                >= IDLE_TIMEOUT
    }
    pub fn idle_wait_ms(&self) -> i32 {
        if self.has_running_children() {
            -1
        } else {
            IDLE_TIMEOUT
                .saturating_sub(
                    self.idle_since
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .elapsed(),
                )
                .as_millis()
                .min(i32::MAX as u128) as i32
        }
    }
    pub fn active_turns(&self) -> Vec<(String, String)> {
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|record| {
                matches!(
                    record.snapshot.status,
                    InteractionStatus::Working | InteractionStatus::Started
                )
            })
            .filter_map(|record| {
                record
                    .snapshot
                    .turn_id
                    .as_ref()
                    .map(|turn| (record.snapshot.id.clone(), turn.clone()))
            })
            .collect()
    }
    pub fn active_children(&self) -> Vec<String> {
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|record| {
                matches!(
                    record.snapshot.status,
                    InteractionStatus::Started | InteractionStatus::Working
                ) || !record.snapshot.requests.is_empty()
            })
            .map(|record| record.snapshot.id.clone())
            .collect()
    }
    pub fn disconnected(&self) {
        self.alive.store(false, Ordering::Release);
        self.busy.store(false, Ordering::Release);
        for child in self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            child.snapshot.connected = false;
            child.snapshot.requests.clear();
            if matches!(
                child.snapshot.status,
                InteractionStatus::Started | InteractionStatus::Working
            ) {
                child.snapshot.status = InteractionStatus::Failed;
                child.snapshot.preview = "Provider disconnected. The turn was not replayed.".into();
            }
        }
        self.flush_settlers();
        self.cleanup
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        changed();
    }
    pub fn hold_settled(&self, sender: async_channel::Sender<()>) {
        let children = self.children.lock().unwrap_or_else(|e| e.into_inner());
        if children.values().any(|child| {
            matches!(
                child.snapshot.status,
                InteractionStatus::Started | InteractionStatus::Working
            ) || !child.snapshot.requests.is_empty()
        }) {
            self.settlers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(sender);
        }
    }
    fn flush_settlers(&self) {
        let children = self.children.lock().unwrap_or_else(|e| e.into_inner());
        if children.values().all(|child| {
            !matches!(
                child.snapshot.status,
                InteractionStatus::Started | InteractionStatus::Working
            ) && child.snapshot.requests.is_empty()
        }) {
            self.settlers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
        }
    }
    pub fn reject(&self, child: &str, request: &str, error: &str) {
        if let Some(record) = self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(child)
            && record.request_errors.len() < 64
        {
            record
                .request_errors
                .insert(request.into(), error.chars().take(1024).collect());
        }
        changed();
    }
    /// Observed thread ownership, never a user-supplied arbitrary thread ID.
    pub fn child(&self, id: &str) -> bool {
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
    }
    pub fn needs_initial_items(&self,id:&str)->bool{self.children.lock().unwrap_or_else(|e|e.into_inner()).get(id).is_some_and(|child|child.snapshot.scope.is_some()&&child.text.is_empty())}
    pub fn has_child_output(&self,id:&str)->bool{self.children.lock().unwrap_or_else(|e|e.into_inner()).get(id).is_some_and(|child|!child.text.is_empty())}
    pub fn has_item_output(&self, child: &str, item: &str) -> bool {
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(child)
            .is_some_and(|record| record.item == item && record.text.len() > record.item_start)
    }
    pub fn observe(&self, id: &str, parent: &str, method: &str, p: &Value) -> Result<()> {
        anyhow::ensure!(
            !id.is_empty() && id.len() <= 256,
            "The provider returned an invalid child ID"
        );
        let mut children = self.children.lock().unwrap_or_else(|e| e.into_inner());
        if !children.contains_key(id)&&children.len()>=MAX_CHILDREN{
            // Retire completed leaves only. An ancestor still owns its live
            // descendants' approvals and controls even after its own turn ends.
            let parents:std::collections::HashSet<&str>=children.values().map(|c|c.snapshot.parent_id.as_str()).chain(std::iter::once(parent)).collect();
            let old=children.iter().filter(|(id,c)|!parents.contains(id.as_str())&&c.snapshot.requests.is_empty()&&!matches!(c.snapshot.status,InteractionStatus::Started|InteractionStatus::Working)).min_by_key(|(_,c)|c.updated).map(|(id,_)|id.clone());
            if let Some(old)=old{children.remove(&old);}
        }
        if !children.contains_key(id) && children.len() >= MAX_CHILDREN {
            bail!(
                "The provider exceeded the 128-child live ownership limit; no partial completion was claimed"
            );
        }
        let child = children.entry(id.into()).or_insert_with(|| ChildRecord {
            snapshot: ChildSnapshot {
                name:None,scope:None,task:None,model:None,effort:None,workspace:None,
                provider: self.agent,
                project: self.project.clone(),
                parent_id: parent.into(),
                id: id.into(),
                turn_id: None,
                status: InteractionStatus::Working,
                preview: String::new(),
                requests: vec![],
                transcript_bytes: 0,
                transcript_revision: 0,
                connected: true,
            },
            text: ResponseBuffer::default(),
            item: String::new(),
            item_start: 0,
            request_errors: HashMap::new(),
            updated:Instant::now(),
        });
        child.updated=Instant::now();
        let bound=|text:&str,limit:usize|text.chars().take(limit).collect::<String>();
        let thread=&p["thread"];let source=&thread["source"]["subAgent"]["thread_spawn"];
        if let Some(name)=thread["agentNickname"].as_str().or_else(||source["agent_nickname"].as_str()).or_else(||thread["name"].as_str()).filter(|s|!s.is_empty()){child.snapshot.name=Some(bound(name,128));}
        if let Some(task)=p["task"].as_str().or_else(||thread["preview"].as_str()).filter(|s|!s.is_empty()){if child.snapshot.task.is_none(){child.snapshot.task=Some(bound(task,4096));}}
        if let Some(model)=p["model"].as_str().or_else(||thread["model"].as_str()).filter(|s|super::valid_model_id(s)){child.snapshot.model=Some(model.into());}
        if let Some(effort)=p["reasoningEffort"].as_str().or_else(||thread["reasoningEffort"].as_str()).filter(|s|s.len()<=64){child.snapshot.effort=Some(effort.into());}
        if let Some(cwd)=thread["cwd"].as_str().filter(|s|s.len()<=4096&&Path::new(s).is_absolute()){child.snapshot.workspace=Some(cwd.into());}
        if let Some(turn) = p["turn"]["id"].as_str().or_else(|| p["turnId"].as_str()) {
            child.snapshot.turn_id = Some(turn.into());
        }
        match method {
            "native/subagent_activity"=>{
                if let Some(path)=p["agentPath"].as_str().filter(|s|!s.is_empty()&&s.len()<=1024){child.snapshot.scope=Some(path.into());}
                if child.snapshot.name.is_none(){child.snapshot.name=p["agentPath"].as_str().and_then(|p|p.rsplit('/').next()).filter(|s|!s.is_empty()).map(|s|bound(s,128));}
                child.snapshot.status=match p["kind"].as_str(){Some("completed")=>InteractionStatus::Completed,Some("interrupted")=>InteractionStatus::Cancelled,Some("started"|"interacted")=>InteractionStatus::Working,_=>child.snapshot.status};
            }
            "native/state" => {
                if let Some(message)=p["message"].as_str().filter(|s|!s.is_empty()){child.snapshot.preview=bound(message,1024);}
                child.snapshot.status = match p["status"].as_str() {
                    Some("completed") => InteractionStatus::Completed,
                    Some("interrupted" | "shutdown") => InteractionStatus::Cancelled,
                    Some("errored" | "not_found" | "notFound") => InteractionStatus::Failed,
                    Some("running" | "pending_init" | "pendingInit") => InteractionStatus::Working,
                    _ => child.snapshot.status,
                };
            }
            "native/idle"
                if matches!(
                    child.snapshot.status,
                    InteractionStatus::Started | InteractionStatus::Working
                ) =>
            {
                child.snapshot.status = InteractionStatus::Completed
            }
            "turn/started" => child.snapshot.status = InteractionStatus::Working,
            "turn/completed" => {
                child.snapshot.status = match p["turn"]["status"].as_str() {
                    Some("failed") => InteractionStatus::Failed,
                    Some("interrupted") => InteractionStatus::Cancelled,
                    Some("completed") => InteractionStatus::Completed,
                    _ => InteractionStatus::Working,
                };
                if let Some(message) = p["turn"]["error"]["message"].as_str() {
                    child.snapshot.preview = message.chars().take(1024).collect();
                }
            }
            "thread/status/changed" => match p["status"]["type"].as_str() {
                Some("active") => child.snapshot.status = InteractionStatus::Working,
                Some("systemError") => child.snapshot.status = InteractionStatus::Failed,
                Some("notLoaded") => child.snapshot.connected = false,
                _ => {}
            },
            "item/agentMessage/delta" | "item/completed"
                if method == "item/agentMessage/delta" || p["item"]["type"] == "agentMessage" =>
            {
                let item = p["itemId"]
                    .as_str()
                    .or_else(|| p["item"]["id"].as_str())
                    .unwrap_or("");
                let mut mutated = false;
                if child.item != item {
                    if !child.text.is_empty() {
                        child.text.push_str("\n\n");
                        mutated = true;
                    }
                    child.item_start = child.text.len();
                    child.item = item.into();
                }
                if method == "item/agentMessage/delta" {
                    let delta = p["delta"].as_str().unwrap_or("");
                    child.text.push_str(delta);
                    mutated |= !delta.is_empty();
                } else if let Some(text) = p["item"]["text"].as_str() {
                    child.text.truncate(child.item_start);
                    child.text.push_str(text);
                    mutated = true;
                }
                if let Some(error) = child.text.error() {
                    bail!("Could not retain child transcript: {error:#}");
                }
                if mutated {
                    child.snapshot.transcript_revision =
                        child.snapshot.transcript_revision.wrapping_add(1);
                }
                child.snapshot.transcript_bytes = child.text.len();
                child.snapshot.preview = child.text.snapshot()?.tail(1024)?;
            }
            _ => {}
        }
        drop(children);
        self.flush_settlers();
        if !matches!(method, "item/agentMessage/delta" | "item/completed")
            || transcript_watch().load(Ordering::Acquire)
        {
            changed();
        }
        Ok(())
    }
    pub fn request(&self, child: &str, request: NativeRequest) {
        if let Some(record) = self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(child)
            && record.snapshot.requests.len() < 64
        {
            record.snapshot.requests.push(request);
        }
        changed();
    }
    pub fn resolve(&self, token: &str) {
        for child in self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values_mut()
        {
            child
                .snapshot
                .requests
                .retain(|request| request.id != token);
        }
        self.flush_settlers();
        changed();
    }
    pub fn turn(&self, child: &str) -> Option<String> {
        self.children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(child)
            .and_then(|record| record.snapshot.turn_id.clone())
    }
    pub fn child_error(&self, child: &str, error: &str) {
        if let Some(record) = self
            .children
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(child)
        {
            record.snapshot.preview = error.chars().take(1024).collect();
        }
        changed();
    }
}
pub(super) fn checkout(request: &RunRequest) -> Result<Option<Arc<Service>>> {
    let Some(id) = request
        .resume_id
        .as_deref()
        .filter(|_| !request.fork_session)
    else {
        return Ok(None);
    };
    let entries = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(entry) = entries.iter().find(|entry| {
        entry.alive.load(Ordering::Acquire)
            && !entry.shutdown.load(Ordering::Acquire)
            && entry.agent == request.agent
            && entry.executable == request.executable
            && entry.project == request.project
            && *entry.parent.lock().unwrap_or_else(|e| e.into_inner()) == id
    }) {
        anyhow::ensure!(
            entry
                .settlers
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .len()
                < 128,
            "This service has too many turns waiting for child completion"
        );
        anyhow::ensure!(
            entry
                .busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_ok(),
            "This native chat already has an active turn in Code"
        );
        return Ok(Some(entry.clone()));
    }
    Ok(None)
}
pub(super) fn register(
    request: &RunRequest,
    pid: i32,
) -> Result<(Arc<Service>, Receiver<Command>, UnixStream)> {
    let mut entries = registry().lock().unwrap_or_else(|e| e.into_inner());
    entries.retain(|entry| entry.alive.load(Ordering::Acquire));
    anyhow::ensure!(
        entries.len() < MAX_SERVICES,
        "Code's native service limit is reached; stop an active task first"
    );
    let (commands, receiver) = mpsc::sync_channel(32);
    let (cleanup, finished) = async_channel::bounded(1);
    let (wake, read) = UnixStream::pair()?;
    wake.set_nonblocking(true)?;
    read.set_nonblocking(true)?;
    let service = Arc::new(Service {
        agent: request.agent,
        executable: request.executable.clone(),
        project: request.project.clone(),
        pid,
        busy: AtomicBool::new(true),
        alive: AtomicBool::new(true),
        shutdown: AtomicBool::new(false),
        idle_cancelled: Arc::new(AtomicBool::new(false)),
        parent: Mutex::new(String::new()),
        commands,
        wake: Mutex::new(wake),
        children: Mutex::new(HashMap::new()),
        idle_since: Mutex::new(Instant::now()),
        settlers: Mutex::new(Vec::new()),
        finished,
        cleanup: Mutex::new(Some(cleanup)),
        active_turn: Mutex::new(None),
        usage: Mutex::new(None),
    });
    entries.push(service.clone());
    changed();
    Ok((service, receiver, read))
}
pub fn services() -> Vec<ServiceSnapshot> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|entry| ServiceSnapshot {
            provider: entry.agent,
            project: entry.project.clone(),
            parent_id: entry
                .parent
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
            process_id: entry.pid as u32,
            active: entry.busy.load(Ordering::Acquire),
            connected: entry.alive.load(Ordering::Acquire),
            usage: entry
                .usage
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        })
        .collect()
}
pub fn children(project: &Path) -> Vec<ChildSnapshot> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|entry| entry.project == project)
        .flat_map(|entry| {
            entry
                .children
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .map(|record| record.snapshot.clone())
                .collect::<Vec<_>>()
        })
        .collect()
}
fn owned(project: &Path, provider: AgentKind, child: &str) -> Result<Arc<Service>> {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .find(|entry| entry.project == project && entry.agent == provider && entry.child(child))
        .cloned()
        .context("This child is not owned by a running Code service")
}
pub fn control_child(
    project: &Path,
    provider: AgentKind,
    child: &str,
    action: ChildAction,
) -> Result<()> {
    let service = owned(project, provider, child)?;
    anyhow::ensure!(
        service.alive.load(Ordering::Acquire),
        "This child's provider disconnected"
    );
    if let ChildAction::Steer(text) = &action {
        anyhow::ensure!(
            provider == AgentKind::Codex,
            "This provider does not expose active-turn child steering"
        );
        anyhow::ensure!(
            !text.trim().is_empty() && text.len() <= PROMPT_LIMIT,
            "Write a valid steering message"
        );
    }
    if let ChildAction::Respond { id, .. } = &action {
        anyhow::ensure!(
            has_child_request(project, provider, child, id),
            "This request is no longer pending for this child"
        );
    }
    service
        .commands
        .try_send(Command::Child(child.into(), action))
        .map_err(|_| anyhow::anyhow!("The native service is busy or disconnected"))?;
    service.notify();
    Ok(())
}
/// Read only one UTF-8 page; worker output spills through ResponseBuffer rather
/// than retaining a giant child transcript or copying it into every snapshot.
pub fn child_transcript_page(
    project: &Path,
    provider: AgentKind,
    child: &str,
    offset: usize,
) -> Result<ChildPage> {
    let service = owned(project, provider, child)?;
    let children = service.children.lock().unwrap_or_else(|e| e.into_inner());
    let record = children
        .get(child)
        .context("Child transcript is unavailable")?;
    transcript_page(record, offset)
}
fn transcript_page(record: &ChildRecord, offset: usize) -> Result<ChildPage> {
    let total = record.text.len();
    // A final correction can shorten text while a page request is in flight.
    let offset = if offset > total {
        total.saturating_sub(PAGE_BYTES)
    } else {
        offset
    };
    // A requested tail offset may bisect a character. Advance at most three
    // continuation bytes, then choose an end at a character boundary too.
    let mut start = offset;
    loop {
        match record.text.copy_range_to(start, start, &mut io::sink()) {
            Ok(()) => break,
            Err(_) if start < total && start - offset < 3 => start += 1,
            Err(error) => return Err(error),
        }
    }
    let requested_end = start.saturating_add(PAGE_BYTES).min(total);
    let mut end = requested_end;
    loop {
        match record.text.copy_range_to(end, end, &mut io::sink()) {
            Ok(()) => break,
            Err(_) if end > start && requested_end - end < 3 => end -= 1,
            Err(error) => return Err(error),
        }
    }
    let mut bytes = Vec::with_capacity(end - start);
    record.text.copy_range_to(start, end, &mut bytes)?;
    Ok(ChildPage {
        text: String::from_utf8(bytes)?,
        offset: start,
        revision: record.snapshot.transcript_revision,
        next_offset: end,
        total_bytes: total,
    })
}
/// App shutdown stops only Code-owned services. Idle eviction never interrupts
/// running children or pending native approvals.
#[allow(dead_code)] // App windows use scoped shutdown_service; reserved for process-wide teardown.
pub fn shutdown() -> async_channel::Receiver<()> {
    let services = registry().lock().unwrap_or_else(|e| e.into_inner()).clone();
    for service in &services {
        service.stop();
    }
    let (done, receiver) = async_channel::bounded(1);
    if !services.is_empty() {
        thread::spawn(move || {
            for service in services {
                let _ = service.finished.recv_blocking();
            }
            drop(done);
        });
    }
    receiver
}
pub fn shutdown_service(
    project: &Path,
    provider: AgentKind,
    parent: &str,
) -> async_channel::Receiver<()> {
    let services: Vec<_> = registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .filter(|service| {
            service.project == project
                && service.agent == provider
                && *service.parent.lock().unwrap_or_else(|e| e.into_inner()) == parent
        })
        .cloned()
        .collect();
    for service in &services {
        service.stop();
    }
    let (done, receiver) = async_channel::bounded(1);
    if !services.is_empty() {
        thread::spawn(move || {
            for service in services {
                let _ = service.finished.recv_blocking();
            }
            drop(done);
        });
    }
    receiver
}
pub fn has_active_children(project: &Path) -> bool {
    registry()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .any(|service| service.project == project && service.has_running_children())
}
pub fn has_child_request(project: &Path, provider: AgentKind, child: &str, request: &str) -> bool {
    owned(project, provider, child).is_ok_and(|service| {
        service.alive.load(Ordering::Acquire)
            && service
                .children
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .get(child)
                .is_some_and(|record| {
                    record
                        .snapshot
                        .requests
                        .iter()
                        .any(|pending| pending.id == request)
                })
    })
}
pub fn take_child_request_error(
    project: &Path,
    provider: AgentKind,
    child: &str,
    request: &str,
) -> Option<String> {
    let service = owned(project, provider, child).ok()?;
    service
        .children
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get_mut(child)?
        .request_errors
        .remove(request)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn service() -> Service {
        let (commands, _) = mpsc::sync_channel(1);
        let (wake, _) = UnixStream::pair().unwrap();
        let (cleanup, finished) = async_channel::bounded(1);
        Service {
            agent: AgentKind::Codex,
            executable: PathBuf::from("fixture"),
            project: PathBuf::from("/fixture"),
            pid: 0,
            busy: AtomicBool::new(false),
            alive: AtomicBool::new(true),
            shutdown: AtomicBool::new(false),
            idle_cancelled: Arc::new(AtomicBool::new(false)),
            parent: Mutex::new("parent".into()),
            commands,
            wake: Mutex::new(wake),
            children: Mutex::new(HashMap::new()),
            idle_since: Mutex::new(Instant::now()),
            settlers: Mutex::new(Vec::new()),
            finished,
            cleanup: Mutex::new(Some(cleanup)),
            active_turn: Mutex::new(None),
            usage: Mutex::new(None),
        }
    }
    #[test]fn child_metadata_and_camel_case_status_match_current_native_protocol(){
        let service=service();service.observe("child","root","thread/started",&serde_json::json!({"thread":{"id":"child","agentNickname":"Hopper","model":"exact-model","reasoningEffort":"high","cwd":"/fixture/worktree","preview":"Inspect the renderer"}})).unwrap();
        service.observe("child","root","native/state",&serde_json::json!({"status":"pendingInit"})).unwrap();let snapshot=service.children.lock().unwrap()["child"].snapshot.clone();assert_eq!(snapshot.name.as_deref(),Some("Hopper"));assert_eq!(snapshot.task.as_deref(),Some("Inspect the renderer"));assert_eq!(snapshot.model.as_deref(),Some("exact-model"));assert_eq!(snapshot.effort.as_deref(),Some("high"));assert_eq!(snapshot.workspace.as_deref(),Some(Path::new("/fixture/worktree")));assert_eq!(snapshot.status,InteractionStatus::Working);
        service.observe("child","root","native/state",&serde_json::json!({"status":"notFound","message":"No longer loaded"})).unwrap();assert_eq!(service.children.lock().unwrap()["child"].snapshot.status,InteractionStatus::Failed);
        for i in 0..128{service.observe(&format!("done-{i}"),"root","native/state",&serde_json::json!({"status":"completed"})).unwrap();}assert!(service.children.lock().unwrap().len()<=128);service.observe("new-working-child","root","native/state",&serde_json::json!({"status":"running"})).unwrap();assert!(service.child("new-working-child"));
    }
    #[test]
    fn terminal_child_retention_preserves_ancestors_of_live_descendants() {
        let service=service();
        service.observe("finished-parent","root","native/state",&serde_json::json!({"status":"completed"})).unwrap();
        service.observe("working-grandchild","finished-parent","native/state",&serde_json::json!({"status":"running"})).unwrap();
        for i in 0..256 { service.observe(&format!("finished-leaf-{i}"),"root","native/state",&serde_json::json!({"status":"completed"})).unwrap(); }
        assert!(service.child("finished-parent"));
        assert!(service.child("working-grandchild"));
        assert!(service.children.lock().unwrap().len()<=MAX_CHILDREN);
    }
    #[test]
    fn child_page_aligns_tail_start_and_end_for_memory_and_spilled_utf8() {
        for repeats in [20_000, 100_000] {
            let service = service();
            let text = "🌊".repeat(repeats) + "x";
            service
                .observe(
                    "child",
                    "parent",
                    "item/agentMessage/delta",
                    &serde_json::json!({"itemId":"item","delta":text}),
                )
                .unwrap();
            let children = service.children.lock().unwrap();
            let record = &children["child"];
            // total-64KiB is one byte into an emoji for this transcript.
            let requested = text.len() - PAGE_BYTES;
            assert!(!text.is_char_boundary(requested));
            let tail = transcript_page(record, requested).unwrap();
            assert_eq!(tail.offset, requested + 3);
            assert_eq!(tail.text, text[tail.offset..]);
            assert_eq!(tail.next_offset, text.len());
            assert!(tail.text.len() <= PAGE_BYTES);
            assert_eq!(tail.revision, 1);
            // A boundary start can still have a non-boundary end.
            let page = transcript_page(record, 4).unwrap();
            assert!(text.is_char_boundary(page.next_offset));
            assert_eq!(page.text, text[page.offset..page.next_offset]);
        }
    }
    #[test]
    fn child_revision_detects_equal_length_final_correction_and_null_preserves_stream() {
        let service = service();
        service
            .observe(
                "child",
                "parent",
                "item/agentMessage/delta",
                &serde_json::json!({"itemId":"item","delta":"old!"}),
            )
            .unwrap();
        service
            .observe(
                "child",
                "parent",
                "item/completed",
                &serde_json::json!({"item":{"id":"item","type":"agentMessage","text":"new!"}}),
            )
            .unwrap();
        let first_revision = {
            let children = service.children.lock().unwrap();
            let page = transcript_page(&children["child"], 0).unwrap();
            assert_eq!(page.text, "new!");
            assert_eq!(page.total_bytes, 4);
            assert_eq!(page.revision, 2);
            page.revision
        };
        service
            .observe(
                "child",
                "parent",
                "item/completed",
                &serde_json::json!({"item":{"id":"item","type":"agentMessage","text":null}}),
            )
            .unwrap();
        let children = service.children.lock().unwrap();
        let page = transcript_page(&children["child"], 0).unwrap();
        assert_eq!(page.text, "new!");
        assert_eq!(page.revision, first_revision);
    }
    #[test]
    fn child_page_rebases_stale_offset_after_final_text_shrinks() {
        let service = service();
        service
            .observe(
                "child",
                "parent",
                "item/agentMessage/delta",
                &serde_json::json!({"itemId":"item","delta":"a".repeat(PAGE_BYTES * 2)}),
            )
            .unwrap();
        service
            .observe(
                "child",
                "parent",
                "item/completed",
                &serde_json::json!({"item":{"id":"item","type":"agentMessage","text":"short"}}),
            )
            .unwrap();
        let children = service.children.lock().unwrap();
        let page = transcript_page(&children["child"], PAGE_BYTES).unwrap();
        assert_eq!(page.offset, 0);
        assert_eq!(page.next_offset, 5);
        assert_eq!(page.revision, 2);
        assert_eq!(page.text, "short");
    }
}
