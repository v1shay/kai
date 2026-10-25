//! Native Gemini and Cursor ACP transport. JSON-RPC stays on owned stdio; authentication,
//! policy, sandboxing and tool execution remain in the installed CLI.
//! References: geminicli.com/docs/cli/acp-mode and agentclientprotocol.com.
use super::{
    NativeAnswer, NativeContext, NativeControl, NativeOption, NativeQuestion, NativeRequest,
    NativeRequestKind, RunHandle, RunRequest, RuntimeEvent, StreamParser, native_handle, send,
    wire,
};
use crate::chat_interactions::{ChatInteraction, InteractionStatus};
use crate::discovery::AgentKind;
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio},
    sync::atomic::Ordering,
    thread,
    time::{Duration, Instant},
};

pub(super) fn launch(request: RunRequest) -> Result<RunHandle> {
    launch_mode(request, false)
}
pub(super) fn launch_command(request: RunRequest) -> Result<RunHandle> {
    launch_mode(request, true)
}
fn launch_mode(request: RunRequest, cli_command: bool) -> Result<RunHandle> {
    super::validate_images(&request)?;
    ensure!(
        !request.fork_session,
        "This ACP provider cannot fork a native session"
    );
    let mut command = Command::new(&request.executable);
    crate::process_env::apply(&mut command);
    command.current_dir(&request.project);
    if request.agent == AgentKind::Cursor {
        // Root options precede the ACP subcommand. Never use --force/--yolo:
        // real permission requests must remain owned by this protocol worker.
        if request.trust_workspace {
            command.arg("--trust");
        }
        command.arg("acp");
    } else {
        command.args([
            "--experimental-acp",
            "--approval-mode",
            if request.allow_edits {
                "default"
            } else {
                "plan"
            },
        ]);
        if request.trust_workspace {
            command.arg("--skip-trust");
        }
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .with_context(|| format!("Could not start {} ACP", request.agent.name()))?;
    let (handle, context) = native_handle(child.id() as i32);
    let pipes = Pipes::new(&mut child);
    let mut pipes = match pipes {
        Ok(pipes) => pipes,
        Err(error) => {
            handle.cancel();
            let mut group = context
                .group
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(pid) = *group {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = child.wait();
            *group = None;
            return Err(error);
        }
    };
    thread::spawn(move || {
        let mut parser = StreamParser::default();
        let result = run(
            &request,
            cli_command,
            &mut pipes,
            &mut child,
            &context,
            &mut parser,
        );
        if result.is_err() {
            pipes.cancel_native();
        }
        // Clear ownership under the same lock used by RunHandle::cancel before
        // reaping. Do not leave descendants or a permission-blocked agent alive.
        {
            let mut group = context.group.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pid) = *group {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = child.wait();
            *group = None;
        }
        let terminal = if context.cancelled.load(Ordering::Acquire) {
            RuntimeEvent::Status("Stopped".into())
        } else {
            match result {
                Ok(()) => RuntimeEvent::Finished,
                Err(error) => RuntimeEvent::Failed(format!("{error:#}")),
            }
        };
        super::record_terminal(&mut parser, &terminal);
        match parser.rendered_source() {
            Ok(source) => {
                *context
                    .finalized_source
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(source)
            }
            Err(error) => {
                let _ = send(
                    &context.tx,
                    RuntimeEvent::Failed(format!("Could not save ACP output: {error}")),
                    &context.cancelled,
                );
                return;
            }
        }
        if context.cancelled.load(Ordering::Acquire) {
            let _ = super::EventSink::try_send(&context.tx, terminal);
        } else {
            let _ = send(&context.tx, terminal, &context.cancelled);
        }
        drop(context.settled);
    });
    Ok(handle)
}

struct Pipes {
    input: ChildStdin,
    output: ChildStdout,
    error: ChildStderr,
    record: wire::Record,
    ready: std::collections::VecDeque<Value>,
    stderr: Vec<u8>,
    eof: bool,
    session_id: Option<String>,
}
impl Pipes {
    fn new(child: &mut Child) -> Result<Self> {
        let input = child.stdin.take().context("ACP stdin unavailable")?;
        let output = child.stdout.take().context("ACP stdout unavailable")?;
        let error = child.stderr.take().context("ACP stderr unavailable")?;
        for fd in [input.as_raw_fd(), output.as_raw_fd(), error.as_raw_fd()] {
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
            ensure!(
                flags >= 0
                    && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
                "Could not configure ACP pipes"
            );
        }
        Ok(Self {
            input,
            output,
            error,
            record: wire::Record::default(),
            ready: Default::default(),
            stderr: vec![],
            eof: false,
            session_id: None,
        })
    }
    fn write(&mut self, value: &Value, cx: &NativeContext) -> Result<()> {
        let mut bytes = serde_json::to_vec(value)?;
        bytes.push(b'\n');
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut offset = 0;
        while offset < bytes.len() {
            ensure!(!cx.cancelled.load(Ordering::Acquire), "Stopped");
            ensure!(
                Instant::now() < deadline,
                "ACP agent stopped accepting protocol input"
            );
            match self.input.write(&bytes[offset..]) {
                Ok(0) => bail!("ACP input closed"),
                Ok(n) => offset += n,
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(())
    }
    fn cancel_native(&mut self) {
        let Some(id) = self.session_id.clone() else {
            return;
        };
        let deadline = Instant::now() + Duration::from_millis(850);
        let Ok(mut bytes) = serde_json::to_vec(
            &json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":id}}),
        ) else {
            return;
        };
        bytes.push(b'\n');
        let mut at = 0;
        while at < bytes.len() && Instant::now() < deadline {
            match self.input.write(&bytes[at..]) {
                Ok(0) => return,
                Ok(n) => at += n,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    thread::sleep(Duration::from_millis(5))
                }
                Err(_) => return,
            }
        }
        if at < bytes.len() {
            return;
        }
        // Cleanup never projects a giant queued event: discard raw frames with
        // a small line buffer so a cancelled multi-MB tool result cannot defeat
        // the grace deadline or delay native child cancellation.
        let mut discard = !self.record.is_empty();
        self.record.clear();
        self.ready.clear();
        let mut line = Vec::with_capacity(4096);
        let mut buffer = [0u8; 8192];
        while Instant::now() < deadline {
            match self.output.read(&mut buffer) {
                Ok(0) => return,
                Ok(n) => {
                    for chunk in buffer[..n].split_inclusive(|b| *b == b'\n') {
                        let complete = chunk.last() == Some(&b'\n');
                        if !discard {
                            if line.len() + chunk.len() <= 16 * 1024 {
                                line.extend_from_slice(chunk);
                            } else {
                                line.clear();
                                discard = true;
                            }
                        }
                        if complete {
                            if !discard
                                && let Ok(value) = serde_json::from_slice::<Value>(&line)
                                && value["id"] == 5
                                && (value.get("result").is_some() || value.get("error").is_some())
                            {
                                return;
                            }
                            line.clear();
                            discard = false;
                        }
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return,
            }
            let _ = self.error.read(&mut buffer);
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn next(&mut self, cx: &NativeContext) -> Result<Option<Value>> {
        self.next_uncancelled(&cx.cancelled)
    }
    fn next_uncancelled(
        &mut self,
        cancelled: &std::sync::atomic::AtomicBool,
    ) -> Result<Option<Value>> {
        if let Some(value) = self.ready.pop_front() {
            return Ok(Some(value));
        }
        let mut buffer = [0u8; 8192];
        for _ in 0..8 {
            match self.error.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    self.stderr.extend_from_slice(&buffer[..n]);
                    if self.stderr.len() > super::STDERR_LIMIT {
                        self.stderr.drain(..self.stderr.len() - super::STDERR_LIMIT);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e.into()),
            }
        }
        // One bounded read batch prevents an aggressive producer from growing
        // an unbounded queue before UI backpressure can apply.
        match self.output.read(&mut buffer) {
            Ok(0) => self.eof = true,
            Ok(n) => {
                for chunk in buffer[..n].split_inclusive(|b| *b == b'\n') {
                    let complete = chunk.last() == Some(&b'\n');
                    let body = if complete {
                        &chunk[..chunk.len() - 1]
                    } else {
                        chunk
                    };
                    self.record.push(body)?;
                    if complete && !self.record.is_empty() {
                        let value = if let Some(bytes) = self.record.small() {
                            serde_json::from_slice(bytes)
                                .context("Agent emitted invalid ACP JSON")?
                        } else {
                            {
                                let (mut value, shortened) = self.record.projected(cancelled)?;
                                if shortened {
                                    value["codeProjectionShortened"] = json!(true);
                                }
                                value
                            }
                        };
                        self.record.clear();
                        self.ready.push_back(value);
                    }
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(self.ready.pop_front())
    }
    fn rpc(&mut self, id: u64, method: &str, params: Value, cx: &NativeContext) -> Result<()> {
        self.write(
            &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            cx,
        )
    }
}

struct Permission {
    wire_id: Value,
    options: HashSet<String>,
    reviewable: bool,
    extension: Option<NativeRequest>,
}
struct ChildSession {
    name: String,
    task: String,
    text: String,
    status: InteractionStatus,
}
struct Session {
    agent: AgentKind,
    allow_edits: bool,
    id: String,
    pending: HashMap<String, Permission>,
    loading: bool,
    modes: HashSet<String>,
    models: HashSet<String>,
    commands: HashSet<String>,
    cursor_todos: BTreeMap<String, (String, String)>,
    children: HashMap<String, ChildSession>,
}
impl Session {
    fn new() -> Self {
        Self {
            agent: AgentKind::Gemini,
            allow_edits: false,
            id: String::new(),
            pending: HashMap::new(),
            loading: false,
            modes: HashSet::new(),
            models: HashSet::new(),
            commands: HashSet::new(),
            cursor_todos: BTreeMap::new(),
            children: HashMap::new(),
        }
    }
    fn owns(&self, id: &Value) -> bool {
        id.as_str() == Some(self.id.as_str())
            || id.as_str().is_some_and(|id| self.children.contains_key(id))
    }
    fn child_update(
        &mut self,
        session_id: &str,
        update: &Value,
        parser: &mut StreamParser,
    ) -> Vec<RuntimeEvent> {
        let kind = update["sessionUpdate"].as_str().unwrap_or("");
        if matches!(kind, "subagent_spawned" | "subagent_state_update") {
            let Some(id) = update["subagentSessionId"]
                .as_str()
                .filter(|id| !id.is_empty() && id.len() <= 1024)
            else {
                return Vec::new();
            };
            if kind == "subagent_spawned" {
                if id == self.id || (self.children.len() >= 64 && !self.children.contains_key(id)) {
                    return Vec::new();
                }
                self.children
                    .entry(id.into())
                    .or_insert_with(|| ChildSession {
                        name: update["name"]
                            .as_str()
                            .unwrap_or("Subagent")
                            .chars()
                            .take(240)
                            .collect(),
                        task: update["task"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(2048)
                            .collect(),
                        text: String::new(),
                        status: InteractionStatus::Started,
                    });
            } else if let Some(child) = self.children.get_mut(id) {
                child.status = match update["state"].as_str().unwrap_or("") {
                    "completed" => InteractionStatus::Completed,
                    "failed" => InteractionStatus::Failed,
                    "cancelled" => InteractionStatus::Cancelled,
                    _ => InteractionStatus::Working,
                };
            }
            return self.child_card(id, parser);
        }
        let Some(child) = self.children.get_mut(session_id) else {
            return Vec::new();
        };
        match kind {
            "agent_message_chunk" => {
                for ch in update["content"]["text"].as_str().unwrap_or("").chars() {
                    if child.text.len() + ch.len_utf8() > 8 * 1024 {
                        break;
                    }
                    child.text.push(ch);
                }
                child.status = InteractionStatus::Working;
            }
            "tool_call" | "tool_call_update" => {
                child.status = InteractionStatus::Working;
                // Tool completion is not child completion. The authoritative
                // subagent_state_update owns its terminal state.
            }
            _ => return Vec::new(),
        }
        self.child_card(session_id, parser)
    }
    fn child_card(&self, id: &str, parser: &mut StreamParser) -> Vec<RuntimeEvent> {
        let Some(child) = self.children.get(id) else {
            return Vec::new();
        };
        parser.interaction(
            ChatInteraction::Delegation {
                title: child.name.clone(),
                details: format!("{}\n{}", child.task, child.text),
                status: child.status,
                agent: Some(id.into()),
            },
            &format!("acp.child.{id}"),
        )
    }
    fn notification(
        &mut self,
        value: &Value,
        parser: &mut StreamParser,
        cx: &NativeContext,
        pipes: &mut Pipes,
    ) -> Result<()> {
        let method = value["method"].as_str().unwrap_or("");
        let params = &value["params"];
        if method == "session/request_permission" && !value["id"].is_null() {
            if value["codeProjectionShortened"] == true {
                pipes.write(&json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32602,"message":"Permission details exceed the safe approval display budget"}}),cx)?;
                return Ok(());
            }
            ensure!(
                self.owns(&params["sessionId"]),
                "ACP agent requested permission for an unrelated session"
            );
            ensure!(
                self.pending.len() < 64,
                "Too many outstanding ACP permission requests"
            );
            let options: Vec<_> = params["options"]
                .as_array()
                .into_iter()
                .flatten()
                .take(32)
                .filter_map(|v| {
                    Some(NativeOption {
                        id: v["optionId"].as_str()?.into(),
                        label: v["name"].as_str()?.into(),
                    })
                })
                .collect();
            ensure!(!options.is_empty(), "ACP permission request has no options");
            let detail = approval_details(params);
            let reviewable =
                detail.is_some() && params["options"].as_array().is_some_and(|v| v.len() <= 32);
            let token = uuid::Uuid::new_v4().to_string();
            self.pending.insert(
                token.clone(),
                Permission {
                    wire_id: value["id"].clone(),
                    options: options.iter().map(|v| v.id.clone()).collect(),
                    reviewable,
                    extension: None,
                },
            );
            let detail = detail.unwrap_or_else(||"This native request is too large to review safely. Decline it and ask the agent for a smaller, specific action. No approval has been sent.".into());
            let request = NativeRequest {
                id: token,
                method: method.into(),
                title: params["toolCall"]["title"]
                    .as_str()
                    .unwrap_or("Agent tool permission")
                    .into(),
                detail,
                kind: if reviewable {
                    NativeRequestKind::Approval { choices: options }
                } else {
                    NativeRequestKind::Unsupported
                },
            };
            emit(
                parser.interaction(
                    ChatInteraction::NativeRequest(request.clone()),
                    "gemini.request",
                ),
                cx,
            )?;
            send(
                &cx.tx,
                RuntimeEvent::Request(Box::new(request)),
                &cx.cancelled,
            )?;
        } else if self.agent == AgentKind::Cursor && method.starts_with("cursor/") {
            // Cursor extensions omit sessionId in the documented schema. If
            // supplied, still require ownership of this one native session.
            if params.get("sessionId").is_some() && !self.owns(&params["sessionId"]) {
                if !value["id"].is_null() {
                    pipes.write(&json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32602,"message":"Unrelated native session"}}),cx)?;
                }
                return Ok(());
            }
            if matches!(method, "cursor/ask_question" | "cursor/create_plan")
                && !value["id"].is_null()
            {
                let request = if value["codeProjectionShortened"] != true && self.pending.len() < 64
                {
                    cursor_request(method, params, self.allow_edits).ok()
                } else {
                    None
                };
                let Some(request) = request else {
                    pipes.write(&json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32602,"message":"Native request cannot be reviewed safely"}}),cx)?;
                    return Ok(());
                };
                self.pending.insert(
                    request.id.clone(),
                    Permission {
                        wire_id: value["id"].clone(),
                        options: HashSet::new(),
                        reviewable: true,
                        extension: Some(request.clone()),
                    },
                );
                emit(
                    parser.interaction(
                        ChatInteraction::NativeRequest(request.clone()),
                        "cursor.request",
                    ),
                    cx,
                )?;
                send(
                    &cx.tx,
                    RuntimeEvent::Request(Box::new(request)),
                    &cx.cancelled,
                )?;
            } else if value["id"].is_null() {
                if !self.loading {
                    emit(
                        cursor_progress(method, params, parser, &mut self.cursor_todos),
                        cx,
                    )?;
                }
            } else {
                pipes.write(&json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Cursor client capability not implemented"}}),cx)?;
            }
        } else if method == "session/update"
            && (self.owns(&params["sessionId"])
                || self.id.is_empty()
                    && params["update"]["sessionUpdate"] == "available_commands_update")
        {
            let update = &params["update"];
            let kind = update["sessionUpdate"].as_str().unwrap_or("");
            // session/load replays the old native history. Code already has its
            // lazy imported transcript; do not duplicate that in the new turn.
            if self.loading && kind != "available_commands_update" {
                return Ok(());
            }
            if self.agent == AgentKind::Cursor
                && (params["sessionId"] != self.id
                    || matches!(kind, "subagent_spawned" | "subagent_state_update"))
            {
                emit(
                    self.child_update(params["sessionId"].as_str().unwrap_or(""), update, parser),
                    cx,
                )?;
                return Ok(());
            }
            if kind == "available_commands_update" {
                self.commands = update["availableCommands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v["name"].as_str().map(str::to_owned))
                    .take(256)
                    .collect();
            }
            let events = match kind {
                "agent_message_chunk" => {
                    parser.delta(update["content"]["text"].as_str().unwrap_or(""))
                }
                "agent_thought_chunk" => vec![RuntimeEvent::Status("Thinking".into())],
                "available_commands_update" => vec![RuntimeEvent::Commands(
                    update["availableCommands"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v["name"].as_str().map(str::to_owned))
                        .take(256)
                        .collect(),
                )],
                "tool_call" | "tool_call_update" => {
                    let id = update["toolCallId"].as_str().unwrap_or("tool");
                    parser.interaction(
                        ChatInteraction::Status {
                            title: update["title"].as_str().unwrap_or("Agent tool").into(),
                            details: tool_details(update),
                            status: match update["status"].as_str().unwrap_or("") {
                                "completed" => InteractionStatus::Completed,
                                "failed" => InteractionStatus::Failed,
                                "pending" => InteractionStatus::Started,
                                _ => InteractionStatus::Working,
                            },
                        },
                        &format!("acp.tool.{id}"),
                    )
                }
                "plan" => parser.interaction(
                    ChatInteraction::Status {
                        title: "Agent plan".into(),
                        details: update["entries"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .take(128)
                            .map(|v| {
                                format!(
                                    "{} · {}",
                                    v["status"].as_str().unwrap_or("pending"),
                                    v["content"].as_str().unwrap_or("")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n"),
                        status: InteractionStatus::Working,
                    },
                    "acp.plan",
                ),
                "current_model_update"|"config_option_update"=>{
                    let selection=crate::provider_defaults::acp_selection(update);
                    vec![RuntimeEvent::Usage(Box::new(crate::usage::Usage{model:selection.model,reasoning_effort:selection.effort,..Default::default()}))]
                },
                "current_mode_update" => vec![RuntimeEvent::Status(format!(
                    "Mode · {}",
                    update["currentModeId"].as_str().unwrap_or("default")
                ))],
                "usage_update" => acp_usage(update)
                    .map(|usage| vec![RuntimeEvent::Usage(Box::new(usage))])
                    .unwrap_or_default(),
                _ => vec![],
            };
            emit(events, cx)?;
        } else if value.get("method").is_some() && value.get("id").is_some() {
            // We advertise no client terminal/filesystem capabilities. Never
            // silently execute an unexpected filesystem/terminal request.
            pipes.write(&json!({"jsonrpc":"2.0","id":value["id"],"error":{"code":-32601,"message":"Client capability not advertised"}}),cx)?;
        }
        Ok(())
    }
    fn controls(&mut self, pipes: &mut Pipes, cx: &NativeContext) -> Result<()> {
        while let Ok(control) = cx.control.try_recv() {
            match control {
            NativeControl::Respond{id,answer}=>{
                if let Some(permission)=self.pending.get(&id) {
                    let result = if let Some(request) = &permission.extension {
                        match cursor_answer(request,&answer) {
                            Ok(result)=>result,
                            Err(error)=>{send(&cx.tx,RuntimeEvent::RequestRejected{id:id.clone(),error:error.to_string()},&cx.cancelled)?;continue;}
                        }
                    } else {
                    let outcome = match answer {
                        NativeAnswer::Choice(option) if permission.reviewable && permission.options.contains(&option) => json!({"outcome":"selected","optionId":option}),
                        NativeAnswer::Cancel => json!({"outcome":"cancelled"}),
                        _ => {send(&cx.tx,RuntimeEvent::RequestRejected{id:id.clone(),error:"Choose one of the options advertised by the agent.".into()},&cx.cancelled)?;continue;},
                    };
                    json!({"outcome":outcome})
                    };
                    pipes.write(&json!({"jsonrpc":"2.0","id":permission.wire_id,"result":result}), cx)?;
                    self.pending.remove(&id);send(&cx.tx,RuntimeEvent::RequestResolved(id),&cx.cancelled)?;
                }
            },
            NativeControl::Interrupt=>pipes.write(&json!({"jsonrpc":"2.0","method":"session/cancel","params":{"sessionId":self.id}}),cx)?,
            NativeControl::Steer(_)=>send(&cx.tx,RuntimeEvent::Status("This ACP agent accepts the next prompt after this turn completes".into()),&cx.cancelled)?,
            NativeControl::Rpc{method,params}=>{
                let allowed=match method.as_str(){"session/set_mode"=>params["modeId"].as_str().is_some_and(|id|self.modes.contains(id)),"session/set_model"=>params["modelId"].as_str().is_some_and(|id|self.models.contains(id)),_=>false};
                if allowed {let mut params=params;params["sessionId"]=json!(self.id);pipes.rpc(900, &method,params,cx)?;}
                else {send(&cx.tx,RuntimeEvent::Status("The agent does not advertise that session operation".into()),&cx.cancelled)?;}
            }
        }
        }
        Ok(())
    }
}
fn emit(events: Vec<RuntimeEvent>, cx: &NativeContext) -> Result<()> {
    for event in events {
        send(&cx.tx, event, &cx.cancelled)?;
    }
    Ok(())
}
fn wait(
    id: u64,
    pipes: &mut Pipes,
    session: &mut Session,
    child: &mut Child,
    parser: &mut StreamParser,
    cx: &NativeContext,
    deadline: Option<Instant>,
) -> Result<Value> {
    loop {
        ensure!(!cx.cancelled.load(Ordering::Acquire), "Stopped");
        if let Some(deadline) = deadline {
            ensure!(
                Instant::now() < deadline,
                "ACP agent did not answer its initialization request"
            );
        }
        session.controls(pipes, cx)?;
        if let Some(value) = pipes.next(cx)? {
            if value["id"] == id && value.get("method").is_none() {
                if let Some(error) = value.get("error") {
                    bail!(
                        "ACP agent: {}",
                        error["message"].as_str().unwrap_or("native request failed")
                    );
                }
                return Ok(value["result"].clone());
            }
            session.notification(&value, parser, cx, pipes)?;
        } else {
            if pipes.eof || super::child_exit_pending(child.id() as i32)? {
                bail!(
                    "ACP agent closed before completing the request. {}",
                    String::from_utf8_lossy(&pipes.stderr).trim()
                );
            }
            thread::sleep(Duration::from_millis(12));
        }
    }
}
fn run(
    request: &RunRequest,
    cli_command: bool,
    pipes: &mut Pipes,
    child: &mut Child,
    cx: &NativeContext,
    parser: &mut StreamParser,
) -> Result<()> {
    let mut session = Session::new();
    session.agent = request.agent;
    session.allow_edits = request.allow_edits;
    let deadline = Some(Instant::now() + Duration::from_secs(90));
    send(
        &cx.tx,
        RuntimeEvent::Status(format!("Connecting to {} ACP", request.agent.name())),
        &cx.cancelled,
    )?;
    let mut capabilities =
        json!({"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false});
    if request.agent == AgentKind::Cursor {
        capabilities["_meta"] = json!({"subagents":true});
    }
    pipes.rpc(1,"initialize",json!({"protocolVersion":1,"clientInfo":{"name":"Code","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":capabilities}),cx)?;
    let initialized = wait(1, pipes, &mut session, child, parser, cx, deadline)?;
    ensure!(
        initialized["protocolVersion"] == 1,
        "Installed agent uses an unsupported ACP version"
    );
    session.id = request.resume_id.clone().unwrap_or_default();
    session.loading = request.resume_id.is_some();
    if session.loading {
        ensure!(
            initialized["agentCapabilities"]["loadSession"] == true,
            "Installed agent does not advertise native session resume"
        );
    }
    let mut params = json!({"cwd":request.project,"mcpServers":[]});
    if session.loading {
        params["sessionId"] = json!(session.id);
    }
    pipes.rpc(
        2,
        if session.loading {
            "session/load"
        } else {
            "session/new"
        },
        params,
        cx,
    )?;
    let created = wait(2, pipes, &mut session, child, parser, cx, deadline)?;
    session.loading = false;
    if request.resume_id.is_none() {
        session.id = created["sessionId"]
            .as_str()
            .context("ACP agent did not return a session ID")?
            .into();
    }
    ensure!(!session.id.is_empty(), "ACP session ID is missing");
    pipes.session_id = Some(session.id.clone());
    send(
        &cx.tx,
        RuntimeEvent::SessionId(session.id.clone()),
        &cx.cancelled,
    )?;
    session.modes = created["modes"]["availableModes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| v["id"].as_str().map(str::to_owned))
        .collect();
    session.models = created["models"]["availableModels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            v["modelId"]
                .as_str()
                .or_else(|| v["id"].as_str())
                .map(str::to_owned)
        })
        .collect();
    let models = created["models"]["availableModels"]
        .as_array()
        .into_iter()
        .flatten()
        .take(256)
        .filter_map(|model| {
            let id = model["modelId"].as_str().or_else(|| model["id"].as_str())?;
            if !super::valid_model_id(id) {
                return None;
            }
            Some(crate::agent_controls::ModelChoice { reasoning_efforts:vec![],
                id: id.into(),
                label: model["name"]
                    .as_str()
                    .unwrap_or(id)
                    .chars()
                    .take(160)
                    .collect(),
            })
        })
        .collect::<Vec<_>>();
    if !models.is_empty() {
        send(&cx.tx, RuntimeEvent::Models(models), &cx.cancelled)?;
    }
    let selection=crate::provider_defaults::acp_selection(&created);
    if selection.model.is_some() || selection.effort.is_some(){send(&cx.tx,RuntimeEvent::Usage(Box::new(crate::usage::Usage{model:request.model.clone().or(selection.model),reasoning_effort:selection.effort,..Default::default()})),&cx.cancelled)?;}
    let desired = if request.allow_edits {
        if request.agent == AgentKind::Cursor {
            "agent"
        } else {
            "default"
        }
    } else {
        "plan"
    };
    ensure!(
        session.modes.contains(desired),
        "The agent does not advertise the requested {desired} policy; update or configure it"
    );
    pipes.rpc(
        3,
        "session/set_mode",
        json!({"sessionId":session.id,"modeId":desired}),
        cx,
    )?;
    wait(3, pipes, &mut session, child, parser, cx, deadline)?;
    if let Some(model) = &request.model {
        ensure!(
            session.models.contains(model),
            "The agent does not advertise model {model}"
        );
        pipes.rpc(
            4,
            "session/set_model",
            json!({"sessionId":session.id,"modelId":model}),
            cx,
        )?;
        wait(4, pipes, &mut session, child, parser, cx, deadline)?;
    }
    if cli_command {
        let name =
            crate::commands::name(&request.prompt).context("A native ACP command is required")?;
        let command_deadline = Instant::now() + Duration::from_secs(3);
        while session.commands.is_empty() && Instant::now() < command_deadline {
            if let Some(value) = pipes.next(cx)? {
                session.notification(&value, parser, cx, pipes)?;
            } else {
                if pipes.eof {
                    bail!("ACP agent closed before advertising its commands");
                }
                thread::sleep(Duration::from_millis(12));
            }
        }
        ensure!(
            session.commands.contains(name),
            "Installed agent does not advertise /{name}; no model prompt was sent"
        );
    }
    let mut prompt = vec![json!({"type":"text","text":request.prompt})];
    for image in &request.images {
        ensure!(
            initialized["agentCapabilities"]["promptCapabilities"]["image"] == true,
            "The agent does not advertise image prompts"
        );
        let bytes = std::fs::read(image)?;
        let mut encoded = vec![0; bytes.len().div_ceil(3) * 4];
        let n = super::base64_chunk(&bytes, &mut encoded);
        prompt.push(json!({"type":"image","mimeType":"image/png","data":std::str::from_utf8(&encoded[..n])?}));
    }
    parser.begin("acp-response");
    pipes.rpc(
        5,
        "session/prompt",
        json!({"sessionId":session.id,"prompt":prompt}),
        cx,
    )?;
    send(
        &cx.tx,
        RuntimeEvent::Status("Thinking".into()),
        &cx.cancelled,
    )?;
    let result = wait(5, pipes, &mut session, child, parser, cx, None)?;
    for id in session.pending.keys() {
        send(
            &cx.tx,
            RuntimeEvent::RequestResolved(id.clone()),
            &cx.cancelled,
        )?;
    }
    ensure!(
        result["stopReason"] != "refusal",
        "The agent declined this request"
    );
    if let Some(error) = parser.storage_error.take() {
        bail!("Could not save ACP output: {error}");
    }
    Ok(())
}
/// Cursor's blocking extensions are native protocol requests, not model prose.
/// Reject invalid/oversized shapes rather than approving a shortened display.
fn cursor_request(method: &str, params: &Value, allow_edits: bool) -> Result<NativeRequest> {
    let detail = approval_details(params).context("Native request exceeds display budget")?;
    let kind = match method {
        "cursor/ask_question" => {
            let wire_questions = params["questions"]
                .as_array()
                .context("Missing questions")?;
            ensure!(
                !wire_questions.is_empty() && wire_questions.len() <= 32,
                "Invalid question count"
            );
            let mut ids = HashSet::new();
            let mut questions = Vec::new();
            for question in wire_questions {
                let id = question["id"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 1024)
                    .context("Invalid question ID")?;
                ensure!(ids.insert(id), "Duplicate question ID");
                let prompt = question["prompt"]
                    .as_str()
                    .context("Missing question text")?;
                let wire_options = question["options"]
                    .as_array()
                    .context("Missing question options")?;
                ensure!(
                    !wire_options.is_empty() && wire_options.len() <= 32,
                    "Invalid option count"
                );
                let mut option_ids = HashSet::new();
                let mut options = Vec::new();
                for option in wire_options {
                    let id = option["id"]
                        .as_str()
                        .filter(|id| !id.is_empty() && id.len() <= 1024)
                        .context("Invalid option ID")?;
                    ensure!(option_ids.insert(id), "Duplicate option ID");
                    options.push(NativeOption {
                        id: id.into(),
                        label: option["label"]
                            .as_str()
                            .context("Missing option label")?
                            .into(),
                    });
                }
                questions.push(NativeQuestion {
                    id: id.into(),
                    header: "Question".into(),
                    question: prompt.into(),
                    options,
                    secret: false,
                    multiple: question["allowMultiple"] == true,
                    required: true,
                });
            }
            NativeRequestKind::Questions { questions }
        }
        "cursor/create_plan" => {
            ensure!(
                params["plan"]
                    .as_str()
                    .is_some_and(|plan| !plan.trim().is_empty()),
                "Missing plan"
            );
            let mut choices = Vec::new();
            // Accepting an implementation plan must not silently escape the
            // read-only policy selected for this invocation.
            if allow_edits {
                choices.push(NativeOption {
                    id: "accepted".into(),
                    label: "Approve plan".into(),
                });
            }
            choices.push(NativeOption {
                id: "rejected".into(),
                label: "Decline plan".into(),
            });
            NativeRequestKind::Approval { choices }
        }
        _ => bail!("Unsupported Cursor extension"),
    };
    Ok(NativeRequest {
        id: uuid::Uuid::new_v4().to_string(),
        method: method.into(),
        title: params["title"]
            .as_str()
            .or_else(|| params["name"].as_str())
            .unwrap_or(if method == "cursor/create_plan" {
                "Review agent plan"
            } else {
                "Agent question"
            })
            .into(),
        detail,
        kind,
    })
}
fn cursor_answer(request: &NativeRequest, answer: &NativeAnswer) -> Result<Value> {
    if matches!(answer, NativeAnswer::Cancel) {
        return Ok(json!({"outcome":{"outcome":"cancelled"}}));
    }
    match (&request.kind, answer) {
        (NativeRequestKind::Approval { choices }, NativeAnswer::Choice(choice)) => {
            ensure!(
                choices.iter().any(|option| option.id == *choice),
                "Choose an offered plan decision"
            );
            Ok(json!({"outcome":{"outcome":choice}}))
        }
        (NativeRequestKind::Questions { questions }, NativeAnswer::Answers(answers)) => {
            ensure!(
                answers.len() == questions.len(),
                "Answer every offered question"
            );
            let mut results = Vec::new();
            for question in questions {
                let selected = answers
                    .get(&question.id)
                    .context("Answer every offered question")?;
                let unique: HashSet<_> = selected.iter().collect();
                ensure!(
                    !selected.is_empty()
                        && unique.len() == selected.len()
                        && (question.multiple || selected.len() == 1)
                        && selected
                            .iter()
                            .all(|id| question.options.iter().any(|option| option.id == *id)),
                    "Choose valid options for each question"
                );
                results.push(json!({"questionId":question.id,"selectedOptionIds":selected}));
            }
            Ok(json!({"outcome":{"outcome":"answered","answers":results}}))
        }
        _ => bail!("This answer does not match the native request"),
    }
}
fn cursor_progress(
    method: &str,
    params: &Value,
    parser: &mut StreamParser,
    todos: &mut BTreeMap<String, (String, String)>,
) -> Vec<RuntimeEvent> {
    let id = params["toolCallId"].as_str().unwrap_or("progress");
    let preview = |value: &Value| {
        value
            .as_str()
            .unwrap_or("")
            .chars()
            .take(8192)
            .collect::<String>()
    };
    let card = match method {
        "cursor/task" => ChatInteraction::Delegation {
            title: params["description"]
                .as_str()
                .unwrap_or("Subagent task")
                .chars()
                .take(240)
                .collect(),
            details: preview(&params["prompt"]),
            status: if params["durationMs"].is_number() {
                InteractionStatus::Completed
            } else {
                InteractionStatus::Working
            },
            agent: params["agentId"].as_str().map(str::to_owned),
        },
        "cursor/update_todos" => {
            if params["merge"] != true {
                todos.clear();
            }
            for todo in params["todos"].as_array().into_iter().flatten().take(256) {
                if let Some(id) = todo["id"]
                    .as_str()
                    .filter(|id| !id.is_empty() && id.len() <= 1024)
                    && (todos.len() < 256 || todos.contains_key(id))
                {
                    todos.insert(
                        id.into(),
                        (preview(&todo["content"]), preview(&todo["status"])),
                    );
                }
            }
            // Bound both state and the projected card. A provider can update
            // one task without repeatedly retaining every historical snapshot.
            let mut details = String::new();
            for (content, status) in todos.values() {
                for ch in format!("{status} · {content}\n").chars() {
                    if details.len() + ch.len_utf8() > 64 * 1024 {
                        break;
                    }
                    details.push(ch);
                }
                if details.len() >= 64 * 1024 {
                    break;
                }
            }
            ChatInteraction::Status {
                title: "Agent task list".into(),
                details,
                status: if !todos.is_empty()
                    && todos
                        .values()
                        .all(|(_, status)| matches!(status.as_str(), "completed" | "cancelled"))
                {
                    InteractionStatus::Completed
                } else {
                    InteractionStatus::Working
                },
            }
        }
        "cursor/generate_image" => ChatInteraction::Status {
            title: "Agent image".into(),
            details: format!(
                "{}\n{}",
                preview(&params["description"]),
                preview(&params["filePath"])
            ),
            status: if params["filePath"].is_string() {
                InteractionStatus::Completed
            } else {
                InteractionStatus::Working
            },
        },
        _ => return Vec::new(),
    };
    parser.interaction(card, &format!("{method}.{id}"))
}
fn acp_usage(update: &Value) -> Option<crate::usage::Usage> {
    let used = update["used"]
        .as_u64()
        .filter(|n| *n <= 1_000_000_000_000)?;
    let size = update["size"]
        .as_u64()
        .filter(|n| *n > 0 && *n <= 1_000_000_000_000)?;
    Some(crate::usage::Usage {
        context_used: Some(used),
        context_limit: Some(size),
        scope: "Native session context".into(),
        // ACP cost is not necessarily dollars. Never relabel another currency.
        cost_usd: if update["cost"]["currency"] == "USD" {
            update["cost"]["amount"]
                .as_f64()
                .filter(|n| n.is_finite() && *n >= 0. && *n < 1_000_000_000.)
        } else {
            None
        },
        ..Default::default()
    })
}
fn approval_details(value: &Value) -> Option<String> {
    let text = serde_json::to_string_pretty(value).ok()?;
    (text.len() <= 64 * 1024).then_some(text)
}
fn tool_details(value: &Value) -> String {
    let mut details = String::new();
    for part in value["content"].as_array().into_iter().flatten().take(32) {
        if let Some(text) = part["content"]["text"].as_str() {
            details.push_str(text);
            details.push('\n');
        }
        if part["type"] == "diff" {
            details.push_str(part["path"].as_str().unwrap_or("File change"));
            details.push('\n');
            if let Some(text) = part["newText"].as_str() {
                details.push_str(text);
            }
        }
        if details.len() > 64 * 1024 {
            break;
        }
    }
    if details.is_empty()
        && let Some(input) = value.get("rawInput")
    {
        details = input.to_string();
    }
    let mut end = details.len().min(64 * 1024);
    while !details.is_char_boundary(end) {
        end -= 1;
    }
    details.truncate(end);
    details
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(source: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("code-acp-fixture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("gemini-fixture");
        std::fs::write(&path, format!("#!/usr/bin/python3\n{source}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (root, path)
    }
    #[test]
    fn native_acp_waits_for_and_delivers_permission_responses() {
        let (root, executable) = fixture(
            r#"
import sys,json

def emit(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line); method=v.get('method')
 if method=='initialize':
  assert v['params']['clientCapabilities']['terminal']==False
  emit({'jsonrpc':'2.0','id':v['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True}}})
 elif method=='session/new': emit({'jsonrpc':'2.0','id':v['id'],'result':{'sessionId':'gemini-session','modes':{'availableModes':[{'id':'default'}]}}})
 elif method=='session/set_mode':
  assert v['params']['modeId']=='default'
  emit({'jsonrpc':'2.0','id':v['id'],'result':{}})
 elif method=='session/prompt':
  emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'gemini-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'Before approval. '}}}})
  emit({'jsonrpc':'2.0','id':91,'method':'session/request_permission','params':{'sessionId':'gemini-session','toolCall':{'toolCallId':'call-1','title':'Run pwd','rawInput':{'command':'pwd'}},'options':[{'optionId':'yes-once','name':'Allow once','kind':'allow_once'},{'optionId':'no','name':'Decline','kind':'reject_once'}]}})
 elif v.get('id')==91:
  assert v['result']['outcome']=={'outcome':'selected','optionId':'yes-once'}
  emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'gemini-session','update':{'sessionUpdate':'tool_call_update','toolCallId':'call-1','title':'pwd','status':'completed','content':[{'type':'content','content':{'type':'text','text':'/fixture'}}]}}})
  emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'gemini-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'After approval.'}}}})
  emit({'jsonrpc':'2.0','id':5,'result':{'stopReason':'end_turn'}})
"#,
        );
        let request = RunRequest { reasoning_effort:None,
            agent: crate::discovery::AgentKind::Gemini,
            executable,
            project: root.clone(),
            prompt: "test permissions".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        let handle = launch(request).unwrap();
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut permission = false;
        let mut resolved = false;
        let mut text = String::new();
        loop {
            assert!(Instant::now() < deadline, "ACP fixture timed out");
            let event = match handle.events.recv_timeout(Duration::from_millis(250)) {
                Ok(event) => event,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(error) => panic!("ACP fixture disconnected: {error}"),
            };
            match event {
                RuntimeEvent::Request(request) => {
                    assert_eq!(request.method, "session/request_permission");
                    assert!(request.detail.contains("pwd"));
                    permission = true;
                    handle
                        .respond(&request.id, NativeAnswer::Choice("yes-once".into()))
                        .unwrap();
                }
                RuntimeEvent::RequestResolved(_) => resolved = true,
                RuntimeEvent::Text(delta) => text.push_str(&delta),
                RuntimeEvent::ReplaceText(replacement) => text = replacement,
                RuntimeEvent::Failed(error) => panic!("ACP fixture failed: {error}"),
                RuntimeEvent::Finished => break,
                _ => {}
            }
        }
        assert!(permission && resolved);
        let source = handle.take_response_source().unwrap();
        let rendered = source.read_range(0..source.len()).unwrap();
        assert!(
            rendered.contains("Before approval.")
                && rendered.contains("After approval.")
                && rendered.contains("/fixture")
        );
        assert!(rendered.contains("Run pwd"));
        assert!(handle.settled.recv_blocking().is_err());
        drop(handle);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn advertised_native_commands_dispatch_without_model_prompt_fallback() {
        let (root, executable) = fixture(
            r#"
import sys,json

def emit(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line);method=v.get('method')
 if method=='initialize':emit({'jsonrpc':'2.0','id':v['id'],'result':{'protocolVersion':1,'agentCapabilities':{}}})
 elif method=='session/new':
  emit({'jsonrpc':'2.0','id':v['id'],'result':{'sessionId':'gemini-command-session','modes':{'availableModes':[{'id':'default'}]}}})
  emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'gemini-command-session','update':{'sessionUpdate':'available_commands_update','availableCommands':[{'name':'fixture-command','description':'Native command'}]}}})
 elif method=='session/set_mode':emit({'jsonrpc':'2.0','id':v['id'],'result':{}})
 elif method=='session/prompt':
  assert v['params']['prompt']==[{'type':'text','text':'/fixture-command args'}]
  emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'gemini-command-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'Native command completed.'}}}})
  emit({'jsonrpc':'2.0','id':v['id'],'result':{'stopReason':'end_turn'}})
"#,
        );
        let request = RunRequest { reasoning_effort:None,
            agent: crate::discovery::AgentKind::Gemini,
            executable,
            project: root.clone(),
            prompt: "/fixture-command args".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        let handle = launch_command(request).unwrap();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                RuntimeEvent::Failed(error) => panic!("Gemini command fixture: {error}"),
                RuntimeEvent::Finished => break,
                _ => {}
            }
        }
        let source = handle.take_response_source().unwrap();
        assert!(
            source
                .read_range(0..source.len())
                .unwrap()
                .contains("Native command completed.")
        );
        assert!(handle.settled.recv_blocking().is_err());
        drop(handle);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn stop_sends_native_cancel_before_reaping_the_acp_process() {
        let (root, executable) = fixture(
            r#"
import sys,json

def emit(v):print(json.dumps(v),flush=True)
for line in sys.stdin:
 v=json.loads(line);method=v.get('method')
 if method=='initialize':emit({'jsonrpc':'2.0','id':v['id'],'result':{'protocolVersion':1,'agentCapabilities':{}}})
 elif method=='session/new':emit({'jsonrpc':'2.0','id':v['id'],'result':{'sessionId':'stop-session','modes':{'availableModes':[{'id':'default'}]}}})
 elif method=='session/set_mode':emit({'jsonrpc':'2.0','id':v['id'],'result':{}})
 elif method=='session/prompt':emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'stop-session','update':{'sessionUpdate':'agent_message_chunk','content':{'type':'text','text':'RUNNING'}}}})
 elif method=='session/cancel':
  assert v['params']['sessionId']=='stop-session'
  open('cancel-received','w').write('native cancel received before termination')
  emit({'jsonrpc':'2.0','id':5,'result':{'stopReason':'cancelled'}})
"#,
        );
        let request = RunRequest { reasoning_effort:None,
            agent: crate::discovery::AgentKind::Gemini,
            executable,
            project: root.clone(),
            prompt: "test native stop".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        let handle = launch(request).unwrap();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                RuntimeEvent::Text(text) if text.contains("RUNNING") => break,
                RuntimeEvent::Failed(error) => panic!("Stop fixture failed: {error}"),
                _ => {}
            }
        }
        let start = Instant::now();
        handle.cancel();
        assert!(handle.settled.recv_blocking().is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(root.join("cancel-received").is_file());
        drop(handle);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn oversized_approval_context_is_not_approvable() {
        let command = format!("{} ; rm -rf hidden-target", "echo harmless ".repeat(6000));
        assert!(approval_details(&json!({"toolCall":{"rawInput":{"command":command}}})).is_none());
        let readable=approval_details(&json!({"toolCall":{"rawInput":{"command":"pwd","unknownFutureSecurityField":"fully preserved"}}})).unwrap();
        assert!(
            readable.contains("unknownFutureSecurityField") && readable.contains("fully preserved")
        );
    }
    #[test]
    fn permission_ids_and_modes_are_provider_native() {
        let value = json!({"toolCall":{"title":"Run command","rawInput":{"command":"pwd"}},"options":[{"optionId":"allow-once","name":"Allow once","kind":"allow_once"},{"optionId":"deny","name":"Decline","kind":"reject_once"}]});
        assert!(tool_details(&value["toolCall"]).contains("pwd"));
        assert_eq!(value["options"][0]["optionId"], "allow-once");
    }
    #[test]
    fn tool_detail_clips_only_at_utf8_boundaries() {
        let text = "🙂".repeat(20000);
        let d = tool_details(&json!({"content":[{"content":{"text":text}}]}));
        assert!(d.len() <= 65536);
        assert!(d.is_char_boundary(d.len()));
    }
    #[test]
    fn cursor_questions_validate_every_id_and_preserve_multiple_answers() {
        use std::collections::BTreeMap;
        let request=cursor_request("cursor/ask_question",&json!({"questions":[{"id":"language","prompt":"Which languages?","allowMultiple":true,"options":[{"id":"rs","label":"Rust"},{"id":"sw","label":"Swift"}]}]}),true).unwrap();
        let answer = NativeAnswer::Answers(BTreeMap::from([(
            "language".into(),
            vec!["rs".into(), "sw".into()],
        )]));
        assert_eq!(
            cursor_answer(&request, &answer).unwrap(),
            json!({"outcome":{"outcome":"answered","answers":[{"questionId":"language","selectedOptionIds":["rs","sw"]}]}})
        );
        for invalid in [
            vec!["invented".into()],
            vec!["rs".into(), "rs".into()],
            vec![],
        ] {
            assert!(
                cursor_answer(
                    &request,
                    &NativeAnswer::Answers(BTreeMap::from([("language".into(), invalid)]))
                )
                .is_err()
            );
        }
        assert_eq!(
            cursor_answer(&request, &NativeAnswer::Cancel).unwrap(),
            json!({"outcome":{"outcome":"cancelled"}})
        );
        assert!(cursor_request("cursor/ask_question",&json!({"questions":[{"id":"q","prompt":"?","options":[{"id":"same","label":"one"},{"id":"same","label":"two"}]}]}),true).is_err());
    }
    #[test]
    fn cursor_plan_decisions_cannot_escape_read_only_or_approve_a_shortened_plan() {
        let plan = json!({"name":"Review changes","plan":"Run a linter"});
        let readonly = cursor_request("cursor/create_plan", &plan, false).unwrap();
        assert!(cursor_answer(&readonly, &NativeAnswer::Choice("accepted".into())).is_err());
        assert_eq!(
            cursor_answer(&readonly, &NativeAnswer::Choice("rejected".into())).unwrap(),
            json!({"outcome":{"outcome":"rejected"}})
        );
        let writable = cursor_request("cursor/create_plan", &plan, true).unwrap();
        assert_eq!(
            cursor_answer(&writable, &NativeAnswer::Choice("accepted".into())).unwrap(),
            json!({"outcome":{"outcome":"accepted"}})
        );
        assert!(
            cursor_request(
                "cursor/create_plan",
                &json!({"plan":"x".repeat(65537)}),
                true
            )
            .is_err()
        );
    }
    #[test]
    fn acp_usage_is_native_context_and_preserves_currency_semantics() {
        let usd = acp_usage(
            &json!({"used":53_000,"size":200_000,"cost":{"amount":0.045,"currency":"USD"}}),
        )
        .unwrap();
        assert_eq!(usd.context_used, Some(53_000));
        assert_eq!(usd.context_limit, Some(200_000));
        assert_eq!(usd.cost_usd, Some(0.045));
        assert!(usd.input.is_none() && usd.output.is_none());
        assert!(acp_usage(&json!({"used":null,"size":200_000})).is_none());
        assert!(acp_usage(&json!({"used":10,"size":0})).is_none());
        assert!(
            acp_usage(&json!({"used":10,"size":20,"cost":{"amount":5,"currency":"EUR"}}))
                .unwrap()
                .cost_usd
                .is_none()
        );
    }
    #[test]
    fn cursor_child_lineage_stays_bounded_and_child_prose_stays_out_of_parent() {
        let mut session = Session::new();
        session.agent = AgentKind::Cursor;
        session.id = "root".into();
        let mut parser = StreamParser::default();
        parser.begin("parent");
        assert!(!session.owns(&json!("unknown-child")));
        session.child_update("root",&json!({"sessionUpdate":"subagent_spawned","subagentSessionId":"child","name":"Research","task":"Inspect"}),&mut parser);
        assert!(session.owns(&json!("child")));
        let events = session.child_update(
            "child",
            &json!({"sessionUpdate":"agent_message_chunk","content":{"text":"Child result"}}),
            &mut parser,
        );
        assert!(
            events.iter().all(|event| !matches!(
                event,
                RuntimeEvent::Text(_) | RuntimeEvent::ReplaceText(_)
            ))
        );
        assert!(events.iter().any(|event|matches!(event,RuntimeEvent::Interaction(card) if matches!(&**card,ChatInteraction::Delegation{details,..} if details.contains("Child result")))));
        for index in 0..100 {
            session.child_update("root",&json!({"sessionUpdate":"subagent_spawned","subagentSessionId":format!("child-{index}")}),&mut parser);
        }
        assert_eq!(session.children.len(), 64);
        assert!(!session.owns(&json!("child-99")));
        let mut todos = BTreeMap::new();
        cursor_progress(
            "cursor/update_todos",
            &json!({"merge":false,"todos":[{"id":"one","content":"First","status":"completed"}]}),
            &mut parser,
            &mut todos,
        );
        let events = cursor_progress(
            "cursor/update_todos",
            &json!({"merge":true,"todos":[{"id":"two","content":"Second","status":"pending"}]}),
            &mut parser,
            &mut todos,
        );
        assert_eq!(todos.len(), 2);
        assert!(events.iter().any(|event|matches!(event,RuntimeEvent::Interaction(card) if matches!(&**card,ChatInteraction::Status{details,..} if details.contains("First") && details.contains("Second")))));
    }
    #[test]
    fn cursor_native_resume_routes_exact_tool_question_plan_and_child_responses() {
        let (root, executable) = fixture(
            r#"
import sys,json
assert sys.argv[1:]==['acp'],sys.argv
def emit(v): print(json.dumps(v),flush=True)
def update(v):emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'cursor-session','update':v}})
for line in sys.stdin:
 v=json.loads(line);m=v.get('method')
 if m=='initialize':
  assert v['params']['clientCapabilities']['terminal']==False
  assert v['params']['clientCapabilities']['_meta']['subagents']==True
  emit({'jsonrpc':'2.0','id':v['id'],'result':{'protocolVersion':1,'agentCapabilities':{'loadSession':True}}})
 elif m=='session/load':
  assert v['params']['sessionId']=='cursor-session'
  update({'sessionUpdate':'available_commands_update','availableCommands':[{'name':'inspect'}]})
  update({'sessionUpdate':'agent_message_chunk','content':{'text':'OLD HISTORY MUST NOT REPLAY'}})
  emit({'jsonrpc':'2.0','id':v['id'],'result':{'modes':{'availableModes':[{'id':'agent'},{'id':'plan'}]},'models':{'availableModels':[{'modelId':'native-model','name':'Native model'}]}}})
 elif m=='session/set_mode':
  assert v['params']['modeId']=='plan'
  emit({'jsonrpc':'2.0','id':v['id'],'result':{}})
 elif m=='session/set_model':
  assert v['params']['modelId']=='native-model'
  emit({'jsonrpc':'2.0','id':v['id'],'result':{}})
 elif m=='session/prompt':
  assert v['params']['prompt']==[{'type':'text','text':'/inspect'}]
  emit({'jsonrpc':'2.0','id':71,'method':'session/request_permission','params':{'sessionId':'cursor-session','toolCall':{'toolCallId':'tool','title':'Run pwd','rawInput':{'command':'pwd'}},'options':[{'optionId':'reject-once','name':'Decline','kind':'reject_once'}]}})
 elif v.get('id')==71:
  assert v['result']=={'outcome':{'outcome':'selected','optionId':'reject-once'}}
  emit({'jsonrpc':'2.0','id':72,'method':'cursor/ask_question','params':{'title':'Choose','questions':[{'id':'language','prompt':'Which language?','options':[{'id':'rust','label':'Rust'}]}]}})
 elif v.get('id')==72:
  assert v['result']=={'outcome':{'outcome':'answered','answers':[{'questionId':'language','selectedOptionIds':['rust']}]}}
  emit({'jsonrpc':'2.0','id':73,'method':'cursor/create_plan','params':{'name':'Implement','plan':'Use Rust'}})
 elif v.get('id')==73:
  assert v['result']=={'outcome':{'outcome':'rejected'}}
  emit({'jsonrpc':'2.0','id':74,'method':'fs/read_text_file','params':{'sessionId':'cursor-session','path':'sensitive'}})
 elif v.get('id')==74:
  assert v['error']['code']==-32601
  update({'sessionUpdate':'subagent_spawned','subagentSessionId':'child-id','name':'Research','task':'Inspect safely'})
  emit({'jsonrpc':'2.0','method':'session/update','params':{'sessionId':'child-id','update':{'sessionUpdate':'agent_message_chunk','content':{'text':'CHILD_OUTPUT'}}}})
  emit({'jsonrpc':'2.0','id':75,'method':'session/request_permission','params':{'sessionId':'child-id','toolCall':{'toolCallId':'child-tool','title':'Child command','rawInput':{'command':'pwd'}},'options':[{'optionId':'reject-once','name':'Decline','kind':'reject_once'}]}})
 elif v.get('id')==75:
  assert v['result']=={'outcome':{'outcome':'selected','optionId':'reject-once'}}
  update({'sessionUpdate':'subagent_state_update','subagentSessionId':'child-id','state':'completed'})
  emit({'jsonrpc':'2.0','method':'cursor/update_todos','params':{'toolCallId':'list','todos':[{'id':'1','content':'Research','status':'completed'}],'merge':False}})
  emit({'jsonrpc':'2.0','method':'cursor/generate_image','params':{'toolCallId':'image','description':'Asset','filePath':'/tmp/generated.png'}})
  update({'sessionUpdate':'usage_update','used':100,'size':2000,'cost':{'amount':0.01,'currency':'USD'}})
  update({'sessionUpdate':'agent_message_chunk','content':{'text':'CURSOR_NATIVE_OK'}})
  emit({'jsonrpc':'2.0','id':5,'result':{'stopReason':'end_turn'}})
"#,
        );
        let handle = super::super::launch_cli_command(RunRequest { reasoning_effort:None,
            agent: AgentKind::Cursor,
            executable,
            project: root.clone(),
            prompt: "/inspect".into(),
            resume_id: Some("cursor-session".into()),
            fork_session: false,
            model: Some("native-model".into()),
            images: vec![],
            allow_edits: false,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        })
        .unwrap();
        let mut requests = 0;
        let mut resolved = 0;
        let mut child = false;
        let mut catalog = false;
        let mut usage = false;
        let mut models = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(15)).unwrap() {
                RuntimeEvent::Request(request) => {
                    requests += 1;
                    let answer = match request.method.as_str() {
                        "session/request_permission" => NativeAnswer::Choice("reject-once".into()),
                        "cursor/ask_question" => NativeAnswer::Answers(BTreeMap::from([(
                            "language".into(),
                            vec!["rust".into()],
                        )])),
                        "cursor/create_plan" => NativeAnswer::Choice("rejected".into()),
                        method => panic!("Unexpected native request {method}"),
                    };
                    handle.respond(&request.id, answer).unwrap();
                }
                RuntimeEvent::RequestResolved(_) => resolved += 1,
                RuntimeEvent::Commands(commands) => catalog = commands == ["inspect"],
                RuntimeEvent::Models(choices) => {
                    models = choices.len() == 1 && choices[0].id == "native-model"
                }
                RuntimeEvent::Usage(report) => {
                    usage = report.context_used == Some(100) && report.cost_usd == Some(0.01)
                }
                RuntimeEvent::Text(text) => assert!(!text.contains("CHILD_OUTPUT")),
                RuntimeEvent::Interaction(card) if matches!(&*card,ChatInteraction::Delegation{agent:Some(id),status:InteractionStatus::Completed,..} if id=="child-id") => {
                    child = true
                }
                RuntimeEvent::Failed(error) => panic!("Cursor ACP fixture: {error}"),
                RuntimeEvent::Finished => break,
                _ => {}
            }
        }
        assert_eq!((requests, resolved), (4, 4));
        assert!(child && catalog && usage && models);
        let source = handle.take_response_source().unwrap();
        let text = source.read_range(0..source.len()).unwrap();
        assert!(text.contains("CURSOR_NATIVE_OK") && !text.contains("OLD HISTORY MUST NOT REPLAY"));
        assert!(
            text.contains("/tmp/generated.png")
                && text.contains("Agent task list")
                && text.contains("CHILD_OUTPUT")
        );
        assert!(handle.settled.recv_blocking().is_err());
        drop(handle);
        std::fs::remove_dir_all(root).unwrap();
    }
}
