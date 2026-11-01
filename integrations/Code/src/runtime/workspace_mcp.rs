//! Per-invocation MCP gateway to the same authorized workspace dispatcher used by
//! Codex dynamic tools. No persistent credentials or remote control authority.
use super::*;
use serde_json::json;
use std::{
    collections::HashMap,
    fs::{self, OpenOptions},
    io::{BufRead, BufReader},
    os::unix::{
        fs::{OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::Path,
};

const LIMIT: u64 = 2 * 1024 * 1024;
const SERVER: &str = "kai_workspace";
struct State {
    stopped: AtomicBool,
    pending: Mutex<HashMap<String, SyncSender<Value>>>,
}
pub(super) struct Bridge {
    directory: PathBuf,
    helper: PathBuf,
    token: String,
    listener: Option<UnixListener>,
    state: Arc<State>,
}
impl Bridge {
    pub(super) fn prepare(request: &RunRequest) -> Result<Option<Self>> {
        if !request.orchestrator && request.identity.is_none() {
            return Ok(None);
        }
        let packaged = std::env::current_exe()?
            .parent()
            .context("Missing executable directory")?
            .join("../Resources/kai_workspace_mcp.py");
        let helper = if packaged.is_file() {
            packaged
        } else {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("native_helpers/kai_workspace_mcp.py")
        };
        anyhow::ensure!(helper.is_file(), "Kai workspace MCP helper is missing");
        // /tmp keeps the socket path below Darwin's 104-byte sockaddr limit.
        let directory =
            PathBuf::from("/private/tmp").join(format!("kai-ws-{}", uuid::Uuid::new_v4()));
        fs::DirBuilder::new().mode(0o700).create(&directory)?;
        let mut bridge = Self {
            directory,
            helper,
            token: format!("{}{}", uuid::Uuid::new_v4(), uuid::Uuid::new_v4()),
            listener: None,
            state: Arc::new(State {
                stopped: AtomicBool::new(false),
                pending: Mutex::new(HashMap::new()),
            }),
        };
        bridge.listener = Some(UnixListener::bind(bridge.socket())?);
        fs::set_permissions(bridge.socket(), fs::Permissions::from_mode(0o600))?;
        let config = json!({"socket":bridge.socket(),"token":bridge.token,"tools":crate::orchestrator::tools_for_request(request),"instructions":crate::orchestrator::INSTRUCTIONS});
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(bridge.config())?;
        serde_json::to_writer(&mut file, &config)?;
        Ok(Some(bridge))
    }
    fn socket(&self) -> PathBuf {
        self.directory.join("rpc.sock")
    }
    fn config(&self) -> PathBuf {
        self.directory.join("route.json")
    }
    fn args(&self) -> Value {
        json!([self.helper, self.config()])
    }
    pub(super) fn acp(&self) -> Value {
        json!({"name":SERVER,"command":"/usr/bin/python3","args":self.args(),"env":[]})
    }
    pub(super) fn configure_claude(&self, command: &mut Command) {
        command.arg("--mcp-config").arg(
            json!({"mcpServers":{SERVER:{"command":"/usr/bin/python3","args":self.args()}}})
                .to_string(),
        );
        command
            .arg("--append-system-prompt")
            .arg(crate::orchestrator::INSTRUCTIONS);
    }
    pub(super) fn configure_opencode(&self, command: &mut Command) -> Result<()> {
        let inherited = command
            .get_envs()
            .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
            .or_else(|| std::env::var("OPENCODE_CONFIG_CONTENT").ok());
        let mut config: Value = match inherited {
            Some(value) => {
                serde_json::from_str(&value).context("Invalid inherited OpenCode inline config")?
            }
            None => json!({}),
        };
        anyhow::ensure!(
            config.is_object(),
            "OpenCode inline config must be an object"
        );
        if config.get("mcp").is_none() {
            config["mcp"] = json!({});
        }
        anyhow::ensure!(
            config["mcp"].is_object(),
            "OpenCode mcp config must be an object"
        );
        config["mcp"][SERVER] = json!({"type":"local","command":["/usr/bin/python3",self.helper,self.config()],"enabled":true,"timeout":95000});
        command.env("OPENCODE_CONFIG_CONTENT", config.to_string());
        Ok(())
    }
    pub(super) fn configure_agy(&self, command: &mut Command, home: &Path) -> Result<()> {
        // Antigravity has no per-session MCP flag. Register one inert entry;
        // its subprocess inherits a route only from the owning Kai invocation.
        register_agy(&home.join(".gemini/config/mcp_config.json"), &self.helper)?;
        command.env("KAI_WORKSPACE_CONFIG", self.config());
        Ok(())
    }
    pub(super) fn attach(
        mut self,
        handle: &mut RunHandle,
        sender: &NotifyingSender,
        cancelled: &Arc<AtomicBool>,
    ) {
        let listener = self.listener.take().expect("Workspace listener");
        let state = self.state.clone();
        let token = self.token.clone();
        let tx = sender.clone();
        let cancelled = cancelled.clone();
        thread::spawn(move || {
            for client in listener.incoming() {
                if state.stopped.load(Ordering::Acquire) || cancelled.load(Ordering::Acquire) {
                    break;
                }
                let Ok(mut stream) = client else { break };
                // Reserve before spawning so slow clients cannot create an unbounded backlog.
                let id = uuid::Uuid::new_v4().to_string();
                let (reply_tx, reply_rx) = mpsc::sync_channel(1);
                {
                    let mut pending = state.pending.lock().unwrap_or_else(|e| e.into_inner());
                    if pending.len() >= 8 {
                        continue;
                    }
                    pending.insert(id.clone(), reply_tx);
                }
                let state = state.clone();
                let token = token.clone();
                let tx = tx.clone();
                let cancelled = cancelled.clone();
                thread::spawn(move || {
                    let result = (|| -> Result<Value> {
                        stream.set_read_timeout(Some(Duration::from_secs(3)))?;
                        stream.set_write_timeout(Some(Duration::from_secs(3)))?;
                        let mut line = String::new();
                        BufReader::new((&stream).take(LIMIT + 1)).read_line(&mut line)?;
                        anyhow::ensure!(
                            line.len() as u64 <= LIMIT && line.ends_with('\n'),
                            "Invalid workspace request"
                        );
                        let call: Value = serde_json::from_str(&line)?;
                        anyhow::ensure!(
                            call["token"].as_str() == Some(token.as_str()),
                            "Unauthorized workspace request"
                        );
                        let tool = call["tool"].as_str().context("Missing tool")?;
                        anyhow::ensure!(
                            ["code_workspace", "code_history", "code_artifacts", "ninja_memory"].contains(&tool)
                                && call["arguments"].is_object(),
                            "Invalid workspace tool"
                        );
                        anyhow::ensure!(
                            !state.stopped.load(Ordering::Acquire)
                                && !cancelled.load(Ordering::Acquire),
                            "Workspace invocation stopped"
                        );
                        send(
                            &tx,
                            RuntimeEvent::WorkspaceTool {
                                id: id.clone(),
                                tool: tool.into(),
                                arguments: call["arguments"].clone(),
                            },
                            &cancelled,
                        )?;
                        let until = Instant::now() + Duration::from_secs(90);
                        loop {
                            anyhow::ensure!(
                                !state.stopped.load(Ordering::Acquire)
                                    && !cancelled.load(Ordering::Acquire),
                                "Workspace invocation stopped"
                            );
                            match reply_rx.recv_timeout(Duration::from_millis(100)) {
                                Ok(value) => return Ok(value),
                                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() < until => {}
                                _ => bail!("Workspace reply timed out"),
                            }
                        }
                    })()
                    .unwrap_or_else(|e| json!({"error":e.to_string()}));
                    state
                        .pending
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .remove(&id);
                    if let Ok(mut bytes) = serde_json::to_vec(&result) {
                        if bytes.len() as u64 > LIMIT - 1 {
                            bytes =
                                b"{\"error\":\"Workspace reply exceeds transport limit\"}".to_vec();
                        }
                        bytes.push(b'\n');
                        let _ = stream.write_all(&bytes);
                    }
                });
            }
        });
        handle.workspace = Some(self);
    }
    pub(super) fn reply(&self, id: &str, value: Value) -> Result<()> {
        anyhow::ensure!(
            !self.state.stopped.load(Ordering::Acquire),
            "Workspace invocation stopped"
        );
        self.state
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .context("Workspace request has ended")?
            .try_send(value)
            .map_err(|_| anyhow::anyhow!("Workspace request has ended"))
    }
    pub(super) fn stop(&self) {
        if !self.state.stopped.swap(true, Ordering::AcqRel) {
            self.state
                .pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clear();
            let _ = UnixStream::connect(self.socket());
        }
    }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        self.stop();
        let _ = fs::remove_dir_all(&self.directory);
    }
}

fn register_agy(path: &Path, helper: &Path) -> Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    let parent = path
        .parent()
        .context("Missing Antigravity config directory")?;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(parent.join(".kai-workspace.lock"))?;
    anyhow::ensure!(
        unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } == 0,
        "Could not lock Antigravity MCP config"
    );
    let mut config: Value = match OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
    {
        Ok(file) => {
            anyhow::ensure!(
                file.metadata()?.len() <= LIMIT,
                "Antigravity MCP config is too large"
            );
            let mut bytes = Vec::new();
            file.take(LIMIT).read_to_end(&mut bytes)?;
            if bytes.iter().all(u8::is_ascii_whitespace) {
                json!({})
            } else {
                serde_json::from_slice(&bytes).context("Invalid existing Antigravity MCP config")?
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(e.into()),
    };
    anyhow::ensure!(
        config.is_object(),
        "Antigravity MCP config must be an object"
    );
    if config.get("mcpServers").is_none() {
        config["mcpServers"] = json!({});
    }
    anyhow::ensure!(
        config["mcpServers"].is_object(),
        "Antigravity MCP servers must be an object"
    );
    let entry = json!({"command":"/usr/bin/python3","args":[helper,"--environment"],"disabled":false,"timeoutSeconds":95});
    if config["mcpServers"].get(SERVER) == Some(&entry) {
        return Ok(());
    }
    anyhow::ensure!(
        config["mcpServers"]
            .get(SERVER)
            .is_none_or(|old| old["command"] == "/usr/bin/python3"
                && old["args"][0].as_str().is_some_and(|p| Path::new(p)
                    .file_name()
                    .is_some_and(|n| n == "kai_workspace_mcp.py"))
                && old["args"][1] == "--environment"),
        "An existing MCP server already uses Kai's workspace name"
    );
    config["mcpServers"][SERVER] = entry;
    let temporary = parent.join(format!(".kai-workspace-{}.json", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, &config)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    let _ = fs::remove_file(temporary);
    result
}
use std::os::unix::fs::DirBuilderExt;

#[cfg(test)]
mod tests {
    use super::*;
    fn request(agent: AgentKind) -> RunRequest {
        RunRequest {
            identity:None, ninja: None,
            orchestrator: true,
            agent,
            executable: "/fixture/agent".into(),
            project: std::env::temp_dir(),
            prompt: "Inspect projects".into(),
            resume_id: None,
            fork_session: false,
            model: Some("selected-model".into()),
            service_tier: None,
            reasoning_effort: Some("high".into()),
            images: vec![],
            allow_edits: false,
            trust_workspace: false,
            codex_approvals: Default::default(),
            codex_access: Default::default(),
        }
    }
    #[test]
    fn mcp_round_trip_is_scoped_authenticated_and_stops_with_its_turn() {
        let bridge = Bridge::prepare(&request(AgentKind::Claude))
            .unwrap()
            .unwrap();
        let socket = bridge.socket();
        let config = bridge.config();
        let script = bridge.helper.clone();
        let mode = fs::metadata(&config).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let (mut handle, context) = native_handle(0);
        *handle.process_group.lock().unwrap() = None;
        let data: Value = serde_json::from_slice(&fs::read(&config).unwrap()).unwrap();
        bridge.attach(&mut handle, &context.tx, &context.cancelled);
        let result = thread::spawn(move || {
            let mut command = Command::new("/usr/bin/python3");
            command
                .arg(script)
                .arg(config)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped());
            let mut child = command.spawn().unwrap();
            let mut input = child.stdin.take().unwrap();
            writeln!(
                input,
                "{}",
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}})
            )
            .unwrap();
            writeln!(
                input,
                "{}",
                json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})
            )
            .unwrap();
            writeln!(input,"{}",json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"code_workspace","arguments":{"action":"list_projects"}}})).unwrap();
            drop(input);
            let output = child.wait_with_output().unwrap();
            assert!(output.status.success());
            let rows = String::from_utf8(output.stdout)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap())
                .collect::<Vec<_>>();
            assert_eq!(rows.len(), 3);
            assert!(
                rows[1]["result"]["tools"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["name"] == "code_workspace")
            );
            let value: Value =
                serde_json::from_str(rows[2]["result"]["content"][0]["text"].as_str().unwrap())
                    .unwrap();
            assert_eq!(value["projects"][0]["name"], "Round trip proof");
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match handle.events.try_recv() {
                Ok(RuntimeEvent::WorkspaceTool {
                    id,
                    tool,
                    arguments,
                }) => {
                    assert_eq!(tool, "code_workspace");
                    assert_eq!(arguments["action"], "list_projects");
                    handle
                        .native_command(
                            "code/workspace/result",
                            json!({"id":id,"value":{"projects":[{"name":"Round trip proof"}]}}),
                        )
                        .unwrap();
                    break;
                }
                _ => {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
            }
        }
        result.join().unwrap();
        let mut unauthorized = UnixStream::connect(&socket).unwrap();
        unauthorized
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        writeln!(
            unauthorized,
            "{}",
            json!({"token":"wrong","tool":"code_workspace","arguments":{"action":"list_projects"}})
        )
        .unwrap();
        let mut line = String::new();
        BufReader::new(unauthorized).read_line(&mut line).unwrap();
        assert!(serde_json::from_str::<Value>(&line).unwrap()["error"].is_string());
        assert!(handle.events.try_recv().is_err());
        let mut client = UnixStream::connect(&socket).unwrap();
        writeln!(client,"{}",json!({"token":data["token"],"tool":"code_workspace","arguments":{"action":"list_chats"}})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        let id = loop {
            match handle.events.try_recv() {
                Ok(RuntimeEvent::WorkspaceTool { id, .. }) => break id,
                _ => {
                    assert!(Instant::now() < deadline);
                    thread::sleep(Duration::from_millis(5));
                }
            }
        };
        handle.cancel();
        assert!(
            handle
                .native_command("code/workspace/result", json!({"id":id,"value":{}}))
                .is_err()
        );
        drop(handle);
        assert!(!socket.exists());
    }
    #[test]
    fn provider_configs_preserve_existing_mcp_and_never_change_model_or_permissions() {
        let r = request(AgentKind::OpenCode);
        let bridge = Bridge::prepare(&r).unwrap().unwrap();
        let mut command = Command::new("opencode");
        command.env("OPENCODE_CONFIG_CONTENT",r#"{"model":"existing/model","permission":{"bash":"ask"},"mcp":{"other":{"type":"remote","url":"https://example.invalid"}}}"#);
        bridge.configure_opencode(&mut command).unwrap();
        let config: Value = serde_json::from_str(
            command
                .get_envs()
                .find(|(k, _)| *k == "OPENCODE_CONFIG_CONTENT")
                .unwrap()
                .1
                .unwrap()
                .to_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(config["model"], "existing/model");
        assert_eq!(config["permission"]["bash"], "ask");
        assert_eq!(config["mcp"]["other"]["type"], "remote");
        assert_eq!(config["mcp"][SERVER]["type"], "local");
        assert_eq!(bridge.acp()["command"], "/usr/bin/python3");
        let mut claude = Command::new("claude");
        bridge.configure_claude(&mut claude);
        let args = claude
            .get_args()
            .map(|v| v.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(args[0], "--mcp-config");
        assert!(!args.iter().any(|s| s.contains("skip-permissions")));
        assert_eq!(r.model.as_deref(), Some("selected-model"));
        let directory = bridge.directory.join("agy");
        fs::create_dir(&directory).unwrap();
        let path = directory.join("mcp_config.json");
        fs::write(
            &path,
            r#"{"mcpServers":{"other":{"command":"existing"}},"otherSetting":true}"#,
        )
        .unwrap();
        register_agy(&path, &bridge.helper).unwrap();
        register_agy(&path, &bridge.helper).unwrap();
        let empty = directory.join("empty.json");
        fs::write(&empty, "\n ").unwrap();
        register_agy(&empty, &bridge.helper).unwrap();
        let empty: Value = serde_json::from_slice(&fs::read(empty).unwrap()).unwrap();
        assert!(empty["mcpServers"][SERVER].is_object());
        let config: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["mcpServers"]["other"]["command"], "existing");
        assert_eq!(config["otherSetting"], true);
        assert_eq!(config["mcpServers"][SERVER]["args"][1], "--environment");
        assert!(!fs::read_to_string(&path).unwrap().contains(&bridge.token));
        let output = Command::new("/usr/bin/python3")
            .arg(&bridge.helper)
            .arg("--environment")
            .env_remove("KAI_WORKSPACE_CONFIG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                writeln!(
                    child.stdin.take().unwrap(),
                    "{}",
                    json!({"jsonrpc":"2.0","id":1,"method":"tools/list"})
                )?;
                child.wait_with_output()
            })
            .unwrap();
        let value: Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(value["result"]["tools"], json!([]));
        let mut ordinary = r.clone();
        ordinary.orchestrator = false;
        assert!(Bridge::prepare(&ordinary).unwrap().is_none());
    }
    #[test]
    fn native_claude_and_acp_adapters_execute_real_mcp_round_trips_on_new_and_resumed_chats() {
        let script = r#"#!/usr/bin/python3
import sys,json,subprocess

def emit(v):print(json.dumps(v),flush=True)
def tool(server):
 p=subprocess.run([server['command'],*server['args']],input=json.dumps({'jsonrpc':'2.0','id':7,'method':'tools/call','params':{'name':'code_workspace','arguments':{'action':'list_projects'}}})+'\n',capture_output=True,text=True,timeout=8)
 v=json.loads(p.stdout);assert 'result' in v,v
 return json.loads(v['result']['content'][0]['text'])['projects'][0]['name']
if '--mcp-config' in sys.argv:
 server=json.loads(sys.argv[sys.argv.index('--mcp-config')+1])['mcpServers']['kai_workspace']
 json.loads(sys.stdin.readline())
 emit({'type':'control_response','response':{'subtype':'success','request_id':'code-initialize','response':{'commands':[]}}})
 json.loads(sys.stdin.readline())
 emit({'type':'system','subtype':'init','session_id':'claude-orchestrator','slash_commands':[]})
 text=tool(server)
 emit({'type':'assistant','message':{'id':'reply','content':[{'type':'text','text':text}]}})
 emit({'type':'result','subtype':'success','is_error':False,'result':text})
 sys.stdin.readline()
else:
 for line in sys.stdin:
  v=json.loads(line);m=v.get('method')
  if m=='initialize':emit({'jsonrpc':'2.0','id':v['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True,'mcpCapabilities':{'http':False,'sse':False}}}})
  elif m in ['session/new','session/load','session/resume']:
   servers=v['params']['mcpServers'];assert len(servers)==1,servers
   server=servers[0];assert server['name']=='kai_workspace'
   emit({'jsonrpc':'2.0','id':v['id'],'result':{'sessionId':'acp-orchestrator','modes':{'availableModes':[{'id':'default'},{'id':'agent'}]}}})
  elif m=='session/set_mode':emit({'jsonrpc':'2.0','id':v['id'],'result':{}})
  elif m=='session/prompt':
   text=tool(server)
   emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'acp-orchestrator','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':text}}}})
   emit({'jsonrpc':'2.0','id':v['id'],'result':{'stopReason':'end_turn'}})
"#;
        for kind in [
            AgentKind::Claude,
            AgentKind::Gemini,
            AgentKind::Cursor,
            AgentKind::Grok,
        ] {
            for resume in [false, true] {
                let root = PathBuf::from("/private/tmp").join(format!(
                    "kai-orchestrator-protocol-{}",
                    uuid::Uuid::new_v4()
                ));
                fs::create_dir(&root).unwrap();
                let executable = root.join("provider");
                fs::write(&executable, script).unwrap();
                fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
                let mut r = request(kind);
                r.executable = executable;
                r.project = root.clone();
                r.model = None;
                r.reasoning_effort = None;
                r.allow_edits = true;
                r.resume_id = resume.then(|| {
                    if kind == AgentKind::Claude {
                        "claude-orchestrator"
                    } else {
                        "acp-orchestrator"
                    }
                    .into()
                });
                let handle = super::super::launch(r).unwrap();
                let mut called = false;
                let mut done = false;
                let deadline = Instant::now() + Duration::from_secs(12);
                let mut text = String::new();
                while Instant::now() < deadline && !done {
                    match handle.events.try_recv() {
                        Ok(RuntimeEvent::WorkspaceTool {
                            id,
                            tool,
                            arguments,
                        }) => {
                            assert_eq!(tool, "code_workspace");
                            assert_eq!(arguments["action"], "list_projects");
                            called = true;
                            handle.native_command("code/workspace/result",json!({"id":id,"value":{"projects":[{"name":"PROTOCOL_WORKSPACE_OK"}]}})).unwrap();
                        }
                        Ok(RuntimeEvent::Text(delta)) => text.push_str(&delta),
                        Ok(
                            RuntimeEvent::ReplaceText(value) | RuntimeEvent::ReplacePreview(value),
                        ) => text = value,
                        Ok(RuntimeEvent::Finished) => done = true,
                        Ok(RuntimeEvent::Failed(error)) => {
                            panic!("{kind:?}, resume={resume}: {error}")
                        }
                        _ => thread::sleep(Duration::from_millis(5)),
                    }
                }
                assert!(
                    called && done,
                    "{kind:?}, resume={resume}, called={called}, done={done}"
                );
                assert!(text.contains("PROTOCOL_WORKSPACE_OK"), "{text}");
                handle.cancel();
                drop(handle);
                let _ = fs::remove_dir_all(root);
            }
        }
    }
    #[test]
    #[ignore = "Explicit live provider login test; uses a temporary project and only lists fixture projects"]
    fn installed_provider_calls_workspace_tools_without_switching_provider() {
        let provider = std::env::var("KAI_ORCHESTRATOR_TEST_PROVIDER").unwrap();
        let kind = match provider.as_str() {
            "claude" => AgentKind::Claude,
            "gemini" => AgentKind::Gemini,
            "cursor" => AgentKind::Cursor,
            "grok" => AgentKind::Grok,
            "opencode" => AgentKind::OpenCode,
            "antigravity" => AgentKind::Antigravity,
            "ninja" => AgentKind::Ninja,
            _ => panic!("Unknown live provider"),
        };
        let root = std::env::temp_dir().join(format!(
            "kai-orchestrator-provider-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir(&root).unwrap();
        let executable = crate::discovery::discover_agent(
            if kind == AgentKind::Ninja {
                AgentKind::Claude
            } else {
                kind
            },
            &dirs::home_dir().unwrap(),
        )
        .executable
        .expect("Installed provider");
        let mut r = request(kind);
        r.project = root.clone();
        r.executable = executable;
        if kind == AgentKind::Ninja {
            r.ninja = Some(crate::ninja::Selection {
                machine: "studio".into(),
                endpoint: std::env::var("KAI_ORCHESTRATOR_TEST_ENDPOINT").unwrap(),
                engine: crate::ninja::Engine::Ollama,
                harness: crate::ninja::Harness::Claude,
                model: std::env::var("KAI_ORCHESTRATOR_TEST_MODEL").unwrap(),
                node: true,
                ..Default::default()
            });
        }
        r.model = std::env::var("KAI_ORCHESTRATOR_TEST_MODEL").ok();
        r.reasoning_effort = None;
        r.allow_edits = true;
        r.prompt="Call the Kai Workspace MCP code_workspace tool with action list_projects. Do not run shell commands, read files, create chats, or do any other action. Read the returned project name and finish with that exact project name alone. This is a transport validation.".into();
        let handle = super::super::launch(r).unwrap();
        let deadline = Instant::now() + Duration::from_secs(100);
        let mut called = false;
        let mut text = String::new();
        let mut failure = None;
        loop {
            match handle.events.try_recv() {
                Ok(RuntimeEvent::WorkspaceTool {
                    id,
                    tool,
                    arguments,
                }) => {
                    assert_eq!(tool, "code_workspace");
                    assert_eq!(arguments["action"], "list_projects");
                    called = true;
                    handle.native_command("code/workspace/result",json!({"id":id,"value":{"projects":[{"name":"KAI_ORCHESTRATOR_PROVIDER_OK","path":root}]}})).unwrap();
                }
                Ok(RuntimeEvent::Text(delta)) => text.push_str(&delta),
                Ok(RuntimeEvent::ReplaceText(value) | RuntimeEvent::ReplacePreview(value)) => {
                    text = value
                }
                Ok(RuntimeEvent::Request(req)) => {
                    assert!(
                        format!("{} {}", req.title, req.detail).contains("code_workspace"),
                        "Unexpected provider request: {req:?}"
                    );
                    let NativeRequestKind::Approval { choices } = &req.kind else {
                        panic!("Unexpected question")
                    };
                    let choice = choices
                        .iter()
                        .find(|c| {
                            c.label.to_lowercase().contains("allow once")
                                || c.label.to_lowercase().contains("allow this")
                                || c.id == "allow_once"
                                || c.id == "allow"
                        })
                        .expect("Allow once option");
                    handle
                        .respond(&req.id, NativeAnswer::Choice(choice.id.clone()))
                        .unwrap();
                }
                Ok(RuntimeEvent::Finished) => break,
                Ok(RuntimeEvent::Failed(e)) => {
                    failure = Some(e);
                    break;
                }
                _ => {
                    assert!(
                        Instant::now() < deadline,
                        "Provider test timeout: {provider}, called={called}"
                    );
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        handle.cancel();
        drop(handle);
        let _ = fs::remove_dir_all(&root);
        assert!(failure.is_none(), "{provider}: {failure:?}");
        assert!(
            called,
            "Provider never called the workspace tool: {provider}: {text}"
        );
        assert!(text.contains("KAI_ORCHESTRATOR_PROVIDER_OK"), "{text}");
    }
}
