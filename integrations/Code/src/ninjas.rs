//! Durable personal agents. Headless execution reuses the native provider runtime;
//! the HTTP inference host only authenticates and forwards this private protocol.
use crate::{
    discovery::{self, AgentKind},
    ninja,
    runtime::{
        self, CodexAccess, CodexApprovals, NativeAnswer, NativeRequest, RunHandle, RunRequest,
        RuntimeEvent,
    },
};
use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{OpenOptionsExt, PermissionsExt},
            net::{UnixListener, UnixStream},
            process::CommandExt,
        },
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Condvar, Mutex},
    thread,
    time::Duration,
};
const MAX_MEMORY: usize = 32 * 1024;
const WIRE_LIMIT: u64 = 2 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Identity {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub persona: String,
    pub style: String,
    pub project: PathBuf,
    pub agent: AgentKind,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub ninja: Option<ninja::Selection>,
    pub edits: bool,
    pub access: CodexAccess,
    pub approvals: CodexApprovals,
    #[serde(default)]
    pub revision: u64,
}
impl Profile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            uuid::Uuid::parse_str(&self.id).is_ok(),
            "Invalid Ninja identity"
        );
        ensure!(
            !self.name.trim().is_empty() && self.name.len() <= 80,
            "Give this Ninja a name (up to 80 bytes)"
        );
        ensure!(self.persona.len() <= 8192, "Persona exceeds 8 KiB");
        ensure!(
            styles().iter().any(|(id, _, _)| *id == self.style),
            "Choose a Ninja style"
        );
        ensure!(
            self.project.is_absolute() && self.project.is_dir(),
            "Choose an existing absolute workspace on this host"
        );
        ensure!(
            self.model
                .as_ref()
                .is_none_or(|m| runtime::valid_model_id(m)),
            "Invalid model ID"
        );
        ensure!(
            self.effort
                .as_ref()
                .is_none_or(|e| e.len() <= 64 && !e.chars().any(char::is_control)),
            "Invalid thinking level"
        );
        if self.agent == AgentKind::Ninja {
            self.ninja
                .as_ref()
                .context("Choose Ninja inference settings")?
                .validate()?;
        }
        Ok(())
    }
}
fn compatible(a: &Profile, b: &Profile) -> bool {
    a.project == b.project
        && a.agent == b.agent
        && a.ninja.as_ref().map(|n| n.harness) == b.ninja.as_ref().map(|n| n.harness)
}
pub fn styles() -> [(&'static str, &'static str, u32); 6] {
    [
        ("fire", "Kai · Fire", 0xef6758),
        ("lightning", "Jay · Lightning", 0x619dff),
        ("earth", "Cole · Earth", 0xa5acba),
        ("ice", "Zane · Ice", 0xc2edf4),
        ("energy", "Lloyd · Energy", 0x74d6a0),
        ("water", "Nya · Water", 0x60ccd7),
    ]
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Memory {
    pub content: String,
    pub revision: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub profile: Profile,
    pub chat: String,
    pub prompt: String,
    pub state: String,
    pub status: String,
    pub created: i64,
    pub due: i64,
    pub started: Option<i64>,
    pub finished: Option<i64>,
    pub repeat: Option<i64>,
    pub native_id: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub brief: String,
    pub requests: Vec<NativeRequest>,
    pub acknowledged: bool,
}
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
fn clip(s: &str, max: usize) -> String {
    let mut n = s.len().min(max);
    while !s.is_char_boundary(n) {
        n -= 1
    }
    s[..n].to_owned()
}
pub fn root() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Kai Unified/ninjas")
}
pub fn socket(root: &Path) -> PathBuf {
    use sha2::Digest;
    let h = sha2::Sha256::digest(root.to_string_lossy().as_bytes());
    PathBuf::from(format!(
        "/private/tmp/kai-ninjas-{}-{:02x}{:02x}{:02x}{:02x}{:02x}.sock",
        unsafe { libc::getuid() },
        h[0],
        h[1],
        h[2],
        h[3],
        h[4]
    ))
}
pub struct Store {
    db: Connection,
}
impl Store {
    pub fn open(root: &Path) -> Result<Self> {
        fs::create_dir_all(root)?;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))?;
        let db = Connection::open(root.join("ninjas.sqlite"))?;
        db.busy_timeout(Duration::from_secs(3))?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS profiles(id TEXT PRIMARY KEY, payload TEXT NOT NULL); CREATE TABLE IF NOT EXISTS memory(id TEXT PRIMARY KEY, content TEXT NOT NULL DEFAULT '', revision INTEGER NOT NULL DEFAULT 0); CREATE TABLE IF NOT EXISTS tasks(id TEXT PRIMARY KEY, profile TEXT NOT NULL, chat TEXT NOT NULL, state TEXT NOT NULL, due INTEGER NOT NULL, created INTEGER NOT NULL, payload TEXT NOT NULL); CREATE INDEX IF NOT EXISTS task_queue ON tasks(state,due,created);")?;
        Ok(Self { db })
    }
    fn profiles(&self) -> Result<Vec<Profile>> {
        let mut q = self
            .db
            .prepare("SELECT payload FROM profiles ORDER BY rowid")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    fn profile(&self, id: &str) -> Result<Profile> {
        let v: String =
            self.db
                .query_row("SELECT payload FROM profiles WHERE id=?", [id], |r| {
                    r.get(0)
                })?;
        Ok(serde_json::from_str(&v)?)
    }
    fn save_profile(&mut self, mut p: Profile) -> Result<Profile> {
        p.validate()?;
        let tx = self.db.transaction()?;
        let current: Option<String> = tx
            .query_row("SELECT payload FROM profiles WHERE id=?", [&p.id], |r| {
                r.get(0)
            })
            .optional()?;
        if let Some(current) = current {
            let old: Profile = serde_json::from_str(&current)?;
            ensure!(
                old.revision == p.revision,
                "This Ninja was changed elsewhere; refresh before saving"
            );
        } else {
            ensure!(p.revision == 0, "This Ninja no longer exists");
        }
        p.revision += 1;
        tx.execute("INSERT INTO profiles(id,payload) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![p.id,serde_json::to_string(&p)?])?;
        tx.execute("INSERT OR IGNORE INTO memory(id) VALUES(?)", [&p.id])?;
        tx.commit()?;
        Ok(p)
    }
    fn memory(&self, id: &str) -> Result<Memory> {
        Ok(self.db.query_row(
            "SELECT content,revision FROM memory WHERE id=?",
            [id],
            |r| {
                Ok(Memory {
                    content: r.get(0)?,
                    revision: r.get(1)?,
                })
            },
        )?)
    }
    fn write_memory(&mut self, id: &str, revision: u64, content: &str) -> Result<Memory> {
        ensure!(
            content.len() <= MAX_MEMORY,
            "Memory exceeds 32 KiB; keep durable facts concise"
        );
        ensure!(
            self.db.execute(
                "UPDATE memory SET content=?1,revision=revision+1 WHERE id=?2 AND revision=?3",
                params![content, id, revision]
            )? == 1,
            "Memory changed; read its current revision before updating"
        );
        self.memory(id)
    }
    fn save_task(&self, t: &Task) -> Result<()> {
        self.db.execute("INSERT INTO tasks(id,profile,chat,state,due,created,payload) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(id) DO UPDATE SET state=excluded.state,due=excluded.due,payload=excluded.payload",params![t.id,t.profile.id,t.chat,t.state,t.due,t.created,serde_json::to_string(t)?])?;
        Ok(())
    }
    fn task(&self, id: &str) -> Result<Task> {
        let v: String = self
            .db
            .query_row("SELECT payload FROM tasks WHERE id=?", [id], |r| r.get(0))?;
        Ok(serde_json::from_str(&v)?)
    }
    fn tasks(&self, profile: Option<&str>) -> Result<Vec<Task>> {
        let mut q=self.db.prepare("SELECT payload FROM tasks WHERE (?1 IS NULL OR profile=?1) ORDER BY created DESC,rowid DESC LIMIT 100")?;
        q.query_map([profile], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    fn latest_chat(&self, profile: &str, chat: &str) -> Result<Option<Task>> {
        let value:Option<String>=self.db.query_row("SELECT payload FROM tasks WHERE profile=?1 AND chat=?2 AND state='completed' ORDER BY created DESC,rowid DESC LIMIT 1",params![profile,chat],|r|r.get(0)).optional()?;
        value.map(|v| Ok(serde_json::from_str(&v)?)).transpose()
    }
    fn pending(&self) -> Result<Vec<Task>> {
        let mut q=self.db.prepare("SELECT payload FROM tasks WHERE state IN ('queued','running','approval') ORDER BY created ASC LIMIT 512")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|r| Ok(serde_json::from_str(&r?)?))
            .collect()
    }
    fn recover(&self) -> Result<()> {
        for mut t in self.pending()? {
            if ["running", "approval"].contains(&t.state.as_str()) {
                t.state = "interrupted".into();
                t.status =
                    "Worker restarted. Review before continuing; no tool actions were replayed."
                        .into();
                t.finished = Some(now());
                t.requests.clear();
                self.save_task(&t)?;
            }
        }
        Ok(())
    }
    fn submit(
        &self,
        id: &str,
        prompt: &str,
        chat: Option<&str>,
        due: i64,
        repeat: Option<i64>,
    ) -> Result<Task> {
        ensure!(
            !prompt.trim().is_empty() && prompt.len() <= 64 * 1024,
            "Write a task up to 64 KiB"
        );
        ensure!(
            repeat.is_none_or(|r| r >= 300),
            "Recurring tasks must be at least five minutes apart"
        );
        ensure!(
            self.db.query_row(
                "SELECT COUNT(*) FROM tasks WHERE state IN ('queued','running','approval')",
                [],
                |r| r.get::<_, i64>(0)
            )? < 512,
            "This host already has 512 pending tasks"
        );
        let profile = self.profile(id)?;
        profile.validate()?;
        let chat = chat
            .map(str::to_owned)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        ensure!(uuid::Uuid::parse_str(&chat).is_ok(), "Invalid Ninja chat");
        let native_id = self
            .latest_chat(id, &chat)?
            .filter(|t| compatible(&t.profile, &profile))
            .and_then(|t| t.native_id);
        let t = Task {
            id: uuid::Uuid::new_v4().to_string(),
            model: profile.model.clone(),
            effort: profile.effort.clone(),
            profile,
            chat,
            prompt: prompt.into(),
            state: "queued".into(),
            status: "Queued".into(),
            created: now(),
            due: due.max(now()),
            started: None,
            finished: None,
            repeat,
            native_id,
            brief: String::new(),
            requests: vec![],
            acknowledged: false,
        };
        self.save_task(&t)?;
        Ok(t)
    }
}
use rusqlite::OptionalExtension;
struct Live {
    handle: Arc<Mutex<RunHandle>>,
    project: PathBuf,
    provider: AgentKind,
    wake: async_channel::Sender<()>,
}
struct Worker {
    store: Mutex<Store>,
    root: PathBuf,
    live: Mutex<HashMap<String, Live>>,
    change: Mutex<u64>,
    wake: Condvar,
}
impl Worker {
    fn changed(&self) {
        let mut n = self.change.lock().unwrap();
        *n += 1;
        self.wake.notify_all();
    }
    fn snapshot(&self) -> Result<Value> {
        let providers = [
            AgentKind::Codex,
            AgentKind::Claude,
            AgentKind::Gemini,
            AgentKind::Cursor,
            AgentKind::Grok,
            AgentKind::OpenCode,
            AgentKind::Antigravity,
            AgentKind::Ninja,
        ]
        .iter()
        .map(|a| {
            let d = discovery::discover_agent(*a, &dirs::home_dir().unwrap_or_default());
            json!({"id":a.command(),"agent":a,"name":a.name(),"installed":d.available})
        })
        .collect::<Vec<_>>();
        let db = self.store.lock().unwrap();
        let profiles = db.profiles()?;
        let mut tasks = db.pending()?;
        let existing = tasks
            .iter()
            .map(|t| t.id.clone())
            .collect::<std::collections::HashSet<_>>();
        tasks.extend(
            db.tasks(None)?
                .into_iter()
                .filter(|t| !existing.contains(&t.id))
                .take(20),
        );
        let tasks=tasks.iter().map(|t|{let mut requests=t.requests.iter().map(|r|json!(r)).collect::<Vec<_>>();for child in runtime::broker::children(&t.profile.project).into_iter().filter(|c|t.native_id.as_deref()==Some(c.parent_id.as_str())){for request in child.requests{let mut v=json!(request);v["childId"]=json!(child.id);requests.push(v);}}json!({"id":t.id,"profile":t.profile.id,"chat":t.chat,"prompt":clip(&t.prompt,600),"state":if requests.is_empty(){t.state.as_str()}else{"approval"},"status":t.status,"model":t.model,"effort":t.effort,"brief":clip(&t.brief,2400),"created":t.created,"due":t.due,"started":t.started,"finished":t.finished,"requests":requests,"acknowledged":t.acknowledged})}).collect::<Vec<_>>();
        Ok(
            json!({"revision":*self.change.lock().unwrap(),"profiles":profiles,"tasks":tasks,"machine":ninja::local_machine().name,"home":dirs::home_dir(),"providers":providers}),
        )
    }
    fn request(self: &Arc<Self>, v: Value) -> Result<Value> {
        let action = v["action"].as_str().context("Missing action")?;
        let id = || v["id"].as_str().context("Missing identity");
        match action {
            "snapshot" => self.snapshot(),
            "watch" => {
                let seen = v["revision"].as_u64().unwrap_or(0);
                let guard = self.change.lock().unwrap();
                if *guard == seen {
                    drop(
                        self.wake
                            .wait_timeout(guard, Duration::from_secs(25))
                            .unwrap(),
                    );
                } else {
                    drop(guard)
                }
                self.snapshot()
            }
            "import_handoff" => {
                let mut value = handoff(
                    &json!({"action":"receive","id":v["handoff"],"sha256":v["sha256"],"root":self.root}),
                )?;
                if value["received"] == true {
                    let mut db = self.store.lock().unwrap();
                    let mut p = db.profile(id()?)?;
                    let imported = PathBuf::from(
                        value["project"]
                            .as_str()
                            .context("Missing imported workspace")?,
                    );
                    if p.project != imported {
                        p.project = imported;
                        p = db.save_profile(p)?;
                    }
                    value["profile"] = json!(p);
                    self.changed();
                }
                Ok(value)
            }
            "save_profile" => {
                let p: Profile = serde_json::from_value(v["profile"].clone())?;
                let p = self.store.lock().unwrap().save_profile(p)?;
                self.changed();
                Ok(json!(p))
            }
            "memory" => {
                let mut db = self.store.lock().unwrap();
                if let Some(content) = v["content"].as_str() {
                    let result = db.write_memory(
                        id()?,
                        v["revision"].as_u64().context("Missing memory revision")?,
                        content,
                    )?;
                    drop(db);
                    self.changed();
                    Ok(json!(result))
                } else {
                    Ok(json!(db.memory(id()?)?))
                }
            }
            "submit" => {
                let prompt = v["prompt"].as_str().context("Write a task")?;
                ensure!(
                    !prompt.trim().is_empty() && prompt.len() <= 64 * 1024,
                    "Write a task up to 64 KiB"
                );
                let mut db = self.store.lock().unwrap();
                let previous = if !v["profile"].is_null() {
                    let profile: Profile = serde_json::from_value(v["profile"].clone())?;
                    ensure!(profile.id == id()?, "Profile and task identity differ");
                    let previous = db.profile(id()?)?;
                    db.save_profile(profile)?;
                    Some(previous)
                } else {
                    None
                };
                let result = db.submit(
                    id()?,
                    prompt,
                    v["chat"].as_str(),
                    v["due"].as_i64().unwrap_or(0),
                    v["repeat"].as_i64(),
                );
                if result.is_err() {
                    if let Some(previous) = previous {
                        db.db.execute(
                            "UPDATE profiles SET payload=?1 WHERE id=?2",
                            params![serde_json::to_string(&previous)?, previous.id],
                        )?;
                    }
                }
                drop(db);
                self.changed();
                Ok(json!(result?))
            }
            "acknowledge" => {
                let db = self.store.lock().unwrap();
                let mut t = db.task(id()?)?;
                t.acknowledged = true;
                db.save_task(&t)?;
                drop(db);
                self.changed();
                Ok(json!({"ok":true}))
            }
            "stop" | "respond" => {
                let live = self.live.lock().unwrap();
                if let Some(run) = live.get(id()?) {
                    if action == "stop" {
                        run.handle.lock().unwrap().cancel();
                        if let Some(parent) = self.store.lock().unwrap().task(id()?)?.native_id {
                            runtime::broker::shutdown_service(&run.project, run.provider, &parent);
                        }
                        let _ = run.wake.try_send(());
                    } else {
                        let token = v["request"].as_str().context("Choose a pending request")?;
                        let answer: NativeAnswer = serde_json::from_value(v["answer"].clone())?;
                        let task = self.store.lock().unwrap().task(id()?)?;
                        let request = if let Some(child) = v["child"].as_str() {
                            runtime::broker::children(&run.project)
                                .into_iter()
                                .filter(|c| {
                                    c.id == child
                                        && task.native_id.as_deref() == Some(c.parent_id.as_str())
                                })
                                .flat_map(|c| c.requests)
                                .find(|r| r.id == token)
                        } else {
                            task.requests.into_iter().find(|r| r.id == token)
                        }
                        .context("This request is no longer pending")?;
                        validate_answer(&request, &answer)?;

                        if let Some(child) = v["child"].as_str() {
                            runtime::broker::control_child(
                                &run.project,
                                run.provider,
                                child,
                                runtime::broker::ChildAction::Respond {
                                    id: token.into(),
                                    answer,
                                },
                            )?;
                        } else {
                            run.handle.lock().unwrap().respond(token, answer)?;
                        }
                    }
                } else {
                    ensure!(action == "stop", "This task is no longer running");
                    let db = self.store.lock().unwrap();
                    let mut t = db.task(id()?)?;
                    ensure!(
                        ["queued", "running", "approval"].contains(&t.state.as_str()),
                        "This task has already stopped"
                    );
                    t.state = "stopped".into();
                    t.status = "Stopped before starting".into();
                    t.finished = Some(now());
                    db.save_task(&t)?;
                }
                drop(live);
                self.changed();
                Ok(json!({"ok":true}))
            }
            "models" => {
                let agent: AgentKind = serde_json::from_value(v["agent"].clone())?;
                let project = PathBuf::from(
                    v["project"]
                        .as_str()
                        .context("Choose the host workspace first")?,
                );
                ensure!(
                    project.is_absolute() && project.is_dir(),
                    "Choose an existing host workspace"
                );
                if agent == AgentKind::Ninja {
                    let selection: ninja::Selection = serde_json::from_value(v["ninja"].clone())?;
                    let mut models = ninja::installed(&selection)?;
                    if let Ok(info) = ninja::inspect(&selection) {
                        if let Some(m) = models.iter_mut().find(|m| m.id == info.id) {
                            *m = info;
                        }
                    }
                    return Ok(
                        json!({"models":models.iter().map(|m|json!({"id":m.id,"name":m.label,"efforts":m.thinking})).collect::<Vec<_>>()}),
                    );
                }
                let cli = discovery::discover_agent(agent, &dirs::home_dir().unwrap_or_default())
                    .executable
                    .context("Provider is not installed on this host")?;
                let info = crate::agent_controls::inspect_model_project(
                    agent,
                    &cli,
                    &dirs::home_dir().unwrap_or_default(),
                    &project,
                    v["model"].as_str(),
                );
                Ok(
                    json!({"model":info.defaults.model,"effort":info.defaults.effort,"models":info.models.iter().map(|m|json!({"id":m.id,"name":m.label,"efforts":m.reasoning_efforts})).collect::<Vec<_>>()}),
                )
            }
            "delete_profile" => {
                let db = self.store.lock().unwrap();
                ensure!(
                    !db.tasks(Some(id()?))?
                        .iter()
                        .any(|t| ["queued", "running", "approval"].contains(&t.state.as_str())),
                    "Stop this Ninja's queued and running tasks first"
                );
                db.db.execute("DELETE FROM profiles WHERE id=?", [id()?])?;
                db.db.execute("DELETE FROM memory WHERE id=?", [id()?])?;
                drop(db);
                self.changed();
                Ok(json!({"ok":true}))
            }
            "task" => {
                let t = self.store.lock().unwrap().task(id()?)?;
                let mut value = json!(t);
                value["subagents"]=json!(runtime::broker::children(&t.profile.project).iter().filter(|c|t.native_id.as_deref()==Some(c.parent_id.as_str())).map(|c|json!({"id":c.id,"name":c.name,"task":c.task,"model":c.model,"status":format!("{:?}",c.status),"preview":c.preview,"requests":c.requests})).collect::<Vec<_>>());
                Ok(value)
            }
            _ => bail!("Unknown Ninja operation"),
        }
    }
    fn queue(self: Arc<Self>) {
        loop {
            let revision = *self.change.lock().unwrap();
            let mut started = false;
            let db = self.store.lock().unwrap();
            let tasks = db.pending().unwrap_or_default();
            let active_profiles = tasks
                .iter()
                .filter(|t| ["running", "approval"].contains(&t.state.as_str()))
                .map(|t| t.profile.id.as_str())
                .collect::<Vec<_>>();
            let next = if active_profiles.len() < 2 {
                tasks
                    .iter()
                    .filter(|t| {
                        t.state == "queued"
                            && t.due <= now()
                            && !active_profiles.contains(&t.profile.id.as_str())
                    })
                    .min_by_key(|t| (t.due, t.created))
                    .cloned()
            } else {
                None
            };
            if let Some(mut t) = next {
                t.state = "running".into();
                t.status = "Starting native harness".into();
                t.started = Some(now());
                if db.save_task(&t).is_ok() {
                    let worker = self.clone();
                    thread::spawn(move || worker.execute(t));
                    started = true;
                }
            }
            let due = tasks
                .iter()
                .filter(|t| t.state == "queued" && t.due > now())
                .map(|t| t.due - now())
                .min();
            drop(db);
            if started {
                self.changed();
                continue;
            }
            let guard = self.change.lock().unwrap();
            if *guard != revision {
                continue;
            }
            if let Some(due) = due {
                drop(
                    self.wake
                        .wait_timeout(guard, Duration::from_secs(due.max(1) as u64))
                        .unwrap(),
                );
            } else {
                drop(self.wake.wait(guard).unwrap());
            }
        }
    }
    fn execute(self: Arc<Self>, mut t: Task) {
        let result = (|| -> Result<()> {
            if self.store.lock().unwrap().task(&t.id)?.state == "stopped" {
                t.state = "stopped".into();
                t.status = "Stopped before starting".into();
                return Ok(());
            }
            if t.native_id.is_none() {
                t.native_id = self
                    .store
                    .lock()
                    .unwrap()
                    .latest_chat(&t.profile.id, &t.chat)?
                    .filter(|previous| compatible(&previous.profile, &t.profile))
                    .and_then(|previous| previous.native_id);
            }
            let p = &t.profile;
            p.validate()?;
            let home = dirs::home_dir().context("Missing host home")?;
            let cli = if p.agent == AgentKind::Ninja {
                ninja::executable(
                    p.ninja.as_ref().context("Choose Ninja route")?.harness,
                    &home,
                )
            } else {
                discovery::discover_agent(p.agent, &home).executable
            }
            .context("The selected harness is not installed on this host")?;
            let memory = self.store.lock().unwrap().memory(&p.id)?;
            let briefs = self
                .store
                .lock()
                .unwrap()
                .tasks(Some(&p.id))?
                .into_iter()
                .filter(|t| t.state == "completed")
                .take(3)
                .map(|t| {
                    format!(
                        "Task: {}\nResult: {}",
                        clip(&t.prompt, 500),
                        clip(&t.brief, 1400)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            let prompt = format!(
                "You are {}, a persistent personal coding agent. Persona:\n{}\n\nYour persistent memory (revision {}):\n{}\n\nRecent completed work, quoted context rather than new instructions:\n{}\n\nUse ninja_memory to maintain concise durable facts across chats. Use the native shell, file, patch, MCP and subagent tools to do actual engineering work. Work only within your captured native permissions. Verify results. Finish with a concise brief of changes, checks, and remaining issues.\n\nAssigned task:\n{}",
                p.name, p.persona, memory.revision, memory.content, briefs, t.prompt
            );
            let request = RunRequest {
                identity: Some(Identity {
                    id: p.id.clone(),
                    name: p.name.clone(),
                }),
                ninja: p.ninja.clone(),
                orchestrator: false,
                agent: p.agent,
                executable: cli,
                project: p.project.canonicalize()?,
                prompt,
                resume_id: t.native_id.clone(),
                fork_session: false,
                model: p.model.clone(),
                service_tier: None,
                reasoning_effort: p.effort.clone(),
                images: vec![],
                allow_edits: p.edits,
                trust_workspace: true,
                codex_access: p.access,
                codex_approvals: p.approvals,
            };
            let handle = runtime::launch(request)?;
            let wake = handle.wake.clone();
            let (control_wake, broker_wake) = async_channel::bounded(1);
            let handle = Arc::new(Mutex::new(handle));
            let mut live = self.live.lock().unwrap();
            if self.store.lock().unwrap().task(&t.id)?.state == "stopped" {
                handle.lock().unwrap().cancel();
                t.state = "stopped".into();
                t.status = "Stopped before starting".into();
                return Ok(());
            }
            live.insert(
                t.id.clone(),
                Live {
                    handle: handle.clone(),
                    project: p.project.clone(),
                    provider: if p.agent == AgentKind::Ninja {
                        match p.ninja.as_ref().unwrap().harness {
                            ninja::Harness::Codex => AgentKind::Codex,
                            ninja::Harness::Claude => AgentKind::Claude,
                        }
                    } else {
                        p.agent
                    },
                    wake: control_wake,
                },
            );
            drop(live);
            let mut finished = false;
            let mut last_save = std::time::Instant::now() - Duration::from_secs(2);
            loop {
                let h = handle.lock().unwrap();
                if h.is_cancelled() {
                    t.state = "stopped".into();
                    t.status = "Stopped".into();
                    break;
                }
                let mut changed = false;
                let mut urgent = false;
                while let Ok(event) = h.events.try_recv() {
                    changed = true;
                    urgent |= !matches!(
                        &event,
                        RuntimeEvent::Text(_)
                            | RuntimeEvent::ReplaceText(_)
                            | RuntimeEvent::ReplacePreview(_)
                    );
                    match event {
                        RuntimeEvent::Text(text) => {
                            t.brief.push_str(&text);
                            t.brief = clip(&t.brief, 64 * 1024);
                        }
                        RuntimeEvent::ReplaceText(text) | RuntimeEvent::ReplacePreview(text) => {
                            t.brief = clip(&text, 64 * 1024)
                        }
                        RuntimeEvent::Status(s) => t.status = s,
                        RuntimeEvent::SessionId(id) => t.native_id = Some(id),
                        RuntimeEvent::Usage(u) => {
                            if u.model.is_some() {
                                t.model = u.model.clone()
                            }
                            if u.reasoning_effort.is_some() {
                                t.effort = u.reasoning_effort.clone()
                            }
                        }
                        RuntimeEvent::Request(r) => {
                            if p.access == CodexAccess::Full
                                && p.approvals == CodexApprovals::Never
                                && p.edits
                            {
                                if let crate::runtime::NativeRequestKind::Approval { choices } =
                                    &r.kind
                                {
                                    if let Some(choice) = choices.iter().find(|c| {
                                        c.label.to_lowercase().contains("allow once")
                                            || c.id == "allow"
                                            || c.id == "allow-once"
                                    }) {
                                        h.respond(&r.id, NativeAnswer::Choice(choice.id.clone()))?;
                                        continue;
                                    }
                                }
                            }
                            t.state = "approval".into();
                            t.status = r.title.clone();
                            t.requests.push(*r)
                        }
                        RuntimeEvent::RequestResolved(id) => {
                            t.requests.retain(|r| r.id != id);
                            if t.requests.is_empty() {
                                t.state = "running".into();
                            }
                        }
                        RuntimeEvent::WorkspaceTool {
                            id,
                            tool,
                            arguments,
                        } => {
                            let value = if tool == "ninja_memory" {
                                let result = (|| -> Result<Memory> {
                                    let mut db = self.store.lock().unwrap();
                                    match arguments["action"].as_str() {
                                        Some("read") => db.memory(&p.id),
                                        Some("write") => db.write_memory(
                                            &p.id,
                                            arguments["revision"]
                                                .as_u64()
                                                .context("Read memory revision first")?,
                                            arguments["content"]
                                                .as_str()
                                                .context("Write memory content")?,
                                        ),
                                        _ => bail!("Unknown memory operation"),
                                    }
                                })();
                                match result {
                                    Ok(m) => json!(m),
                                    Err(e) => json!({"error":e.to_string()}),
                                }
                            } else {
                                json!({"error":"This worker's managed capability is ninja_memory; use native project tools for engineering"})
                            };
                            h.native_command(
                                "code/workspace/result",
                                json!({"id":id,"value":value}),
                            )?;
                        }
                        RuntimeEvent::Failed(error) => {
                            t.state = "failed".into();
                            t.status = error;
                            finished = true;
                        }
                        RuntimeEvent::Finished => {
                            finished = true;
                            t.status = "Finishing native child work".into();
                        }
                        _ => {}
                    }
                }
                if finished && h.settled.is_closed() {
                    if t.state != "failed" {
                        t.state = "completed".into();
                        t.status = "Completed".into();
                    }
                    if let Some(source) = h.take_response_source() {
                        let output = self.root.join(format!("{}.transcript", t.id));
                        let mut file = OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .mode(0o600)
                            .open(output)?;
                        source.copy_range_to(0, source.len(), &mut file)?;
                        let tail = source.tail(64 * 1024)?;
                        let paragraphs = crate::presentation::blocks(&tail)
                            .into_iter()
                            .filter_map(|b| match b {
                                crate::presentation::MessageBlock::Paragraph(s)
                                | crate::presentation::MessageBlock::RichParagraph {
                                    text: s,
                                    ..
                                } => Some(s),
                                _ => None,
                            })
                            .collect::<Vec<_>>();
                        if !paragraphs.is_empty() {
                            t.brief = clip(&paragraphs.join("\n\n"), 64 * 1024)
                        }
                    }
                    break;
                }
                drop(h);
                if changed && (urgent || last_save.elapsed() >= Duration::from_millis(750)) {
                    last_save = std::time::Instant::now();
                    self.store.lock().unwrap().save_task(&t)?;
                    self.changed();
                }
                crate::ninjas::wait_events(&wake, &broker_wake);
            }
            Ok(())
        })();
        if let Err(e) = result {
            t.state = "failed".into();
            t.status = e.to_string();
        }
        t.finished = Some(now());
        t.requests.clear();
        self.live.lock().unwrap().remove(&t.id);
        let db = self.store.lock().unwrap();
        let _ = db.save_task(&t);
        if t.state == "completed" {
            if let Some(repeat) = t.repeat {
                let _ = db.submit(
                    &t.profile.id,
                    &t.prompt,
                    Some(&t.chat),
                    (t.due + repeat).max(now() + repeat),
                    Some(repeat),
                );
            }
        }
        drop(db);
        self.changed();
    }
}
fn validate_answer(request: &NativeRequest, answer: &NativeAnswer) -> Result<()> {
    match (answer, &request.kind) {
        (NativeAnswer::Cancel, _) => Ok(()),
        (NativeAnswer::Choice(id), runtime::NativeRequestKind::Approval { choices }) => {
            ensure!(
                choices.iter().any(|c| c.id == *id),
                "Choose an offered approval response"
            );
            Ok(())
        }
        (NativeAnswer::Answers(answers), runtime::NativeRequestKind::Questions { questions }) => {
            ensure!(
                answers
                    .keys()
                    .all(|id| questions.iter().any(|q| q.id == *id)),
                "Unknown question"
            );
            for q in questions {
                let values = answers.get(&q.id).map(Vec::as_slice).unwrap_or(&[]);
                ensure!(
                    !q.required || values.iter().any(|v| !v.trim().is_empty()),
                    "Answer each required question"
                );
                ensure!(q.multiple || values.len() <= 1, "Choose one answer");
                ensure!(
                    values.len() <= 32
                        && values.iter().all(|v| v.len() <= 16000 && !v.contains('\0')),
                    "Answer too large"
                );
            }
            Ok(())
        }
        _ => bail!("Unsupported response for this request"),
    }
}
fn wait_events(a: &async_channel::Receiver<()>, b: &async_channel::Receiver<()>) {
    if a.is_closed() {
        b.recv_blocking().ok();
    } else {
        futures_lite::future::block_on(futures_lite::future::or(a.recv(), b.recv())).ok();
    }
}
pub fn serve() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    let folder = args
        .windows(2)
        .find(|p| p[0] == "--data")
        .map(|p| PathBuf::from(&p[1]))
        .unwrap_or_else(root);
    let db = Store::open(&folder)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(folder.join("worker.lock"))?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0,
        "This Ninja worker is already running"
    );
    db.recover()?;
    let socket = socket(&folder);
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let _awake = if args.iter().any(|a| a == "--keep-reachable") {
        Some(
            Command::new("/usr/bin/caffeinate")
                .args(["-s", "-w", &std::process::id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()?,
        )
    } else {
        None
    };
    let worker = Arc::new(Worker {
        store: Mutex::new(db),
        root: folder,
        live: Mutex::new(HashMap::new()),
        change: Mutex::new(1),
        wake: Condvar::new(),
    });
    {
        let w = worker.clone();
        thread::spawn(move || w.queue());
    }
    {
        let w = worker.clone();
        thread::spawn(move || {
            let signal = runtime::broker::wake();
            while signal.recv_blocking().is_ok() {
                for run in w.live.lock().unwrap().values() {
                    let _ = run.wake.try_send(());
                }
                w.changed();
                thread::sleep(Duration::from_millis(100));
            }
        });
    }

    let slots = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    for incoming in listener.incoming() {
        let mut stream = incoming?;
        use std::sync::atomic::Ordering;
        if slots.fetch_add(1, Ordering::AcqRel) >= 16 {
            slots.fetch_sub(1, Ordering::AcqRel);
            continue;
        }
        let worker = worker.clone();
        let slots = slots.clone();
        thread::spawn(move || {
            let _ = (|| -> Result<()> {
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                stream.set_write_timeout(Some(Duration::from_secs(5)))?;
                let mut line = String::new();
                BufReader::new((&stream).take(WIRE_LIMIT + 1)).read_line(&mut line)?;
                ensure!(
                    line.len() as u64 <= WIRE_LIMIT && line.ends_with('\n'),
                    "Request too large"
                );
                let result = worker
                    .request(serde_json::from_str(&line)?)
                    .unwrap_or_else(|e| json!({"error":e.to_string()}));
                serde_json::to_writer(&mut stream, &result)?;
                stream.write_all(b"\n")?;
                Ok(())
            })();
            slots.fetch_sub(1, Ordering::AcqRel);
        });
    }
    Ok(())
}
pub fn call_local(data: &Path, request: &Value, start: bool) -> Result<Value> {
    let address = socket(data);
    if start && data == root() {
        install_service(&std::env::current_exe()?, false)?;
    }
    if start && UnixStream::connect(&address).is_err() {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .args(["--ninja-worker", "--data"])
            .arg(data)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        let _ = command.spawn()?;
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while UnixStream::connect(&address).is_err() && std::time::Instant::now() < until {
            thread::sleep(Duration::from_millis(20));
        }
    }
    let mut stream =
        UnixStream::connect(address).context("Ninja worker is not running on this host")?;
    stream.set_read_timeout(Some(Duration::from_secs(
        if request["action"] == "import_handoff" {
            130
        } else {
            35
        },
    )))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    serde_json::to_writer(&mut stream, request)?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream.take(8 * 1024 * 1024 + 1)).read_line(&mut line)?;
    ensure!(
        line.len() <= 8 * 1024 * 1024,
        "Worker reply exceeds its bound"
    );
    let value: Value = serde_json::from_str(&line)?;
    if let Some(error) = value["error"].as_str() {
        bail!("{error}")
    }
    Ok(value)
}

/// Fixed helper code, bounded JSON input; neither archives nor remote commands are shell evaluated.
pub fn handoff(value: &Value) -> Result<Value> {
    let mut command = Command::new("/usr/bin/python3");
    command.args(["-c", include_str!("../native_helpers/kai_ninja_handoff.py")]);
    let output = ninja::http::run(
        command,
        Some(&serde_json::to_vec(value)?),
        Duration::from_secs(120),
    )
    .context("Workspace handoff did not finish")?;
    let v: Value = serde_json::from_slice(&output).context("Invalid handoff reply")?;
    if let Some(error) = v["error"].as_str() {
        bail!("{error}")
    }
    Ok(v)
}
/// Stable user service survives application exit and starts after the user's next login.
pub fn install_service(executable: &Path, keep_reachable: bool) -> Result<()> {
    let folder = dirs::home_dir()
        .context("Missing home")?
        .join("Library/LaunchAgents");
    fs::create_dir_all(&folder)?;
    let path = folder.join("com.kai.ninjas.worker.plist");
    let escape = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    };
    let extra = if keep_reachable {
        "<string>--keep-reachable</string>"
    } else {
        ""
    };
    let plist = format!(
        "<?xml version=\"1.0\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>com.kai.ninjas.worker</string><key>ProgramArguments</key><array><string>{}</string><string>--ninja-worker</string>{extra}</array><key>RunAtLoad</key><true/><key>KeepAlive</key><true/><key>ThrottleInterval</key><integer>10</integer><key>EnvironmentVariables</key><dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:{}/.local/bin</string></dict></dict></plist>",
        escape(&executable.to_string_lossy()),
        escape(&dirs::home_dir().unwrap().to_string_lossy())
    );
    if fs::read_to_string(&path).ok().as_deref() == Some(&plist) {
        return Ok(());
    }
    fs::write(&path, plist)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    let domain = format!("gui/{}", unsafe { libc::getuid() });
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &domain])
        .arg(&path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    ensure!(
        Command::new("/bin/launchctl")
            .args(["bootstrap", &domain])
            .arg(&path)
            .status()?
            .success(),
        "Could not install the Ninja login service"
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let p =
                std::env::temp_dir().join(format!("kai-personal-test-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn profile(&self) -> Profile {
            Profile {
                id: uuid::Uuid::new_v4().to_string(),
                name: "Ice".into(),
                persona: "Verify changes".into(),
                style: "ice".into(),
                project: self.0.clone(),
                agent: AgentKind::Codex,
                model: Some("gpt-6.1-sol".into()),
                effort: Some("medium".into()),
                ninja: None,
                edits: true,
                access: CodexAccess::Workspace,
                approvals: CodexApprovals::Ask,
                revision: 0,
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn memory_is_scoped_and_revision_checked() {
        let f = Fixture::new();
        let mut db = Store::open(&f.0).unwrap();
        let a = db.save_profile(f.profile()).unwrap();
        let b = db.save_profile(f.profile()).unwrap();
        assert_eq!(db.memory(&a.id).unwrap().revision, 0);
        db.write_memory(&a.id, 0, "Check formatting").unwrap();
        assert!(db.write_memory(&a.id, 0, "stale").is_err());
        assert_eq!(db.memory(&b.id).unwrap().content, "");
        assert!(
            db.write_memory(&a.id, 1, &"a".repeat(MAX_MEMORY + 1))
                .is_err()
        );
        let mut stale = a.clone();
        stale.name = "Updated".into();
        db.save_profile(stale.clone()).unwrap();
        assert!(db.save_profile(stale).is_err());
    }
    #[test]
    fn captures_permissions_and_hands_off_provider_changes() {
        let f = Fixture::new();
        let mut db = Store::open(&f.0).unwrap();
        let mut p = db.save_profile(f.profile()).unwrap();
        let mut task = db.submit(&p.id, "Inspect project", None, 0, None).unwrap();
        task.state = "completed".into();
        task.native_id = Some("native-codex".into());
        db.save_task(&task).unwrap();
        p.agent = AgentKind::Claude;
        p.access = CodexAccess::Full;
        p.approvals = CodexApprovals::Never;
        db.save_profile(p).unwrap();
        let next = db
            .submit(&task.profile.id, "Continue", Some(&task.chat), 0, None)
            .unwrap();
        assert_eq!(next.chat, task.chat);
        assert!(next.native_id.is_none());
        assert_eq!(next.profile.agent, AgentKind::Claude);
        assert_eq!(next.profile.access, CodexAccess::Full);
        assert_eq!(
            db.task(&task.id).unwrap().profile.access,
            CodexAccess::Workspace
        );
    }
    #[test]
    fn recovery_never_replays_ambiguous_work() {
        let f = Fixture::new();
        let mut db = Store::open(&f.0).unwrap();
        let p = db.save_profile(f.profile()).unwrap();
        let mut running = db.submit(&p.id, "Edit once", None, 0, None).unwrap();
        running.state = "running".into();
        db.save_task(&running).unwrap();
        let queued = db
            .submit(&p.id, "Later", None, now() + 3600, Some(86400))
            .unwrap();
        db.recover().unwrap();
        assert_eq!(db.task(&running.id).unwrap().state, "interrupted");
        assert_eq!(db.task(&queued.id).unwrap().state, "queued");
        assert!(db.submit(&p.id, "too often", None, 0, Some(1)).is_err());
    }
    #[test]
    fn native_questions_require_known_ids_and_complete_answers() {
        let request = NativeRequest {
            id: "q".into(),
            method: "question".into(),
            title: "Target".into(),
            detail: String::new(),
            kind: runtime::NativeRequestKind::Questions {
                questions: vec![runtime::NativeQuestion {
                    id: "target".into(),
                    header: "Target".into(),
                    question: "Which?".into(),
                    options: vec![],
                    secret: false,
                    multiple: false,
                    required: true,
                }],
            },
        };
        assert!(validate_answer(&request, &NativeAnswer::Answers(Default::default())).is_err());
        assert!(
            validate_answer(
                &request,
                &NativeAnswer::Answers(
                    [(String::from("target"), vec![String::from("staging")])].into()
                )
            )
            .is_ok()
        );
        assert!(validate_answer(&request, &NativeAnswer::Choice("allow".into())).is_err());
    }
    #[test]
    fn submit_captures_changed_settings_and_rolls_back_rejected_task() {
        let f = Fixture::new();
        let mut db = Store::open(&f.0).unwrap();
        let mut p = db.save_profile(f.profile()).unwrap();
        let worker = Arc::new(Worker {
            store: Mutex::new(db),
            root: f.0.clone(),
            live: Mutex::new(HashMap::new()),
            change: Mutex::new(1),
            wake: Condvar::new(),
        });
        p.agent = AgentKind::Claude;
        p.model = Some("selected-claude".into());
        let result = worker
            .request(
                json!({"action":"submit","id":p.id,"profile":p,"prompt":"Do the isolated task"}),
            )
            .unwrap();
        assert_eq!(result["profile"]["agent"], "Claude");
        p.revision = result["profile"]["revision"].as_u64().unwrap();
        let revision = p.revision;
        p.name = "Unsaved change".into();
        assert!(
            worker
                .request(
                    json!({"action":"submit","id":p.id,"profile":p,"prompt":"Repeat","repeat":1})
                )
                .is_err()
        );
        let stored = worker.store.lock().unwrap().profile(&p.id).unwrap();
        assert_eq!(stored.revision, revision);
        assert_ne!(stored.name, "Unsaved change");
    }
    #[test]
    fn one_event_wakes_worker_once() {
        let (a, b) = async_channel::bounded(1);
        let (_c, d) = async_channel::bounded(1);
        a.try_send(()).unwrap();
        wait_events(&b, &d);
        assert!(b.is_empty());
    }
}
