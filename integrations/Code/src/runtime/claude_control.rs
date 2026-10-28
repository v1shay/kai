//! Rust implementation of Claude Agent SDK's bidirectional JSONL protocol.
//! Contract: anthropics/claude-agent-sdk-python _internal/query.py and the
//! official Agent SDK permissions/user-input documentation. No SDK runtime.
use super::*;
use serde_json::json;
use std::collections::{BTreeMap, HashMap};

struct Pending {
    wire: String,
    input: Value,
    ui: NativeRequest,
}
#[derive(Default)]
struct Session {
    parser: StreamParser,
    pending: HashMap<String, Pending>,
    initialized: bool,
    result: Option<Instant>,
}

pub(super) fn launch(request: RunRequest) -> Result<RunHandle> {
    validate_images(&request)?;
    if let Some(model) = &request.model {
        anyhow::ensure!(valid_model_id(model), "Invalid Claude model");
    }
    let mut command = Command::new(&request.executable);
    crate::process_env::apply(&mut command);
    command.args([
        "--print",
        "--verbose",
        "--output-format",
        "stream-json",
        "--input-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
        "--permission-mode",
        if request.allow_edits {
            "default"
        } else {
            "plan"
        },
    ]);
    if !request.allow_edits {
        // Older installed Claude versions do not gate all shell writes in plan
        // mode. Remove mutating built-ins entirely instead of weakening plan.
        command.args(["--disallowedTools", "Bash,Edit,Write,NotebookEdit"]);
    }
    if let Some(model) = &request.model {
        command.arg("--model").arg(model);
    }
    if let Some(effort)=&request.reasoning_effort{
        anyhow::ensure!(["low","medium","high","xhigh","max"].contains(&effort.as_str()),"Unsupported Claude reasoning effort");
        command.arg("--effort").arg(effort);
    }
    if let Some(id) = &request.resume_id {
        anyhow::ensure!(
            !id.is_empty() && id.len() <= 256 && !id.chars().any(char::is_control),
            "Invalid Claude session ID"
        );
        command.arg(format!("--resume={id}"));
    }
    command
        .current_dir(&request.project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command.spawn().context("Could not start Claude Code")?;
    let (handle, ctx) = native_handle(child.id() as i32);
    let stdout = child.stdout.take().context("Claude output unavailable")?;
    let stderr = child
        .stderr
        .take()
        .context("Claude diagnostics unavailable")?;
    let stdin = child.stdin.take().context("Claude input unavailable")?;
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd(), stdin.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            handle.cancel();
            let mut group = ctx.group.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(pid) = *group {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = child.wait();
            *group = None;
            bail!("Could not configure Claude transport");
        }
    }
    thread::spawn(move || run(request, child, stdin, stdout, stderr, ctx));
    Ok(handle)
}
fn write(
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
fn response(
    stdin: &mut std::process::ChildStdin,
    ctx: &NativeContext,
    wire: &str,
    result: Value,
) -> Result<()> {
    write(
        stdin,
        &ctx.cancelled,
        &json!({"type":"control_response","response":{"subtype":"success","request_id":wire,"response":result}}),
    )
}
fn control_request(
    stdin: &mut std::process::ChildStdin,
    ctx: &NativeContext,
    subtype: &str,
) -> Result<()> {
    write(
        stdin,
        &ctx.cancelled,
        &json!({"type":"control_request","request_id":uuid::Uuid::new_v4().to_string(),"request":{"subtype":subtype}}),
    )
}
impl Session {
    fn message(
        &mut self,
        request: &RunRequest,
        value: Value,
        stdin: &mut std::process::ChildStdin,
        ctx: &NativeContext,
    ) -> Result<()> {
        match value["type"].as_str().unwrap_or("") {
            "control_response" if value["response"]["request_id"] == "code-initialize" => {
                if self.initialized {
                    return Ok(());
                }
                anyhow::ensure!(
                    value["response"]["subtype"] == "success",
                    "Claude rejected native protocol initialization"
                );
                self.initialized = true;
                let commands = value
                    .pointer("/response/response/commands")
                    .and_then(Value::as_array)
                    .map(|values| {
                        Value::Array(
                            values
                                .iter()
                                .take(256)
                                .filter_map(|value| value.get("name").cloned())
                                .collect(),
                        )
                    })
                    .unwrap_or(Value::Null);
                let names = crate::commands::advertised(&commands);
                if let Some(name) = crate::commands::name(&request.prompt) {
                    anyhow::ensure!(
                        names.iter().any(|known| known == name),
                        "The installed Claude no longer advertises this native command"
                    );
                }
                send(&ctx.tx, RuntimeEvent::Commands(names), &ctx.cancelled)?;
                let models = model_choices(&value["response"]["response"]["models"]);
                if !models.is_empty() {
                    send(&ctx.tx, RuntimeEvent::Models(models), &ctx.cancelled)?;
                }
                if request.images.is_empty() {
                    write(
                        stdin,
                        &ctx.cancelled,
                        &json!({"type":"user","message":{"role":"user","content":request.prompt},"parent_tool_use_id":null}),
                    )?;
                } else {
                    let done = AtomicBool::new(false);
                    let mut writer = InputPipe {
                        stdin,
                        done: &done,
                        cancelled: &ctx.cancelled,
                    };
                    write_agent_input(
                        AgentKind::Claude,
                        &request.prompt,
                        &request.images,
                        &mut writer,
                    )?;
                }
            }
            "control_response" => { /* Control acknowledgments are not chat text. */ }
            "control_cancel_request" => {
                if let Some(wire) = value["request_id"].as_str() {
                    let tokens: Vec<_> = self
                        .pending
                        .iter()
                        .filter(|(_, pending)| pending.wire == wire)
                        .map(|(token, _)| token.clone())
                        .collect();
                    for token in tokens {
                        self.pending.remove(&token);
                        send(
                            &ctx.tx,
                            RuntimeEvent::RequestResolved(token),
                            &ctx.cancelled,
                        )?;
                    }
                }
            }
            "control_request" => {
                let wire = value["request_id"]
                    .as_str()
                    .filter(|wire| wire.len() <= 256 && !wire.chars().any(char::is_control))
                    .context("Claude emitted an invalid request ID")?;
                if self.pending.values().any(|pending| pending.wire == wire) {
                    return Ok(());
                }
                let native = &value["request"];
                if native["subtype"] != "can_use_tool" {
                    return write(
                        stdin,
                        &ctx.cancelled,
                        &json!({"type":"control_response","response":{"subtype":"error","request_id":wire,"error":"This native client does not support this control request"}}),
                    );
                }
                if self.pending.len() >= 32 {
                    return response(
                        stdin,
                        ctx,
                        wire,
                        json!({"behavior":"deny","message":"Too many pending requests"}),
                    );
                }
                let pending = permission(wire, native, request.allow_edits)?;
                let token = pending.ui.id.clone();
                send(
                    &ctx.tx,
                    RuntimeEvent::Request(Box::new(pending.ui.clone())),
                    &ctx.cancelled,
                )?;
                self.pending.insert(token, pending);
            }
            _ => {
                let parent_result =
                    value["type"] == "result" && value["parent_tool_use_id"].is_null();
                dispatch_value(
                    &mut self.parser,
                    AgentKind::Claude,
                    &value,
                    &ctx.tx,
                    &ctx.cancelled,
                )?;
                if parent_result {
                    if crate::commands::name(&request.prompt).is_some()
                        && self.parser.output.is_empty()
                    {
                        send(
                            &ctx.tx,
                            RuntimeEvent::Status("Command completed".into()),
                            &ctx.cancelled,
                        )?;
                    }
                    self.result = Some(Instant::now());
                }
            }
        }
        Ok(())
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
                    .context("This Claude request is no longer pending")?;
                let result = answer_for(pending, answer)?;
                response(stdin, ctx, &pending.wire, result)?;
                self.pending.remove(&id);
                send(&ctx.tx, RuntimeEvent::RequestResolved(id), &ctx.cancelled)?;
            }
            NativeControl::Interrupt => control_request(stdin, ctx, "interrupt")?,
            NativeControl::Steer(_) => {
                bail!("Claude steering requires a new turn; finish or interrupt this turn first")
            }
            NativeControl::Rpc { .. } => bail!("This Claude control method is not exposed"),
        }
        Ok(())
    }
}
fn model_choices(value: &Value) -> Vec<crate::agent_controls::ModelChoice> {
    let mut models = Vec::<crate::agent_controls::ModelChoice>::new();
    if let Some(values) = value.as_array() {
        for model in values.iter().take(128) {
            let Some(id) = model["value"].as_str().filter(|id| valid_model_id(id)) else {
                continue;
            };
            if models.iter().any(|model| model.id == id) {
                continue;
            }
            let label = model.get("resolvedModel").unwrap_or(&model["displayName"])
                .as_str()
                .unwrap_or(id)
                .chars()
                .filter(|c| !c.is_control())
                .take(80)
                .collect();
            models.push(crate::agent_controls::ModelChoice { reasoning_efforts:model["supportedEffortLevels"].as_array().into_iter().flatten().filter_map(Value::as_str).filter(|v|["low","medium","high","xhigh","max"].contains(v)).take(8).map(str::to_owned).collect(),
                id: id.into(),
                label,
            });
        }
    }
    models
}

fn permission(wire: &str, request: &Value, allow_edits: bool) -> Result<Pending> {
    let tool = request["tool_name"]
        .as_str()
        .filter(|name| name.len() <= 256)
        .context("Claude request has no tool name")?;
    let input = request["input"]
        .as_object()
        .context("Claude request has no tool input")?;
    let input = Value::Object(input.clone());
    let token = uuid::Uuid::new_v4().to_string();
    let clean = |text: &str| {
        text.chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(12000)
            .collect::<String>()
    };
    let review = serde_json::to_string_pretty(&input)?;
    let review_complete = review.len() <= 64 * 1024;
    let kind = if tool == "AskUserQuestion" {
        let values = input["questions"]
            .as_array()
            .filter(|items| !items.is_empty() && items.len() <= 16)
            .context("Claude question request has an unsupported shape")?;
        let mut questions = Vec::new();
        for (ix, value) in values.iter().enumerate() {
            let question = value["question"]
                .as_str()
                .filter(|text| text.len() <= 8192)
                .context("Claude question text is unavailable")?;
            let options = value["options"]
                .as_array()
                .filter(|items| items.len() <= 32)
                .context("Claude question options are unavailable")?;
            questions.push(NativeQuestion {
                id: ix.to_string(),
                header: clean(value["header"].as_str().unwrap_or("Question")),
                question: question.into(),
                options: options
                    .iter()
                    .filter_map(|item| item["label"].as_str())
                    .map(|label| NativeOption {
                        id: label.into(),
                        label: clean(label),
                    })
                    .collect(),
                secret: false,
                multiple: value["multiSelect"] == true,
                required: true,
            });
        }
        NativeRequestKind::Questions { questions }
    } else {
        let mut choices = Vec::new();
        if allow_edits && review_complete {
            choices.push(NativeOption {
                id: "allow".into(),
                label: "Allow once".into(),
            });
        }
        choices.push(NativeOption {
            id: "deny".into(),
            label: "Decline".into(),
        });
        NativeRequestKind::Approval { choices }
    };
    let detail = if !review_complete {
        "The complete tool input is too large to review safely. This request can only be declined."
            .into()
    } else if !allow_edits && tool != "AskUserQuestion" {
        format!(
            "Read-only / plan mode cannot allow this request. Switch to workspace edits before allowing tools that require approval.\n\n{review}"
        )
    } else {
        let description = clean(request["description"].as_str().unwrap_or(""));
        if description.is_empty() {
            review
        } else {
            format!("{description}\n\n{review}")
        }
    };
    Ok(Pending {
        wire: wire.into(),
        input,
        ui: NativeRequest {
            id: token,
            method: "claude/can_use_tool".into(),
            title: clean(request["title"].as_str().unwrap_or(tool)),
            detail,
            kind,
        },
    })
}
fn answer_for(pending: &Pending, answer: NativeAnswer) -> Result<Value> {
    match (&pending.ui.kind, answer) {
        (_, NativeAnswer::Cancel) => {
            Ok(json!({"behavior":"deny","message":"User canceled this request"}))
        }
        (NativeRequestKind::Approval { choices }, NativeAnswer::Choice(choice)) => {
            anyhow::ensure!(
                choices.iter().any(|allowed| allowed.id == choice),
                "Choose an available permission decision"
            );
            Ok(if choice == "allow" {
                json!({"behavior":"allow","updatedInput":pending.input})
            } else {
                json!({"behavior":"deny","message":"User declined this tool"})
            })
        }
        (NativeRequestKind::Questions { questions }, NativeAnswer::Answers(answers)) => {
            anyhow::ensure!(
                answers.len() == questions.len(),
                "Answer every requested question"
            );
            let mut mapped = BTreeMap::<String, Value>::new();
            for question in questions {
                let values = answers
                    .get(&question.id)
                    .context("Answer every requested question")?;
                anyhow::ensure!(
                    !values.is_empty()
                        && values.len() <= 32
                        && (question.multiple || values.len() == 1)
                        && values
                            .iter()
                            .all(|text| !text.trim().is_empty() && text.len() <= 8192),
                    "Choose valid question answers"
                );
                mapped.insert(
                    question.question.clone(),
                    if question.multiple {
                        json!(values)
                    } else {
                        json!(values[0])
                    },
                );
            }
            let mut input = pending.input.clone();
            input["answers"] = json!(mapped);
            Ok(json!({"behavior":"allow","updatedInput":input}))
        }
        _ => bail!("This response does not match the pending Claude request"),
    }
}
/// Cancellation has already released UI/event backpressure. Give the CLI's
/// native interrupt handler a bounded chance to stop its independently grouped
/// tools before reaping its server. This path never waits on the event sink and
/// uses a real deadline even if stdin is full or the peer stops reading.
fn graceful_stop(
    stdin: &mut std::process::ChildStdin,
    stdout: &mut std::process::ChildStdout,
    stderr: &mut std::process::ChildStderr,
    pid: i32,
) {
    let deadline = Instant::now() + Duration::from_millis(850);
    let Ok(mut bytes) = serde_json::to_vec(
        &json!({"type":"control_request","request_id":"code-stop","request":{"subtype":"interrupt"}}),
    ) else {
        return;
    };
    bytes.push(b'\n');
    let mut written = 0;
    let mut closed = [false, false];
    let mut buffer = [0u8; 8192];
    while Instant::now() < deadline {
        if written < bytes.len() {
            match stdin.write(&bytes[written..]) {
                Ok(0) => return,
                Ok(n) => written += n,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => return,
            }
        }
        for (index, reader) in [&mut *stdout as &mut dyn Read, &mut *stderr as &mut dyn Read]
            .into_iter()
            .enumerate()
        {
            if closed[index] {
                continue;
            }
            for _ in 0..4 {
                match reader.read(&mut buffer) {
                    Ok(0) => {
                        closed[index] = true;
                        break;
                    }
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
        }
        if child_exit_pending(pid).unwrap_or(false) {
            return;
        }
        let mut pipes = [
            libc::pollfd {
                fd: if closed[0] { -1 } else { stdout.as_raw_fd() },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if closed[1] { -1 } else { stderr.as_raw_fd() },
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: if written < bytes.len() {
                    stdin.as_raw_fd()
                } else {
                    -1
                },
                events: libc::POLLOUT,
                revents: 0,
            },
        ];
        let wait = (deadline
            .saturating_duration_since(Instant::now())
            .as_millis() as i32)
            .clamp(0, 20);
        unsafe {
            libc::poll(pipes.as_mut_ptr(), pipes.len() as libc::nfds_t, wait);
        }
    }
}

fn run(
    request: RunRequest,
    mut child: std::process::Child,
    mut stdin: std::process::ChildStdin,
    mut stdout: std::process::ChildStdout,
    mut stderr: std::process::ChildStderr,
    ctx: NativeContext,
) {
    let mut state = Session::default();
    let mut failure=write(&mut stdin,&ctx.cancelled,&json!({"type":"control_request","request_id":"code-initialize","request":{"subtype":"initialize","hooks":null}})).err().map(|error|error.to_string());
    let start = Instant::now();
    let mut record = wire::Record::default();
    let mut buffer = [0u8; 8192];
    let mut diagnostics = Vec::new();
    let _ = send(
        &ctx.tx,
        RuntimeEvent::Status("Connecting to Claude".into()),
        &ctx.cancelled,
    );
    'running: while failure.is_none() && !ctx.cancelled.load(Ordering::Acquire) {
        for _ in 0..32 {
            let Ok(control) = ctx.control.try_recv() else {
                break;
            };
            let token = if let NativeControl::Respond { id, .. } = &control {
                Some(id.clone())
            } else {
                None
            };
            if let Err(error) = state.control(control, &mut stdin, &ctx) {
                let event = token.map_or_else(
                    || RuntimeEvent::Status(error.to_string()),
                    |id| RuntimeEvent::RequestRejected {
                        id,
                        error: error.to_string(),
                    },
                );
                let _ = send(&ctx.tx, event, &ctx.cancelled);
            }
        }
        for _ in 0..8 {
            match stdout.read(&mut buffer) {
                Ok(0) => {
                    if state.result.is_none() {
                        failure = Some("Claude closed before completing this turn".into());
                    }
                    break 'running;
                }
                Ok(n) => {
                    for chunk in buffer[..n].split_inclusive(|byte| *byte == b'\n') {
                        let complete = chunk.last() == Some(&b'\n');
                        let bytes = if complete {
                            &chunk[..chunk.len() - 1]
                        } else {
                            chunk
                        };
                        let result = (|| -> Result<()> {
                            record.push(bytes)?;
                            if complete && !record.is_empty() {
                                let (value, shortened) = if let Some(bytes) = record.small() {
                                    (serde_json::from_slice::<Value>(bytes)?, false)
                                } else {
                                    record.projected(&ctx.cancelled)?
                                };
                                if shortened && value["type"] == "control_request" {
                                    if let Some(wire) = value["request_id"].as_str() {
                                        response(
                                            &mut stdin,
                                            &ctx,
                                            wire,
                                            json!({"behavior":"deny","message":"This request is too large to review safely"}),
                                        )?;
                                    }
                                } else {
                                    state.message(&request, value, &mut stdin, &ctx)?;
                                }
                                record.clear();
                            }
                            Ok(())
                        })();
                        if let Err(error) = result {
                            failure = Some(error.to_string());
                            break 'running;
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    failure = Some(error.to_string());
                    break 'running;
                }
            }
        }
        for _ in 0..8 {
            match stderr.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => {
                    diagnostics.extend_from_slice(&buffer[..n]);
                    if diagnostics.len() > STDERR_LIMIT {
                        diagnostics.drain(..diagnostics.len() - STDERR_LIMIT);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        if !state.initialized && start.elapsed() > Duration::from_secs(30) {
            failure = Some("Claude did not initialize within 30 seconds".into());
            break;
        }
        if state
            .result
            .is_some_and(|when| when.elapsed() > Duration::from_millis(500))
            && state.pending.is_empty()
            && state.parser.child_tasks.is_empty()
        {
            break;
        }
        if child_exit_pending(child.id() as i32).unwrap_or(false) {
            if state.result.is_none() {
                failure = Some(format!(
                    "Claude exited before completion. {}",
                    String::from_utf8_lossy(&diagnostics).trim()
                ));
            }
            break;
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
        ];
        unsafe {
            libc::poll(fds.as_mut_ptr(), 2, 50);
        }
    }
    if ctx.cancelled.load(Ordering::Acquire) || failure.is_some() {
        graceful_stop(&mut stdin, &mut stdout, &mut stderr, child.id() as i32);
    }
    for token in state.pending.keys() {
        let _ = ctx
            .tx
            .try_send(RuntimeEvent::RequestResolved(token.clone()));
    }
    let mut terminal = if ctx.cancelled.load(Ordering::Acquire) {
        RuntimeEvent::Status("Stopped".into())
    } else if let Some(error) = &failure {
        RuntimeEvent::Failed(error.clone())
    } else {
        RuntimeEvent::Finished
    };
    super::record_terminal(&mut state.parser, &terminal);
    if let Ok(source) = state.parser.rendered_source() {
        *ctx.finalized_source
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(source);
    } else {
        terminal = RuntimeEvent::Failed("Could not preserve Claude's response".into());
    }
    {
        let mut group = ctx.group.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(pid) = *group {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        *group = None;
    }
    if ctx.cancelled.load(Ordering::Acquire) {
        let _ = ctx.tx.try_send(terminal);
    } else {
        let _ = send(&ctx.tx, terminal, &ctx.cancelled);
    }
    drop(ctx.settled);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::prelude::v1::test;
    use std::{fs, os::unix::fs::PermissionsExt};
    #[test]
    fn model_catalog_uses_negotiated_ids_and_excludes_malformed_entries() {
        let models = model_choices(
            &json!([{"value":"new-live-model","displayName":"Live Model"},{"value":"--invalid"},{"value":"new-live-model"},{"value":"second-model"}]),
        );
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "new-live-model");
        assert_eq!(models[0].label, "Live Model");
        assert_eq!(models[1].label, "second-model");
    }

    #[test]
    fn approvals_preserve_exact_input_and_readonly_never_offers_allow() {
        let value = json!({"tool_name":"Bash","input":{"command":"printf '%s' 'quoted 🦀'","timeout":2500},"description":"Description cannot hide the actual command"});
        let pending = permission("wire-id", &value, true).unwrap();
        assert!(pending.ui.detail.contains("printf"));
        let answer = answer_for(&pending, NativeAnswer::Choice("allow".into())).unwrap();
        assert_eq!(answer["updatedInput"], value["input"]);
        assert!(answer.get("updatedPermissions").is_none());
        assert_eq!(
            answer_for(&pending, NativeAnswer::Cancel).unwrap()["behavior"],
            "deny"
        );
        let readonly = permission("wire-id", &value, false).unwrap();
        assert!(answer_for(&readonly, NativeAnswer::Choice("allow".into())).is_err());
        assert_eq!(
            answer_for(&readonly, NativeAnswer::Choice("deny".into())).unwrap()["behavior"],
            "deny"
        );
        let large = permission(
            "large",
            &json!({"tool_name":"Bash","input":{"command":"x".repeat(70*1024)}}),
            true,
        )
        .unwrap();
        assert!(answer_for(&large, NativeAnswer::Choice("allow".into())).is_err());
    }
    #[test]
    fn questions_keep_original_shape_and_map_answers_to_question_text() {
        let input = json!({"questions":[{"header":"Format","question":"Which format?","options":[{"label":"Rust"},{"label":"Other"}],"multiSelect":false},{"header":"Files","question":"Which files?","options":[{"label":"A"},{"label":"B"}],"multiSelect":true}]});
        let pending = permission(
            "wire",
            &json!({"tool_name":"AskUserQuestion","input":input}),
            false,
        )
        .unwrap();
        let answer = answer_for(
            &pending,
            NativeAnswer::Answers(
                [
                    ("0".into(), vec!["custom format".into()]),
                    ("1".into(), vec!["A".into(), "B".into()]),
                ]
                .into_iter()
                .collect(),
            ),
        )
        .unwrap();
        assert_eq!(answer["updatedInput"]["questions"], input["questions"]);
        assert_eq!(
            answer["updatedInput"]["answers"]["Which format?"],
            "custom format"
        );
        assert_eq!(
            answer["updatedInput"]["answers"]["Which files?"],
            json!(["A", "B"])
        );
        assert!(answer_for(&pending, NativeAnswer::Answers(BTreeMap::new())).is_err());
    }
    #[test]
    fn bidirectional_claude_fixture_waits_for_ui_decision_and_preserves_output() {
        let root =
            std::env::temp_dir().join(format!("code-claude-protocol-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("claude-fixture");
        fs::write(&executable,r#"#!/bin/sh
read -r init
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"code-initialize","response":{"commands":[]}}}'
read -r prompt
printf '%s\n' '{"type":"system","subtype":"init","session_id":"claude-native","slash_commands":[]}'
printf '%s\n' '{"type":"assistant","message":{"id":"message","content":[{"type":"text","text":"Before permission. "}]}}'
printf '%s\n' '{"type":"control_request","request_id":"wire-permission","request":{"subtype":"can_use_tool","tool_name":"Bash","input":{"command":"printf verified"}}}'
read -r approval
case "$approval" in *'"behavior":"allow"'*) ;; *) exit 9;; esac
printf '%s\n' '{"type":"assistant","message":{"id":"next","content":[{"type":"text","text":"Approved and completed."}]}}'
printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"Approved and completed."}'
read -r wait
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let handle = launch(RunRequest { service_tier:None, reasoning_effort:None,
            agent: AgentKind::Claude,
            executable,
            project: root.clone(),
            prompt: "Fixture only".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: Vec::new(),
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            codex_access: Default::default(),
            trust_workspace: false,
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut approved = false;
        let mut finished = false;
        while Instant::now() < deadline && !finished {
            match handle.events.try_recv() {
                Ok(RuntimeEvent::Request(request)) => {
                    assert!(!approved);
                    assert!(request.detail.contains("printf verified"));
                    handle
                        .respond(&request.id, NativeAnswer::Choice("allow".into()))
                        .unwrap();
                    approved = true;
                }
                Ok(RuntimeEvent::Finished) => finished = true,
                Ok(RuntimeEvent::Failed(error)) => panic!("{error}"),
                Ok(_) => {}
                Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
                Err(TryRecvError::Disconnected) => break,
            }
        }
        assert!(approved && finished);
        let source = handle.take_response_source().unwrap();
        assert!(
            source
                .tail(64000)
                .unwrap()
                .contains("Approved and completed.")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn native_commands_validate_initialization_catalog_before_sending_any_prompt() {
        for available in [true, false] {
            let root =
                std::env::temp_dir().join(format!("code-claude-command-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&root).unwrap();
            let executable = root.join("claude-command");
            let commands = if available {
                r#"[{"name":"compact"}]"#
            } else {
                "[]"
            };
            fs::write(&executable,format!(r#"#!/bin/sh
read -r init
printf '%s\n' '{{"type":"control_response","response":{{"subtype":"success","request_id":"code-initialize","response":{{"commands":{commands}}}}}}}'
read -r prompt
case "$prompt" in *'"type":"user"'*) printf '%s' "$prompt" > received-prompt;; esac
printf '%s\n' '{{"type":"result","subtype":"success","is_error":false,"result":""}}'
read -r wait
"#)).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
            let handle = launch(RunRequest { service_tier:None, reasoning_effort:None,
                agent: AgentKind::Claude,
                executable,
                project: root.clone(),
                prompt: "/compact".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: Vec::new(),
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                codex_access: Default::default(),
                trust_workspace: false,
            })
            .unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut terminal = None;
            while Instant::now() < deadline && terminal.is_none() {
                match handle.events.try_recv() {
                    Ok(RuntimeEvent::Finished) => terminal = Some(true),
                    Ok(RuntimeEvent::Failed(error)) => {
                        assert!(error.contains("no longer advertises"));
                        terminal = Some(false);
                    }
                    Ok(_) => {}
                    Err(TryRecvError::Empty) => thread::sleep(Duration::from_millis(5)),
                    Err(TryRecvError::Disconnected) => break,
                }
            }
            assert_eq!(terminal, Some(available));
            assert_eq!(root.join("received-prompt").exists(), available);
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    #[ignore = "Read-only installed Claude initialization, no inference; opt in with CODE_CLAUDE_INIT_BIN"]
    fn installed_claude_negotiates_bidirectional_protocol_without_inference() {
        let executable = std::env::var_os("CODE_CLAUDE_INIT_BIN").expect("CODE_CLAUDE_INIT_BIN");
        let mut command = Command::new(executable);
        crate::process_env::apply(&mut command);
        command
            .args([
                "--print",
                "--verbose",
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--permission-prompt-tool",
                "stdio",
                "--permission-mode",
                "default",
            ])
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0);
        struct Cleanup(std::process::Child);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    libc::kill(-(self.0.id() as i32), libc::SIGKILL);
                }
                let _ = self.0.wait();
            }
        }
        let mut child = Cleanup(command.spawn().unwrap());
        let mut stdin = child.0.stdin.take().unwrap();
        stdin.write_all(b"{\"type\":\"control_request\",\"request_id\":\"metadata\",\"request\":{\"subtype\":\"initialize\",\"hooks\":null}}\n").unwrap();
        stdin.flush().unwrap();
        let mut stdout = child.0.stdout.take().unwrap();
        let fd = stdout.as_raw_fd();
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        assert_eq!(
            unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) },
            0
        );
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 8192];
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            assert!(
                Instant::now() < deadline,
                "Installed Claude initialization timed out"
            );
            match stdout.read(&mut buffer) {
                Ok(0) => panic!("Claude closed before initialization"),
                Ok(n) => {
                    bytes.extend_from_slice(&buffer[..n]);
                    assert!(bytes.len() <= 2 * 1024 * 1024);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    let mut poll = libc::pollfd {
                        fd,
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    unsafe {
                        libc::poll(&mut poll, 1, 100);
                    }
                }
                Err(error) => panic!("{error}"),
            }
            if let Some(end) = bytes.iter().position(|byte| *byte == b'\n') {
                let message: Value = serde_json::from_slice(&bytes[..end]).unwrap();
                assert_eq!(message["response"]["subtype"], "success");
                let result = &message["response"]["response"];
                assert!(result["commands"].as_array().is_some());
                assert!(result["models"].as_array().is_some());
                eprintln!(
                    "Installed Claude: {} native commands, {} model choices; no prompt or account identifier logged",
                    result["commands"].as_array().unwrap().len(),
                    result["models"].as_array().unwrap().len()
                );
                break;
            }
        }
    }

    #[test]
    fn stop_interrupts_claude_before_killing_the_owned_server_and_detached_tool() {
        let root = std::env::temp_dir().join(format!("code-claude-stop-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("claude-stop-fixture");
        fs::write(&executable,r#"#!/usr/bin/python3
import os,sys,json,subprocess,signal
init=json.loads(sys.stdin.readline())
print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':'code-initialize','response':{'commands':[]}}}),flush=True)
prompt=json.loads(sys.stdin.readline())
tool=subprocess.Popen(['/bin/sleep','30'],start_new_session=True)
with open('tool.pid.next','w') as output: output.write(str(tool.pid))
os.replace('tool.pid.next','tool.pid')
print(json.dumps({'type':'system','subtype':'init','session_id':'fixture-stop'}),flush=True)
for line in sys.stdin:
    request=json.loads(line)
    if request.get('request',{}).get('subtype')=='interrupt':
        os.killpg(tool.pid,signal.SIGKILL)
        tool.wait()
        with open('interrupted','w') as output: output.write('native interrupt received before process termination')
        print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':request['request_id'],'response':{}}}),flush=True)
        print(json.dumps({'type':'result','subtype':'success','is_error':False,'result':''}),flush=True)
        break
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let handle = launch(RunRequest { service_tier:None, reasoning_effort:None,
            agent: AgentKind::Claude,
            executable,
            project: root.clone(),
            prompt: "No model, fixture only".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: Vec::new(),
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            codex_access: Default::default(),
            trust_workspace: false,
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("tool.pid").exists() {
            assert!(Instant::now() < deadline);
            while handle.events.try_recv().is_ok() {}
            thread::sleep(Duration::from_millis(5));
        }
        let tool: i32 = fs::read_to_string(root.join("tool.pid"))
            .unwrap()
            .parse()
            .unwrap();
        let start = Instant::now();
        handle.cancel();
        while !handle.settled.is_closed() {
            assert!(
                start.elapsed() < Duration::from_millis(1600),
                "Claude cancellation exceeded its native interrupt grace"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            root.join("interrupted").exists(),
            "The CLI was killed before receiving a native interrupt"
        );
        assert_eq!(
            unsafe { libc::kill(tool, 0) },
            -1,
            "Detached tool survived native Stop"
        );
        assert!(handle.process_group.lock().unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stop_has_a_deadline_when_claude_never_reads_the_interrupt() {
        let root =
            std::env::temp_dir().join(format!("code-claude-stop-stall-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("claude-stall-fixture");
        fs::write(&executable,r#"#!/bin/sh
read -r initialize
printf '%s\n' '{"type":"control_response","response":{"subtype":"success","request_id":"code-initialize","response":{"commands":[]}}}'
read -r prompt
printf '%s' ready > ready
sleep 30
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let handle = launch(RunRequest { service_tier:None, reasoning_effort:None,
            agent: AgentKind::Claude,
            executable,
            project: root.clone(),
            prompt: "Fixture only".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: Vec::new(),
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            codex_access: Default::default(),
            trust_workspace: false,
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("ready").exists() {
            assert!(Instant::now() < deadline);
            while handle.events.try_recv().is_ok() {}
            thread::sleep(Duration::from_millis(5));
        }
        let start = Instant::now();
        handle.cancel();
        while !handle.settled.is_closed() {
            assert!(start.elapsed() < Duration::from_millis(1600));
            thread::sleep(Duration::from_millis(5));
        }
        assert!(handle.process_group.lock().unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn stop_reaches_claude_even_when_ui_event_queue_is_full() {
        let root = std::env::temp_dir().join(format!(
            "code-claude-stop-backpressure-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&root).unwrap();
        let executable = root.join("claude-busy-fixture");
        fs::write(&executable,r#"#!/usr/bin/python3
import os,sys,json,threading,time
json.loads(sys.stdin.readline())
print(json.dumps({'type':'control_response','response':{'subtype':'success','request_id':'code-initialize','response':{'commands':[]}}}),flush=True)
json.loads(sys.stdin.readline())
with open('ready','w') as output: output.write('ready')
def produce():
    frame=json.dumps({'type':'stream_event','event':{'type':'content_block_delta','delta':{'type':'text_delta','text':'x'*8192}}})+'\n'
    while True:
        try: sys.stdout.write(frame);sys.stdout.flush()
        except: return
threading.Thread(target=produce,daemon=True).start()
for line in sys.stdin:
    if json.loads(line).get('request',{}).get('subtype')=='interrupt':
        with open('interrupted','w') as output: output.write('received')
        time.sleep(30)
"#).unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let handle = launch(RunRequest { service_tier:None, reasoning_effort:None,
            agent: AgentKind::Claude,
            executable,
            project: root.clone(),
            prompt: "Fixture only".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: Vec::new(),
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            codex_access: Default::default(),
            trust_workspace: false,
        })
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !root.join("ready").exists() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        // Intentionally leave every event unread so production hits the bounded
        // UI queue. Atomic cancellation must wake that producer before cleanup.
        thread::sleep(Duration::from_millis(180));
        let start = Instant::now();
        handle.cancel();
        while !handle.settled.is_closed() {
            assert!(start.elapsed() < Duration::from_millis(1600));
            thread::sleep(Duration::from_millis(5));
        }
        assert!(root.join("interrupted").exists());
        assert!(handle.process_group.lock().unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }
}
