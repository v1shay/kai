//! Local, bounded Kai transport. The Code UI remains the only owner of runs.
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs,
    io::{self, BufRead, BufReader, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
const REQUEST_LIMIT: u64 = 65_536;
const RESPONSE_LIMIT: usize = 2 * 1024 * 1024;
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub protocol: u32,
    pub id: String,
    pub method: String,
    #[serde(default)]
    pub epoch: Option<String>,
    #[serde(default)]
    pub params: Value,
}
pub struct Pending {
    pub request: Request,
    pub deadline: Instant,
    reply: SyncSender<Value>,
}
impl Pending {
    #[cfg(test)]
    pub fn fixture(request: Request, expired: bool) -> (Self, Receiver<Value>) {
        let (reply, response) = mpsc::sync_channel(1);
        let deadline = if expired {
            Instant::now() - Duration::from_secs(1)
        } else {
            Instant::now() + Duration::from_secs(1)
        };
        (
            Self {
                request,
                deadline,
                reply,
            },
            response,
        )
    }
    pub fn respond(self, result: Result<Value, String>) {
        let _ = self.reply.send(match result {
            Ok(value) => json!({"ok":true,"result":value}),
            Err(error) => json!({"ok":false,"error":error}),
        });
    }
}
pub struct Bridge {
    pub epoch: String,
    pub requests: Receiver<Pending>,
    state: Arc<Mutex<Value>>,
    stopped: Arc<AtomicBool>,
    path: PathBuf,
    inode: u64,
}
impl Bridge {
    pub fn start(data: &Path) -> io::Result<Self> {
        let directory = data.join("kai");
        fs::create_dir_all(&directory)?;
        let meta = fs::symlink_metadata(&directory)?;
        if !meta.is_dir() || meta.uid() != unsafe { libc::geteuid() } {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Unsafe bridge directory",
            ));
        }
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
        let path = directory.join("control.sock");
        if let Ok(meta) = fs::symlink_metadata(&path) {
            if !meta.file_type().is_socket() || meta.uid() != unsafe { libc::geteuid() } {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Unsafe socket",
                ));
            }
            if UnixStream::connect(&path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Code bridge already running",
                ));
            }
            fs::remove_file(&path)?;
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        listener.set_nonblocking(true)?;
        let inode = fs::metadata(&path)?.ino();
        let (tx, requests) = mpsc::sync_channel(16);
        let state = Arc::new(Mutex::new(Value::Null));
        let stopped = Arc::new(AtomicBool::new(false));
        let epoch = format!(
            "{}-{:x}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let (shared, stop, generation) = (state.clone(), stopped.clone(), epoch.clone());
        thread::spawn(move || {
            // One short-lived connection at a time bounds threads and memory.
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => serve(stream, &tx, &shared, &generation),
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(20))
                    }
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            epoch,
            requests,
            state,
            stopped,
            path,
            inode,
        })
    }
    #[cfg(test)]
    pub fn fixture() -> (Self, SyncSender<Pending>) {
        let (sender, requests) = mpsc::sync_channel(16);
        (
            Self {
                epoch: "test".into(),
                requests,
                state: Arc::new(Mutex::new(Value::Null)),
                stopped: Arc::new(AtomicBool::new(false)),
                path: PathBuf::new(),
                inode: 0,
            },
            sender,
        )
    }
    pub fn publish(&self, value: Value) {
        if let Ok(mut state) = self.state.lock() {
            *state = value
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Relaxed);
        if fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
            let _ = fs::remove_file(&self.path);
        }
    }
}
fn validate(request: &Request, epoch: &str) -> Result<(), String> {
    if request.protocol != 1 || request.id.is_empty() || request.id.len() > 128 {
        return Err("Unsupported protocol or invalid request ID".into());
    }
    if !["snapshot", "open", "send", "cancel"].contains(&request.method.as_str()) {
        return Err("Unsupported command".into());
    }
    if request.method != "snapshot" && request.epoch.as_deref() != Some(epoch) {
        return Err("Code restarted; refresh before sending".into());
    }
    Ok(())
}
fn serve(mut stream: UnixStream, tx: &SyncSender<Pending>, state: &Arc<Mutex<Value>>, epoch: &str) {
    let timeout = Duration::from_millis(750);
    let _ = stream.set_read_timeout(Some(timeout));
    let _ = stream.set_write_timeout(Some(timeout));
    #[cfg(target_os = "macos")]
    {
        use std::os::fd::AsRawFd;
        let (mut uid, mut gid) = (0, 0);
        if unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } != 0
            || uid != unsafe { libc::geteuid() }
        {
            return;
        }
    }
    let mut input = String::new();
    use std::io::Read;
    if BufReader::new((&stream).take(REQUEST_LIMIT + 1))
        .read_line(&mut input)
        .is_err()
        || input.len() as u64 > REQUEST_LIMIT
        || !input.ends_with('\n')
    {
        return;
    }
    let request: Request = match serde_json::from_str(&input) {
        Ok(r) => r,
        Err(_) => return,
    };
    let id = request.id.clone();
    let body = if let Err(error) = validate(&request, epoch) {
        json!({"ok":false,"error":error})
    } else if request.method == "snapshot" {
        match state.lock() {
            Ok(s) if !s.is_null() => json!({"ok":true,"result":s.clone()}),
            _ => json!({"ok":false,"error":"Code is connecting"}),
        }
    } else if request.epoch.as_deref() != Some(epoch) {
        json!({"ok":false,"error":"Code restarted; refresh before sending"})
    } else if !["open", "send", "cancel"].contains(&request.method.as_str()) {
        json!({"ok":false,"error":"Unsupported command"})
    } else {
        let (reply, response) = mpsc::sync_channel(1);
        match tx.try_send(Pending{request,deadline:Instant::now()+Duration::from_millis(500),reply}){
            Ok(())=>response.recv_timeout(Duration::from_millis(650)).unwrap_or_else(|_|json!({"ok":false,"error":"Command acknowledgement timed out; do not automatically retry"})),
            Err(_)=>json!({"ok":false,"error":"Code is busy; try again"})
        }
    };
    let mut response = body;
    response["protocol"] = json!(1);
    response["epoch"] = json!(epoch);
    response["id"] = json!(id);
    if let Ok(bytes) = serde_json::to_vec(&response) {
        if bytes.len() <= RESPONSE_LIMIT {
            let _ = stream.write_all(&bytes);
            let _ = stream.write_all(b"\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> PathBuf {
        PathBuf::from("/private/tmp").join(format!(
            "kai-host-test-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }
    fn call(path: &Path, value: Value) -> Value {
        let mut stream = UnixStream::connect(path).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        writeln!(stream, "{}", value).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }
    #[test]
    fn protocol_rejects_unknown_methods_and_stale_epochs() {
        for value in [
            json!({"protocol":2,"id":"a","method":"snapshot"}),
            json!({"protocol":1,"id":"a","method":"execute","epoch":"current"}),
            json!({"protocol":1,"id":"a","method":"send","epoch":"old"}),
        ] {
            let request = serde_json::from_value(value).unwrap();
            assert!(validate(&request, "current").is_err())
        }
        let request =
            serde_json::from_value(json!({"protocol":1,"id":"a","method":"snapshot"})).unwrap();
        assert!(validate(&request, "current").is_ok());
    }
    #[test]
    fn expired_requests_do_not_mutate() {
        let (reply, response) = mpsc::sync_channel(1);
        let request = serde_json::from_value(
            json!({"protocol":1,"id":"a","method":"send","epoch":"current"}),
        )
        .unwrap();
        let pending = Pending {
            request,
            deadline: Instant::now() - Duration::from_secs(1),
            reply,
        };
        assert!(pending.deadline < Instant::now());
        pending.respond(Err("Expired".into()));
        assert_eq!(response.recv().unwrap()["ok"], false);
    }
    #[test]
    fn snapshots_and_epoch_gate() {
        let root = fixture();
        let host = Bridge::start(&root).unwrap();
        host.publish(json!({"sessions":[]}));
        let path = root.join("kai/control.sock");
        let response = call(&path, json!({"protocol":1,"id":"read","method":"snapshot"}));
        assert_eq!(response["result"]["sessions"], json!([]));
        let stale = call(
            &path,
            json!({"protocol":1,"id":"send","method":"send","epoch":"old"}),
        );
        assert_eq!(stale["ok"], false);
        assert!(host.requests.try_recv().is_err());
        assert!(Bridge::start(&root).is_err());
        drop(host);
        let _ = fs::remove_dir_all(root);
    }
    #[test]
    fn command_queued_once_and_acknowledged() {
        let root = fixture();
        let host = Bridge::start(&root).unwrap();
        let epoch = host.epoch.clone();
        let path = root.join("kai/control.sock");
        let client = thread::spawn(move || {
            call(
                &path,
                json!({"protocol":1,"id":"cancel","epoch":epoch,"method":"cancel","params":{"sessionId":"a"}}),
            )
        });
        let pending = host.requests.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(pending.deadline > Instant::now());
        assert_eq!(pending.request.method, "cancel");
        pending.respond(Ok(json!({"accepted":true})));
        assert_eq!(client.join().unwrap()["ok"], true);
        drop(host);
        let _ = fs::remove_dir_all(root);
    }
}
