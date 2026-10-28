//! Native Codex App Server JSON-RPC transport. Checked against the installed
//! 0.162.1 generated schema and https://learn.chatgpt.com/docs/app-server.
//! JSONL stays bidirectional until the authoritative turn completion. Approvals
//! are scoped to an opaque per-run token and never inferred from prose.
use super::*;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

struct Pending {
    wire_id: Value,
    method: String,
    params: Value,
    ui: NativeRequest,
}
struct Session {
    thread: String,
    turn: String,
    next_id: u64,
    pending: HashMap<String, Pending>,
    calls: HashMap<u64, String>,
    parser: StreamParser,
    complete: bool,
    maintenance: Option<(String, Value)>,
    command_process: String,
    terminal_utf8: BTreeMap<String, Vec<u8>>,
    file_changes: HashMap<String, Option<String>>,
    file_order: std::collections::VecDeque<String>,
    stream_sequence: u64,
    reasoning: BTreeMap<String,BTreeMap<u64,String>>,
    request_reasoning:bool,
    tool_streamed: std::collections::HashSet<String>,
    service: Option<Arc<broker::Service>>,
    voice: Option<VoiceState>,
}
#[derive(Default)]
struct VoiceState {
    id: String,
    canonical: bool,
    closed: bool,
    coding: bool,
    preparing_context:bool,
    segments: HashMap<String, (String, ResponseBuffer)>,
    flat_ids: HashMap<String,String>,
    final_ids: std::collections::HashSet<String>,
    final_order: std::collections::VecDeque<String>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            thread: String::new(),
            turn: String::new(),
            next_id: 10,
            pending: HashMap::new(),
            calls: HashMap::new(),
            parser: StreamParser::default(),
            complete: false,
            maintenance: None,
            command_process: String::new(),
            terminal_utf8: BTreeMap::new(),
            file_changes: HashMap::new(),
            file_order: std::collections::VecDeque::new(),
            stream_sequence: 0,
            reasoning:BTreeMap::new(),
            request_reasoning:false,
            tool_streamed: std::collections::HashSet::new(),
            service: None,
            voice: None,
        }
    }
}

pub(super) fn launch(request: RunRequest) -> Result<RunHandle> {
    launch_mode(request, None)
}
pub(super) fn maintenance(request: RunRequest, method: &str, params: Value) -> Result<RunHandle> {
    anyhow::ensure!(
        !request.fork_session || method == "thread/realtime/start",
        "A native maintenance action cannot fork a chat"
    );
    anyhow::ensure!(
        matches!(
            method,
            "thread/compact/start"
                | "command/exec"
                | "review/start"
                | "thread/goal/get"
                | "thread/goal/set"
                | "thread/goal/clear"
                | "thread/realtime/start"
        ),
        "This native maintenance action is not exposed"
    );
    if method == "thread/realtime/start" {
        crate::voice::session_id(&params)?;
    }
    if !matches!(method, "command/exec" | "thread/realtime/start") {
        anyhow::ensure!(
            request.resume_id.is_some(),
            "Choose a Code-owned Codex chat first"
        );
    } else if method == "command/exec" {
        anyhow::ensure!(
            !request.prompt.trim().is_empty()
                && request.prompt.len() <= PROMPT_LIMIT
                && !request.prompt.contains('\0'),
            "Enter a valid terminal command"
        );
    }
    launch_mode(request, Some((method.into(), params)))
}

fn launch_mode(request: RunRequest, maintenance: Option<(String, Value)>) -> Result<RunHandle> {
    validate_images(&request)?;
    if let Some(model) = &request.model {
        anyhow::ensure!(valid_model_id(model), "Invalid agent model ID");
    }
    if let Some(id) = &request.resume_id {
        anyhow::ensure!(
            uuid::Uuid::parse_str(id).is_ok(),
            "Invalid Codex session ID"
        );
    }
    anyhow::ensure!(
        !request.fork_session || request.resume_id.is_some(),
        "A Codex source session is required to continue in a new chat"
    );
    if let Some(service) = broker::checkout(&request)? {
        let (handle, context) = native_handle(service.pid);
        service.attach(&context)?;
        if let Err(error) = service.commands.try_send(broker::Command::Turn(Box::new((
            request,
            context,
            maintenance,
        )))) {
            if let broker::Command::Turn(turn) = match error {
                TrySendError::Full(command) | TrySendError::Disconnected(command) => command,
            } {
                let (_, context, _) = *turn;
                *context.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            service.release();
            bail!("Codex service did not receive this prompt; retry explicitly");
        }
        service.notify();
        return Ok(handle);
    }
    let mut command = Command::new(&request.executable);
    crate::process_env::apply(&mut command);
    command
        .args(["app-server", "--listen", "stdio://"])
        // This app's first-party Codex route uses HTTPS/SSE directly. Realtime
        // voice still uses its independent native WebRTC transport.
        .args(["-c","model_providers.kai-openai-https.name=\"OpenAI\"",
               "-c","model_providers.kai-openai-https.wire_api=\"responses\"",
               "-c","model_providers.kai-openai-https.requires_openai_auth=true",
               "-c","model_providers.kai-openai-https.supports_websockets=false"])
        .current_dir(&request.project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .context("Could not start Codex App Server")?;
    let (mut handle, context) = native_handle(child.id() as i32);
    let stdout = child.stdout.take().context("Codex stdout unavailable")?;
    let stderr = child.stderr.take().context("Codex stderr unavailable")?;
    let stdin = child.stdin.take().context("Codex stdin unavailable")?;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd(), stdin.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let error = io::Error::last_os_error();
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            *context.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(error).context("Could not configure Codex transport");
        }
    }
    handle.deferred_native_stop = true;
    let (service, commands, wake) = match broker::register(&request, child.id() as i32) {
        Ok(registered) => registered,
        Err(error) => {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            *context.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(error);
        }
    };
    if let Err(error) = service.attach(&context) {
        unsafe {
            libc::kill(-(child.id() as i32), libc::SIGKILL);
        }
        let _ = child.wait();
        service.disconnected();
        return Err(error);
    }
    thread::spawn(move || {
        service_loop(
            request,
            child,
            stdin,
            stdout,
            stderr,
            context,
            maintenance,
            service,
            commands,
            wake,
        )
    });
    Ok(handle)
}

fn write_message(
    stdin: &mut std::process::ChildStdin,
    cancelled: &AtomicBool,
    value: &Value,
) -> Result<()> {
    let done = AtomicBool::new(false);
    let mut writer = InputPipe {
        stdin,
        done: &done,
        cancelled,
    };
    serde_json::to_writer(&mut writer, value)?;
    writer.write_all(b"\n")?;
    Ok(())
}
fn rpc(
    stdin: &mut std::process::ChildStdin,
    cancelled: &AtomicBool,
    id: u64,
    method: &str,
    params: Value,
) -> Result<()> {
    write_message(
        stdin,
        cancelled,
        &json!({"id":id,"method":method,"params":params}),
    )
}
fn input(request: &RunRequest) -> Value {
    let mut values = vec![json!({"type":"text","text":request.prompt,"text_elements":[]})];
    values.extend(
        request
            .images
            .iter()
            .map(|path| json!({"type":"localImage","path":path})),
    );
    Value::Array(values)
}
fn turn_params(thread:&str,request:&RunRequest)->Value{json!({"threadId":thread,"input":input(request),"approvalPolicy":request.codex_approvals.policy(),"approvalsReviewer":request.codex_approvals.reviewer(),"effort":request.reasoning_effort,"summary":"detailed","sandboxPolicy":request.codex_access.policy(request.allow_edits,&request.project)})}
fn thread_params(request: &RunRequest) -> Value {
    let mut params = json!({"cwd":request.project,
        "approvalPolicy":request.codex_approvals.policy(),
        "approvalsReviewer":request.codex_approvals.reviewer(),
        "sandbox":request.codex_access.sandbox(request.allow_edits),
        "config":{"features.default_mode_request_user_input":true}});

    if let Some(effort)=&request.reasoning_effort{params["config"]["model_reasoning_effort"]=json!(effort);}
    if let Some(model) = &request.model {
        params["model"] = json!(model);
    }
    if let Some(id) = &request.resume_id {
        params["threadId"] = json!(id);
        params["excludeTurns"] = json!(true);
        if request.fork_session {
            params["ephemeral"] = json!(false);
        }
    }
    params
}

// Read Codex's effective per-project configuration before choosing transport.
// Custom providers retain their native route; only built-in OpenAI uses our
// first-party HTTPS profile. No user configuration or credentials are changed.
fn routed_thread_params(request:&RunRequest,config:&Value)->Value{
    let mut params=thread_params(request);
    if config["model_provider"].as_str().is_none_or(|p|p=="openai"){
        params["modelProvider"]=json!("kai-openai-https");
        params["config"]["model_reasoning_summary"]=json!("detailed");
    }
    params
}

#[allow(clippy::too_many_arguments)]
fn service_loop(
    mut request: RunRequest,
    mut child: std::process::Child,
    mut stdin: std::process::ChildStdin,
    mut stdout: std::process::ChildStdout,
    mut stderr: std::process::ChildStderr,
    context: NativeContext,
    mut maintenance: Option<(String, Value)>,
    service: Arc<broker::Service>,
    commands: Receiver<broker::Command>,
    mut wake: std::os::unix::net::UnixStream,
) {
    let mut context = context;
    let mut initialized = false;
    let mut pending = HashMap::new();
    let mut record = wire::Record::default();
    'service: loop {
        let (healthy, mut state) = run_turn(
            &request,
            &mut child,
            &mut stdin,
            &mut stdout,
            &mut stderr,
            context,
            maintenance,
            initialized,
            &service,
            &commands,
            pending,
            &mut record,
        );
        if !healthy {
            return;
        }
        initialized = true;
        let (idle_handle, idle_ctx) = native_handle(child.id() as i32);
        *idle_ctx
            .group
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        let mut buffer = [0u8; 8192];
        loop {
            if service.shutdown.load(Ordering::Acquire) || service.idle_expired() {
                if service.has_running_children() || !state.pending.is_empty() {
                    graceful_stop(&state, &mut stdin, &mut stdout);
                }
                break 'service;
            }
            while wake.read(&mut buffer).is_ok_and(|count| count > 0) {}
            for _ in 0..32 {
                match commands.try_recv() {
                    Ok(broker::Command::Turn(turn)) => {
                        let (next, next_ctx, next_maintenance) = *turn;
                        request = next;
                        context = next_ctx;
                        maintenance = next_maintenance;
                        pending = std::mem::take(&mut state.pending);
                        continue 'service;
                    }
                    Ok(broker::Command::Child(id, action)) => {
                        let token = if let broker::ChildAction::Respond { id, .. } = &action {
                            Some(id.clone())
                        } else {
                            None
                        };
                        if let Err(error) = state.child_control(&id, action, &mut stdin, &idle_ctx)
                        {
                            service.child_error(&id, &error.to_string());
                            if let Some(token) = token {
                                service.reject(&id, &token, &error.to_string());
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
            for _ in 0..8 {
                match stdout.read(&mut buffer) {
                    Ok(0) => break 'service,
                    Ok(count) => {
                        for chunk in buffer[..count].split_inclusive(|byte| *byte == b'\n') {
                            let complete = chunk.last() == Some(&b'\n');
                            let bytes = if complete {
                                &chunk[..chunk.len() - 1]
                            } else {
                                chunk
                            };
                            if record.push(bytes).is_err() {
                                break 'service;
                            }
                            if complete && !record.is_empty() {
                                let value = if let Some(bytes) = record.small() {
                                    serde_json::from_slice(bytes)
                                        .map(|value| (value, false))
                                        .map_err(anyhow::Error::from)
                                } else {
                                    record.projected(&idle_ctx.cancelled)
                                };
                                record.clear();
                                let Ok((mut value, shortened)) = value else {
                                    break 'service;
                                };
                                if retain_streamed_agent_snapshot(
                                    &mut value, shortened, &state, &service,
                                )
                                .is_err()
                                {
                                    break 'service;
                                }
                                if state
                                    .message(
                                        &request,
                                        value,
                                        &mut stdin,
                                        &idle_ctx,
                                        &mut initialized,
                                    )
                                    .is_err()
                                {
                                    break 'service;
                                }
                                while idle_handle.events.try_recv().is_ok() {}
                            }
                        }
                    }
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => break 'service,
                }
            }
            // Quiet providers block in poll; control writes wake this descriptor.
            // Both pipes are drained so a warning burst cannot pin a retained CLI.
            for _ in 0..8 {
                match stderr.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            if child_exit_pending(child.id() as i32).unwrap_or(true) {
                break 'service;
            }
            let mut fds = [
                libc::pollfd {
                    fd: stdout.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: stderr.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: wake.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            unsafe {
                libc::poll(
                    fds.as_mut_ptr(),
                    fds.len() as libc::nfds_t,
                    service.idle_wait_ms(),
                );
            }
        }
    }
    service.begin_cleanup();
    // PID remains reserved until all owned descendants have been terminated.
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
    service.disconnected();
}
/// The wire projector is a bounded preview. Preserve complete, already
/// streamed text when its final snapshot was shortened, including after the
/// parent has finished and only background child work remains.
fn retain_streamed_agent_snapshot(
    value: &mut Value,
    shortened: bool,
    state: &Session,
    service: &broker::Service,
) -> Result<()> {
    if shortened
        && value["method"] == "item/completed"
        && value["params"]["item"]["type"] == "agentMessage"
    {
        let item = value["params"]["item"]["id"].as_str().unwrap_or("");
        let thread = value["params"]["threadId"].as_str();
        let child_streamed = thread
            .is_some_and(|thread| thread != state.thread && service.has_item_output(thread, item));
        let parent_streamed = thread.is_none_or(|thread| thread == state.thread)
            && state.parser.current_text_len > 0
            && item == state.parser.current_id;
        if child_streamed || parent_streamed {
            value["params"]["item"]["text"] = Value::Null;
        } else {
            bail!(
                "Codex delivered a final-only message too large for its bounded event preview. The complete output remains in the native Codex transcript; Code did not mark this preview as complete."
            );
        }
    }
    Ok(())
}
fn process_child_controls(
    state: &mut Session,
    commands: &Receiver<broker::Command>,
    stdin: &mut std::process::ChildStdin,
    ctx: &NativeContext,
) {
    for _ in 0..32 {
        match commands.try_recv() {
            Ok(broker::Command::Child(id, action)) => {
                let token = if let broker::ChildAction::Respond { id, .. } = &action {
                    Some(id.clone())
                } else {
                    None
                };
                if let Err(error) = state.child_control(&id, action, stdin, ctx)
                    && let Some(service) = &state.service
                {
                    service.child_error(&id, &error.to_string());
                    if let Some(token) = token {
                        service.reject(&id, &token, &error.to_string());
                    }
                }
            }
            Ok(broker::Command::Turn(turn)) => {
                let (_, context, _) = *turn;
                *context
                    .group
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = None;
                let _ = context.tx.try_send(RuntimeEvent::Failed(
                    "This native chat already has an active turn; the prompt was not dispatched"
                        .into(),
                ));
            }
            Err(_) => break,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_turn(
    request: &RunRequest,
    child: &mut std::process::Child,
    stdin: &mut std::process::ChildStdin,
    stdout: &mut std::process::ChildStdout,
    stderr: &mut std::process::ChildStderr,
    ctx: NativeContext,
    maintenance: Option<(String, Value)>,
    initialized_before: bool,
    service: &Arc<broker::Service>,
    commands: &Receiver<broker::Command>,
    pending: HashMap<String, Pending>,
    record: &mut wire::Record,
) -> (bool, Session) {
    let mut state = Session {
        maintenance,
        service: Some(service.clone()),
        pending,
        ..Session::default()
    };
    if let Some((method, params)) = &mut state.maintenance
        && method == "thread/realtime/start"
    {
        let prepare=params.as_object_mut().and_then(|p|p.remove("_codePrepareVoiceContext")).and_then(|v|v.as_bool()).unwrap_or(false);
        state.voice = crate::voice::session_id(params).ok().map(|id| VoiceState {
            id,preparing_context:prepare,
            ..Default::default()
        });
    }
    state.parser.fork_source_id = request
        .fork_session
        .then(|| request.resume_id.clone())
        .flatten();
    let mut tail = Vec::new();
    let mut buffer = [0u8; 8192];
    let mut failure = if initialized_before {
        rpc(stdin, &ctx.cancelled, 0, "config/read", json!({"cwd":request.project,"includeLayers":false}))
    } else {
        rpc(
            stdin,
            &ctx.cancelled,
            1,
            "initialize",
            json!({
            "clientInfo":{"name":"code","title":"Code","version":env!("CARGO_PKG_VERSION")},
            "capabilities":{"experimentalApi":true}}),
        )
    }
    .err()
    .map(|error| error.to_string());
    let started = Instant::now();
    let mut initialized = initialized_before;
    let _ = send(
        &ctx.tx,
        RuntimeEvent::Status("Connecting to Codex".into()),
        &ctx.cancelled,
    );
    'running: while failure.is_none() && !state.complete && !ctx.cancelled.load(Ordering::Acquire) {
        process_child_controls(&mut state, commands, stdin, &ctx);
        if service.shutdown.load(Ordering::Acquire) {
            ctx.cancelled.store(true, Ordering::Release);
            break;
        }
        for _ in 0..32 {
            match ctx.control.try_recv() {
                Ok(control) => {
                    let request = if let NativeControl::Respond { id, .. } = &control {
                        Some(id.clone())
                    } else {
                        None
                    };
                    if let Err(error) = state.control(control, stdin, &ctx) {
                        let event = if let Some(id) = request {
                            RuntimeEvent::RequestRejected {
                                id,
                                error: error.to_string(),
                            }
                        } else {
                            RuntimeEvent::Status(error.to_string())
                        };
                        let _ = send(&ctx.tx, event, &ctx.cancelled);
                    }
                }
                Err(_) => break,
            }
        }
        for _ in 0..8 {
            match stdout.read(&mut buffer) {
                Ok(0) => {
                    failure = Some("Codex App Server closed before the turn completed".into());
                    break 'running;
                }
                Ok(n) => {
                    for chunk in buffer[..n].split_inclusive(|b| *b == b'\n') {
                        let complete = chunk.last() == Some(&b'\n');
                        let bytes = if complete {
                            &chunk[..chunk.len() - 1]
                        } else {
                            chunk
                        };
                        let result = (|| -> Result<()> {
                            record.push(bytes)?;
                            if complete && !record.is_empty() {
                                let (mut value, shortened) = if let Some(bytes) = record.small() {
                                    (serde_json::from_slice::<Value>(bytes)?, false)
                                } else {
                                    record.projected(&ctx.cancelled)?
                                };
                                if shortened
                                    && !value["id"].is_null()
                                    && value["method"].is_string()
                                {
                                    write_message(
                                        stdin,
                                        &ctx.cancelled,
                                        &json!({"id":value["id"],"error":{"code":-32602,"message":"This interactive request is too large to display completely"}}),
                                    )?;
                                    record.clear();
                                    return Ok(());
                                }
                                retain_streamed_agent_snapshot(
                                    &mut value, shortened, &state, service,
                                )?;
                                if shortened
                                    && value["method"] == "item/completed"
                                    && value["params"]["item"]["type"] == "commandExecution"
                                    && !state.tool_streamed.contains(
                                        value["params"]["item"]["id"].as_str().unwrap_or(""),
                                    )
                                {
                                    bail!(
                                        "Codex delivered oversized final-only tool output. Its complete output remains in the native transcript; Code did not mark this preview as complete."
                                    );
                                }
                                if shortened
                                    && value["method"] == "item/completed"
                                    && value["params"]["item"]["type"] == "exitedReviewMode"
                                {
                                    bail!(
                                        "Codex delivered a review larger than its bounded wire preview. Its complete findings remain in the native transcript; Code did not mark a partial review as complete."
                                    );
                                }
                                if shortened
                                    && state.voice.is_some()
                                    && matches!(
                                        value["method"].as_str(),
                                        Some(
                                            "thread/realtime/item/completed"
                                                | "thread/realtime/transcript/done"
                                        )
                                    )
                                {
                                    bail!(
                                        "Codex delivered an oversized spoken transcript snapshot. Its complete source remains in the native transcript; Code did not mark this preview as complete."
                                    );
                                }
                                state.message(request, value, stdin, &ctx, &mut initialized)?;
                                record.clear();
                            }
                            Ok(())
                        })();
                        if let Err(e) = result {
                            failure = Some(e.to_string());
                            break 'running;
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    failure = Some(e.to_string());
                    break 'running;
                }
            }
        }
        for _ in 0..8 {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    tail.extend_from_slice(&buffer[..n]);
                    if tail.len() > STDERR_LIMIT {
                        tail.drain(..tail.len() - STDERR_LIMIT);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if started.elapsed()>Duration::from_secs(30) && (!initialized || state.launch_pending()) {
            failure = Some("Codex did not acknowledge the pending launch within 30 seconds".into());
            break;
        }
        if state.complete {
            break;
        }
        if child_exit_pending(child.id() as i32).unwrap_or(false) {
            failure = Some(format!(
                "Codex App Server exited before completion. {}",
                String::from_utf8_lossy(&tail).trim()
            ));
            break;
        }
        let mut pipes = [
            libc::pollfd {
                fd: stdout.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stderr.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        unsafe {
            libc::poll(pipes.as_mut_ptr(), 2, 50);
        }
    }
    if ctx.cancelled.load(Ordering::Acquire) || (failure.is_some() && state.voice.is_some()) {
        graceful_stop(&state, stdin, stdout);
    }
    // Publish request retirement before terminal state; late UI clicks can no
    // longer authorize a different request or a future run.
    let retire: Vec<_> = state
        .pending
        .iter()
        .filter(|(_, pending)| {
            pending.params["threadId"]
                .as_str()
                .is_none_or(|id| !service.child(id))
        })
        .map(|(token, _)| token.clone())
        .collect();
    for token in retire {
        let _ = ctx
            .tx
            .try_send(RuntimeEvent::RequestResolved(token.clone()));
        state.pending.remove(&token);
    }
    let mut terminal = if ctx.cancelled.load(Ordering::Acquire) {
        RuntimeEvent::Status("Stopped".into())
    } else if let Some(error) = &failure {
        RuntimeEvent::Failed(error.clone())
    } else {
        RuntimeEvent::Finished
    };
    // A canceled or failed stream may never deliver item/completed. Retire its
    // live reasoning cards before preserving the response for history.
    for (id,parts) in std::mem::take(&mut state.reasoning){
        let status=if ctx.cancelled.load(Ordering::Acquire){crate::chat_interactions::InteractionStatus::Cancelled}else if failure.is_some(){crate::chat_interactions::InteractionStatus::Failed}else{crate::chat_interactions::InteractionStatus::Completed};
        state.parser.upsert_interaction(crate::chat_interactions::ChatInteraction::Status{title:"Thinking".into(),details:parts.into_values().collect::<Vec<_>>().join("\n\n"),status},&format!("reasoning:{id}"));
    }
    super::record_terminal(&mut state.parser, &terminal);
    if let Ok(source) = state.parser.rendered_source() {
        *ctx.finalized_source
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = Some(source);
    } else {
        terminal = RuntimeEvent::Failed("Could not preserve the complete Codex response".into());
        failure = Some("Could not preserve the complete Codex response".into());
    }
    let healthy = initialized && failure.is_none() && !ctx.cancelled.load(Ordering::Acquire);
    // Per-turn handles relinquish process ownership before publishing completion.
    // The broker retains the unreaped child; dropping an old handle can never
    // kill the service currently executing a later turn.
    {
        let mut group = ctx.group.lock().unwrap_or_else(|e| e.into_inner());
        if !healthy {
            service.begin_cleanup();
            if let Some(pid) = *group {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = child.kill();
            let _ = child.wait();
            service.disconnected();
        }
        *group = None;
    }
    if healthy {
        service.release();
    }
    if ctx.cancelled.load(Ordering::Acquire) {
        let _ = ctx.tx.try_send(terminal);
    } else {
        let _ = send(&ctx.tx, terminal, &ctx.cancelled);
    }
    service.hold_settled(ctx.settled);
    (healthy, state)
}

/// Stop the provider-owned tool/terminal groups while its process manager is
/// still alive. Never wait for UI queue space during cancellation.
fn graceful_stop(
    state: &Session,
    stdin: &mut std::process::ChildStdin,
    stdout: &mut std::process::ChildStdout,
) {
    let deadline = Instant::now() + Duration::from_millis(900);
    let write = |stdin: &mut std::process::ChildStdin, value: Value| -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&value).map_err(io::Error::other)?;
        bytes.push(b'\n');
        let mut position = 0;
        while position < bytes.len() && Instant::now() < deadline {
            match stdin.write(&bytes[position..]) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Native input closed",
                    ));
                }
                Ok(n) => position += n,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let mut pipe = libc::pollfd {
                        fd: stdin.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    unsafe {
                        libc::poll(&mut pipe, 1, 10);
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        if position < bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Native shutdown input timed out",
            ));
        }
        Ok(())
    };
    for pending in state.pending.values() {
        if let Ok(result) = response(pending, &NativeAnswer::Cancel) {
            let _ = write(stdin, json!({"id":pending.wire_id,"result":result}));
        }
    }
    let mut child_turns: std::collections::HashSet<_> = state
        .service
        .as_ref()
        .map(|service| service.active_turns().into_iter().collect())
        .unwrap_or_default();
    for (thread, turn) in &child_turns {
        let _ = write(
            stdin,
            json!({"id":format!("code-child-stop:{thread}"),"method":"turn/interrupt","params":{"threadId":thread,"turnId":turn}}),
        );
    }
    if state.voice.is_some() {
        let _ = write(
            stdin,
            json!({"id":"code-voice-stop","method":"thread/realtime/stop","params":{"threadId":state.thread}}),
        );
    }
    let action = if !state.command_process.is_empty() {
        json!({"id":"code-stop","method":"command/exec/terminate","params":{"processId":state.command_process}})
    } else if !state.thread.is_empty() && !state.turn.is_empty() {
        json!({"id":"code-stop","method":"turn/interrupt","params":{"threadId":state.thread,"turnId":state.turn}})
    } else {
        return;
    };
    if write(stdin, action).is_err() {
        return;
    }
    let mut bytes = [0u8; 8192];
    let mut line = Vec::with_capacity(8192);
    let mut oversized = false;
    while Instant::now() < deadline {
        match stdout.read(&mut bytes) {
            Ok(0) => return,
            Ok(n) => {
                for &byte in &bytes[..n] {
                    if byte == b'\n' {
                        if !oversized && let Ok(value) = serde_json::from_slice::<Value>(&line) {
                            let terminal_done =
                                !state.command_process.is_empty() && value["id"] == 3;
                            let turn_done = value["method"] == "turn/completed"
                                && value["params"]["threadId"] == state.thread
                                && value["params"]["turn"]["id"] == state.turn;
                            if value["method"] == "turn/completed"
                                && let (Some(thread), Some(turn)) = (
                                    value["params"]["threadId"].as_str(),
                                    value["params"]["turn"]["id"].as_str(),
                                )
                            {
                                child_turns.remove(&(thread.into(), turn.into()));
                            }
                            if (terminal_done || turn_done) && child_turns.is_empty() {
                                return;
                            }
                            if value["method"].is_string() && !value["id"].is_null() {
                                let _ = write(
                                    stdin,
                                    json!({"id":value["id"],"error":{"code":-32800,"message":"Code is stopping this native service"}}),
                                );
                            }
                        }
                        line.clear();
                        oversized = false;
                    } else if !oversized {
                        if line.len() < 64 * 1024 {
                            line.push(byte);
                        } else {
                            line.clear();
                            oversized = true;
                        }
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                let mut pipe = libc::pollfd {
                    fd: stdout.as_raw_fd(),
                    events: libc::POLLIN,
                    revents: 0,
                };
                unsafe {
                    libc::poll(&mut pipe, 1, 20);
                }
            }
            Err(_) => return,
        }
    }
}

impl Session {
    fn turn_params(&self,request:&RunRequest)->Value{
        let mut params=turn_params(&self.thread,request);
        if !self.request_reasoning{params.as_object_mut().unwrap().remove("summary");}
        params
    }
    fn launch_pending(&self)->bool{
        (self.thread.is_empty() && self.command_process.is_empty())
            || (self.maintenance.is_none() && self.turn.is_empty())
            || self.calls.values().any(|method|matches!(method.as_str(),"turn/start"|"thread/realtime/start"))
    }

    fn events(&mut self, events: Vec<RuntimeEvent>, ctx: &NativeContext) -> Result<()> {
        if let Some(error) = self.parser.storage_error.take() {
            bail!("Could not save response interactions: {error}");
        }
        if let Some(error) = self.parser.output.error() {
            bail!("Could not save response: {error:#}");
        }
        for event in events {
            if let RuntimeEvent::Failed(error) = event {
                bail!("{error}");
            }
            send(&ctx.tx, event, &ctx.cancelled)?;
        }
        Ok(())
    }
    fn message(
        &mut self,
        request: &RunRequest,
        value: Value,
        stdin: &mut std::process::ChildStdin,
        ctx: &NativeContext,
        initialized: &mut bool,
    ) -> Result<()> {
        if let Some(method) = value["method"].as_str() {
            if let Some(service) = self.service.clone() {
                let p = &value["params"];
                if method == "thread/started" {
                    let thread = &p["thread"];
                    let parent = thread["parentThreadId"].as_str().or_else(|| {
                        thread["source"]["subAgent"]["thread_spawn"]["parent_thread_id"].as_str()
                    });
                    if let (Some(id), Some(parent)) = (thread["id"].as_str(), parent)
                        && (parent == self.thread || service.child(parent))
                        && id != self.thread
                    {
                        let known = service.child(id);
                        service.observe(id, parent, method, p)?;
                        if !known {
                            write_message(
                                stdin,
                                &ctx.cancelled,
                                &json!({"id":format!("code-child-subscribe:{id}"),"method":"thread/resume","params":{"threadId":id,"excludeTurns":true}}),
                            )?;
                        }
                    }
                }
                if matches!(method, "item/started" | "item/completed")
                    && matches!(
                        p["item"]["type"].as_str(),
                        Some("collabAgentToolCall" | "collabToolCall")
                    )
                {
                    let item = &p["item"];
                    let mut ids: Vec<_> = item["receiverThreadIds"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .take(32)
                        .collect();
                    if let Some(id) = item["receiverThreadId"].as_str()
                        && !ids.contains(&id)
                    {
                        ids.push(id);
                    }
                    for id in ids.into_iter().filter(|id| *id != self.thread) {
                        let known = service.child(id);
                        service.observe(
                            id,
                            &self.thread,
                            "native/state",
                            &item["agentsStates"][id],
                        )?;
                        if !known {
                            write_message(
                                stdin,
                                &ctx.cancelled,
                                &json!({"id":format!("code-child-subscribe:{id}"),"method":"thread/resume","params":{"threadId":id,"excludeTurns":true}}),
                            )?;
                        }
                    }
                }
                if let Some(id) = p["threadId"]
                    .as_str()
                    .filter(|id| *id != self.thread && service.child(id))
                {
                    service.observe(id, &self.thread, method, p)?;
                }
            }
            if !value["id"].is_null() {
                return self.request(method, &value["id"], &value["params"], stdin, ctx);
            }
            let context_ready=method=="turn/completed" && value["params"]["threadId"]==self.thread && self.voice.as_ref().is_some_and(|v|v.preparing_context);
            self.notification(method,&value["params"],ctx)?;
            if context_ready{
                let (method,mut params)=self.maintenance.clone().context("Missing voice start")?;params["threadId"]=json!(self.thread);
                let id=self.next_id;self.next_id+=1;self.calls.insert(id,method.clone());rpc(stdin,&ctx.cancelled,id,&method,params)?;
            }
            return Ok(());
        }
        if let Some(id) = value["id"].as_str() {
            if let Some(child) = id.strip_prefix("code-child-subscribe:") {
                if let Some(service) = &self.service {
                    if let Some(error) = value["error"]["message"].as_str() {
                        service.child_error(child, error);
                    }
                    if value.get("error").is_none() {
                        write_message(
                            stdin,
                            &ctx.cancelled,
                            &json!({"id":format!("code-child-latest:{child}"),"method":"thread/turns/list","params":{"threadId":child,"limit":1,"itemsView":"notLoaded","sortDirection":"desc"}}),
                        )?;
                    }
                    for turn in value["result"]["thread"]["turns"]
                        .as_array()
                        .into_iter()
                        .flatten()
                    {
                        if turn["status"] == "inProgress" {
                            service.observe(
                                child,
                                &self.thread,
                                "turn/started",
                                &json!({"turn":turn}),
                            )?;
                        }
                    }
                }
                return Ok(());
            }
            if let Some(child) = id.strip_prefix("code-child-latest:") {
                if let Some(service) = &self.service {
                    if let Some(error) = value["error"]["message"].as_str() {
                        service.child_error(child, error);
                    }
                    for turn in value["result"]["data"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(1)
                    {
                        let method = if turn["status"] == "inProgress" {
                            "turn/started"
                        } else {
                            "turn/completed"
                        };
                        service.observe(child, &self.thread, method, &json!({"turn":turn}))?;
                    }
                }
                return Ok(());
            }
            if let Some((child, _)) = id
                .strip_prefix("code-child-control:")
                .and_then(|suffix| suffix.split_once(':'))
            {
                if let Some(service) = &self.service
                    && let Some(error) = value["error"]["message"].as_str()
                {
                    service.child_error(child, error);
                }
                return Ok(());
            }
        }
        if let Some(error) = value.get("error") {
            if value["id"].as_u64().is_some_and(|id| id <= 3 || self.calls.get(&id).is_some_and(|m|m=="thread/realtime/start")) {
                bail!(
                    "{}",
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex rejected the request")
                );
            }
            return send(
                &ctx.tx,
                RuntimeEvent::Status(
                    error["message"]
                        .as_str()
                        .unwrap_or("Codex request failed")
                        .chars()
                        .take(1000)
                        .collect(),
                ),
                &ctx.cancelled,
            );
        }
        match value["id"].as_u64() {
            Some(1) => {
                *initialized = true;
                write_message(stdin, &ctx.cancelled, &json!({"method":"initialized"}))?;
                if self
                    .maintenance
                    .as_ref()
                    .is_some_and(|(method, _)| method == "command/exec")
                {
                    self.command_process = uuid::Uuid::new_v4().to_string();
                    let project = std::fs::canonicalize(&request.project)
                        .context("Project folder is unavailable")?;
                    let sandbox = request.codex_access.policy(request.allow_edits, &project);
                    rpc(
                        stdin,
                        &ctx.cancelled,
                        3,
                        "command/exec",
                        json!({"command":["/bin/zsh","-c",request.prompt],"cwd":project,"sandboxPolicy":sandbox,"processId":self.command_process,"streamStdin":false,"streamStdoutStderr":true,"tty":false,"disableOutputCap":true,"disableTimeout":true}),
                    )?;
                    self.parser.begin("terminal-output");
                    let events = self.parser.delta("````````text\n");
                    self.events(events, ctx)?;
                    send(
                        &ctx.tx,
                        RuntimeEvent::Status("Running terminal command".into()),
                        &ctx.cancelled,
                    )?;
                } else {
                    rpc(stdin, &ctx.cancelled, 0, "config/read", json!({"cwd":request.project,"includeLayers":false}))?;
                }
            }
            Some(0) => {
                let config=value["result"].get("config").context("Codex returned no effective configuration")?;
                self.request_reasoning=config["model_provider"].as_str().is_none_or(|p|p=="openai" || p=="kai-openai-https");
                rpc(stdin,&ctx.cancelled,2,
                    if request.resume_id.is_none(){"thread/start"}else if request.fork_session{"thread/fork"}else{"thread/resume"},
                    routed_thread_params(request,config))?;
            }
            Some(2) => {
                self.thread = value["result"]["thread"]["id"]
                    .as_str()
                    .context("Codex returned no thread ID")?
                    .into();
                if let Some(service) = &self.service {
                    service.set_parent(&self.thread);
                }
                let events = self
                    .parser
                    .parse_codex(&json!({"type":"thread.started","thread_id":self.thread}));
                self.events(events, ctx)?;
                if let Some(model) = value["result"]["model"].as_str() {
                    send(
                        &ctx.tx,
                        RuntimeEvent::Usage(Box::new(crate::usage::Usage {
                            model: Some(model.into()),
                            reasoning_effort:value["result"]["reasoningEffort"].as_str().map(str::to_owned),
                            ..Default::default()
                        })),
                        &ctx.cancelled,
                    )?;
                }
                if let Some((method, mut params)) = self.maintenance.clone() {
                    if method=="thread/realtime/start" && self.voice.as_ref().is_some_and(|v|v.preparing_context){
                        self.voice.as_mut().context("Missing voice context owner")?.preparing_context=true;
                        rpc(stdin,&ctx.cancelled,3,"turn/start",self.turn_params(request))?;
                        send(&ctx.tx,RuntimeEvent::Status("Preparing conversation context for voice".into()),&ctx.cancelled)?;return Ok(())
                    }
                    params["threadId"] = json!(self.thread);
                    rpc(stdin, &ctx.cancelled, 3, &method, params)?;
                    let status = match method.as_str() {
                        "thread/realtime/start" => "Connecting Codex voice",
                        "review/start" => "Reviewing changes",
                        "thread/goal/get" => "Reading goal",
                        "thread/goal/set" => "Updating goal",
                        "thread/goal/clear" => "Clearing goal",
                        _ => "Compacting conversation",
                    };
                    send(&ctx.tx, RuntimeEvent::Status(status.into()), &ctx.cancelled)?;
                } else {
                    rpc(
                        stdin,
                        &ctx.cancelled,
                        3,
                        "turn/start",
                        self.turn_params(request),
                    )?;
                }
            }
            Some(3) => {
                if self.voice.is_some() {
                    return Ok(());
                }

                if let Some((method, _)) = self
                    .maintenance
                    .as_ref()
                    .filter(|(m, _)| m.starts_with("thread/goal/"))
                {
                    let events = if method == "thread/goal/clear" {
                        self.parser.interaction(
                            crate::chat_interactions::ChatInteraction::Status {
                                title: if value["result"]["cleared"] == true {
                                    "Goal cleared"
                                } else {
                                    "No active goal"
                                }
                                .into(),
                                details: String::new(),
                                status: crate::chat_interactions::InteractionStatus::Completed,
                            },
                            "native-goal",
                        )
                    } else {
                        self.goal(&value["result"]["goal"])
                    };
                    self.events(events, ctx)?;
                    self.complete = true;
                } else if !self.command_process.is_empty() {
                    let tails = std::mem::take(&mut self.terminal_utf8);
                    for tail in tails.values().filter(|tail| !tail.is_empty()) {
                        let pending = String::from_utf8_lossy(tail).into_owned();
                        let events = self.parser.delta(&pending);
                        self.events(events, ctx)?;
                    }
                    let events = self.parser.delta("\n````````");
                    self.events(events, ctx)?;
                    let code = value["result"]["exitCode"]
                        .as_i64()
                        .context("Codex command returned no exit status")?;
                    let events = self.parser.interaction(
                        crate::chat_interactions::ChatInteraction::Status {
                            title: format!("Command exited with status {code}"),
                            details: String::new(),
                            status: if code == 0 {
                                crate::chat_interactions::InteractionStatus::Completed
                            } else {
                                crate::chat_interactions::InteractionStatus::Failed
                            },
                        },
                        "terminal-exit",
                    );
                    self.events(events, ctx)?;
                    self.complete = true;
                } else {
                    self.turn = value["result"]["turn"]["id"].as_str().unwrap_or("").into();
                }
            }
            Some(id) => {
                if let Some(method) = self.calls.remove(&id) {
                    send(
                        &ctx.tx,
                        RuntimeEvent::Status(format!(
                            "{method}: {}",
                            summary(&value["result"], 4096)
                        )),
                        &ctx.cancelled,
                    )?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn spoken(&mut self, id: &str, role: &str, text: &str, final_result:bool, ctx: &NativeContext) -> Result<()> {
        let speaker = match role {
            "user" => crate::chat_interactions::Speaker::User,
            "assistant" => crate::chat_interactions::Speaker::Assistant,
            _ => return Ok(()),
        };
        let prefix=format!("voice:{id}:");
        let mut start = 0;
        while start < text.len() {
            let mut end = (start + 48 * 1024).min(text.len());
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            let events = self.parser.upsert_interaction(
                crate::chat_interactions::ChatInteraction::Speaker {
                    speaker,
                    text: text[start..end].into(),
                },
                &format!("{prefix}{start}"),
            );
            self.events(events, ctx)?;
            start = end;
        }
        if final_result {
            // Final recognition may correct or shorten a partial segment.
            let keys:Vec<_>=self.parser.live_interactions.keys().filter(|key|key.strip_prefix(&prefix).is_some_and(|suffix|suffix.parse::<usize>().is_ok())).cloned().collect();
            for key in keys{
                if key.strip_prefix(&prefix).and_then(|s|s.parse::<usize>().ok()).is_some_and(|start|start>=text.len()){
                    let events=self.parser.upsert_interaction(crate::chat_interactions::ChatInteraction::Speaker{speaker,text:String::new()},&key);self.events(events,ctx)?;
                }
                self.parser.live_interactions.remove(&key);
            }
        }
        Ok(())
    }
    fn voice_notification(&mut self, method: &str, p: &Value, ctx: &NativeContext) -> Result<()> {
        if p["threadId"].as_str() != Some(self.thread.as_str()) {
            return Ok(());
        }
        let Some(voice) = &self.voice else {
            return Ok(());
        };
        let session_id = voice.id.clone();
        if p["realtimeSessionId"]
            .as_str()
            .or_else(|| p["item"]["realtimeSessionId"].as_str())
            .is_some_and(|id| id != session_id)
        {
            return Ok(());
        }
        match method {
            "thread/realtime/started" => send(
                &ctx.tx,
                RuntimeEvent::Voice(crate::voice::Signal::Started { session_id }),
                &ctx.cancelled,
            )?,
            "thread/realtime/sdp" => {
                let sdp = p["sdp"]
                    .as_str()
                    .filter(|s| crate::voice::valid_sdp(s))
                    .context("Codex returned an invalid voice SDP answer")?;
                send(
                    &ctx.tx,
                    RuntimeEvent::Voice(crate::voice::Signal::Answer {
                        session_id,
                        sdp: sdp.into(),
                    }),
                    &ctx.cancelled,
                )?;
            }
            "thread/realtime/error" => bail!(
                "{}",
                p["message"]
                    .as_str()
                    .unwrap_or("The Codex voice service failed")
            ),
            "thread/realtime/closed" => {
                let reason = p["reason"].as_str().map(str::to_owned);
                if let Some(voice) = &mut self.voice {
                    voice.closed = true;
                    self.complete = !voice.coding;
                }
                send(
                    &ctx.tx,
                    RuntimeEvent::Voice(crate::voice::Signal::Closed { session_id, reason }),
                    &ctx.cancelled,
                )?;
            }
            "thread/realtime/item/started" | "thread/realtime/item/completed" => {
                let item = &p["item"];
                let id = item["id"].as_str().unwrap_or("");
                if let Some(voice) = &mut self.voice {
                    voice.canonical = true;
                }
                if item["type"] == "transcriptSegment" {
                    if self.voice.as_ref().is_some_and(|voice|voice.final_ids.contains(id)){return Ok(())}
                    let role = item["role"].as_str().unwrap_or("");
                    let text = item["text"].as_str().unwrap_or("");
                    if method.ends_with("/completed") {
                        let voice = self.voice.as_mut().expect("voicepresent");
                        if !voice.final_ids.insert(id.into()) {
                            return Ok(());
                        }
                        voice.final_order.push_back(id.into());
                        while voice.final_order.len() > 512 {
                            if let Some(old) = voice.final_order.pop_front() {
                                voice.final_ids.remove(&old);
                            }
                        }
                        voice.segments.remove(id);
                        self.spoken(id, role, text, true, ctx)?;
                        send(
                            &ctx.tx,
                            RuntimeEvent::Voice(crate::voice::Signal::Transcript {
                                session_id,
                                id: id.into(),
                                role: role.into(),
                                text: voice_tail(text, 64 * 1024).to_owned(),
                                final_result: true,
                            }),
                            &ctx.cancelled,
                        )?;
                    } else {
                        let voice = self.voice.as_mut().expect("voicepresent");
                        if voice.segments.len() < 64 {
                            let mut buffer = ResponseBuffer::default();
                            buffer.push_str(text);
                            voice.segments.insert(id.into(), (role.into(), buffer));
                            self.spoken(id,role,text,false,ctx)?;
                        }
                    }
                }
            }
            "thread/realtime/item/transcript/delta" => {
                let id = p["itemId"].as_str().unwrap_or("");
                if let Some((role, buffer)) =
                    self.voice.as_mut().and_then(|v| v.segments.get_mut(id))
                {
                    buffer.push_str(p["delta"].as_str().unwrap_or(""));
                    let text = buffer.snapshot()?.tail(64 * 1024)?;
                    let role=role.clone();
                    self.spoken(id,&role,&text,false,ctx)?;
                    send(
                        &ctx.tx,
                        RuntimeEvent::Voice(crate::voice::Signal::Transcript {
                            session_id,
                            id: id.into(),
                            role,
                            text,
                            final_result: false,
                        }),
                        &ctx.cancelled,
                    )?;
                }
            }
            "thread/realtime/transcript/delta" | "thread/realtime/transcript/done" => {
                if self.voice.as_ref().is_some_and(|v| v.canonical) {
                    return Ok(());
                }
                let role = p["role"].as_str().unwrap_or("");
                if !matches!(role, "user" | "assistant") {
                    return Ok(());
                }
                let id = format!("flat:{role}");
                let final_result = method.ends_with("/done");
                let voice = self.voice.as_mut().expect("voicepresent");
                let live_id=voice.flat_ids.entry(id.clone()).or_insert_with(||uuid::Uuid::new_v4().to_string()).clone();
                let (_, buffer) = voice
                    .segments
                    .entry(id.clone())
                    .or_insert_with(|| (role.into(), ResponseBuffer::default()));
                if final_result {
                    buffer.truncate(0);
                    buffer.push_str(p["text"].as_str().unwrap_or(""));
                } else {
                    buffer.push_str(p["delta"].as_str().unwrap_or(""));
                }
                let text = buffer.snapshot()?.tail(64 * 1024)?;
                self.spoken(&live_id,role,&text,final_result,ctx)?;
                if final_result {
                    if let Some(v) = &mut self.voice {
                        v.segments.remove(&id);
                        v.flat_ids.remove(&id);
                    }
                }
                send(
                    &ctx.tx,
                    RuntimeEvent::Voice(crate::voice::Signal::Transcript {
                        session_id,
                        id,
                        role: role.into(),
                        text,
                        final_result,
                    }),
                    &ctx.cancelled,
                )?;
            }
            // Audio travels on WebRTC, never decoded/copied onto Rust's event queue.
            "thread/realtime/outputAudio/delta" | "thread/realtime/itemAdded" => {}
            _ => {}
        }
        Ok(())
    }
    fn notification(&mut self, method: &str, p: &Value, ctx: &NativeContext) -> Result<()> {
        if method.starts_with("thread/realtime/") && self.voice.is_some() {
            return self.voice_notification(method, p, ctx);
        }
        if matches!(method, "item/started" | "item/completed") && p["item"]["type"] == "fileChange"
        {
            self.cache_file_changes(
                p["threadId"].as_str().unwrap_or(&self.thread).to_owned(),
                &p["item"],
            );
        }
        if method == "command/exec/outputDelta"
            && p["processId"] == self.command_process
            && !self.command_process.is_empty()
        {
            let bytes = decode_base64(
                p["deltaBase64"]
                    .as_str()
                    .context("Terminal output has no bytes")?,
            )?;
            let stream = p["stream"].as_str().unwrap_or("stdout");
            anyhow::ensure!(
                matches!(stream, "stdout" | "stderr"),
                "Unknown terminal output stream"
            );
            let tail = self.terminal_utf8.entry(stream.into()).or_default();
            tail.extend_from_slice(&bytes);
            let (text, consumed) = match std::str::from_utf8(tail) {
                Ok(text) => (text.to_owned(), tail.len()),
                Err(error) if error.error_len().is_none() => (
                    String::from_utf8_lossy(&tail[..error.valid_up_to()]).into_owned(),
                    error.valid_up_to(),
                ),
                Err(_) => (String::from_utf8_lossy(tail).into_owned(), tail.len()),
            };
            tail.drain(..consumed);
            let events = self.parser.delta(&text);
            self.events(events, ctx)?;
            if p["capReached"] == true {
                send(
                    &ctx.tx,
                    RuntimeEvent::Status(
                        "The terminal provider shortened its captured output".into(),
                    ),
                    &ctx.cancelled,
                )?;
            }
            return Ok(());
        }
        // Child-thread notifications must not enter the parent's prose or
        // terminate its response. They are visible as native progress cards.
        if let Some(id) = p["threadId"].as_str().filter(|id| {
            !self.thread.is_empty() && *id != self.thread && method != "serverRequest/resolved"
        }) {
            if method == "item/agentMessage/delta" && self.service.is_some() {
                return Ok(());
            }
            if matches!(
                method,
                "item/agentMessage/delta" | "turn/completed" | "turn/started"
            ) {
                let card = crate::chat_interactions::ChatInteraction::Delegation {
                    title: match (method, p["turn"]["status"].as_str()) {
                        ("turn/completed", Some("failed")) => "Subagent needs attention",
                        ("turn/completed", Some("interrupted")) => "Subagent stopped",
                        ("turn/completed", Some("completed")) => "Subagent completed",
                        _ => "Subagent working",
                    }
                    .into(),
                    details: p["delta"]
                        .as_str()
                        .or_else(|| p["turn"]["error"]["message"].as_str())
                        .unwrap_or("")
                        .chars()
                        .take(8192)
                        .collect(),
                    agent: Some(id.into()),
                    status: match (method, p["turn"]["status"].as_str()) {
                        ("turn/completed", Some("failed")) => {
                            crate::chat_interactions::InteractionStatus::Failed
                        }
                        ("turn/completed", Some("interrupted")) => {
                            crate::chat_interactions::InteractionStatus::Cancelled
                        }
                        ("turn/completed", Some("completed")) => {
                            crate::chat_interactions::InteractionStatus::Completed
                        }
                        _ => crate::chat_interactions::InteractionStatus::Working,
                    },
                };
                self.stream_sequence = self.stream_sequence.wrapping_add(1);
                let events = self
                    .parser
                    .interaction(card, &format!("child:{id}:{}", self.stream_sequence));
                return self.events(events, ctx);
            }
            return Ok(());
        }
        match method {
            "thread/goal/updated" => {
                let events = self.goal(&p["goal"]);
                self.events(events, ctx)?;
            }
            "thread/goal/cleared" => {
                let events = self.parser.interaction(
                    crate::chat_interactions::ChatInteraction::Status {
                        title: "Goal cleared".into(),
                        details: String::new(),
                        status: crate::chat_interactions::InteractionStatus::Completed,
                    },
                    "native-goal",
                );
                self.events(events, ctx)?;
            }
            "hook/started" | "hook/completed" => {
                let run = &p["run"];
                let mut detail = run["statusMessage"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(2048)
                    .collect::<String>();
                for entry in run["entries"].as_array().into_iter().flatten().take(32) {
                    if let Some(text) = entry["text"].as_str() {
                        if !detail.is_empty() {
                            detail.push('\n');
                        }
                        detail.extend(text.chars().take(8192usize.saturating_sub(detail.len())));
                    }
                }
                let card = crate::chat_interactions::ChatInteraction::Status {
                    title: format!(
                        "Hook · {}",
                        run["eventName"]
                            .as_str()
                            .unwrap_or("lifecycle")
                            .chars()
                            .take(64)
                            .collect::<String>()
                    ),
                    details: detail,
                    status: match run["status"].as_str() {
                        Some("failed") => crate::chat_interactions::InteractionStatus::Failed,
                        Some("blocked") => crate::chat_interactions::InteractionStatus::Failed,
                        Some("stopped") => crate::chat_interactions::InteractionStatus::Cancelled,
                        _ if method == "hook/completed" => {
                            crate::chat_interactions::InteractionStatus::Completed
                        }
                        _ => crate::chat_interactions::InteractionStatus::Working,
                    },
                };
                let events = self.parser.interaction(
                    card,
                    &format!("hook:{}", run["id"].as_str().unwrap_or("lifecycle")),
                );
                self.events(events, ctx)?;
            }
            "item/mcpToolCall/progress" => {
                self.stream_sequence = self.stream_sequence.wrapping_add(1);
                let events = self.parser.interaction(
                    crate::chat_interactions::ChatInteraction::Status {
                        title: "Connected tool progress".into(),
                        details: p["message"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(8192)
                            .collect(),
                        status: crate::chat_interactions::InteractionStatus::Working,
                    },
                    &format!(
                        "mcp:{}:{}",
                        p["itemId"].as_str().unwrap_or("tool"),
                        self.stream_sequence
                    ),
                );
                self.events(events, ctx)?;
            }
            "model/rerouted" => {
                let model = p["toModel"].as_str().unwrap_or("");
                if valid_model_id(model) {
                    send(
                        &ctx.tx,
                        RuntimeEvent::Usage(Box::new(crate::usage::Usage {
                            model: Some(model.into()),
                            ..Default::default()
                        })),
                        &ctx.cancelled,
                    )?;
                }
                let events = self.parser.interaction(
                    crate::chat_interactions::ChatInteraction::Status {
                        title: "Model changed by provider".into(),
                        details: format!(
                            "{} → {}\n{}",
                            p["fromModel"]
                                .as_str()
                                .unwrap_or("Previous model")
                                .chars()
                                .take(128)
                                .collect::<String>(),
                            model.chars().take(128).collect::<String>(),
                            p["reason"]
                                .as_str()
                                .unwrap_or("")
                                .chars()
                                .take(512)
                                .collect::<String>()
                        ),
                        status: crate::chat_interactions::InteractionStatus::Completed,
                    },
                    "model-rerouted",
                );
                self.events(events, ctx)?;
            }
            "warning" | "guardianWarning" | "configWarning" | "deprecationNotice" => {
                let title = p["summary"]
                    .as_str()
                    .or_else(|| p["message"].as_str())
                    .unwrap_or("Provider notice");
                self.stream_sequence = self.stream_sequence.wrapping_add(1);
                let events = self.parser.interaction(
                    crate::chat_interactions::ChatInteraction::Status {
                        title: title.chars().take(256).collect(),
                        details: p["details"]
                            .as_str()
                            .unwrap_or("")
                            .chars()
                            .take(8192)
                            .collect(),
                        status: crate::chat_interactions::InteractionStatus::Completed,
                    },
                    &format!("notice:{}", self.stream_sequence),
                );
                self.events(events, ctx)?;
            }
            "turn/started" => {
                if let Some(voice) = &mut self.voice {
                    voice.coding = true;
                }
                self.turn = p["turn"]["id"].as_str().unwrap_or("").into();
                send(
                    &ctx.tx,
                    RuntimeEvent::Status("Thinking".into()),
                    &ctx.cancelled,
                )?;
            }
            "item/agentMessage/delta" => {
                self.parser.begin(p["itemId"].as_str().unwrap_or("message"));
                let events = self.parser.delta(p["delta"].as_str().unwrap_or(""));
                self.events(events, ctx)?;
            }
            "item/started" | "item/completed" => {
                if method == "item/completed" && p["item"]["type"] == "commandExecution" {
                    let id = p["item"]["id"].as_str().unwrap_or("item");
                    if !self.tool_streamed.contains(id)
                        && let Some(output) = p["item"]["aggregatedOutput"]
                            .as_str()
                            .filter(|s| s.len() > 8192)
                    {
                        let mut start = 0;
                        while start < output.len() {
                            let mut end = (start + 8192).min(output.len());
                            while !output.is_char_boundary(end) {
                                end -= 1;
                            }
                            self.stream_sequence = self.stream_sequence.wrapping_add(1);
                            let events = self.parser.interaction(
                                crate::chat_interactions::ChatInteraction::Status {
                                    title: "Terminal output".into(),
                                    details: output[start..end].into(),
                                    status: crate::chat_interactions::InteractionStatus::Completed,
                                },
                                &format!("{id}:final-output:{}", self.stream_sequence),
                            );
                            self.events(events, ctx)?;
                            start = end;
                        }
                        self.tool_streamed.insert(id.into());
                    }
                }
                let events = self.item(&p["item"], method == "item/completed");
                self.events(events, ctx)?;
            }
            "turn/completed" => {
                if self.voice.as_ref().is_some_and(|v|v.preparing_context){
                    anyhow::ensure!(p["turn"]["status"]=="completed","Voice context preparation did not complete successfully");
                    let voice=self.voice.as_mut().unwrap();voice.preparing_context=false;voice.coding=false;self.turn.clear();
                    return Ok(())
                }
                if let Some(voice) = &mut self.voice {
                    voice.coding = false;
                    if voice.closed {
                        self.complete = true;
                    }
                    if p["turn"]["status"] == "failed" {
                        let events = self.parser.interaction(
                            crate::chat_interactions::ChatInteraction::Status {
                                title: "Delegated coding needs attention".into(),
                                details: p["turn"]["error"]["message"]
                                    .as_str()
                                    .unwrap_or("The backing coding turn failed")
                                    .into(),
                                status: crate::chat_interactions::InteractionStatus::Failed,
                            },
                            "voice-coding-failure",
                        );
                        self.events(events, ctx)?;
                    }
                    return Ok(());
                }
                self.complete = true;
                match p["turn"]["status"].as_str() {
                    Some("failed") => bail!(
                        "{}",
                        p["turn"]["error"]["message"]
                            .as_str()
                            .unwrap_or("Codex could not finish this response")
                    ),
                    Some("interrupted") => send(
                        &ctx.tx,
                        RuntimeEvent::Status("Interrupted".into()),
                        &ctx.cancelled,
                    )?,
                    _ => {
                        let events = self.parser.parse_codex(&json!({"type":"turn.completed"}));
                        self.events(events, ctx)?;
                    }
                }
            }
            "error" => {
                if p["willRetry"] == false {
                    self.parser.codex_pending_error =
                        p["error"]["message"].as_str().map(str::to_owned);
                }
                send(
                    &ctx.tx,
                    RuntimeEvent::Status(
                        p["error"]["message"]
                            .as_str()
                            .unwrap_or("Codex is reconnecting")
                            .chars()
                            .take(500)
                            .collect(),
                    ),
                    &ctx.cancelled,
                )?;
            }
            "account/rateLimits/updated" => {
                if let Some(usage) = crate::usage::Usage::codex_limits(p, false) {
                    if let Some(service) = &self.service {
                        service.report_usage(usage.clone());
                    }
                    send(
                        &ctx.tx,
                        RuntimeEvent::Usage(Box::new(usage)),
                        &ctx.cancelled,
                    )?;
                }
            }
            "thread/status/changed" => {
                let status = match p["status"]["type"].as_str() {
                    Some("active") => {
                        let flags = p["status"]["activeFlags"].as_array();
                        if flags.is_some_and(|flags| {
                            flags.iter().any(|flag| flag == "waitingOnApproval")
                        }) {
                            "Waiting for approval"
                        } else if flags.is_some_and(|flags| {
                            flags.iter().any(|flag| flag == "waitingOnUserInput")
                        }) {
                            "Waiting for your answer"
                        } else {
                            "Working"
                        }
                    }
                    Some("idle") => "Ready",
                    Some("notLoaded") => "Chat unloaded",
                    Some("systemError") => "Provider needs attention",
                    _ => return Ok(()),
                };
                send(&ctx.tx, RuntimeEvent::Status(status.into()), &ctx.cancelled)?;
            }
            "thread/tokenUsage/updated" => {
                let u = &p["tokenUsage"];
                let last = &u["last"];
                send(
                    &ctx.tx,
                    RuntimeEvent::Usage(Box::new(crate::usage::Usage {
                        input: last["inputTokens"].as_u64(),
                        output: last["outputTokens"].as_u64(),
                        cached: last["cachedInputTokens"].as_u64(),
                        context_used: last["totalTokens"].as_u64(),
                        context_limit: u["modelContextWindow"].as_u64(),
                        scope: "Last model request".into(),
                        ..Default::default()
                    })),
                    &ctx.cancelled,
                )?;
            }
            "serverRequest/resolved" => {
                let tokens: Vec<_> = self
                    .pending
                    .iter()
                    .filter(|(_, pending)| pending.wire_id == p["requestId"])
                    .map(|(id, _)| id.clone())
                    .collect();
                for token in tokens {
                    self.pending.remove(&token);
                    if let Some(service) = &self.service {
                        service.resolve(&token);
                    }
                    send(
                        &ctx.tx,
                        RuntimeEvent::RequestResolved(token),
                        &ctx.cancelled,
                    )?;
                }
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/summaryPartAdded" => {
                let id=p["itemId"].as_str().context("Reasoning update missing item ID")?;
                let index=p["summaryIndex"].as_u64().unwrap_or(0);
                let parts=self.reasoning.entry(id.into()).or_default();
                if let Some(delta)=p["delta"].as_str(){parts.entry(index).or_default().push_str(delta);}
                let text=parts.values().cloned().collect::<Vec<_>>().join("\n\n");
                let events=self.parser.upsert_interaction(crate::chat_interactions::ChatInteraction::Status{title:"Thinking".into(),details:text,status:crate::chat_interactions::InteractionStatus::Working},&format!("reasoning:{id}"));self.events(events,ctx)?;
            }
            "turn/plan/updated"
            | "turn/diff/updated"
            | "item/commandExecution/outputDelta"
            | "item/plan/delta" => {
                // Keep terminal/code/plan output inside typed glass cards, never
                // splice it into the assistant's natural-language response.
                if method == "item/commandExecution/outputDelta"
                    && self.tool_streamed.len() < 128
                    && let Some(id) = p["itemId"].as_str()
                {
                    self.tool_streamed.insert(id.into());
                }
                let detail = p["delta"]
                    .as_str()
                    .or_else(|| p["diff"].as_str())
                    .map(str::to_owned)
                    .unwrap_or_else(|| native_plan(&p["plan"]));
                let title = if method.contains("commandExecution") {
                    "Terminal output"
                } else if method.contains("diff") {
                    "Changes"
                } else if method.contains("reasoning") {
                    "Thinking"
                } else {
                    "Plan"
                };
                let card = crate::chat_interactions::ChatInteraction::Status {
                    title: title.into(),
                    details: detail,
                    status: crate::chat_interactions::InteractionStatus::Working,
                };
                let key = if method.ends_with("Delta") || method.ends_with("/delta") {
                    self.stream_sequence = self.stream_sequence.wrapping_add(1);
                    format!(
                        "{}:delta:{}",
                        p["itemId"].as_str().unwrap_or(method),
                        self.stream_sequence
                    )
                } else {
                    p["itemId"].as_str().unwrap_or(method).into()
                };
                let events = self.parser.interaction(card, &key);
                self.events(events, ctx)?;
            }
            _ => {}
        }
        Ok(())
    }
    fn item(&mut self, item: &Value, completed: bool) -> Vec<RuntimeEvent> {
        let id = item["id"].as_str().unwrap_or("item");
        match item["type"].as_str().unwrap_or("") {
            "agentMessage" => {
                self.parser.begin(id);
                if completed && item["text"].is_string() {
                    self.parser.snapshot(item["text"].as_str().unwrap_or(""))
                } else {
                    Vec::new()
                }
            }
            "collabAgentToolCall" | "collabToolCall" => {
                let mut value = item.clone();
                value["agents_states"] = item["agentsStates"].clone();
                value["receiver_thread_ids"] = item["receiverThreadIds"].clone();
                if value["receiver_thread_ids"].is_null() {
                    value["receiver_thread_ids"] = json!([item["receiverThreadId"]
                        .as_str()
                        .or_else(|| item["newThreadId"].as_str())
                        .unwrap_or("")]);
                }
                value["tool"] = json!(match item["tool"].as_str().unwrap_or("") {
                    "spawnAgent" => "spawn_agent",
                    "sendInput" => "send_input",
                    "resumeAgent" => "resume_agent",
                    "closeAgent" => "close_agent",
                    "sendMessage" => "send_message",
                    "followupTask" => "followup_task",
                    "interruptAgent" => "interrupt_agent",
                    "listAgents" => "list_agents",
                    other => other,
                });
                if let Some(states) = value["agents_states"].as_object_mut() {
                    for state in states.values_mut() {
                        let native = state["status"].as_str().unwrap_or("");
                        if native == "pendingInit" {
                            state["status"] = json!("pending_init");
                        } else if native == "notFound" {
                            state["status"] = json!("not_found");
                        }
                    }
                }
                self.parser.codex_delegation(&value)
            }
            "subAgentActivity" => self.parser.interaction(
                crate::chat_interactions::ChatInteraction::Delegation {
                    title: format!("Subagent {}", item["kind"].as_str().unwrap_or("working")),
                    details: item["agentPath"].as_str().unwrap_or("").into(),
                    agent: Some("Codex".into()),
                    status: status(item["kind"].as_str().unwrap_or("")),
                },
                id,
            ),
            "enteredReviewMode" | "exitedReviewMode" => {
                let text = item["review"].as_str().unwrap_or("");
                let title = if item["type"] == "enteredReviewMode" {
                    "Reviewing changes"
                } else {
                    "Code review"
                };
                let status = if item["type"] == "exitedReviewMode" && completed {
                    crate::chat_interactions::InteractionStatus::Completed
                } else {
                    crate::chat_interactions::InteractionStatus::Working
                };
                let mut events = Vec::new();
                let mut start = 0;
                let mut part = 0;
                loop {
                    let mut end = (start + 16 * 1024).min(text.len());
                    while !text.is_char_boundary(end) {
                        end -= 1;
                    }
                    events.extend(self.parser.interaction(
                        crate::chat_interactions::ChatInteraction::Status {
                            title: title.into(),
                            details: text[start..end].into(),
                            status,
                        },
                        &format!("review:{id}:{part}"),
                    ));
                    if end == text.len() {
                        break;
                    }
                    start = end;
                    part += 1;
                }
                events
            }
            "reasoning" => {
                let summary=item["summary"].as_array().into_iter().flatten().filter_map(|v|v.as_str().or_else(||v["text"].as_str())).collect::<Vec<_>>().join("\n\n");
                let streamed=self.reasoning.remove(id).map(|parts|parts.into_values().collect::<Vec<_>>().join("\n\n")).unwrap_or_default();
                let text=if summary.is_empty(){streamed}else{summary};
                let events=self.parser.upsert_interaction(crate::chat_interactions::ChatInteraction::Status{title:"Thinking".into(),details:text,status:if completed{crate::chat_interactions::InteractionStatus::Completed}else{crate::chat_interactions::InteractionStatus::Working}},&format!("reasoning:{id}"));
                if completed && let Err(error)=self.parser.interactions.commit_updates(){self.parser.storage_error=Some(error.to_string());}
                events
            }
            "userMessage" | "hookPrompt" => Vec::new(),
            kind => {
                let title = match kind {
                    "commandExecution" => "Terminal command",
                    "fileChange" => "File changes",
                    "mcpToolCall" => "Connected tool",
                    "reasoning" => "Thinking",
                    "webSearch" => "Web search",
                    "contextCompaction" => {
                        if completed {
                            "Context compacted"
                        } else {
                            "Compacting context"
                        }
                    }
                    "plan" => "Plan",
                    "imageGeneration" => "Generated image",
                    "imageView" => "Image",
                    "dynamicToolCall" => "Tool",
                    _ => kind,
                };
                let mut output_events = Vec::new();
                let detail = if kind == "commandExecution" {
                    if completed
                        && !self.tool_streamed.remove(id)
                        && let Some(output) = item["aggregatedOutput"].as_str()
                        && output.len() <= 8192
                    {
                        self.stream_sequence = self.stream_sequence.wrapping_add(1);
                        output_events.extend(self.parser.interaction(
                            crate::chat_interactions::ChatInteraction::Status {
                                title: "Terminal output".into(),
                                details: output.into(),
                                status: crate::chat_interactions::InteractionStatus::Completed,
                            },
                            &format!("{id}:final-output:{}", self.stream_sequence),
                        ));
                    }
                    item["command"]
                        .as_str()
                        .unwrap_or("")
                        .chars()
                        .take(8192)
                        .collect()
                } else if kind == "fileChange" {
                    item["changes"].as_array().into_iter().flatten().map(|change|format!("{}\n{}",change["path"].as_str().unwrap_or("File"),change["diff"].as_str().unwrap_or(""))).collect::<Vec<_>>().join("\n\n")
                } else if kind == "plan" {
                    item["text"].as_str().unwrap_or("").into()
                } else {
                    summary(item, 8192)
                };
                output_events.extend(self.parser.interaction(
                    crate::chat_interactions::ChatInteraction::Status {
                        title: title.into(),
                        details: detail,
                        status: if completed {
                            status(item["status"].as_str().unwrap_or("completed"))
                        } else {
                            crate::chat_interactions::InteractionStatus::Working
                        },
                    },
                    id,
                ));
                output_events
            }
        }
    }
    fn goal(&mut self, goal: &Value) -> Vec<RuntimeEvent> {
        use crate::chat_interactions::{ChatInteraction, InteractionStatus};
        let objective = goal["objective"].as_str().unwrap_or("");
        let mut details = objective.chars().take(4000).collect::<String>();
        let native_status = goal["status"].as_str().unwrap_or("");
        if !native_status.is_empty() {
            details.push_str(&format!(
                "\n\nState: {}",
                native_status.chars().take(32).collect::<String>()
            ));
        }
        if let Some(tokens) = goal["tokensUsed"].as_u64() {
            details.push_str(&format!("\nTokens used: {tokens}"));
        }
        if let Some(budget) = goal["tokenBudget"].as_u64() {
            details.push_str(&format!("\nToken budget: {budget}"));
        }
        if let Some(seconds) = goal["timeUsedSeconds"].as_u64() {
            details.push_str(&format!("\nActive time: {seconds}s"));
        }
        self.parser.interaction(
            ChatInteraction::Status {
                title: if goal.is_null() {
                    "No active goal"
                } else {
                    "Conversation goal"
                }
                .into(),
                details,
                status: match native_status {
                    "active" => InteractionStatus::Working,
                    "paused" => InteractionStatus::Cancelled,
                    "blocked" | "budgetLimited" | "usageLimited" => InteractionStatus::Failed,
                    _ => InteractionStatus::Completed,
                },
            },
            "native-goal",
        )
    }
    fn cache_file_changes(&mut self, thread: String, item: &Value) {
        let Some(id) = item["id"].as_str().filter(|id| id.len() <= 256) else {
            return;
        };
        let key = format!("{thread}:{id}");
        let mut details = String::new();
        let mut complete = true;
        for change in item["changes"].as_array().into_iter().flatten() {
            for field in ["path", "kind", "diff"] {
                if let Some(value) = change.get(field) {
                    let text = if let Some(s) = value.as_str() {
                        s.to_owned()
                    } else {
                        summary(value, 65537)
                    };
                    if details.len() + text.len() + 2 > 65536 {
                        complete = false;
                        break;
                    }
                    details.push_str(&text);
                    details.push('\n');
                }
            }
            if !complete {
                break;
            }
        }
        if !self.file_changes.contains_key(&key) {
            self.file_order.push_back(key.clone());
        }
        self.file_changes.insert(
            key,
            if complete && !details.is_empty() {
                Some(details)
            } else {
                None
            },
        );
        while self.file_order.len() > 64 {
            if let Some(old) = self.file_order.pop_front() {
                self.file_changes.remove(&old);
            }
        }
    }
    fn request(
        &mut self,
        method: &str,
        wire_id: &Value,
        p: &Value,
        stdin: &mut std::process::ChildStdin,
        ctx: &NativeContext,
    ) -> Result<()> {
        let oversized = p["questions"].as_array().is_some_and(|v| {
            v.len() > 16
                || v.iter().any(|q| {
                    q["options"].as_array().is_some_and(|a| a.len() > 32)
                        || ["header", "question", "id"]
                            .iter()
                            .any(|k| q[*k].as_str().is_some_and(|s| s.len() > 8192))
                })
        }) || p["requestedSchema"]["properties"]
            .as_object()
            .is_some_and(|v| v.len() > 32 || v.values().any(|q| form_options(q).len() > 32));
        if oversized {
            return write_message(
                stdin,
                &ctx.cancelled,
                &json!({"id":wire_id,"error":{"code":-32602,"message":"The complete question/form exceeds the native display budget"}}),
            );
        }
        if self.pending.len() >= 64 {
            return write_message(
                stdin,
                &ctx.cancelled,
                &json!({"id":wire_id,"error":{"code":-32000,"message":"Too many unresolved requests"}}),
            );
        }
        // Never authorize an incompletely displayed command or grant scope.
        // Large normal output remains streamed/spilled; permission metadata is
        // deliberately rejected rather than silently clipped.
        if [
            "command",
            "cwd",
            "grantRoot",
            "reason",
            "permissions",
            "networkApprovalContext",
            "proposedExecpolicyAmendment",
            "proposedNetworkPolicyAmendments",
        ]
        .iter()
        .any(|key| p.get(key).is_some_and(|v| summary(v, 32769).len() > 32768))
        {
            return write_message(
                stdin,
                &ctx.cancelled,
                &json!({"id":wire_id,"error":{"code":-32602,"message":"Approval details exceed the safe display size"}}),
            );
        }
        let token = uuid::Uuid::new_v4().to_string();
        let approval = || NativeRequestKind::Approval {
            choices: vec![
                option("accept", "Allow once"),
                option("acceptForSession", "Allow this session"),
                option("decline", "Decline"),
                option("cancel", "Stop turn"),
            ],
        };
        let kind = match method {
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval" => {
                approval()
            }
            "execCommandApproval" | "applyPatchApproval" => NativeRequestKind::Approval {
                choices: vec![
                    option("approved", "Allow once"),
                    option("denied", "Decline"),
                    option("abort", "Stop turn"),
                ],
            },
            "item/permissions/requestApproval" => {
                let mut choices = vec![
                    option("accept", "Allow this turn"),
                    option("acceptForSession", "Allow this session"),
                    option("decline", "Decline"),
                ];
                if !p["permissions"]["network"].is_null() {
                    choices.push(option("networkOnly", "Allow network this turn"));
                }
                if !p["permissions"]["fileSystem"].is_null() {
                    choices.push(option("filesOnly", "Allow files this turn"));
                }
                NativeRequestKind::Approval { choices }
            }
            "item/tool/requestUserInput" | "tool/requestUserInput" => {
                NativeRequestKind::Questions {
                    questions: p["questions"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .take(16)
                        .map(|q| NativeQuestion {
                            id: q["id"].as_str().unwrap_or("").into(),
                            header: q["header"]
                                .as_str()
                                .unwrap_or("")
                                .chars()
                                .take(200)
                                .collect(),
                            question: {
                                let mut text = q["question"].as_str().unwrap_or("").to_owned();
                                for option in q["options"].as_array().into_iter().flatten() {
                                    if let Some(description) =
                                        option["description"].as_str().filter(|s| !s.is_empty())
                                    {
                                        text.push_str(&format!(
                                            "\n{} — {description}",
                                            option["label"].as_str().unwrap_or("")
                                        ));
                                    }
                                }
                                text
                            },
                            secret: q["isSecret"].as_bool().unwrap_or(false),
                            multiple: false,
                            required: true,
                            options: q["options"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .take(32)
                                .map(|o| {
                                    option(
                                        o["label"].as_str().unwrap_or(""),
                                        o["label"].as_str().unwrap_or(""),
                                    )
                                })
                                .collect(),
                        })
                        .collect(),
                }
            }
            "mcpServer/elicitation/request" => {
                if p["mode"] == "url" {
                    NativeRequestKind::Approval {
                        choices: vec![
                            option("accept", "Continue"),
                            option("decline", "Decline"),
                            option("cancel", "Cancel"),
                        ],
                    }
                } else if p["mode"] == "form"
                    || p["mode"] == "openai/form"
                    || p["mode"] == "openaiForm"
                {
                    NativeRequestKind::Questions {
                        questions: p["requestedSchema"]["properties"]
                            .as_object()
                            .into_iter()
                            .flat_map(|o| o.iter())
                            .take(32)
                            .map(|(id, schema)| NativeQuestion {
                                id: id.clone(),
                                header: schema["title"]
                                    .as_str()
                                    .unwrap_or(id)
                                    .chars()
                                    .take(200)
                                    .collect(),
                                question: schema["description"]
                                    .as_str()
                                    .unwrap_or(id)
                                    .chars()
                                    .take(8192)
                                    .collect(),
                                secret: schema["format"] == "password",
                                multiple: schema["type"] == "array",
                                required: p["requestedSchema"]["required"]
                                    .as_array()
                                    .is_some_and(|a| a.iter().any(|s| s == id)),
                                options: form_options(schema),
                            })
                            .collect(),
                    }
                } else {
                    return unsupported(stdin, &ctx.cancelled, wire_id, method);
                }
            }
            _ => return unsupported(stdin, &ctx.cancelled, wire_id, method),
        };
        let title = match method {
            "item/commandExecution/requestApproval" | "execCommandApproval" => {
                if !p["networkApprovalContext"].is_null() {
                    "Approve network access"
                } else if p["kind"] == "writeStdin" {
                    "Approve terminal input"
                } else {
                    "Approve terminal command"
                }
            }
            "item/fileChange/requestApproval" | "applyPatchApproval" => "Approve file changes",
            "item/permissions/requestApproval" => "Approve sandbox permissions",
            "mcpServer/elicitation/request" => "Connected tool needs input",
            _ => "Agent question",
        };
        let mut detail = Vec::new();
        for key in [
            "command",
            "reason",
            "message",
            "url",
            "cwd",
            "grantRoot",
            "networkApprovalContext",
        ] {
            if let Some(value) = p.get(key).filter(|v| !v.is_null()) {
                detail.push(if let Some(s) = value.as_str() {
                    s.to_owned()
                } else {
                    summary(value, 32768)
                });
            }
        }
        if method == "item/permissions/requestApproval" {
            detail.push(summary(&p["permissions"], 32768));
        }
        if method == "item/fileChange/requestApproval" {
            let key = format!(
                "{}:{}",
                p["threadId"].as_str().unwrap_or(""),
                p["itemId"].as_str().unwrap_or("")
            );
            let Some(Some(changes)) = self.file_changes.get(&key) else {
                return write_message(
                    stdin,
                    &ctx.cancelled,
                    &json!({"id":wire_id,"error":{"code":-32602,"message":"Complete file change details are unavailable for this approval; no edits were approved"}}),
                );
            };
            detail.push(changes.clone());
        }
        if method == "applyPatchApproval" {
            let changes = summary(&p["fileChanges"], 65537);
            if changes.len() > 65536 {
                return write_message(
                    stdin,
                    &ctx.cancelled,
                    &json!({"id":wire_id,"error":{"code":-32602,"message":"File change approval is too large to display completely"}}),
                );
            }
            detail.push(changes);
        }
        let mut kind = kind;
        if method == "item/commandExecution/requestApproval"
            && let NativeRequestKind::Approval { choices } = &mut kind
        {
            if p["proposedExecpolicyAmendment"].is_array() {
                choices.push(option("execpolicy", "Allow matching command in future"));
                detail.push(format!(
                    "Proposed command rule: {}",
                    summary(&p["proposedExecpolicyAmendment"], 32768)
                ));
            }
            for (index, amendment) in p["proposedNetworkPolicyAmendments"]
                .as_array()
                .into_iter()
                .flatten()
                .take(16)
                .enumerate()
            {
                choices.push(option(
                    &format!("network:{index}"),
                    "Apply proposed network rule",
                ));
                detail.push(format!(
                    "Proposed network rule {index}: {}",
                    summary(amendment, 32768)
                ));
            }
        }
        if matches!(
            method,
            "item/commandExecution/requestApproval" | "item/fileChange/requestApproval"
        ) && let Some(offered) = p["availableDecisions"].as_array()
            && let NativeRequestKind::Approval { choices } = &mut kind
        {
            choices.retain(|choice| offered.iter().any(|decision| {
                if choice.id == "execpolicy" {
                    return decision["acceptWithExecpolicyAmendment"]["execpolicy_amendment"]
                        .as_array().is_some_and(|rule| Some(rule) == p["proposedExecpolicyAmendment"].as_array());
                }
                if let Some(index) = choice.id.strip_prefix("network:").and_then(|s| s.parse::<usize>().ok()) {
                    return p["proposedNetworkPolicyAmendments"].get(index)
                        .is_some_and(|rule| !decision["applyNetworkPolicyAmendment"]["network_policy_amendment"].is_null()
                            && rule == &decision["applyNetworkPolicyAmendment"]["network_policy_amendment"]);
                }
                decision.as_str() == Some(choice.id.as_str())
            }));
        }
        let ui = NativeRequest {
            id: token.clone(),
            method: method.into(),
            title: title.into(),
            detail: detail.join("\n"),
            kind,
        };
        let child = self.service.as_ref().and_then(|service| {
            p["threadId"]
                .as_str()
                .filter(|id| service.child(id))
                .map(|id| (service.clone(), id.to_owned()))
        });
        if let Some((service, child)) = child {
            service.request(&child, ui.clone());
        } else {
            // Parent requests use the per-turn channel; children use the broker
            // only, so their inputs and opaque tokens survive parent completion.
            let events = self.parser.interaction(
                crate::chat_interactions::ChatInteraction::NativeRequest(ui.clone()),
                &token,
            );
            self.events(events, ctx)?;
            send(
                &ctx.tx,
                RuntimeEvent::Request(Box::new(ui.clone())),
                &ctx.cancelled,
            )?;
        }
        self.pending.insert(
            token,
            Pending {
                wire_id: wire_id.clone(),
                method: method.into(),
                params: p.clone(),
                ui,
            },
        );
        Ok(())
    }
    fn child_control(
        &mut self,
        child: &str,
        action: broker::ChildAction,
        stdin: &mut std::process::ChildStdin,
        ctx: &NativeContext,
    ) -> Result<()> {
        let service = self
            .service
            .as_ref()
            .context("This worker does not own children")?;
        anyhow::ensure!(
            service.child(child),
            "This child does not belong to this service"
        );
        let (method, params) = match action {
            broker::ChildAction::Respond { id, answer } => {
                anyhow::ensure!(
                    self.pending
                        .get(&id)
                        .is_some_and(|pending| pending.params["threadId"] == child),
                    "This approval is not pending for this child"
                );
                return self.control(NativeControl::Respond { id, answer }, stdin, ctx);
            }
            broker::ChildAction::Stop => {
                let turn = service
                    .turn(child)
                    .context("This child has not reported an active turn ID yet")?;
                ("turn/interrupt", json!({"threadId":child,"turnId":turn}))
            }
            broker::ChildAction::Steer(text) => {
                let turn = service
                    .turn(child)
                    .context("This child has not reported an active turn ID yet")?;
                (
                    "turn/steer",
                    json!({"threadId":child,"expectedTurnId":turn,"input":[{"type":"text","text":text,"text_elements":[]}]}),
                )
            }
        };
        write_message(
            stdin,
            &ctx.cancelled,
            &json!({"id":format!("code-child-control:{child}:{}",uuid::Uuid::new_v4()),"method":method,"params":params}),
        )
    }
    fn control(
        &mut self,
        control: NativeControl,
        stdin: &mut std::process::ChildStdin,
        ctx: &NativeContext,
    ) -> Result<()> {
        match control {
            NativeControl::Snapshot(reply)=>{let _=reply.try_send(self.parser.rendered_source().map_err(|e|e.to_string()));}
            NativeControl::Respond { id, answer } => {
                let pending = self
                    .pending
                    .get(&id)
                    .context("This request is no longer waiting for an answer")?;
                let result = response(pending, &answer)?;
                write_message(
                    stdin,
                    &ctx.cancelled,
                    &json!({"id":pending.wire_id,"result":result}),
                )?;
                let title = pending.ui.title.clone();
                let child = self.service.as_ref().is_some_and(|service| {
                    pending.params["threadId"]
                        .as_str()
                        .is_some_and(|id| service.child(id))
                });
                self.pending.remove(&id);
                if let Some(service) = &self.service {
                    service.resolve(&id);
                }
                if child {
                    return Ok(());
                }
                send(
                    &ctx.tx,
                    RuntimeEvent::RequestResolved(id.clone()),
                    &ctx.cancelled,
                )?;
                let details = match answer {
                    NativeAnswer::Cancel => "Cancelled",
                    NativeAnswer::Choice(choice) => {
                        if choice == "decline" || choice == "denied" {
                            "Declined"
                        } else if choice == "cancel" || choice == "abort" {
                            "Cancelled"
                        } else if choice.starts_with("network:") {
                            "Network rule applied"
                        } else if choice == "acceptForSession" || choice == "approved_for_session" {
                            "Approved for this session"
                        } else {
                            "Approved"
                        }
                    }
                    NativeAnswer::Answers(_) => "Answer sent",
                };
                let events = self.parser.interaction(
                    crate::chat_interactions::ChatInteraction::Status {
                        title,
                        details: details.into(),
                        status: crate::chat_interactions::InteractionStatus::Completed,
                    },
                    &format!("resolved:{id}"),
                );
                self.events(events, ctx)?;
            }
            NativeControl::Steer(text) => {
                if self.voice.is_some() {
                    let id = self.next_id;
                    self.next_id += 1;
                    return rpc(
                        stdin,
                        &ctx.cancelled,
                        id,
                        "thread/realtime/appendText",
                        json!({"threadId":self.thread,"text":text}),
                    );
                }
                anyhow::ensure!(
                    self.command_process.is_empty(),
                    "This is a sandboxed command; it does not accept agent steering"
                );
                anyhow::ensure!(!self.turn.is_empty(), "Codex has no active turn to steer");
                let id = self.next_id;
                self.next_id += 1;
                self.calls.insert(id, "Steering accepted".into());
                rpc(
                    stdin,
                    &ctx.cancelled,
                    id,
                    "turn/steer",
                    json!({"threadId":self.thread,"expectedTurnId":self.turn,"input":[{"type":"text","text":text,"text_elements":[]}]}),
                )?;
            }
            NativeControl::Interrupt => {
                if self.voice.as_ref().is_some_and(|voice| !voice.coding) {
                    // Speech interruption travels on WebRTC. There is no
                    // coding turn to interrupt, and the call stays connected.
                    return Ok(());
                }
                if !self.command_process.is_empty() {
                    let id = self.next_id;
                    self.next_id += 1;
                    return rpc(
                        stdin,
                        &ctx.cancelled,
                        id,
                        "command/exec/terminate",
                        json!({"processId":self.command_process}),
                    );
                }
                anyhow::ensure!(
                    !self.turn.is_empty(),
                    "Codex has no active turn to interrupt"
                );
                let id = self.next_id;
                self.next_id += 1;
                rpc(
                    stdin,
                    &ctx.cancelled,
                    id,
                    "turn/interrupt",
                    json!({"threadId":self.thread,"turnId":self.turn}),
                )?;
            }
            NativeControl::Rpc { method, mut params } => {
                // Deliberate exposed native controls only; arbitrary dynamic
                // tools, unrestricted terminals, and permission bypasses cannot
                // be injected through the UI's generic control channel.
                anyhow::ensure!(
                    matches!(
                        method.as_str(),
                        "thread/compact/start"
                            | "thread/realtime/stop"
                            | "thread/realtime/appendText"
                            | "thread/realtime/listVoices"
                            | "thread/rollback"
                            | "thread/read"
                            | "thread/turns/list"
                            | "thread/items/list"
                            | "account/read"
                            | "account/rateLimits/read"
                            | "model/list"
                            | "skills/list"
                            | "mcpServerStatus/list"
                            | "config/read"
                    ),
                    "This native command is not exposed by Code"
                );
                if method.starts_with("thread/") {
                    params["threadId"] = json!(self.thread);
                }
                let id = self.next_id;
                self.next_id += 1;
                self.calls.insert(id, method.clone());
                rpc(stdin, &ctx.cancelled, id, &method, params)?;
            }
        }
        Ok(())
    }
}
fn voice_tail(text: &str, max: usize) -> &str {
    let mut start = text.len().saturating_sub(max);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    &text[start..]
}

fn unsupported(
    stdin: &mut std::process::ChildStdin,
    cancelled: &AtomicBool,
    id: &Value,
    method: &str,
) -> Result<()> {
    write_message(
        stdin,
        cancelled,
        &json!({"id":id,"error":{"code":-32601,"message":format!("Code does not expose this client capability: {method}")}}),
    )
}
fn option(id: &str, label: &str) -> NativeOption {
    NativeOption {
        id: id.into(),
        label: label.into(),
    }
}
fn summary(value: &Value, limit: usize) -> String {
    struct Preview {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl Write for Preview {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            let n = bytes.len().min(self.limit.saturating_sub(self.bytes.len()));
            self.bytes.extend_from_slice(&bytes[..n]);
            if n < bytes.len() {
                return Err(io::Error::other("Preview complete"));
            }
            Ok(n)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut preview = Preview {
        bytes: Vec::with_capacity(limit.min(8192)),
        limit,
    };
    let _ = serde_json::to_writer_pretty(&mut preview, value);
    while std::str::from_utf8(&preview.bytes).is_err() {
        preview.bytes.pop();
    }
    String::from_utf8(preview.bytes).unwrap_or_default()
}

fn form_options(schema: &Value) -> Vec<NativeOption> {
    if schema["type"] == "boolean" {
        return vec![option("true", "Yes"), option("false", "No")];
    }
    let field = if schema["type"] == "array" {
        &schema["items"]
    } else {
        schema
    };
    if let Some(values) = field["enum"].as_array() {
        return values
            .iter()
            .enumerate()
            .filter_map(|(index, value)| {
                let id = value.as_str()?;
                Some(option(
                    id,
                    schema["enumNames"]
                        .get(index)
                        .and_then(Value::as_str)
                        .unwrap_or(id),
                ))
            })
            .collect();
    }
    field["oneOf"]
        .as_array()
        .or_else(|| field["anyOf"].as_array())
        .into_iter()
        .flatten()
        .filter_map(|value| {
            let id = value["const"].as_str()?;
            Some(option(id, value["title"].as_str().unwrap_or(id)))
        })
        .collect()
}
fn validate_form_value(schema: &Value, value: &Value) -> Result<()> {
    let options = form_options(schema);
    if schema["type"] == "array" {
        let values = value.as_array().context("Choose an array of values")?;
        anyhow::ensure!(
            schema["minItems"]
                .as_u64()
                .is_none_or(|n| values.len() >= n as usize)
                && schema["maxItems"]
                    .as_u64()
                    .is_none_or(|n| values.len() <= n as usize),
            "Choose the requested number of values"
        );
        if !options.is_empty() {
            anyhow::ensure!(
                values.iter().all(|v| v
                    .as_str()
                    .is_some_and(|s| options.iter().any(|o| o.id == s))),
                "Choose one of the offered values"
            );
        }
    } else if let Some(text) = value.as_str() {
        let length = text.chars().count() as u64;
        anyhow::ensure!(
            schema["minLength"].as_u64().is_none_or(|n| length >= n)
                && schema["maxLength"].as_u64().is_none_or(|n| length <= n),
            "Enter text within the requested length"
        );
        if !options.is_empty() {
            anyhow::ensure!(
                options.iter().any(|o| o.id == text),
                "Choose one of the offered values"
            );
        }
    } else if let Some(number) = value.as_f64() {
        anyhow::ensure!(
            schema["minimum"].as_f64().is_none_or(|n| number >= n)
                && schema["maximum"].as_f64().is_none_or(|n| number <= n),
            "Enter a number within the requested range"
        );
    }
    Ok(())
}

fn native_plan(value: &Value) -> String {
    value
        .as_array()
        .into_iter()
        .flatten()
        .take(64)
        .map(|step| {
            let marker = match step["status"].as_str() {
                Some("completed") => "✓",
                Some("inProgress" | "in_progress") => "→",
                _ => "○",
            };
            format!(
                "{marker} {}",
                step["step"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(1024)
                    .collect::<String>()
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_base64(text: &str) -> Result<Vec<u8>> {
    anyhow::ensure!(
        text.len() <= 1024 * 1024 && text.len().is_multiple_of(4),
        "Invalid terminal output encoding"
    );
    let mut result = Vec::with_capacity(text.len() / 4 * 3);
    let digit = |b| -> Result<u8> {
        Ok(match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 0,
            _ => bail!("Invalid terminal output encoding"),
        })
    };
    for (index, group) in text.as_bytes().chunks_exact(4).enumerate() {
        anyhow::ensure!(
            group[0] != b'='
                && group[1] != b'='
                && (group[2] != b'=' || group[3] == b'=')
                && (index + 1 == text.len() / 4 || !group.contains(&b'=')),
            "Invalid terminal output encoding"
        );
        let [a, b, c, d] = [
            digit(group[0])?,
            digit(group[1])?,
            digit(group[2])?,
            digit(group[3])?,
        ];
        result.push((a << 2) | (b >> 4));
        if group[2] != b'=' {
            result.push((b << 4) | (c >> 2));
        }
        if group[3] != b'=' {
            result.push((c << 6) | d);
        }
    }
    Ok(result)
}

fn status(value: &str) -> crate::chat_interactions::InteractionStatus {
    use crate::chat_interactions::InteractionStatus as S;
    match value {
        "failed" | "errored" => S::Failed,
        "interrupted" | "shutdown" => S::Cancelled,
        "completed" => S::Completed,
        _ => S::Working,
    }
}
fn response(p: &Pending, answer: &NativeAnswer) -> Result<Value> {
    if matches!(answer, NativeAnswer::Cancel) {
        return Ok(match p.method.as_str() {
            "item/tool/requestUserInput" | "tool/requestUserInput" => json!({"answers":{}}),
            "mcpServer/elicitation/request" => json!({"action":"cancel","content":null}),
            "item/permissions/requestApproval" => json!({"permissions":{},"scope":"turn"}),
            "execCommandApproval" | "applyPatchApproval" => json!({"decision":"abort"}),
            _ => json!({"decision":"cancel"}),
        });
    }
    match (&p.ui.kind, answer) {
        (NativeRequestKind::Approval { choices }, NativeAnswer::Choice(choice)) => {
            anyhow::ensure!(
                choices.iter().any(|o| o.id == *choice),
                "That decision is not offered by this request"
            );
            if p.method == "item/permissions/requestApproval" {
                let permissions = match choice.as_str() {
                    "accept" | "acceptForSession" => p.params["permissions"].clone(),
                    "networkOnly" => json!({"network":p.params["permissions"]["network"]}),
                    "filesOnly" => json!({"fileSystem":p.params["permissions"]["fileSystem"]}),
                    _ => json!({}),
                };
                return Ok(
                    json!({"permissions":permissions,"scope":if choice=="acceptForSession"{"session"}else{"turn"}}),
                );
            }
            if p.method == "mcpServer/elicitation/request" {
                return Ok(json!({"action":choice,"content":null}));
            }
            if choice == "execpolicy" {
                return Ok(
                    json!({"decision":{"acceptWithExecpolicyAmendment":{"execpolicy_amendment":p.params["proposedExecpolicyAmendment"]}}}),
                );
            }
            if let Some(index) = choice
                .strip_prefix("network:")
                .and_then(|s| s.parse::<usize>().ok())
            {
                let amendment = p.params["proposedNetworkPolicyAmendments"]
                    .get(index)
                    .context("This network rule is no longer offered")?;
                return Ok(
                    json!({"decision":{"applyNetworkPolicyAmendment":{"network_policy_amendment":amendment}}}),
                );
            }
            if choice == "denied"
                && matches!(
                    p.method.as_str(),
                    "execCommandApproval" | "applyPatchApproval"
                )
            {
                return Ok(
                    json!({"decision":{"denied":{"rejection":"User declined this request in Code"}}}),
                );
            }
            Ok(json!({"decision":choice}))
        }
        (NativeRequestKind::Questions { questions }, NativeAnswer::Answers(answers)) => {
            anyhow::ensure!(answers.len() <= questions.len(), "Unexpected answer fields");
            for q in questions {
                anyhow::ensure!(
                    !q.required
                        || answers.get(&q.id).is_some_and(
                            |v| !v.is_empty() && v.iter().any(|s| !s.trim().is_empty())
                        ),
                    "Answer all required questions"
                );
            }
            for (id, values) in answers {
                anyhow::ensure!(
                    questions.iter().any(|q| q.id == *id),
                    "Unexpected answer field"
                );
                anyhow::ensure!(
                    values.len() <= 32 && values.iter().all(|s| s.len() <= 8192),
                    "Answer exceeds the native input limit"
                );
            }
            if p.method == "mcpServer/elicitation/request" {
                let mut content = serde_json::Map::new();
                for (id, values) in answers {
                    if values.is_empty() && questions.iter().any(|q| q.id == *id && !q.required) {
                        continue;
                    }
                    let schema = &p.params["requestedSchema"]["properties"][id];
                    let text = values.first().map(String::as_str).unwrap_or("");
                    let value = match schema["type"].as_str() {
                        Some("boolean") => {
                            json!(text.parse::<bool>().context("Enter true or false")?)
                        }
                        Some("integer") => {
                            json!(text.parse::<i64>().context("Enter a whole number")?)
                        }
                        Some("number") => {
                            let n = text.parse::<f64>().context("Enter a number")?;
                            anyhow::ensure!(n.is_finite(), "Enter a finite number");
                            json!(n)
                        }
                        Some("array") => json!(values),
                        _ => json!(text),
                    };
                    validate_form_value(schema, &value)?;
                    content.insert(id.clone(), value);
                }
                Ok(json!({"action":"accept","content":content}))
            } else {
                let mapped: BTreeMap<_, _> = answers
                    .iter()
                    .map(|(id, values)| (id, json!({"answers":values})))
                    .collect();
                Ok(json!({"answers":mapped}))
            }
        }
        _ => bail!("The answer does not match this native request"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn fixture(body: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("code-native-rpc-{}", uuid::Uuid::new_v4()));
        std::fs::write(&path,format!("#!/usr/bin/python3\nimport json,sys,time\ndef send(v):\n print(json.dumps(v),flush=True)\ndef recv():\n v=json.loads(sys.stdin.readline())\n if v.get('method')=='config/read':\n  send({{'id':v['id'],'result':{{'config':{{'model_provider':'openai'}}}}}})\n  return recv()\n return v\n{body}")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }
    fn request(path: &std::path::Path) -> RunRequest {
        RunRequest { reasoning_effort:None,
            agent: AgentKind::Codex,
            executable: path.into(),
            project: std::env::temp_dir(),
            prompt: "Hello".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: false,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            codex_access: Default::default(),
            trust_workspace: false,
        }
    }
    #[test]
    fn voice_appshot_uses_real_image_input_before_realtime_on_same_native_thread(){
        let thread="12345678-1234-4234-8234-123456789012";
        let script=fixture(r#"v=recv();send({'id':v['id'],'result':{}})
assert recv()['method']=='initialized'
v=recv();assert v['method']=='thread/start';send({'id':v['id'],'result':{'thread':{'id':'12345678-1234-4234-8234-123456789012'}}})
v=recv();assert v['method']=='turn/start'
assert any(i['type']=='localImage' for i in v['params']['input'])
assert 'untrusted' in v['params']['input'][0]['text']
send({'id':v['id'],'result':{'turn':{'id':'context-turn'}}})
send({'method':'turn/started','params':{'threadId':'12345678-1234-4234-8234-123456789012','turn':{'id':'context-turn'}}})
send({'method':'item/agentMessage/delta','params':{'threadId':'12345678-1234-4234-8234-123456789012','itemId':'context','delta':'The fixture image is blue.'}})
send({'method':'turn/completed','params':{'threadId':'12345678-1234-4234-8234-123456789012','turn':{'id':'context-turn','status':'completed'}}})
v=recv();assert v['method']=='thread/realtime/start';assert '_codePrepareVoiceContext' not in v['params']
sid=v['params']['realtimeSessionId'];send({'id':v['id'],'result':{}})
send({'method':'thread/realtime/started','params':{'threadId':'12345678-1234-4234-8234-123456789012','realtimeSessionId':sid}})
v=recv();assert v['method']=='thread/realtime/stop';send({'id':v['id'],'result':{}})
send({'method':'thread/realtime/closed','params':{'threadId':'12345678-1234-4234-8234-123456789012','realtimeSessionId':sid,'reason':'userRequested'}})
"#);
        let image=std::env::temp_dir().join(format!("code-voice-context-{}.png",uuid::Uuid::new_v4()));
        image::RgbaImage::new(8,8).save(&image).unwrap();
        let mut r=request(&script);r.prompt="Describe untrusted captured context".into();r.images=vec![image.clone()];
        let mut params=crate::voice::Start{session_id:uuid::Uuid::new_v4().to_string(),sdp:"v=0\r\nfixture".into(),voice:None}.params().unwrap();params["_codePrepareVoiceContext"]=json!(true);
        let handle=maintenance(r,"thread/realtime/start",params).unwrap();let mut text=String::new();let mut started=false;
        while let Ok(event)=handle.events.recv_timeout(Duration::from_secs(10)){
            match event{RuntimeEvent::Text(t)=>text.push_str(&t),RuntimeEvent::Voice(crate::voice::Signal::Started{..})=>{started=true;handle.native_command("thread/realtime/stop",json!({})).unwrap()},RuntimeEvent::Failed(e)=>panic!("{e}"),RuntimeEvent::Finished=>break,_=>{}}
        }
        handle.cancel();assert!(started,"Realtime did not follow context turn in {thread}");assert!(text.contains("fixture image is blue"));let _=std::fs::remove_file(script);let _=std::fs::remove_file(image);
    }
    #[test]
    #[ignore="Native Codex subscription inference in an isolated temporary workspace"]
    fn installed_codex_streams_readable_reasoning(){
        let project=std::env::temp_dir().join(format!("kai-agent-ui-reasoning-{}",uuid::Uuid::new_v4()));std::fs::create_dir_all(&project).unwrap();
        let executable=std::env::var("KAI_NATIVE_TEST_EXECUTABLE").unwrap_or_else(|_|"codex".into());
        let mut r=request(std::path::Path::new(&executable));r.project=project;r.model=std::env::var("KAI_NATIVE_TEST_MODEL").ok();r.reasoning_effort=Some("high".into());r.prompt="For a scheduler that runs up to four agents, explain which invariants prevent a canceled startup from attaching to another chat. Consider an out-of-order history load, a model change during launch and new work during a close animation. Give a concise final design. Do not use tools.".into();
        let handle=launch(r).unwrap();let mut seen=false;let started=Instant::now();
        while started.elapsed()<Duration::from_secs(90){
            let event=handle.events.recv_timeout(Duration::from_secs(30)).expect("Native Codex stalled");
            let source=match event{RuntimeEvent::Interaction(card)=>crate::chat_interactions::serialize_interaction(&card),RuntimeEvent::ReplaceText(text)|RuntimeEvent::ReplacePreview(text)|RuntimeEvent::Text(text)=>text,RuntimeEvent::Failed(error)=>panic!("{error}"),RuntimeEvent::Finished=>break,_=>continue};
            seen |= crate::presentation::message_blocks("assistant",&source).iter().any(|b|matches!(b,crate::presentation::MessageBlock::Interaction(crate::chat_interactions::ChatInteraction::Status{title,details,..})if title=="Thinking"&&!details.is_empty()));
        }
        handle.cancel();assert!(seen,"Native subscription did not emit readable reasoning with detailed summaries");eprintln!("Real Codex subscription streamed a readable reasoning summary in {:.1}s",started.elapsed().as_secs_f64());
    }
    #[test]
    fn voice_partials_update_one_transcript_card_and_final_corrections_replace_it(){
        let (_handle,ctx)=native_handle(0);*ctx.group.lock().unwrap()=None;
        let mut state=Session::default();state.thread="thread".into();state.voice=Some(VoiceState{id:"voice".into(),..Default::default()});
        let item=json!({"id":"segment","type":"transcriptSegment","role":"assistant","text":"Hello"});
        state.voice_notification("thread/realtime/item/started",&json!({"threadId":"thread","item":item}),&ctx).unwrap();
        assert!(state.parser.rendered_output().contains("Hello"),"Initial transcript must appear before completion");
        state.voice_notification("thread/realtime/item/transcript/delta",&json!({"threadId":"thread","itemId":"segment","delta":" 🐈 friend"}),&ctx).unwrap();
        let partial=state.parser.rendered_output();assert!(partial.contains("Hello 🐈 friend"));assert_eq!(state.parser.interactions.len(),1);
        let done=json!({"threadId":"thread","item":{"id":"segment","type":"transcriptSegment","role":"assistant","text":"Hello 🐈"}});
        state.voice_notification("thread/realtime/item/completed",&done,&ctx).unwrap();
        let final_text=state.parser.rendered_output();assert!(final_text.contains("Hello 🐈"));assert!(!final_text.contains("friend"));assert_eq!(state.parser.interactions.len(),1);assert!(state.parser.live_interactions.is_empty());
        state.voice_notification("thread/realtime/item/completed",&done,&ctx).unwrap();assert_eq!(state.parser.interactions.len(),1);
        state.voice_notification("thread/realtime/item/started",&json!({"threadId":"thread","item":item}),&ctx).unwrap();assert_eq!(state.parser.interactions.len(),1,"A replayed start duplicated the finalized segment");
    }
    #[test]fn live_snapshot_reads_the_retained_source_without_a_provider_rpc(){
        let(handle,ctx)=native_handle(0);*ctx.group.lock().unwrap()=None;
        let mut state=Session::default();state.parser.output.push_str(&"Earlier work 🐈\n".repeat(9000));
        let received=handle.live_snapshot().unwrap();let request=ctx.control.try_recv().unwrap();
        let mut child=std::process::Command::new("/usr/bin/true").stdin(std::process::Stdio::piped()).spawn().unwrap();let mut stdin=child.stdin.take().unwrap();
        state.control(request,&mut stdin,&ctx).unwrap();let source=received.try_recv().unwrap().unwrap();assert!(source.len()>64*1024);assert!(source.tail(1024).unwrap().contains("Earlier work"));child.wait().unwrap();
    }
    #[test]fn canonical_voice_relay_does_not_split_or_duplicate_the_written_answer(){
        let mut state=Session::default();state.parser.output.push_str("Read [source](https://example.com/ter");
        state.parser.interaction(crate::chat_interactions::ChatInteraction::Speaker{speaker:crate::chat_interactions::Speaker::Assistant,text:"Read this source".into()},"voice-relay");
        state.parser.output.push_str("ra/).");
        let text=state.parser.rendered_output();assert!(text.contains("https://example.com/terra/"));assert!(!text.contains("Read this source"));
        state.parser.interaction(crate::chat_interactions::ChatInteraction::Speaker{speaker:crate::chat_interactions::Speaker::User,text:"Thanks".into()},"next-user");
        state.parser.interaction(crate::chat_interactions::ChatInteraction::Speaker{speaker:crate::chat_interactions::Speaker::Assistant,text:"Welcome".into()},"next-voice");assert!(state.parser.rendered_output().contains("Welcome"));
    }
    #[test]
    fn legacy_voice_deltas_keep_separate_utterances_and_clear_retracted_text(){
        let (_handle,ctx)=native_handle(0);*ctx.group.lock().unwrap()=None;let mut state=Session::default();state.thread="thread".into();state.voice=Some(VoiceState{id:"voice".into(),..Default::default()});
        for (delta,done) in [("First words","First words."),("Retracted","")]{
            state.voice_notification("thread/realtime/transcript/delta",&json!({"threadId":"thread","role":"assistant","delta":delta}),&ctx).unwrap();
            assert!(state.parser.rendered_output().contains(delta));
            state.voice_notification("thread/realtime/transcript/done",&json!({"threadId":"thread","role":"assistant","text":done}),&ctx).unwrap();
        }
        let text=state.parser.rendered_output();assert!(text.contains("First words."));assert!(!text.contains("Retracted"));assert!(state.parser.live_interactions.is_empty());assert!(state.voice.as_ref().unwrap().flat_ids.is_empty());
    }
    #[test]
    fn reasoning_sections_update_one_card_and_final_snapshot_replaces_stream(){
        let mut state=Session::default();state.request_reasoning=true;
        let r=request(std::path::Path::new("/fixture"));assert_eq!(state.turn_params(&r)["summary"],"detailed");state.request_reasoning=false;assert!(state.turn_params(&r)["summary"].is_null());
        state.item(&json!({"id":"reason-one","type":"reasoning","summary":[]}),false);
        state.reasoning.entry("reason-one".into()).or_default().insert(0,"Streamed thought".into());
        state.parser.upsert_interaction(crate::chat_interactions::ChatInteraction::Status{title:"Thinking".into(),details:"Streamed thought".into(),status:crate::chat_interactions::InteractionStatus::Working},"reasoning:reason-one");
        state.item(&json!({"id":"reason-one","type":"reasoning","summary":["Authoritative thought","Second section"]}),true);
        let text=state.parser.rendered_output();assert_eq!(text.matches("<orbit_interaction version=").count(),1);assert!(text.contains("Authoritative thought"));assert!(text.contains("Second section"));assert!(!text.contains("Streamed thought"));assert!(text.contains("completed"));assert!(state.reasoning.is_empty());assert!(state.parser.interactions.len()==1);
    }
    #[test]
    fn launch_deadline_does_not_limit_active_turns_or_native_maintenance(){
        let mut state=Session::default();assert!(state.launch_pending());
        state.thread="thread".into();assert!(state.launch_pending());
        state.turn="turn".into();assert!(!state.launch_pending());
        state.turn.clear();state.maintenance=Some(("thread/compact/start".into(),json!({})));assert!(!state.launch_pending());
        state.thread.clear();state.command_process="terminal".into();assert!(!state.launch_pending());
        state.calls.insert(10,"turn/start".into());assert!(state.launch_pending());
    }
    #[test]
    fn direct_https_transport_preserves_custom_native_provider_configuration(){
        let r=request(std::path::Path::new("/fixture"));
        for config in [json!({}),json!({"model_provider":"openai"})]{
            assert_eq!(routed_thread_params(&r,&config)["modelProvider"],"kai-openai-https");
        }
        assert!(routed_thread_params(&r,&json!({"model_provider":"my-provider"}))["modelProvider"].is_null());
    }
    #[test]
    fn native_turn_transmits_selected_reasoning_without_changing_model_id(){
        let mut r=request(std::path::Path::new("/fixture"));r.model=Some("exact-model".into());r.reasoning_effort=Some("high".into());
        assert_eq!(thread_params(&r)["model"],"exact-model");assert_eq!(turn_params("thread",&r)["effort"],"high");
        r.reasoning_effort=None;assert!(turn_params("thread",&r)["effort"].is_null());
    }
    #[test]
    fn native_voice_uses_webrtc_same_chat_approvals_and_survives_delegated_turn_completion() {
        let voice_id = uuid::Uuid::new_v4().to_string();
        let path = fixture(
            r#"v=recv()
assert v['method']=='initialize' and v['params']['capabilities']['experimentalApi']
send({'id':v['id'],'result':{}})
assert recv()['method']=='initialized'
v=recv()
assert v['method']=='thread/start'
assert v['params']['approvalPolicy']=='on-request' and v['params']['sandbox']=='read-only'
t='12345678-1234-4234-8234-123456789012'
send({'id':v['id'],'result':{'thread':{'id':t}}})
v=recv();assert v['method']=='thread/realtime/start',v
p=v['params'];sid=p['realtimeSessionId']
assert p['threadId']==t and p['transport']=={'type':'webrtc','sdp':'v=0\r\nOFFER'}
assert p['version']=='v3' and p['model']=='gpt-live-1-codex'
assert p['clientManagedHandoffs'] is False and p['voice']=='cove'
send({'id':v['id'],'result':{}})
send({'method':'thread/realtime/started','params':{'threadId':t,'realtimeSessionId':sid,'version':'v3'}})
send({'method':'thread/realtime/sdp','params':{'threadId':t,'sdp':'v=0\r\nANSWER'}})
item={'id':'spoken-1','realtimeSessionId':sid,'type':'transcriptSegment','role':'user','text':'Inspect the repository'}
send({'method':'thread/realtime/item/started','params':{'threadId':t,'item':item}})
send({'method':'thread/realtime/item/completed','params':{'threadId':t,'item':item}})
send({'method':'turn/started','params':{'threadId':t,'turn':{'id':'voice-code-1'}}})
send({'id':'native-question','method':'item/tool/requestUserInput','params':{'threadId':t,'turnId':'voice-code-1','itemId':'ask','questions':[{'id':'q','question':'Which branch?','options':[{'label':'main'}]}]}})
v=recv();assert v['id']=='native-question' and v['result']['answers']['q']['answers']==['main'],v
send({'method':'turn/completed','params':{'threadId':t,'turn':{'id':'voice-code-1','status':'completed'}}})
send({'method':'thread/realtime/transcript/delta','params':{'threadId':t,'role':'assistant','delta':'SHOULD_NOT_DUPLICATE'}})
item={'id':'spoken-2','realtimeSessionId':sid,'type':'transcriptSegment','role':'assistant','text':'I am still here'}
send({'method':'thread/realtime/item/started','params':{'threadId':t,'item':item}})
send({'method':'thread/realtime/item/completed','params':{'threadId':t,'item':item}})
v=recv();assert v['method']=='thread/realtime/stop' and v['params']['threadId']==t,v
send({'id':v['id'],'result':{}})
send({'method':'thread/realtime/closed','params':{'threadId':t,'reason':'stopped'}})
while sys.stdin.readline(): pass
"#,
        );
        let project = broker_project();
        let mut req = request(&path);
        req.project = project.clone();
        let handle = maintenance(
            req,
            "thread/realtime/start",
            crate::voice::Start {
                session_id: voice_id.clone(),
                sdp: "v=0\r\nOFFER".into(),
                voice: Some("cove".into()),
            }
            .params()
            .unwrap(),
        )
        .unwrap();
        let mut started = false;
        let mut answer = false;
        let mut asked = false;
        let mut closed = false;
        let mut text = String::new();
        let mut transcripts = 0;
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            assert!(Instant::now() < deadline, "Voice fixture timed out");
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Voice(crate::voice::Signal::Started { session_id }) => {
                    assert_eq!(session_id, voice_id);
                    started = true;
                }
                RuntimeEvent::Voice(crate::voice::Signal::Answer { sdp, .. }) => {
                    assert_eq!(sdp, "v=0\r\nANSWER");
                    answer = true;
                }
                RuntimeEvent::Voice(crate::voice::Signal::Transcript {
                    final_result: true,
                    text: spoken,
                    ..
                }) => {
                    transcripts += 1;
                    if spoken == "I am still here" {
                        handle
                            .native_command("thread/realtime/stop", json!({}))
                            .unwrap();
                    }
                }
                RuntimeEvent::Voice(crate::voice::Signal::Closed { .. }) => closed = true,
                RuntimeEvent::Request(request) => {
                    asked = true;
                    handle
                        .respond(
                            &request.id,
                            NativeAnswer::Answers(BTreeMap::from([(
                                "q".into(),
                                vec!["main".into()],
                            )])),
                        )
                        .unwrap();
                }
                RuntimeEvent::Interaction(card) => {
                    if let crate::chat_interactions::ChatInteraction::Speaker {
                        text: spoken, ..
                    } = card.as_ref()
                    {
                        text.push_str(spoken);
                    }
                }
                RuntimeEvent::Text(delta) => text.push_str(&delta),
                RuntimeEvent::ReplaceText(full) => text = full,
                RuntimeEvent::Failed(error) => panic!("{error}"),
                RuntimeEvent::Finished => {
                    assert!(closed, "Delegated turn ended the voice session");
                    break;
                }
                _ => {}
            }
        }
        assert!(started && answer && asked && closed);
        assert_eq!(transcripts, 2);
        assert!(text.contains("Inspect the repository") && text.contains("I am still here"));
        assert!(!text.contains("SHOULD_NOT_DUPLICATE"));
        let done = broker::shutdown_service(
            &project,
            AgentKind::Codex,
            "12345678-1234-4234-8234-123456789012",
        );
        wait_for(|| done.is_closed());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(project).unwrap();
    }

    #[test]
    fn approval_modes_preserve_the_sandbox_and_enable_native_questions_for_all_thread_paths() {
        for mode in [
            CodexApprovals::Ask,
            CodexApprovals::AutoReview,
            CodexApprovals::Never,
        ] {
            for edits in [false, true] {
                for resume in [false, true] {
                    let mut req = request(std::path::Path::new("/fixture/codex"));
                    req.codex_approvals = mode;
                    req.allow_edits = edits;
                    req.resume_id = resume.then(|| "12345678-1234-4234-8234-123456789012".into());
                    let params = thread_params(&req);
                    assert_eq!(params["approvalPolicy"], mode.policy());
                    assert_eq!(params["approvalsReviewer"], mode.reviewer());
                    assert_eq!(
                        params["sandbox"],
                        if edits {
                            "workspace-write"
                        } else {
                            "read-only"
                        }
                    );
                    assert_eq!(
                        params["config"]["features.default_mode_request_user_input"],
                        true
                    );
                    assert_eq!(params["threadId"].is_string(), resume);
                    req.fork_session = resume;
                    assert_eq!(thread_params(&req)["config"], params["config"]);
                }
            }
        }
    }

    #[test]
    fn selected_approval_mode_reaches_both_native_thread_and_turn_without_auto_answering_questions()
    {
        for mode in [CodexApprovals::AutoReview, CodexApprovals::Never] {
            let start = START
                .replace("=='on-request'", &format!("=='{}'", mode.policy()))
                .replace("=='user'", &format!("=='{}'", mode.reviewer()));
            let body = format!(
                r#"{start}
assert v['params']['approvalPolicy']=='{policy}'
assert v['params']['approvalsReviewer']=='{reviewer}'
send({{'id':'question','method':'item/tool/requestUserInput','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turnId':'turn-1','itemId':'ask','questions':[{{'id':'choice','question':'Continue?','options':[{{'label':'Yes'}}]}}]}}}})
v=recv()
assert v['result']=={{'answers':{{'choice':{{'answers':['Yes']}}}}}},v
send({{'method':'item/agentMessage/delta','params':{{'threadId':'12345678-1234-4234-8234-123456789012','itemId':'reply','delta':'Answered'}}}})
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-1','status':'completed'}}}}}})
"#,
                policy = mode.policy(),
                reviewer = mode.reviewer()
            );
            let path = fixture(&body);
            let mut req = request(&path);
            req.codex_approvals = mode;
            let handle = launch(req).unwrap();
            let mut asked = false;
            loop {
                match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    RuntimeEvent::Request(request) => {
                        asked = true;
                        assert!(matches!(request.kind, NativeRequestKind::Questions { .. }));
                        handle
                            .respond(
                                &request.id,
                                NativeAnswer::Answers(BTreeMap::from([(
                                    "choice".into(),
                                    vec!["Yes".into()],
                                )])),
                            )
                            .unwrap();
                    }
                    RuntimeEvent::Failed(error) => panic!("{error}"),
                    RuntimeEvent::Finished => break,
                    _ => {}
                }
            }
            assert!(asked);
            assert!(handle.settled.recv_blocking().is_err());
            std::fs::remove_file(path).unwrap();
        }
    }

    const START: &str = r#"v=recv()
assert v['method']=='initialize'
assert v['params']['capabilities']['experimentalApi'] is True
send({'id':v['id'],'result':{}})
assert recv()['method']=='initialized'
v=recv()
assert v['method'] in ['thread/start','thread/resume','thread/fork']
assert v['params']['config']['features.default_mode_request_user_input'] is True
assert v['params']['approvalPolicy']=='on-request'
assert v['params']['sandbox']=='read-only'
assert v['params']['approvalsReviewer']=='user'
send({'id':v['id'],'result':{'thread':{'id':'12345678-1234-4234-8234-123456789012'}}})
v=recv()
assert v['method']=='turn/start'
send({'id':v['id'],'result':{'turn':{'id':'turn-1'}}})
send({'method':'turn/started','params':{'threadId':'12345678-1234-4234-8234-123456789012','turn':{'id':'turn-1'}}})
"#;
    fn broker_project() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("code-broker-fixture-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&path).unwrap();
        path
    }
    fn wait_for(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !predicate() {
            assert!(Instant::now() < deadline, "Native broker fixture timed out");
            thread::sleep(Duration::from_millis(5));
        }
    }
    fn finish(handle: &RunHandle) -> String {
        let mut text = String::new();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Text(delta) => text.push_str(&delta),
                RuntimeEvent::ReplaceText(full) => text = full,
                RuntimeEvent::Failed(error) => panic!("{error}"),
                RuntimeEvent::Finished => return text,
                _ => {}
            }
        }
    }
    #[test]
    fn native_quota_notification_preserves_both_windows_and_exact_resets() {
        let ready = std::env::temp_dir().join(format!("code-quota-ready-{}", uuid::Uuid::new_v4()));
        let ready_path = ready.to_string_lossy();
        let path = fixture(&format!(
            r#"{START}
send({{'method':'account/rateLimits/updated','params':{{'rateLimits':{{'primary':{{'usedPercent':0,'windowDurationMins':300,'resetsAt':1800000000}},'secondary':{{'usedPercent':42,'windowDurationMins':10080,'resetsAt':1800600000}}}}}}}})
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-1','status':'completed'}}}}}})
import os
while not os.path.exists({ready_path:?}): time.sleep(0.005)
send({{'method':'account/rateLimits/updated','params':{{'rateLimits':{{'primary':{{'usedPercent':9,'windowDurationMins':300,'resetsAt':1800000000}}}}}}}})
while sys.stdin.readline(): pass
"#
        ));
        let project = broker_project();
        let mut request = request(&path);
        request.project = project.clone();
        let handle = launch(request).unwrap();
        let mut windows = Vec::new();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Usage(usage) if !usage.limit_windows.is_empty() => {
                    windows = usage.limit_windows
                }
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
        assert_eq!(windows.len(), 2);
        assert!(windows.iter().any(|window| window.used_percent == 0.
            && window.duration_minutes == Some(300)
            && window.resets_at == Some(1800000000)));
        assert!(windows.iter().any(|window| window.used_percent == 42.
            && window.duration_minutes == Some(10080)
            && window.resets_at == Some(1800600000)));
        std::fs::write(&ready, "parent finished").unwrap();
        wait_for(|| {
            broker::services().iter().any(|service| {
                service.project == project
                    && service.usage.as_ref().is_some_and(|usage| {
                        usage.limit_windows.len() == 2
                            && usage
                                .limit_windows
                                .iter()
                                .any(|window| window.used_percent == 9.)
                            && usage
                                .limit_windows
                                .iter()
                                .any(|window| window.used_percent == 42.)
                    })
            })
        });
        let done = broker::shutdown_service(
            &project,
            AgentKind::Codex,
            "12345678-1234-4234-8234-123456789012",
        );
        wait_for(|| done.is_closed());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(ready).unwrap();
        std::fs::remove_dir(project).unwrap();
    }
    #[test]
    fn owned_service_reuses_handshake_and_old_handle_cannot_cancel_the_next_turn() {
        let path = fixture(&format!(
            r#"{START}
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-1','status':'completed'}}}}}})
v=recv()
assert v['method']=='thread/resume',v
send({{'id':v['id'],'result':{{'thread':{{'id':'12345678-1234-4234-8234-123456789012'}}}}}})
v=recv()
assert v['method']=='turn/start'
send({{'id':v['id'],'result':{{'turn':{{'id':'turn-2'}}}}}})
send({{'method':'turn/started','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-2'}}}}}})
send({{'method':'item/agentMessage/delta','params':{{'threadId':'12345678-1234-4234-8234-123456789012','itemId':'reply','delta':'Second reply'}}}})
v=recv()
assert v['method']=='turn/steer' and v['params']['expectedTurnId']=='turn-2',v
send({{'id':v['id'],'result':{{}}}})
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-2','status':'completed'}}}}}})
while sys.stdin.readline(): pass
"#
        ));
        let project = broker_project();
        let mut next = request(&path);
        next.project = project.clone();
        let first = launch(next.clone()).unwrap();
        let pid = first.process_group.lock().unwrap().unwrap();
        finish(&first);
        assert!(first.settled.is_closed());
        next.resume_id = Some("12345678-1234-4234-8234-123456789012".into());
        let second = launch(next).unwrap();
        assert_eq!(*second.process_group.lock().unwrap(), Some(pid));
        loop {
            if matches!(
                second.events.recv_timeout(Duration::from_secs(5)).unwrap(),
                RuntimeEvent::Text(_)
            ) {
                break;
            }
        }
        drop(first);
        thread::sleep(Duration::from_millis(2100));
        second.steer("Finish this fixture").unwrap();
        finish(&second);
        let stopped = broker::shutdown_service(
            &project,
            AgentKind::Codex,
            "12345678-1234-4234-8234-123456789012",
        );
        wait_for(|| stopped.is_closed());
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(project).unwrap();
    }
    #[test]
    fn background_child_approval_steering_stop_and_spilled_transcript_outlive_parent_reply() {
        const CHILD: &str = "22345678-1234-4234-8234-123456789012";
        let project = broker_project();
        let progress = project.join("provider-phase");
        let progress_path = progress.to_string_lossy();
        let path = fixture(&format!(
            r#"{START}
def mark(phase):
 with open({progress_path:?},'a') as f: f.write(phase+'\n')
root='12345678-1234-4234-8234-123456789012'
child='{CHILD}'
mark('child registration')
send({{'method':'thread/started','params':{{'thread':{{'id':child,'parentThreadId':root,'status':{{'type':'active','activeFlags':[]}}}}}}}})
v=recv()
assert v['method']=='thread/resume' and v['params']=={{'threadId':child,'excludeTurns':True}},v
send({{'id':v['id'],'result':{{'thread':{{'id':child,'status':{{'type':'active'}},'turns':[]}}}}}})
v=recv()
assert v['method']=='thread/turns/list' and v['params']['limit']==1,v
send({{'id':v['id'],'result':{{'data':[{{'id':'child-turn','status':'inProgress'}}]}}}})
send({{'method':'turn/started','params':{{'threadId':child,'turn':{{'id':'child-turn'}}}}}})
send({{'method':'item/agentMessage/delta','params':{{'threadId':root,'itemId':'parent-reply','delta':'Parent finished'}}}})
send({{'method':'turn/completed','params':{{'threadId':root,'turn':{{'id':'turn-1','status':'completed'}}}}}})
parts=[]
mark('streaming deltas')
for i in range(2100):
 text='Child 🌊 '+str(i)+'\n'+'x'*8192
 parts.append(text)
 if i%100==0: mark('delta '+str(i))
 send({{'method':'item/agentMessage/delta','params':{{'threadId':child,'turnId':'child-turn','itemId':'child-reply','delta':text}}}})
mark('writing 17 MB final snapshot')
send({{'method':'item/completed','params':{{'threadId':child,'item':{{'id':'child-reply','type':'agentMessage','text':''.join(parts)}}}}}})
mark('snapshot written; writing approval')
send({{'id':'child-approval','method':'item/commandExecution/requestApproval','params':{{'threadId':child,'turnId':'child-turn','itemId':'child-command','command':'printf child','cwd':'/tmp','reason':'Fixture command'}}}})
mark('waiting for approval reply')
v=recv()
assert v['id']=='child-approval' and v['result']['decision']=='accept',v
mark('waiting for child steering')
v=recv()
assert v['method']=='turn/steer' and v['params']['threadId']==child and v['params']['expectedTurnId']=='child-turn',v
send({{'id':v['id'],'result':{{}}}})
send({{'method':'item/agentMessage/delta','params':{{'threadId':child,'itemId':'child-reply','delta':'STEER_CONFIRMED'}}}})
mark('waiting for child stop')
v=recv()
assert v['method']=='turn/interrupt' and v['params']['threadId']==child and v['params']['turnId']=='child-turn',v
send({{'id':v['id'],'result':{{}}}})
send({{'method':'turn/completed','params':{{'threadId':child,'turn':{{'id':'child-turn','status':'interrupted'}}}}}})
mark('child interrupted; waiting for shutdown')
while sys.stdin.readline(): pass
"#
        ));
        let mut request = request(&path);
        request.project = project.clone();
        let parent = launch(request).unwrap();
        let pid = parent.process_group.lock().unwrap().unwrap();
        assert_eq!(finish(&parent), "Parent finished");
        assert!(
            !parent.settled.is_closed(),
            "Child work must keep checkpoint sealing open"
        );
        let wait = |phase: &str, timeout: Duration, predicate: &mut dyn FnMut() -> bool| {
            let start = Instant::now();
            while !predicate() {
                assert!(
                    start.elapsed() < timeout,
                    "Native broker phase {phase:?} timed out after {:?}; provider progress: {}; child state: {:?}",
                    start.elapsed(),
                    std::fs::read_to_string(&progress).unwrap_or_default(),
                    broker::children(&project)
                );
                thread::sleep(Duration::from_millis(5));
            }
            eprintln!("Native broker phase {phase:?}: {:?}", start.elapsed());
        };
        let mut token = String::new();
        // This phase processes 17 MB of deltas plus a second 17 MB wire
        // snapshot. Bound bulk I/O separately from approval/control latency;
        // the full suite intentionally runs many giant-history tests in parallel.
        wait(
            "bulk transcript then child approval",
            Duration::from_secs(30),
            &mut || {
                if let Some(child) = broker::children(&project)
                    .iter()
                    .find(|child| child.id == CHILD)
                    && let Some(request) = child.requests.first()
                {
                    token = request.id.clone();
                    return true;
                }
                false
            },
        );
        assert!(broker::has_child_request(
            &project,
            AgentKind::Codex,
            CHILD,
            &token
        ));
        broker::control_child(
            &project,
            AgentKind::Codex,
            CHILD,
            broker::ChildAction::Respond {
                id: token.clone(),
                answer: NativeAnswer::Choice("invalid-choice".into()),
            },
        )
        .unwrap();
        wait(
            "invalid approval rejected",
            Duration::from_secs(5),
            &mut || {
                broker::take_child_request_error(&project, AgentKind::Codex, CHILD, &token)
                    .is_some()
            },
        );
        assert!(broker::has_child_request(
            &project,
            AgentKind::Codex,
            CHILD,
            &token
        ));
        broker::control_child(
            &project,
            AgentKind::Codex,
            CHILD,
            broker::ChildAction::Respond {
                id: token.clone(),
                answer: NativeAnswer::Choice("accept".into()),
            },
        )
        .unwrap();
        wait(
            "accepted approval resolved",
            Duration::from_secs(5),
            &mut || !broker::has_child_request(&project, AgentKind::Codex, CHILD, &token),
        );
        broker::control_child(
            &project,
            AgentKind::Codex,
            CHILD,
            broker::ChildAction::Steer("Focus on tests".into()),
        )
        .unwrap();
        wait(
            "child steering confirmed",
            Duration::from_secs(5),
            &mut || {
                broker::children(&project)
                    .iter()
                    .any(|child| child.id == CHILD && child.preview.contains("STEER_CONFIRMED"))
            },
        );
        assert!(!parent.settled.is_closed());
        broker::control_child(&project, AgentKind::Codex, CHILD, broker::ChildAction::Stop)
            .unwrap();
        wait(
            "child stop and turn quiescence",
            Duration::from_secs(5),
            &mut || parent.settled.is_closed(),
        );
        let child = broker::children(&project)
            .into_iter()
            .find(|child| child.id == CHILD)
            .unwrap();
        assert_eq!(
            child.status,
            crate::chat_interactions::InteractionStatus::Cancelled
        );
        assert!(!broker::has_active_children(&project));
        assert!(child.transcript_bytes > super::super::LIVE_OUTPUT_LIMIT);
        let expected_bytes = (0..2100)
            .map(|i| format!("Child 🌊 {i}\n").len() + 8192)
            .sum::<usize>()
            + "STEER_CONFIRMED".len();
        assert_eq!(child.transcript_bytes, expected_bytes);
        let mut complete = String::new();
        let mut offset = 0;
        loop {
            let page =
                broker::child_transcript_page(&project, AgentKind::Codex, CHILD, offset).unwrap();
            assert!(page.text.len() <= 64 * 1024);
            complete.push_str(&page.text);
            offset = page.next_offset;
            if offset == page.total_bytes {
                break;
            }
        }
        assert_eq!(complete.len(), child.transcript_bytes);
        assert!(complete.starts_with("Child 🌊 0\n"));
        assert!(complete.ends_with("STEER_CONFIRMED"));
        assert!(!complete.contains("Parent finished"));
        let done = broker::shutdown_service(
            &project,
            AgentKind::Codex,
            "12345678-1234-4234-8234-123456789012",
        );
        wait(
            "provider shutdown and reap",
            Duration::from_secs(5),
            &mut || done.is_closed(),
        );
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        std::fs::remove_file(path).unwrap();
        std::fs::remove_file(progress).unwrap();
        std::fs::remove_dir(project).unwrap();
    }
    #[test]
    fn multi_megabyte_final_review_preserves_every_finding_in_paged_cards() {
        let expected = format!(
            "{}FINAL_REVIEW_FINDING",
            "Finding · 🦀 <source>\n".repeat(100_000)
        );
        let bytes = serde_json::to_vec(&json!({"method":"item/completed","params":{"item":{"id":"review-large","type":"exitedReviewMode","review":expected}}})).unwrap();
        let mut record = wire::Record::default();
        for chunk in bytes.chunks(8192) {
            record.push(chunk).unwrap();
        }
        assert!(record.small().is_none());
        let (projected, shortened) = record.projected(&AtomicBool::new(false)).unwrap();
        assert!(!shortened);
        let mut state = Session::default();
        let events = state.item(&projected["params"]["item"], true);
        let mut actual = String::new();
        for event in events {
            if let RuntimeEvent::Interaction(card) = event
                && let crate::chat_interactions::ChatInteraction::Status { details, .. } =
                    card.as_ref()
            {
                assert!(details.len() <= 16 * 1024);
                actual.push_str(details);
            }
        }
        assert_eq!(actual, expected);
        let canonical = state.parser.rendered_output();
        let mut saved = String::new();
        for block in crate::presentation::blocks(&canonical) {
            if let crate::presentation::MessageBlock::Interaction(
                crate::chat_interactions::ChatInteraction::Status { details, .. },
            ) = block
            {
                saved.push_str(&details);
            }
        }
        assert_eq!(
            saved, expected,
            "The saved transcript must retain findings beyond the visible preview"
        );
    }
    #[test]
    fn native_reviews_and_goals_use_rpc_without_a_prompt_and_wait_for_review_completion() {
        let actions = [
            super::super::codex_actions::review("base main").unwrap(),
            super::super::codex_actions::goal("set Verify native controls").unwrap(),
            super::super::codex_actions::goal("").unwrap(),
            super::super::codex_actions::goal("clear").unwrap(),
        ];
        for action in actions {
            let startup = START
                .split("assert v['method']=='turn/start'")
                .next()
                .unwrap();
            let body = format!(
                r#"{startup}
assert v['method']=={method:?}, v
assert v['params']['threadId']=='12345678-1234-4234-8234-123456789012'
assert 'input' not in v['params']
if v['method']=='review/start':
 assert v['params']['delivery']=='inline' and v['params']['target']=={{'type':'baseBranch','branch':'main'}}
 send({{'id':v['id'],'result':{{'turn':{{'id':'review-1'}}}}}})
 send({{'method':'item/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','item':{{'id':'review-1','type':'exitedReviewMode','review':'REVIEW_FINDING_OK'}}}}}})
 send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'review-1','status':'completed'}}}}}})
elif v['method']=='thread/goal/clear':
 assert v['params']['origin']=='user'
 send({{'id':v['id'],'result':{{'cleared':True}}}})
else:
 if v['method']=='thread/goal/set':
  assert v['params']['objective']=='Verify native controls' and v['params']['origin']=='user'
  assert 'tokenBudget' not in v['params']
 send({{'id':v['id'],'result':{{'goal':{{'objective':'Verify native controls','status':'active','tokensUsed':12,'tokenBudget':99,'timeUsedSeconds':3}}}}}})
time.sleep(30)
"#,
                method = action.method
            );
            let path = fixture(&body);
            let mut request = request(&path);
            request.resume_id = Some("12345678-1234-4234-8234-123456789012".into());
            let handle = maintenance(request, action.method, action.params).unwrap();
            let mut detail = String::new();
            loop {
                match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    RuntimeEvent::Interaction(card) => detail.push_str(&format!("{card:?}")),
                    RuntimeEvent::Finished => break,
                    RuntimeEvent::Failed(error) => panic!("{error}"),
                    _ => {}
                }
            }
            match action.method {
                "review/start" => assert!(detail.contains("REVIEW_FINDING_OK")),
                "thread/goal/clear" => assert!(detail.contains("Goal cleared")),
                _ => assert!(
                    detail.contains("Token budget: 99") && detail.contains("Tokens used: 12")
                ),
            }
            assert!(handle.settled.recv_blocking().is_err());
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn native_hook_tool_notice_and_model_events_remain_typed_without_polluting_prose() {
        let path = fixture(&format!(
            r#"{START}
thread='12345678-1234-4234-8234-123456789012'
for method,params in [
 ('hook/started',{{'run':{{'id':'hook-1','eventName':'preToolUse','status':'running','statusMessage':'Checking'}}}}),
 ('hook/completed',{{'run':{{'id':'hook-1','eventName':'preToolUse','status':'completed','entries':[{{'text':'HOOK_OUTPUT_OK'}}]}}}}),
 ('item/mcpToolCall/progress',{{'itemId':'tool-1','message':'MCP_PROGRESS_OK'}}),
 ('configWarning',{{'summary':'CONFIG_WARNING_OK','details':'Check project configuration'}}),
 ('model/rerouted',{{'fromModel':'previous','toModel':'real-model','reason':'rateLimited'}}),
 ('thread/goal/updated',{{'goal':{{'objective':'GOAL_OUTPUT_OK','status':'paused','tokensUsed':7}}}}),
 ('item/reasoning/summaryTextDelta',{{'itemId':'thinking-1','delta':'THINKING_OK'}}),
]:
 params['threadId']=thread
 send({{'method':method,'params':params}})
send({{'method':'turn/completed','params':{{'threadId':thread,'turn':{{'id':'turn-1','status':'completed'}}}}}})
time.sleep(30)
"#
        ));
        let handle = launch(request(&path)).unwrap();
        let mut cards = String::new();
        let mut reported_model = None;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Interaction(card) => {
                    if let crate::chat_interactions::ChatInteraction::Status {
                        details, status, ..
                    } = card.as_ref()
                        && details.contains("GOAL_OUTPUT_OK")
                    {
                        assert_eq!(
                            *status,
                            crate::chat_interactions::InteractionStatus::Cancelled,
                            "A paused native goal must not be presented as running or complete"
                        );
                    }
                    cards.push_str(&format!("{card:?}"));
                }
                RuntimeEvent::Text(_) | RuntimeEvent::ReplaceText(_) => {
                    panic!("Native events must not enter assistant prose")
                }
                RuntimeEvent::Usage(usage) if usage.model.is_some() => reported_model = usage.model,
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
        for marker in [
            "HOOK_OUTPUT_OK",
            "MCP_PROGRESS_OK",
            "CONFIG_WARNING_OK",
            "GOAL_OUTPUT_OK",
            "THINKING_OK",
        ] {
            assert!(cards.contains(marker), "Missing {marker}: {cards}");
        }
        assert!(cards.contains("Thinking"));
        assert_eq!(reported_model.as_deref(), Some("real-model"));
        assert!(handle.settled.recv_blocking().is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_terminal_exec_uses_a_real_sandbox_and_streams_utf8_without_a_thread() {
        let path = fixture(
            r#"v=recv()
assert v['method']=='initialize'
send({'id':v['id'],'result':{}})
assert recv()['method']=='initialized'
v=recv()
assert v['method']=='command/exec'
p=v['params']
assert p['command']==['/bin/zsh','-c','printf test']
assert p['sandboxPolicy']=={'type':'readOnly','networkAccess':False}
assert p['streamStdoutStderr'] is True and p['disableOutputCap'] is True
assert p['tty'] is False and p['disableTimeout'] is True and 'timeoutMs' not in p
process=p['processId']
send({'method':'command/exec/outputDelta','params':{'processId':process,'stream':'stdout','deltaBase64':'dGVzdCDwnw==','capReached':False}})
send({'method':'command/exec/outputDelta','params':{'processId':process,'stream':'stderr','deltaBase64':'IQ==','capReached':False}})
send({'method':'command/exec/outputDelta','params':{'processId':process,'stream':'stdout','deltaBase64':'poA=','capReached':False}})
send({'id':v['id'],'result':{'exitCode':0,'stdout':'','stderr':''}})
time.sleep(30)
"#,
        );
        let mut req = request(&path);
        req.prompt = "printf test".into();
        let handle = maintenance(req, "command/exec", json!({})).unwrap();
        let mut text = String::new();
        let mut exited = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Text(t) => text.push_str(&t),
                RuntimeEvent::Interaction(card) => {
                    exited |= matches!(card.as_ref(),crate::chat_interactions::ChatInteraction::Status{title,..} if title=="Command exited with status 0")
                }
                RuntimeEvent::SessionId(_) => {
                    panic!("Standalone terminal must not create a thread")
                }
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(text.contains("test !🦀"), "{text}");
        assert!(exited);
        assert!(handle.settled.recv_blocking().is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    #[ignore = "uses installed Codex App Server and macOS sandbox, without model inference"]
    fn installed_native_sandbox_terminal() {
        let executable = PathBuf::from(
            std::env::var_os("CODE_SMOKE_EXECUTABLE").expect("Set CODE_SMOKE_EXECUTABLE"),
        );
        let mut req = request(&executable);
        req.prompt = "printf CODE_NATIVE_SANDBOX_OK".into();
        let handle = maintenance(req, "command/exec", json!({})).unwrap();
        let mut text = String::new();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(30)).unwrap() {
                RuntimeEvent::Text(t) => text.push_str(&t),
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(text.contains("CODE_NATIVE_SANDBOX_OK"), "{text}");
        assert!(handle.settled.recv_blocking().is_err());
    }
    #[test]
    #[ignore = "uses installed Codex App Server and macOS sandbox, without model inference"]
    fn installed_native_readonly_terminal_blocks_writes() {
        let executable = PathBuf::from(
            std::env::var_os("CODE_SMOKE_EXECUTABLE").expect("Set CODE_SMOKE_EXECUTABLE"),
        );
        let folder =
            std::env::temp_dir().join(format!("code-readonly-proof-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&folder).unwrap();
        let target = folder.join("must-not-exist.txt");
        let mut req = request(&executable);
        req.project = folder.clone();
        req.prompt = "printf SHOULD_NOT_WRITE > must-not-exist.txt".into();
        let handle = maintenance(req, "command/exec", json!({})).unwrap();
        let mut failed = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(30)).unwrap() {
                RuntimeEvent::Interaction(card) => {
                    failed |= matches!(card.as_ref(),crate::chat_interactions::ChatInteraction::Status{title,status:crate::chat_interactions::InteractionStatus::Failed,..} if title.starts_with("Command exited with status"))
                }
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(failed, "The sandbox must report a nonzero command exit");
        assert!(!target.exists(), "Read-only sandbox allowed a file write");
        assert!(handle.settled.recv_blocking().is_err());
        std::fs::remove_dir_all(folder).unwrap();
    }
    #[test]
    #[ignore = "uses installed Codex App Server and checks only its known owned sandbox command PID; no inference"]
    fn installed_native_terminal_stop_reaps_owned_command() {
        let executable = PathBuf::from(
            std::env::var_os("CODE_SMOKE_EXECUTABLE").expect("Set CODE_SMOKE_EXECUTABLE"),
        );
        let mut req = request(&executable);
        req.prompt = "printf 'CODE_TERMINAL_PID=%s\\n' $$; exec /bin/sleep 30".into();
        let handle = maintenance(req, "command/exec", json!({})).unwrap();
        let mut text = String::new();
        let pid = loop {
            match handle.events.recv_timeout(Duration::from_secs(30)).unwrap() {
                RuntimeEvent::Text(delta) => {
                    text.push_str(&delta);
                    if let Some(after) = text.split("CODE_TERMINAL_PID=").nth(1)
                        && let Some((line, _)) = after.split_once('\n')
                        && let Ok(pid) = line.trim().parse::<i32>()
                    {
                        break pid;
                    }
                }
                RuntimeEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        };
        assert!(pid > 1);
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "The fixture command exited before Stop"
        );
        handle.cancel();
        assert!(handle.settled.recv_blocking().is_err());
        let deadline = Instant::now() + Duration::from_secs(3);
        while unsafe { libc::kill(pid, 0) } == 0 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        if unsafe { libc::kill(pid, 0) } == 0 {
            let command = Command::new("/bin/ps")
                .args(["-p", &pid.to_string(), "-o", "command="])
                .output()
                .unwrap();
            let command = String::from_utf8_lossy(&command.stdout);
            if command.contains("sleep 30") {
                unsafe {
                    libc::kill(pid, libc::SIGKILL);
                }
            }
            panic!("Stop left its owned native command PID alive: {command}");
        }
    }
    #[test]
    fn terminal_encoding_is_bounded_and_rejects_invalid_padding() {
        assert_eq!(decode_base64("8J+mgA==").unwrap(), "🦀".as_bytes());
        for invalid in ["=AAA", "AA=A", "AA==AAAA", "????", "A"] {
            assert!(decode_base64(invalid).is_err(), "{invalid}");
        }
        assert!(decode_base64(&"A".repeat(1024 * 1024 + 4)).is_err());
    }
    #[test]
    fn native_rpc_round_trips_approvals_questions_tools_and_completion() {
        for question_method in ["item/tool/requestUserInput", "tool/requestUserInput"] {
            let path = fixture(&format!(
                r#"{START}
send({{'id':'wire-approval','method':'item/commandExecution/requestApproval','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turnId':'turn-1','itemId':'command-1','command':'printf test','cwd':'/tmp','reason':'Run a check','availableDecisions':['decline','cancel']}}}})
v=recv()
assert v=={{'id':'wire-approval','result':{{'decision':'decline'}}}},v
send({{'id':77,'method':'{question_method}','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turnId':'turn-1','itemId':'ask-1','questions':[{{'id':'scope','header':'Scope','question':'Which scope?','options':[{{'label':'Small','description':'One file'}}]}}]}}}})
v=recv()
assert v=={{'id':77,'result':{{'answers':{{'scope':{{'answers':['Small']}}}}}}}},v
send({{'method':'item/agentMessage/delta','params':{{'threadId':'12345678-1234-4234-8234-123456789012','itemId':'reply','delta':'Ready'}}}})
send({{'method':'item/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','item':{{'id':'reply','type':'agentMessage','text':'Ready'}}}}}})
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-1','status':'completed'}}}}}})
time.sleep(30)
"#
            ));
            let handle = launch(request(&path)).unwrap();
            let mut requested = 0;
            let mut resolved = 0;
            let mut rejected = false;
            let mut text = String::new();
            loop {
                match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    RuntimeEvent::Request(r) => {
                        if let NativeRequestKind::Approval { choices } = &r.kind {
                            assert_eq!(
                                choices.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(),
                                vec!["decline", "cancel"]
                            );
                        }

                        assert_ne!(r.id, "wire-approval");
                        requested += 1;
                        if requested == 1 {
                            handle
                                .respond(&r.id, NativeAnswer::Choice("not-offered".into()))
                                .unwrap();
                        } else {
                            handle
                                .respond(
                                    &r.id,
                                    NativeAnswer::Answers(BTreeMap::from([(
                                        "scope".into(),
                                        vec!["Small".into()],
                                    )])),
                                )
                                .unwrap();
                        }
                    }
                    RuntimeEvent::RequestRejected { id, error } => {
                        assert!(error.contains("not offered"));
                        rejected = true;
                        handle
                            .respond(&id, NativeAnswer::Choice("decline".into()))
                            .unwrap();
                    }
                    RuntimeEvent::RequestResolved(_) => resolved += 1,
                    RuntimeEvent::Text(delta) => text.push_str(&delta),
                    RuntimeEvent::Finished => break,
                    RuntimeEvent::Failed(e) => panic!("{e}"),
                    _ => {}
                }
            }
            assert_eq!(requested, 2);
            assert_eq!(resolved, 2);
            assert!(
                rejected,
                "Invalid native responses must leave the original request answerable"
            );
            assert_eq!(text, "Ready");
            assert!(handle.settled.recv_blocking().is_err());
            assert!(handle.process_group.lock().unwrap().is_none());
            let source = handle.take_response_source().unwrap();
            let stored = source.read_range(0..source.len()).unwrap();
            assert!(stored.contains("native_request"));
            assert!(stored.contains("Declined"));
            assert!(!stored.contains("wire-approval"));
            std::fs::remove_file(path).unwrap();
        }
    }
    #[test]
    fn native_rpc_cancel_reaps_process_waiting_for_approval() {
        let path = fixture(&format!(
            r#"{START}
send({{'method':'item/started','params':{{'threadId':'12345678-1234-4234-8234-123456789012','item':{{'id':'files','type':'fileChange','changes':[{{'path':'/tmp/test','kind':{{'type':'add'}},'diff':'+one line'}}],'status':'inProgress'}}}}}})
send({{'id':80,'method':'item/fileChange/requestApproval','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turnId':'turn-1','itemId':'files','reason':'Write a file'}}}})
recv()
time.sleep(30)
"#
        ));
        let handle = launch(request(&path)).unwrap();
        let pid = handle.process_group.lock().unwrap().unwrap();
        loop {
            if let RuntimeEvent::Request(request) =
                handle.events.recv_timeout(Duration::from_secs(5)).unwrap()
            {
                assert!(
                    request.detail.contains("/tmp/test") && request.detail.contains("+one line")
                );
                break;
            }
        }
        handle.cancel();
        assert!(handle.settled.recv_blocking().is_err());
        assert!(handle.process_group.lock().unwrap().is_none());
        let source = handle.take_response_source().unwrap();
        assert!(
            source
                .tail(2048)
                .unwrap()
                .contains("You stopped this response.")
        );
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_rpc_unknown_requests_fail_closed_and_retry_is_nonterminal() {
        let path = fixture(&format!(
            r#"{START}
send({{'id':'unknown','method':'unknown/tool','params':{{}}}})
v=recv()
assert v['id']=='unknown' and v['error']['code']==-32601
send({{'method':'error','params':{{'error':{{'message':'Reconnecting 1/5'}},'willRetry':True}}}})
send({{'method':'item/agentMessage/delta','params':{{'threadId':'12345678-1234-4234-8234-123456789012','itemId':'reply','delta':'Recovered'}}}})
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-1','status':'completed'}}}}}})
time.sleep(30)
"#
        ));
        let handle = launch(request(&path)).unwrap();
        let mut retry = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Status(s) => retry |= s.contains("Reconnecting"),
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(retry);
        assert!(handle.settled.recv_blocking().is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_rpc_steering_and_interrupt_use_authoritative_turn_ids() {
        let path = fixture(&format!(
            r#"{START}
send({{'method':'item/agentMessage/delta','params':{{'threadId':'12345678-1234-4234-8234-123456789012','itemId':'reply','delta':'Working'}}}})
v=recv()
assert v['method']=='turn/steer' and v['params']['expectedTurnId']=='turn-1'
assert v['params']['input'][0]['text']=='Focus on tests'
send({{'id':v['id'],'result':{{}}}})
v=recv()
assert v['method']=='turn/interrupt' and v['params']['turnId']=='turn-1'
send({{'id':v['id'],'result':{{}}}})
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-1','status':'interrupted'}}}}}})
time.sleep(30)
"#
        ));
        let handle = launch(request(&path)).unwrap();
        let mut steered = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Text(_) => {
                    handle.steer("Focus on tests").unwrap();
                    handle.interrupt().unwrap();
                    steered = true;
                }
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(steered);
        assert!(handle.settled.recv_blocking().is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_tool_output_preserves_repeated_chunks_and_complete_buffered_fallback() {
        let path = fixture(&format!(
            r#"{START}
for _ in range(2):
 send({{'method':'item/commandExecution/outputDelta','params':{{'threadId':'12345678-1234-4234-8234-123456789012','itemId':'streamed','delta':'same\n'}}}})
send({{'method':'item/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','item':{{'id':'streamed','type':'commandExecution','command':'printf repeated','aggregatedOutput':'same\nsame\n','status':'completed'}}}}}})
send({{'method':'item/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','item':{{'id':'buffered','type':'commandExecution','command':'printf large','aggregatedOutput':'🦀' * 10000,'status':'completed'}}}}}})
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-1','status':'completed'}}}}}})
time.sleep(30)
"#
        ));
        let handle = launch(request(&path)).unwrap();
        let mut output = String::new();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Interaction(card) => match card.as_ref() {
                    crate::chat_interactions::ChatInteraction::Status {
                        title, details, ..
                    } if title == "Terminal output" => output.push_str(details),
                    _ => {}
                },
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert_eq!(output, format!("same\nsame\n{}", "🦀".repeat(10000)));
        assert!(handle.settled.recv_blocking().is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_oversized_persistent_rule_is_rejected_without_offering_clipped_approval() {
        let path = fixture(&format!(
            r#"{START}
send({{'id':81,'method':'item/commandExecution/requestApproval','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turnId':'turn-1','itemId':'rule','command':'echo safe','proposedExecpolicyAmendment':['x'*40000]}}}})
v=recv()
assert v['id']==81 and v['error']['code']==-32602
send({{'method':'turn/completed','params':{{'threadId':'12345678-1234-4234-8234-123456789012','turn':{{'id':'turn-1','status':'completed'}}}}}})
time.sleep(30)
"#
        ));
        let handle = launch(request(&path)).unwrap();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Request(_) => {
                    panic!("A clipped policy must not be offered for approval")
                }
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        assert!(handle.settled.recv_blocking().is_err());
        std::fs::remove_file(path).unwrap();
    }
    #[test]
    fn native_mcp_form_titles_types_and_constraints_match_wire_values() {
        let schema = json!({"type":"array","items":{"anyOf":[{"const":"fast","title":"Fast mode"},{"const":"full","title":"Full mode"}]},"minItems":1,"maxItems":2});
        assert_eq!(
            form_options(&schema),
            vec![option("fast", "Fast mode"), option("full", "Full mode")]
        );
        assert!(validate_form_value(&schema, &json!(["fast", "full"])).is_ok());
        assert!(validate_form_value(&schema, &json!([])).is_err());
        assert!(validate_form_value(&schema, &json!(["made-up"])).is_err());
        assert_eq!(
            form_options(&json!({"type":"boolean"})),
            vec![option("true", "Yes"), option("false", "No")]
        );
        assert!(
            validate_form_value(&json!({"type":"number","minimum":2,"maximum":4}), &json!(8))
                .is_err()
        );
        assert!(
            validate_form_value(
                &json!({"type":"string","minLength":2,"maxLength":3}),
                &json!("🦀🌊")
            )
            .is_ok()
        );
    }
    #[test]
    fn native_pending_request_card_round_trips_without_tag_collision() {
        let request = NativeRequest {
            id: "run-scoped-token".into(),
            method: "item/tool/requestUserInput".into(),
            title: "Question".into(),
            detail: String::new(),
            kind: NativeRequestKind::Questions {
                questions: vec![NativeQuestion {
                    id: "q".into(),
                    header: "Header".into(),
                    question: "Choose".into(),
                    options: vec![option("yes", "Yes")],
                    secret: false,
                    multiple: false,
                    required: true,
                }],
            },
        };
        let card = crate::chat_interactions::ChatInteraction::NativeRequest(request);
        let encoded = crate::chat_interactions::encode(&card);
        assert_eq!(crate::chat_interactions::decode(&encoded).unwrap(), card);
        let value: Value = serde_json::from_str(&encoded).unwrap();
        assert_eq!(value["card"]["kind"], "native_request");
        assert!(value["card"]["request_kind"].is_object());
    }
    #[test]
    fn native_responses_validate_decisions_required_and_secret_answers() {
        let mut p = Pending {
            wire_id: json!(1),
            method: "item/commandExecution/requestApproval".into(),
            params: json!({}),
            ui: NativeRequest {
                id: "opaque".into(),
                method: "x".into(),
                title: "approval".into(),
                detail: "".into(),
                kind: NativeRequestKind::Approval {
                    choices: vec![option("decline", "Decline")],
                },
            },
        };
        assert!(response(&p, &NativeAnswer::Choice("accept".into())).is_err());
        assert_eq!(
            response(&p, &NativeAnswer::Choice("decline".into())).unwrap(),
            json!({"decision":"decline"})
        );
        p.method = "mcpServer/elicitation/request".into();
        p.params = json!({"requestedSchema":{"properties":{"count":{"type":"integer"}},"required":["count"]}});
        p.ui.kind = NativeRequestKind::Questions {
            questions: vec![NativeQuestion {
                id: "count".into(),
                header: "Count".into(),
                question: "How many?".into(),
                options: vec![],
                secret: false,
                multiple: false,
                required: true,
            }],
        };
        assert!(response(&p, &NativeAnswer::Answers(BTreeMap::new())).is_err());
        assert_eq!(
            response(
                &p,
                &NativeAnswer::Answers(BTreeMap::from([("count".into(), vec!["3".into()])]))
            )
            .unwrap(),
            json!({"action":"accept","content":{"count":3}})
        );
        p.params=json!({"requestedSchema":{"properties":{"count":{"type":"boolean"}},"required":["count"]}});
        for (text,expected) in [("true",true),("false",false)] {
            assert_eq!(response(&p,&NativeAnswer::Answers(BTreeMap::from([("count".into(),vec![text.into()])]))).unwrap(),json!({"action":"accept","content":{"count":expected}}));
        }
        assert_eq!(response(&p,&NativeAnswer::Cancel).unwrap(),json!({"action":"cancel","content":null}));
    }
}

#[cfg(test)] mod access_regressions {
 use super::*;
 #[test] fn access_is_independent_of_approvals_on_new_resumed_and_subsequent_turns() {
  for access in [CodexAccess::Workspace,CodexAccess::Full] {
   for edits in [false,true] {
    for approvals in [CodexApprovals::Ask,CodexApprovals::AutoReview,CodexApprovals::Never] {
     for resume in [false,true] {
      let req=RunRequest{agent:AgentKind::Codex,executable:"/fixture/codex".into(),project:"/fixture/workspace".into(),prompt:"Continue".into(),resume_id:resume.then(||"native".into()),fork_session:false,model:None,reasoning_effort:None,images:vec![],allow_edits:edits,trust_workspace:false,codex_access:access,codex_approvals:approvals};
      let thread=thread_params(&req);let turn=turn_params("native",&req);
      assert_eq!(thread["sandbox"],access.sandbox(edits));assert_eq!(turn["sandboxPolicy"],access.policy(edits,&req.project));
      assert_eq!(turn["approvalPolicy"],approvals.policy());assert_eq!(thread["approvalPolicy"],approvals.policy());
      assert_eq!(turn["approvalsReviewer"],approvals.reviewer());
      if !edits { assert_eq!(turn["sandboxPolicy"]["type"],"readOnly"); }
     }
    }
   }
  }
 }
}

#[cfg(test)] mod installed_access_tests {
 use super::*;
 #[test]
 #[ignore="Runs installed Codex command/exec in an isolated temporary workspace"]
 fn installed_codex_enforces_workspace_and_full_access_without_approval_prompts() {
  let root=std::env::temp_dir().join(format!("kai-access-proof-{}",uuid::Uuid::new_v4()));let project=root.join("workspace");std::fs::create_dir_all(&project).unwrap();
  for (edits,access,inside_ok,outside_ok) in [(false,CodexAccess::Workspace,false,false),(true,CodexAccess::Workspace,true,false),(true,CodexAccess::Full,true,true)] {
   let inside=project.join("inside.txt");let outside=root.join("outside.txt");let _=std::fs::remove_file(&inside);let _=std::fs::remove_file(&outside);
   let prompt=format!("/usr/bin/printf inside > '{}'; /usr/bin/printf outside > '{}'; /usr/bin/printf ACCESS_PROBE_DONE",inside.display(),outside.display());
   let request=RunRequest{agent:AgentKind::Codex,executable:std::env::var("KAI_NATIVE_TEST_EXECUTABLE").unwrap_or_else(|_|"/opt/homebrew/bin/codex".into()).into(),project:project.clone(),prompt,resume_id:None,fork_session:false,model:None,reasoning_effort:None,images:vec![],allow_edits:edits,trust_workspace:false,codex_access:access,codex_approvals:CodexApprovals::Never};
   let handle=maintenance(request,"command/exec",json!({})).unwrap();let mut finished=false;
   while let Ok(event)=handle.events.recv_timeout(Duration::from_secs(20)) {
    match event {RuntimeEvent::Failed(e)=>panic!("{e}"),RuntimeEvent::Request(_)=>panic!("Never ask unexpectedly raised an approval"),RuntimeEvent::Finished=>{finished=true;break},_=>{}}
   }
   handle.cancel();assert!(finished,"Native terminal did not finish");assert_eq!(inside.is_file(),inside_ok,"Wrong workspace boundary for {access:?}, edits={edits}");assert_eq!(outside.is_file(),outside_ok,"Wrong external boundary for {access:?}");
  }
  std::fs::remove_dir_all(root).unwrap();
 }
}
