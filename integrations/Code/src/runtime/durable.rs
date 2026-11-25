//! UI-independent provider owner. Socket readers never block provider ingestion.
//! A private journal and final response survive UI detachment and owner completion.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, VecDeque},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
};
const PREVIEW: usize = 64 * 1024;
#[derive(Serialize, Deserialize)]
struct Input {
    request: RunRequest,
    checkpoint: Option<(PathBuf, String)>,
}
#[derive(Clone, Default, Serialize, Deserialize)]
struct Snapshot {
    sequence: u64,
    events: Vec<RuntimeEvent>,
    tail: String,
    done: bool,
    error: Option<String>,
    #[serde(default)]
    broker: broker::ExternalSnapshot,
}
struct State {
    snapshot: Snapshot,
    recent: VecDeque<(u64, RuntimeEvent)>,
    workspace: BTreeMap<String, RuntimeEvent>,
    broker_sequence: u64,
}
struct Shared {
    dir:PathBuf,
    state: Mutex<State>,
    changed: Condvar,
}
fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let temp = path.with_extension("next");
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temp)?;
    serde_json::to_writer(&mut file, value)?;
    file.flush()?;
    fs::rename(temp, path)?;
    Ok(())
}
fn socket(dir: &Path) -> PathBuf {
    use sha2::{Digest, Sha256};
    let hash = format!("{:x}", Sha256::digest(dir.as_os_str().as_encoded_bytes()));
    std::env::temp_dir().join(format!("kai-run-{}.sock", &hash[..24]))
}
fn source(path: &Path) -> Result<ResponseSource> {
    let mut buffer = ResponseBuffer::default();
    io::copy(&mut File::open(path)?, &mut buffer)?;
    buffer.into_source()
}
fn command(dir: &Path, value: &Value) -> Result<Value> {
    let mut stream = UnixStream::connect(socket(dir))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    serde_json::to_writer(&mut stream, value)?;
    stream.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(stream).read_line(&mut line)?;
    let result: Value = serde_json::from_str(&line)?;
    if result["ok"] == false {
        bail!(
            "{}",
            result["error"].as_str().unwrap_or("Session command failed")
        )
    }
    Ok(result)
}
pub(crate) fn launch(
    request: RunRequest,
    checkpoint: Option<(PathBuf, String)>,
) -> Result<RunHandle> {
    let root = std::env::var_os("ORBIT_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::home_dir()
                .unwrap_or_default()
                .join("Library/Application Support/Kai Unified")
        });
    let dir = root
        .join("session-owners")
        .join(uuid::Uuid::new_v4().to_string());
    fs::create_dir_all(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    write_json(
        &dir.join("input.json"),
        &Input {
            request,
            checkpoint,
        },
    )?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(dir.join("owner.log"))?;
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--kai-session-owner")
        .arg(&dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log)
        .process_group(0)
        .spawn()?;
    let start = Instant::now();
    while !socket(&dir).exists() && !dir.join("snapshot.json").exists() {
        if child.try_wait()?.is_some() {
            if dir.join("snapshot.json").is_file() { break; }
            bail!("The session owner could not start; task was not replayed")
        }
        anyhow::ensure!(
            start.elapsed() < Duration::from_secs(10),
            "The session owner is still starting; task was not replayed"
        );
        thread::sleep(Duration::from_millis(10));
    }
    thread::spawn(move || {
        let _ = child.wait();
    });
    attach(dir)
}
pub(crate) fn attach(dir: PathBuf) -> Result<RunHandle> {
    anyhow::ensure!(
        dir.is_absolute() && dir.join("input.json").is_file(),
        "Invalid saved session owner"
    );
    let live = UnixStream::connect(socket(&dir));
    let mut stream = match live {
        Ok(mut s) => {
            serde_json::to_writer(&mut s, &serde_json::json!({"op":"subscribe"}))?;
            s.write_all(b"\n")?;
            Some(s)
        }
        Err(_) => None,
    };
    if stream.is_none() {
        let saved: Snapshot = serde_json::from_reader(File::open(dir.join("snapshot.json")).context("The session owner exited without reporting its result")?)?;
        anyhow::ensure!(
            saved.done,
            "The session owner is unavailable; the task was held without replaying it"
        );
    }
    let (mut handle, context) = native_handle(0);
    *handle.process_group.lock().unwrap() = None;
    handle.deferred_native_stop = false;
    handle.durable = Some(dir.clone());
    let (control_tx, control_rx) = mpsc::sync_channel(32);
    handle.control = Some(control_tx);
    let client_cancel = Arc::new(AtomicBool::new(false));
    handle.durable_detach = Some(client_cancel.clone());
    let ctrl_dir = dir.clone();
    thread::Builder::new()
        .name("kai-session-controls".into())
        .spawn(move || {
            while let Ok(control) = control_rx.recv() {
                if let NativeControl::Snapshot(reply) = control {
                    let result=(||->Result<ResponseSource>{
                        if ctrl_dir.join("response.txt").is_file(){return source(&ctrl_dir.join("response.txt"));}
                        let value=command(&ctrl_dir,&serde_json::json!({"op":"snapshot"}))?;
                        let path=PathBuf::from(value["path"].as_str().context("Missing history snapshot")?);
                        anyhow::ensure!(path.parent()==Some(ctrl_dir.as_path()),"Invalid history snapshot location");source(&path)
                    })().map_err(|e|e.to_string());
                    let _ = reply.send_blocking(result);
                    continue;
                }
                let value = match control {
                    NativeControl::Respond { id, answer } => {
                        serde_json::json!({"op":"respond","id":id,"answer":answer})
                    }
                    NativeControl::Steer(text) => serde_json::json!({"op":"steer","text":text}),
                    NativeControl::Interrupt => serde_json::json!({"op":"interrupt"}),
                    NativeControl::Rpc { method, params } => {
                        serde_json::json!({"op":"rpc","method":method,"params":params})
                    }
                    _ => continue,
                };
                if let Err(error) = command(&ctrl_dir, &value) {
                    eprintln!("Session control: {error}");
                }
            }
        })?;
    thread::Builder::new().name("kai-session-events".into()).spawn(move||{
        let tx=context.tx;let final_source=context.finalized_source;
        let send_snapshot=|snap:Snapshot|->Result<()> {
            broker::mirror(dir.clone(),snap.broker);
            for event in snap.events{send(&tx,event,&client_cancel)?;}
            if !snap.tail.is_empty(){send(&tx,RuntimeEvent::ReplacePreview(snap.tail),&client_cancel)?;}
            if snap.done{
                if let Ok(s)=source(&dir.join("response.txt")){*final_source.lock().unwrap()=Some(s);}
                if let Some(error)=snap.error{send(&tx,RuntimeEvent::Failed(error),&client_cancel)?;}else{send(&tx,RuntimeEvent::Finished,&client_cancel)?;}
            }
            Ok(())
        };
        let result=(||->Result<()>{
            if let Some(s)=stream.take(){
                s.set_read_timeout(Some(Duration::from_millis(500)))?;let mut reader=BufReader::new(s);let mut line=String::new();
                while !client_cancel.load(Ordering::Acquire){
                    match reader.read_line(&mut line){Ok(0)=>break,Ok(_)=>{let v:Value=serde_json::from_str(&line)?;line.clear();
                        if let Some(snapshot)=v.get("snapshot"){let snap:Snapshot=serde_json::from_value(snapshot.clone())?;let done=snap.done;send_snapshot(snap)?;if done{return Ok(())}}
                        else if let Some(event)=v.get("event"){let event:RuntimeEvent=serde_json::from_value(event.clone())?;send(&tx,event,&client_cancel)?;}
                    },Err(e)if matches!(e.kind(),io::ErrorKind::WouldBlock|io::ErrorKind::TimedOut)=>continue,Err(e)=>return Err(e.into())}
                }
                if client_cancel.load(Ordering::Acquire){return Ok(())}
            }
            let snap:Snapshot=serde_json::from_reader(File::open(dir.join("snapshot.json")).context("The session owner exited without reporting its result")?)?;
            anyhow::ensure!(snap.done,"Session connection ended before a durable completion. Reopen to reconnect; no action was replayed.");send_snapshot(snap)
        })();
        if let Err(error)=result{let _=send(&tx,RuntimeEvent::Failed(error.to_string()),&client_cancel);}
        drop(context.settled);
    })?;
    Ok(handle)
}
pub(crate) fn cancel(dir: &Path) {
    let dir = dir.to_owned();
    thread::spawn(move || {
        let _ = request_cancel(&dir);
    });
}

pub(crate) fn request_cancel(dir: &Path) -> Result<()> {
    match command(dir, &serde_json::json!({"op":"cancel"})) {
        Ok(_) => Ok(()),
        Err(error) => {
            let completed = File::open(dir.join("snapshot.json")).ok()
                .and_then(|file| serde_json::from_reader::<_, Snapshot>(file).ok())
                .is_some_and(|snapshot| snapshot.done);
            if completed { Ok(()) } else { Err(error) }
        }
    }
}
pub(crate) fn serve(dir: PathBuf) -> Result<()> {
    if !cfg!(test) {
        unsafe {
            libc::setpriority(libc::PRIO_PROCESS, 0, 5);
        }
    }
    let input: Input = serde_json::from_reader(File::open(dir.join("input.json"))?)?;
    let listener = UnixListener::bind(socket(&dir))?;
    fs::set_permissions(socket(&dir), fs::Permissions::from_mode(0o600))?;
    let shared = Arc::new(Shared {
        dir:dir.clone(),
        state: Mutex::new(State {
            snapshot: Snapshot::default(),
            recent: VecDeque::new(),
            workspace: BTreeMap::new(),
            broker_sequence: 0,
        }),
        changed: Condvar::new(),
    });
    let owner_project = input.request.project.clone();
    let mut handle = match launch_mode_internal(input.request, false, true) {
        Ok(handle) => handle,
        Err(error) => {
            let failed = Snapshot { done: true, error: Some(format!("{error:#}")), ..Default::default() };
            write_json(&dir.join("snapshot.json"), &failed)
                .context("Could not save the session's startup failure")?;
            let _ = fs::remove_file(socket(&dir));
            return Err(error);
        }
    };
    let (_, dummy) = mpsc::sync_channel(1);
    let events = std::mem::replace(
        &mut handle.events,
        EventReceiver {
            inner: dummy,
            budget: Arc::new(QueueBudget::default()),
        },
    );
    let wake = handle.wake.clone();
    let settled = handle.settled.clone();
    let handle = Arc::new(Mutex::new(handle));
    let listen_handle = handle.clone();
    let listen_shared = shared.clone();
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let handle = listen_handle.clone();
            let shared = listen_shared.clone();
            thread::spawn(move || {
                let _ = serve_client(stream, handle, shared);
            });
        }
    });
    let mut journal = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(dir.join("events.jsonl"))?;
    let mut text = ResponseBuffer::default();
    let mut meta: BTreeMap<String, RuntimeEvent> = BTreeMap::new();
    let mut error = None;
    let mut last_snapshot = Instant::now();
    broker::watch_transcripts(true);
    let broker_wake = broker::wake();
    loop {
        let broker_changed = if wake.is_closed() {
            futures_lite::future::block_on(futures_lite::future::or(
                async {
                    let _ = broker_wake.recv().await;
                    true
                },
                async {
                    let _ = settled.recv().await;
                    false
                },
            ))
        } else {
            futures_lite::future::block_on(futures_lite::future::or(
                async {
                    let _ = broker_wake.recv().await;
                    true
                },
                async {
                    let _ = futures_lite::future::or(wake.recv(), settled.recv()).await;
                    false
                },
            ))
        };
        if broker_changed {
            let snapshot = broker::ExternalSnapshot {
                services: broker::local_services()
                    .into_iter()
                    .filter(|s| s.project == owner_project)
                    .collect(),
                children: broker::local_children(&owner_project),
            };
            let mut state = shared.state.lock().unwrap();
            state.snapshot.broker = snapshot;
            state.snapshot.sequence += 1;
            state.broker_sequence = state.snapshot.sequence;
            drop(state);
            shared.changed.notify_all();
        }
        let mut batch = Vec::new();
        loop {
            match events.try_recv() {
                Ok(event) => {
                    serde_json::to_writer(&mut journal, &event)?;
                    journal.write_all(b"\n")?;
                    match &event {
                        RuntimeEvent::WorkspaceTool { id, .. } => {
                            shared
                                .state
                                .lock()
                                .unwrap()
                                .workspace
                                .insert(id.clone(), event.clone());
                        }
                        RuntimeEvent::Text(s) => text.push_str(s),
                        RuntimeEvent::ReplaceText(s) | RuntimeEvent::ReplacePreview(s) => {
                            text = ResponseBuffer::default();
                            text.push_str(s);
                        }
                        RuntimeEvent::SessionId(_) => {
                            meta.insert("session".into(), event.clone());
                        }
                        RuntimeEvent::Usage(_) => {
                            meta.insert("usage".into(), event.clone());
                        }
                        RuntimeEvent::Status(_) => {
                            meta.insert("status".into(), event.clone());
                        }
                        RuntimeEvent::Models(_) => {
                            meta.insert("models".into(), event.clone());
                        }
                        RuntimeEvent::Commands(_) => {
                            meta.insert("commands".into(), event.clone());
                        }
                        RuntimeEvent::Request(r) => {
                            meta.insert(format!("request:{}", r.id), event.clone());
                        }
                        RuntimeEvent::RequestResolved(id) => {
                            meta.remove(&format!("request:{id}"));
                        }
                        RuntimeEvent::Failed(e) => error = Some(e.clone()),
                        _ => {}
                    }
                    if !matches!(event, RuntimeEvent::Finished | RuntimeEvent::Failed(_)) {
                        batch.push(event);
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if !batch.is_empty() {
            let tail = text.snapshot()?.tail(PREVIEW)?;
            let mut state = shared.state.lock().unwrap();
            for event in batch {
                state.snapshot.sequence += 1;
                let seq = state.snapshot.sequence;
                state.recent.push_back((seq, event));
            }
            while state.recent.len() > 256 {
                state.recent.pop_front();
            }
            state.snapshot.events = meta.values().cloned().collect();
            state.snapshot.tail = tail;
            drop(state);
            shared.changed.notify_all();
        }
        journal.flush()?;
        if last_snapshot.elapsed() > Duration::from_millis(250) {
            write_json(
                &dir.join("snapshot.json"),
                &shared.state.lock().unwrap().snapshot,
            )?;
            last_snapshot = Instant::now();
        }
        if settled.is_closed() {
            break;
        }
        if wake.is_closed() {
            let _ = settled.recv_blocking();
        }
    }
    let canonical = handle
        .lock()
        .unwrap()
        .take_response_source()
        .unwrap_or(text.into_source()?);
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(dir.join("response.txt"))?;
    canonical.copy_range_to(0, canonical.len(), &mut file)?;
    file.sync_all()?;
    journal.sync_all()?;
    if let Some((path, id)) = input.checkpoint {
        let _ = crate::checkpoints::Repository::at(path).seal(&id, &AtomicBool::new(false));
    }
    shared.state.lock().unwrap().snapshot.broker=broker::ExternalSnapshot{
        services:broker::local_services().into_iter().filter(|s|s.project==owner_project).collect(),
        children:broker::local_children(&owner_project),
    };
    // Release idle native writer connections and their inference leases before
    // announcing completion. A new turn can then own the same provider thread.
    let cleanup=broker::shutdown();let deadline=Instant::now()+Duration::from_secs(5);
    while !cleanup.is_closed()&&Instant::now()<deadline{thread::sleep(Duration::from_millis(10));}
    if !cleanup.is_closed() && error.is_none() {
        error = Some("Provider cleanup did not finish. The saved output is retained for review.".into());
    }
    {
        let mut state = shared.state.lock().unwrap();
        state.snapshot.done = true;
        for service in &mut state.snapshot.broker.services {
            service.active = false;
            service.connected = false;
        }
        for child in &mut state.snapshot.broker.children {
            child.connected = false;
        }
        state.snapshot.error = error;
        state.snapshot.tail = canonical.tail(PREVIEW)?;
        write_json(&dir.join("snapshot.json"), &state.snapshot)?;
    }
    shared.changed.notify_all();
    // Give current subscribers time to read final state; closed owners replay from disk.
    thread::sleep(Duration::from_millis(200));
    let _ = fs::remove_file(socket(&dir));
    Ok(())
}
fn serve_client(
    mut stream: UnixStream,
    handle: Arc<Mutex<RunHandle>>,
    shared: Arc<Shared>,
) -> Result<()> {
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut line = String::new();
    BufReader::new(stream.try_clone()?)
        .take(2 * 1024 * 1024)
        .read_line(&mut line)?;
    let v: Value = serde_json::from_str(&line)?;
    if v["op"] == "subscribe" {
        let mut seq = 0;
        loop {
            let mut state = shared.state.lock().unwrap();
            while seq > 0 && seq == state.snapshot.sequence && !state.snapshot.done {
                state = shared.changed.wait(state).unwrap();
            }
            let snapshot = seq == 0
                || state.snapshot.done
                || state.broker_sequence > seq
                || state.recent.front().is_some_and(|(n, _)| *n > seq + 1);
            let mut current = state.snapshot.clone();
            current.events.extend(state.workspace.values().cloned());
            let packets: Vec<Value> = if snapshot {
                vec![serde_json::json!({"snapshot":current})]
            } else {
                state
                    .recent
                    .iter()
                    .filter(|(n, _)| *n > seq)
                    .map(|(_, e)| serde_json::json!({"event":e}))
                    .collect()
            };
            seq = state.snapshot.sequence;
            let done = state.snapshot.done;
            drop(state);
            for packet in packets {
                serde_json::to_writer(&mut stream, &packet)?;
                stream.write_all(b"\n")?;
            }
            stream.flush()?;
            if done {
                return Ok(());
            }
            // Sequence zero has no events yet: wait rather than spin or emit empty snapshots.
            if seq == 0 {
                let state = shared.state.lock().unwrap();
                if state.snapshot.sequence == 0 && !state.snapshot.done {
                    drop(shared.changed.wait(state).unwrap());
                }
            }
        }
    }
    if v["op"]=="snapshot"{
        let rx=handle.lock().unwrap().live_snapshot()?;
        let source=rx.recv_blocking()?.map_err(anyhow::Error::msg)?;
        let path=shared.dir.join("snapshot-response.txt");let temp=path.with_extension("next");
        let mut file=OpenOptions::new().create(true).truncate(true).write(true).mode(0o600).open(&temp)?;
        source.copy_range_to(0,source.len(),&mut file)?;file.flush()?;fs::rename(temp,&path)?;
        serde_json::to_writer(&mut stream,&serde_json::json!({"ok":true,"path":path}))?;stream.write_all(b"\n")?;return Ok(())
    }
    if matches!(v["op"].as_str(), Some("broker_action" | "broker_page")) {
        let project: PathBuf = serde_json::from_value(v["project"].clone())?;
        let provider: AgentKind = serde_json::from_value(v["provider"].clone())?;
        let child = v["child"].as_str().context("Missing child")?;
        let result: Result<Value> = if v["op"] == "broker_action" {
            broker::local_control_child(
                &project,
                provider,
                child,
                serde_json::from_value(v["action"].clone())?,
            )
            .map(|_| serde_json::json!({"ok":true}))
        } else {
            broker::local_child_page(
                &project,
                provider,
                child,
                v["offset"].as_u64().unwrap_or(0) as usize,
            )
            .map(|page| serde_json::json!({"ok":true,"page":page}))
        };
        let value =
            result.unwrap_or_else(|e| serde_json::json!({"ok":false,"error":e.to_string()}));
        serde_json::to_writer(&mut stream, &value)?;
        stream.write_all(b"\n")?;
        return Ok(());
    }
    let result = (|| -> Result<()> {
        let h = handle.lock().unwrap();
        match v["op"].as_str().unwrap_or("") {
            "cancel" => {
                h.cancel();
                Ok(())
            }
            "interrupt" => h.interrupt(),
            "respond" => h.respond(
                v["id"].as_str().context("Missing request")?,
                serde_json::from_value(v["answer"].clone())?,
            ),
            "steer" => h.steer(v["text"].as_str().context("Missing text")?),
            "rpc" => h.native_command(
                v["method"].as_str().context("Missing method")?,
                v["params"].clone(),
            ),
            _ => bail!("Unknown owner command"),
        }
    })();
    if result.is_ok() && v["op"] == "rpc" && v["method"] == "code/workspace/result" {
        if let Some(id) = v["params"]["id"].as_str() {
            shared.state.lock().unwrap().workspace.remove(id);
        }
    }
    serde_json::to_writer(
        &mut stream,
        &match result {
            Ok(()) => serde_json::json!({"ok":true}),
            Err(e) => serde_json::json!({"ok":false,"error":e.to_string()}),
        },
    )?;
    stream.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct OwnerWorker {
        thread: Option<thread::JoinHandle<Result<()>>>,
        dir: PathBuf,
    }
    impl OwnerWorker {
        fn join(mut self) -> thread::Result<Result<()>> {
            self.thread.take().unwrap().join()
        }
    }
    impl Drop for OwnerWorker {
        fn drop(&mut self) {
            if let Some(worker) = self.thread.take() {
                let _ = command(&self.dir, &serde_json::json!({"op":"cancel"}));
                // The child watchdog bounds this join even after a test panic.
                let _ = worker.join();
            }
        }
    }
    #[test]
    fn provider_startup_failure_remains_available_after_the_owner_exits() {
        let dir = std::env::temp_dir().join(format!("kai-owner-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let request = RunRequest {
            identity: None,
            ninja: None,
            orchestrator: false,
            agent: AgentKind::Codex,
            executable: dir.join("unused"),
            project: dir.join("missing-project"),
            prompt: "Fixture only".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            service_tier: None,
            reasoning_effort: None,
            images: vec![],
            allow_edits: false,
            trust_workspace: false,
            codex_access: Default::default(),
            codex_approvals: Default::default(),
            provider_unrestricted: false,
        };
        write_json(&dir.join("input.json"), &Input { request, checkpoint: None }).unwrap();
        assert_eq!(serve(dir.clone()).unwrap_err().to_string(), "Project folder is unavailable");
        assert!(!socket(&dir).exists());
        let handle = attach(dir.clone()).unwrap();
        let until = Instant::now() + Duration::from_secs(4);
        loop {
            if let Ok(RuntimeEvent::Failed(error)) = handle.events.try_recv() {
                assert_eq!(error, "Project folder is unavailable");
                break;
            }
            assert!(Instant::now() < until, "Client did not receive the saved startup error");
            thread::sleep(Duration::from_millis(10));
        }
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    #[ignore = "Internal subprocess fixture; launched only by durable-owner tests"]
    fn isolated_owner_fixture_process() {
        let dir = PathBuf::from(std::env::var_os("KAI_DURABLE_OWNER_FIXTURE").expect("Owner fixture directory"));
        let canonical = dir.canonicalize().unwrap();
        assert_eq!(canonical.parent(), Some(std::env::temp_dir().canonicalize().unwrap().as_path()));
        assert!(dir.file_name().unwrap().to_str().unwrap().starts_with("kai-owner-test-"));
        serve(dir).unwrap();
    }
    fn wait(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(12);
        while !predicate() {
            assert!(Instant::now() < deadline, "Session fixture timed out");
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn start() -> (PathBuf, OwnerWorker) {
        start_scenario(false)
    }
    fn start_scenario(child: bool) -> (PathBuf, OwnerWorker) {
        let dir = std::env::temp_dir().join(format!("kai-owner-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("provider.py");
        fs::write(&executable,r#"#!/usr/bin/python3
import json,sys,time,pathlib
root=pathlib.Path(__file__).parent
def send(v):print(json.dumps(v),flush=True)
def recv():
 v=json.loads(sys.stdin.readline())
 if v.get('method')=='config/read':
  send({'id':v['id'],'result':{'config':{'model_provider':'openai'}}});return recv()
 return v
v=recv();send({'id':v['id'],'result':{}})
assert recv()['method']=='initialized'
v=recv();assert v['method']=='thread/start'
t='12345678-1234-4234-8234-123456789012'
send({'id':v['id'],'result':{'thread':{'id':t}}})
v=recv();assert v['method']=='turn/start'
with (root/'starts').open('a') as f:f.write('one\n')
send({'id':v['id'],'result':{'turn':{'id':'turn1'}}})
send({'method':'item/agentMessage/delta','params':{'threadId':t,'turnId':'turn1','itemId':'a','delta':'before detach\n'}})
while not (root/'continue').exists():time.sleep(.02)
for i in range(400):send({'method':'item/agentMessage/delta','params':{'threadId':t,'turnId':'turn1','itemId':'a','delta':'z'*2048}})
send({'method':'turn/completed','params':{'threadId':t,'turn':{'id':'turn1','status':'completed'}}})
while sys.stdin.readline():pass
"#).unwrap();
        if child {
            let body = fs::read_to_string(&executable).unwrap();
            let extra = r#"child='22345678-1234-4234-8234-123456789012'
send({'method':'thread/started','params':{'thread':{'id':child,'parentThreadId':t,'status':{'type':'active','activeFlags':[]}}}})
v=recv();assert v['method']=='thread/resume'
send({'id':v['id'],'result':{'thread':{'id':child,'status':{'type':'active'},'turns':[]}}})
v=recv();assert v['method']=='thread/turns/list'
send({'id':v['id'],'result':{'data':[{'id':'child-turn','status':'inProgress'}]}})
send({'method':'turn/started','params':{'threadId':child,'turn':{'id':'child-turn'}}})
send({'id':'child-approval','method':'item/commandExecution/requestApproval','params':{'threadId':child,'turnId':'child-turn','itemId':'cmd','command':'fixture only','cwd':str(root)}})
(root/'child-ready').write_text('ready')
v=recv();assert v['id']=='child-approval' and v['result']['decision']=='accept',v
send({'method':'item/agentMessage/delta','params':{'threadId':child,'turnId':'child-turn','itemId':'answer','delta':'child completed'}})
send({'method':'turn/completed','params':{'threadId':child,'turn':{'id':'child-turn','status':'completed'}}})
"#;
            fs::write(
                &executable,
                body.replace(
                    "while not (root/'continue').exists():time.sleep(.02)",
                    &format!("{extra}\nwhile not (root/'continue').exists():time.sleep(.02)"),
                ),
            )
            .unwrap();
        }
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let request = RunRequest {
            identity: None,
            ninja: None,
            orchestrator: false,
            agent: AgentKind::Codex,
            executable,
            project: dir.clone(),
            prompt: "Fixture only".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            service_tier: None,
            reasoning_effort: None,
            images: vec![],
            allow_edits: false,
            trust_workspace: false,
            codex_access: Default::default(),
            codex_approvals: Default::default(),
            provider_unrestricted: false,
        };
        write_json(
            &dir.join("input.json"),
            &Input {
                request,
                checkpoint: None,
            },
        )
        .unwrap();
        // Production owners have distinct process-local broker registries.
        // Thread-only fixtures shared the registry, so one owner's orderly
        // global shutdown could cancel another test's still-running provider.
        let worker = dir.clone();
        let thread = thread::spawn(move || -> Result<()> {
            struct OwnedChild(std::process::Child, bool);
            impl Drop for OwnedChild {
                fn drop(&mut self) {
                    if !self.1 {
                        let _ = self.0.kill();
                        let _ = self.0.wait();
                    }
                }
            }
            let log = File::create(worker.join("owner-process.log"))?;
            let child = std::process::Command::new(std::env::current_exe()?)
                .args(["--exact", "runtime::durable::tests::isolated_owner_fixture_process", "--ignored", "--nocapture", "--test-threads=1"])
                .env("KAI_DURABLE_OWNER_FIXTURE", &worker)
                .stdin(std::process::Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log)
                .spawn()?;
            let mut child = OwnedChild(child, false);
            let deadline = Instant::now() + Duration::from_secs(30);
            loop {
                if let Some(status) = child.0.try_wait()? {
                    child.1 = true;
                    anyhow::ensure!(status.success(), "Owner fixture failed; see {}", worker.join("owner-process.log").display());
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    // Give the owner its normal cancellation/reaping path
                    // before the guard's final kill/reap fallback.
                    let _ = command(&worker, &serde_json::json!({"op":"cancel"}));
                    let cleanup_deadline = Instant::now() + Duration::from_secs(5);
                    while Instant::now() < cleanup_deadline {
                        if child.0.try_wait()?.is_some() { child.1 = true; break; }
                        thread::sleep(Duration::from_millis(10));
                    }
                    bail!("Owner fixture exceeded thirty seconds; see {}", worker.join("owner-process.log").display());
                }
                thread::sleep(Duration::from_millis(10));
            }
        });
        let worker = OwnerWorker { thread: Some(thread), dir: dir.clone() };
        wait(|| socket(&dir).exists());
        (dir, worker)
    }
    #[test]
    fn detached_owner_finishes_once_and_reopens_full_response_after_exit() {
        let (dir, worker) = start();
        let client = attach(dir.clone()).unwrap();
        wait(|| dir.join("starts").exists());
        wait(||client.events.try_recv().is_ok_and(|e|matches!(e,RuntimeEvent::Text(_)|RuntimeEvent::ReplacePreview(_))));
        let snapshot=client.live_snapshot().unwrap();let live=snapshot.recv_blocking().unwrap().unwrap();assert!(live.tail(live.len()).unwrap().contains("before detach"));
        drop(client);
        fs::write(dir.join("continue"), b"go").unwrap();
        worker.join().unwrap().unwrap();
        let restored = attach(dir.clone()).unwrap();
        wait(|| restored.settled.is_closed());
        let full = restored.take_response_source().unwrap();
        let all = full.tail(full.len()).unwrap();
        assert!(all.contains(&"z".repeat(400 * 2048)));
        assert_eq!(all.matches("before detach").count(), 1);
        assert_eq!(fs::read_to_string(dir.join("starts")).unwrap(), "one\n");
        drop(restored);
        assert!(broker::local_services().iter().filter(|s|s.project==dir).all(|s|!s.connected));
    }
    #[test]
    fn stalled_ui_does_not_backpressure_provider_owner() {
        let (dir, worker) = start();
        let unread = attach(dir.clone()).unwrap();
        wait(|| dir.join("starts").exists());
        fs::write(dir.join("continue"), b"go").unwrap();
        // Deliberately never consume unread.events. The owner must still persist completion.
        wait(|| {
            fs::read(dir.join("snapshot.json"))
                .ok()
                .and_then(|b| serde_json::from_slice::<Snapshot>(&b).ok())
                .is_some_and(|s| s.done)
        });
        worker.join().unwrap().unwrap();
        drop(unread);
        assert!(fs::metadata(dir.join("response.txt")).unwrap().len() > 800_000);
    }
    #[test]
    fn child_approval_survives_detach_and_round_trips_to_owning_provider() {
        let (dir, worker) = start_scenario(true);
        let client = attach(dir.clone()).unwrap();
        wait(|| dir.join("child-ready").exists());
        drop(client);
        let restored = attach(dir.clone()).unwrap();
        wait(|| external_child_request(&dir).is_some());
        let request = external_child_request(&dir).unwrap();
        broker_call(&dir,&serde_json::json!({"op":"broker_action","project":dir,"provider":AgentKind::Codex,"child":"22345678-1234-4234-8234-123456789012","action":broker::ChildAction::Respond{id:request,answer:NativeAnswer::Choice("accept".into())}})).unwrap();
        fs::write(dir.join("continue"), b"go").unwrap();
        worker.join().unwrap().unwrap();
        drop(restored);
        assert!(dir.join("response.txt").is_file());
    }
    fn external_child_request(dir: &Path) -> Option<String> {
        broker::children(dir)
            .into_iter()
            .flat_map(|c| c.requests.into_iter())
            .next()
            .map(|r| r.id)
    }
}

pub(crate) fn broker_call(dir: &Path, value: &Value) -> Result<Value> {
    command(dir, value)
}
