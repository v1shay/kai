//! Antigravity's documented PreToolUse hook supplies the approval channel its
//! headless stream lacks. Auto-approval is returned only for a reviewed call;
//! explicit provider deny/ask rules retain their documented precedence.
use super::*;
use anyhow::ensure;
use serde_json::json;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
};
const LIMIT: usize = 64 * 1024;
const KEY: &str = "kai-unified-approvals-v1";
pub(super) struct Gate {
    listener: UnixListener,
    path: PathBuf,
    token: String,
    project: PathBuf,
    resume: Option<String>,
    edits: bool,
    pending: HashMap<String, (UnixStream, Instant)>,
}
impl Drop for Gate {
    fn drop(&mut self) {
        for (_, (mut stream, _)) in self.pending.drain() {
            let _ = stream.write_all(b"{\"decision\":\"deny\"}\n");
        }
        let _ = fs::remove_file(&self.path);
    }
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn helper() -> Result<PathBuf> {
    let packaged = std::env::current_exe()?
        .parent()
        .context("Missing executable folder")?
        .join("../Resources/kai_agy_hook.py");
    Ok(if packaged.is_file() {
        packaged
    } else {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native_helpers/kai_agy_hook.py")
    })
}
/// Merge one named, inactive-outside-Kai hook; never replace user hooks or rules.
fn register(path: &std::path::Path, script: &std::path::Path) -> Result<()> {
    let parent = path.parent().context("Missing hook directory")?;
    fs::create_dir_all(parent)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent.join(".kai-hook.lock"))?;
    ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0,
        "Could not lock hook registration"
    );
    let mut hooks: Value = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => serde_json::from_reader(file.take(1024 * 1024))
            .context("Existing hook file is not valid JSON")?,
        Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.into()),
    };
    ensure!(hooks.is_object(), "Existing hook file must be an object");
    let command = format!(
        "if [ -n \"$KAI_AGY_PERMISSION_SOCKET\" ] && [ -n \"$KAI_AGY_PERMISSION_TOKEN\" ]; then exec /usr/bin/python3 {}; else printf '%s\\n' '{{\"decision\":\"allow\"}}'; fi",
        quote(&script.to_string_lossy())
    );
    let entry = json!({"PreToolUse":[{"matcher":"*","hooks":[{"type":"command","command":command,"timeout":600}]}]});
    if hooks.get(KEY) == Some(&entry) {
        return Ok(());
    }
    if let Some(old) = hooks.get(KEY) {
        ensure!(
            old["PreToolUse"].is_array(),
            "An unrelated hook already owns Kai's registration name"
        );
    }
    hooks[KEY] = entry;
    let temporary = parent.join(format!(".kai-hook-{}.json", uuid::Uuid::new_v4()));
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    serde_json::to_writer_pretty(&mut output, &hooks)?;
    output.sync_all()?;
    fs::rename(&temporary, path)?;
    Ok(())
}
impl Gate {
    pub(super) fn prepare(request: &RunRequest, command: &mut Command) -> Result<Self> {
        let script = helper()?;
        ensure!(
            script.is_file(),
            "Kai's Antigravity approval helper is missing"
        );
        #[cfg(test)]
        let config = std::env::var_os("CODE_SMOKE_AGY_HOOKS")
            .map(PathBuf::from)
            .unwrap_or_else(|| request.project.join(".agents/hooks.json"));
        #[cfg(not(test))]
        let config = dirs::home_dir()
            .context("No user home")?
            .join(".gemini/config/hooks.json");
        register(&config, &script)?;
        let path = std::env::temp_dir().join(format!("kai-agy-{}.sock", uuid::Uuid::new_v4()));
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let token = format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4());
        command
            .env("KAI_AGY_PERMISSION_SOCKET", &path)
            .env("KAI_AGY_PERMISSION_TOKEN", &token);
        Ok(Self {
            listener,
            path,
            token,
            project: request.project.canonicalize()?,
            resume: request.resume_id.clone(),
            edits: request.allow_edits,
            pending: HashMap::new(),
        })
    }
    fn decision(stream: &mut UnixStream, allow: bool) -> io::Result<()> {
        stream.write_all(if allow {
            b"{\"decision\":\"auto_approve\"}\n"
        } else {
            b"{\"decision\":\"deny\"}\n"
        })
    }
    pub(super) fn poll(
        &mut self,
        controls: &Receiver<NativeControl>,
        parser: &mut StreamParser,
        tx: &NotifyingSender,
        cancel: &AtomicBool,
    ) -> Result<()> {
        while let Ok(control) = controls.try_recv() {
            match control {
                NativeControl::Respond { id, answer } => {
                    if let Some((mut stream, _)) = self.pending.remove(&id) {
                        let allow = matches!(answer,NativeAnswer::Choice(ref choice)if choice=="allow-once");
                        Self::decision(&mut stream, allow)?;
                        send(tx, RuntimeEvent::RequestResolved(id), cancel)?;
                    } else {
                        send(
                            tx,
                            RuntimeEvent::RequestRejected {
                                id,
                                error: "This approval request has ended".into(),
                            },
                            cancel,
                        )?;
                    }
                }
                NativeControl::Interrupt => {
                    cancel.store(true, Ordering::Release);
                    return Ok(());
                }
                _ => {}
            }
        }
        for id in self
            .pending
            .iter()
            .filter(|(_, (_, at))| at.elapsed() > Duration::from_secs(590))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>()
        {
            if let Some((mut stream, _)) = self.pending.remove(&id) {
                let _ = Self::decision(&mut stream, false);
                send(tx, RuntimeEvent::RequestResolved(id), cancel)?;
            }
        }
        for _ in 0..8 {
            let (mut stream, _) = match self.listener.accept() {
                Ok(value) => value,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e.into()),
            };
            stream.set_read_timeout(Some(Duration::from_millis(30)))?;
            stream.set_write_timeout(Some(Duration::from_millis(100)))?;
            let mut uid = 0;
            let mut gid = 0;
            if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0
                || uid != unsafe { libc::geteuid() }
            {
                let _ = Self::decision(&mut stream, false);
                continue;
            }
            let mut line = Vec::new();
            let read = BufReader::new(&mut stream)
                .take((LIMIT + 1) as u64)
                .read_until(b'\n', &mut line);
            let packet = read
                .ok()
                .filter(|_| line.len() <= LIMIT)
                .and_then(|_| serde_json::from_slice::<Value>(&line).ok());
            let Some(packet) = packet else {
                let _ = Self::decision(&mut stream, false);
                continue;
            };
            let payload = &packet["payload"];
            if parser.agy_native_id.as_deref()!=payload["conversationId"].as_str() || parser.agy_native_id.is_none() || packet["token"] != self.token
                || self
                    .resume
                    .as_deref()
                    .is_some_and(|id| payload["conversationId"].as_str() != Some(id))
                || !payload["workspacePaths"].as_array().is_some_and(|roots| {
                    roots.iter().filter_map(Value::as_str).any(|p| {
                        std::path::Path::new(p).canonicalize().ok().as_ref() == Some(&self.project)
                    })
                })
            {
                let _ = Self::decision(&mut stream, false);
                continue;
            }
            if let Some(model)=payload["modelName"].as_str().filter(|m|super::valid_model_id(m)){send(tx,RuntimeEvent::Usage(Box::new(crate::usage::Usage{model:Some(model.into()),..Default::default()})),cancel)?;}
            let call = &payload["toolCall"];
            let name = call["name"].as_str().unwrap_or("");
            let args = &call["args"];
            let local = |key: &str| {
                args[key].as_str().is_some_and(|p| {
                    std::path::Path::new(p)
                        .canonicalize()
                        .is_ok_and(|p| p.starts_with(&self.project))
                })
            };
            let read_only = matches!(
                name,
                "view_file" | "list_dir" | "find_by_name" | "grep_search"
            );
            let in_workspace = match name {
                "view_file" => local("AbsolutePath"),
                "list_dir" => local("DirectoryPath"),
                "find_by_name" => local("SearchDirectory"),
                "grep_search" => local("SearchPath"),
                _ => false,
            };
            if read_only && in_workspace {
                stream.write_all(b"{\"decision\":\"allow\"}\n")?;
                continue;
            }
            if !self.edits && !read_only {
                let _ = Self::decision(&mut stream, false);
                continue;
            }
            if self.pending.len() >= 16 {
                let _ = Self::decision(&mut stream, false);
                continue;
            }
            let id = uuid::Uuid::new_v4().to_string();
            let detail = serde_json::to_string_pretty(payload)?;
            let request = NativeRequest {
                id: id.clone(),
                method: "antigravity/PreToolUse".into(),
                title: format!("Antigravity · {name}"),
                detail,
                kind: NativeRequestKind::Approval {
                    choices: vec![
                        NativeOption {
                            id: "allow-once".into(),
                            label: "Allow once".into(),
                        },
                        NativeOption {
                            id: "reject-once".into(),
                            label: "Decline".into(),
                        },
                    ],
                },
            };
            self.pending.insert(id.clone(), (stream, Instant::now()));
            for event in parser.interaction(
                crate::chat_interactions::ChatInteraction::NativeRequest(request.clone()),
                "agy.request",
            ) {
                send(tx, event, cancel)?;
            }
            send(tx, RuntimeEvent::Request(Box::new(request)), cancel)?;
        }
        Ok(())
    }
}

#[cfg(test)]mod tests{
 use super::*;
 #[test]fn registration_is_idempotent_and_preserves_existing_hooks(){
  let root=std::env::temp_dir().join(format!("kai-agy-register-{}",uuid::Uuid::new_v4()));fs::create_dir_all(&root).unwrap();let path=root.join("hooks.json");fs::write(&path,r#"{"existing":{"PostToolUse":[{"matcher":"run_command","hooks":[{"command":"true"}]}]}}"#).unwrap();
  register(&path,std::path::Path::new("/private/tmp/Kai's App/hook.py")).unwrap();let once=fs::read(&path).unwrap();register(&path,std::path::Path::new("/private/tmp/Kai's App/hook.py")).unwrap();assert_eq!(once,fs::read(&path).unwrap());let value:Value=serde_json::from_slice(&once).unwrap();assert_eq!(value["existing"]["PostToolUse"][0]["hooks"][0]["command"],"true");assert!(value[KEY]["PreToolUse"][0]["hooks"][0]["command"].as_str().unwrap().contains("KAI_AGY_PERMISSION_TOKEN"));
  fs::write(&path,"broken user JSON").unwrap();assert!(register(&path,std::path::Path::new("/hook.py")).is_err());assert_eq!(fs::read_to_string(&path).unwrap(),"broken user JSON");let _=fs::remove_dir_all(root);
 }
}
