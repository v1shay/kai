//! Native provider protocols and CLI adapters. Each CLI owns authentication.
//! Agents launch directly; explicit terminal commands use the native sandbox.
//! See docs/FEATURE-COVERAGE.md for protocol versions and verified coverage.

#[path = "runtime/adapters.rs"]
mod adapters;
#[path = "runtime/native_requests.rs"]
mod native_requests;
pub use native_requests::{
    CodexApprovals, NativeAnswer, NativeControl, NativeOption, NativeQuestion, NativeRequest,
    NativeRequestKind,
};
#[path = "runtime/broker.rs"]
pub mod broker;
#[path = "runtime/claude_control.rs"]
mod claude_control;
#[path = "runtime/codex_actions.rs"]
pub mod codex_actions;
#[path = "runtime/codex_server.rs"]
mod codex_server;
#[path = "runtime/delegation.rs"]
mod delegation;
#[path = "runtime/interaction_log.rs"]
mod interaction_log;
#[path = "runtime/native_activity.rs"]
mod native_activity;
pub use delegation::{delegation_prompt, supports_native_delegation};
#[path = "runtime/gemini_acp.rs"]
mod gemini_acp;
#[path = "runtime/history.rs"]
pub mod history;
#[path = "runtime/opencode_server.rs"]
mod opencode_server;
#[path = "runtime/wire.rs"]
mod wire;

use crate::discovery::AgentKind;
use crate::presentation;
use crate::response_buffer::{ResponseBuffer, ResponseSource};
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::{
    borrow::Cow,
    io::{self, Read, Write},
    os::{fd::AsRawFd, unix::process::CommandExt},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};
#[cfg(test)]
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Seek, SeekFrom},
};
#[cfg(test)]
use std::{fs::File, path::Path};

const PROMPT_LIMIT: usize = 1024 * 1024;
#[cfg(test)]
const TRANSCRIPT_LIMIT: u64 = 2 * 1024 * 1024;
pub const LIVE_OUTPUT_LIMIT: usize = 16 * 1024 * 1024;
const STDERR_LIMIT: usize = 16 * 1024;
const QUEUED_EVENTS_LIMIT: usize = 128;
const QUEUED_BYTES_LIMIT: usize = 2 * 1024 * 1024;

#[derive(Clone)]
pub struct RunRequest {
    pub agent: AgentKind,
    pub executable: PathBuf,
    pub project: PathBuf,
    pub prompt: String,
    pub resume_id: Option<String>,
    /// Continue an externally owned Codex thread in a new native thread.
    pub fork_session: bool,
    /// Per-invocation override; None retains the CLI native default.
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub images: Vec<PathBuf>,
    pub allow_edits: bool,
    /// Explicit app workspace confirmation; never enables tool auto-approval.
    pub trust_workspace: bool,
    pub codex_approvals: CodexApprovals,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RuntimeEvent {
    Voice(crate::voice::Signal),
    Request(Box<NativeRequest>),
    RequestResolved(String),
    RequestRejected {
        id: String,
        error: String,
    },
    Commands(Vec<String>),
    Models(Vec<crate::agent_controls::ModelChoice>),
    Usage(Box<crate::usage::Usage>),
    SessionId(String),
    Text(String),
    /// Replaces the complete assistant output of the current invocation.
    ReplaceText(String),
    /// Worker-rendered tail of a larger canonical response held by RunHandle.
    ReplacePreview(String),
    Interaction(Box<crate::chat_interactions::ChatInteraction>),
    Status(String),
    Finished,
    Failed(String),
}

pub struct RunHandle {
    /// Turn quiescence: output is finalized and owned child work/requests have
    /// settled. A healthy native broker may retain its idle provider process.
    pub settled: async_channel::Receiver<()>,
    pub events: EventReceiver,
    /// Coalesced readiness signal; closes when the producer has finished.
    pub wake: async_channel::Receiver<()>,
    control: Option<SyncSender<NativeControl>>,
    deferred_native_stop: bool,
    cancelled: Arc<AtomicBool>,
    process_group: Arc<Mutex<Option<i32>>>,
    finalized_source: Arc<Mutex<Option<ResponseSource>>>,
}

/// Shared bounded event/control channels for native bidirectional providers.
struct NativeContext {
    tx: NotifyingSender,
    cancelled: Arc<AtomicBool>,
    group: Arc<Mutex<Option<i32>>>,
    finalized_source: Arc<Mutex<Option<ResponseSource>>>,
    control: Receiver<NativeControl>,
    settled: async_channel::Sender<()>,
}
fn native_handle(pid: i32) -> (RunHandle, NativeContext) {
    let (settled_tx, settled) = async_channel::bounded(1);
    let (event_tx, event_rx) = mpsc::sync_channel(QUEUED_EVENTS_LIMIT);
    let (wake_tx, wake) = async_channel::bounded(1);
    let (control_tx, control) = mpsc::sync_channel(32);
    let budget = Arc::new(QueueBudget::default());
    let cancelled = Arc::new(AtomicBool::new(false));
    let group = Arc::new(Mutex::new(Some(pid)));
    let finalized_source = Arc::new(Mutex::new(None));
    (
        RunHandle {
            settled,
            events: EventReceiver {
                inner: event_rx,
                budget: budget.clone(),
            },
            wake,
            control: Some(control_tx),
            deferred_native_stop: true,
            cancelled: cancelled.clone(),
            process_group: group.clone(),
            finalized_source: finalized_source.clone(),
        },
        NativeContext {
            tx: NotifyingSender {
                events: event_tx,
                wake: wake_tx,
                budget,
            },
            cancelled,
            group,
            finalized_source,
            control,
            settled: settled_tx,
        },
    )
}

impl RunHandle {
    #[cfg(test)]
    pub(crate) fn interactive_test_handle() -> (Self, Receiver<NativeControl>) {
        let mut handle = Self::idle_test_handle();
        let (tx, rx) = mpsc::sync_channel(32);
        handle.control = Some(tx);
        (handle, rx)
    }

    #[cfg(test)]
    pub fn idle_test_handle() -> Self {
        let (_, events) = mpsc::sync_channel(QUEUED_EVENTS_LIMIT);
        let (_, wake) = async_channel::bounded(1);
        Self {
            settled: {
                let (_, receiver) = async_channel::bounded(1);
                receiver
            },
            events: EventReceiver {
                inner: events,
                budget: Arc::new(QueueBudget::default()),
            },
            wake,
            control: None,
            deferred_native_stop: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            process_group: Arc::new(Mutex::new(None)),
            finalized_source: Arc::new(Mutex::new(None)),
        }
    }
    #[cfg(test)]
    pub fn test_events(events: Vec<RuntimeEvent>) -> Self {
        let (tx, rx) = mpsc::sync_channel(QUEUED_EVENTS_LIMIT);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let budget = Arc::new(QueueBudget::default());
        let sender = NotifyingSender {
            events: tx,
            wake: wake_tx,
            budget: budget.clone(),
        };
        for event in events {
            sender.try_send(event).unwrap();
        }
        Self {
            settled: {
                let (_, receiver) = async_channel::bounded(1);
                receiver
            },
            events: EventReceiver { inner: rx, budget },
            wake: wake_rx,
            control: None,
            deferred_native_stop: false,
            cancelled: Arc::new(AtomicBool::new(false)),
            process_group: Arc::new(Mutex::new(None)),
            finalized_source: Arc::new(Mutex::new(None)),
        }
    }
    pub fn respond(&self, id: &str, answer: NativeAnswer) -> Result<()> {
        self.send_control(NativeControl::Respond {
            id: id.into(),
            answer,
        })
    }
    pub fn steer(&self, text: &str) -> Result<()> {
        anyhow::ensure!(
            !text.trim().is_empty() && text.len() <= PROMPT_LIMIT,
            "Write a valid steering message"
        );
        self.send_control(NativeControl::Steer(text.into()))
    }
    pub fn interrupt(&self) -> Result<()> {
        self.send_control(NativeControl::Interrupt)
    }
    #[allow(dead_code)] // Reserved capability-gated native controls; typed UI uses dedicated methods.
    pub fn native_command(&self, method: &str, params: Value) -> Result<()> {
        self.send_control(NativeControl::Rpc {
            method: method.into(),
            params,
        })
    }
    fn send_control(&self, control: NativeControl) -> Result<()> {
        anyhow::ensure!(!self.is_cancelled(), "This response has stopped");
        self.control
            .as_ref()
            .context("This agent does not expose interactive requests")?
            .try_send(control)
            .map_err(|_| anyhow::anyhow!("The agent is busy or no longer running"))
    }
    pub(crate) fn take_response_source(&self) -> Option<ResponseSource> {
        self.finalized_source
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
    }
    pub fn cancel(&self) {
        let first_cancel = !self.cancelled.swap(true, Ordering::AcqRel);
        // Synchronize notification with the waiter's mutex so cancellation
        // cannot fall between its predicate check and condition-variable wait.
        drop(
            self.events
                .budget
                .bytes
                .lock()
                .unwrap_or_else(|e| e.into_inner()),
        );
        self.events.budget.changed.notify_all();
        // Native Codex must terminate sandbox/tool process groups through its
        // protocol before its server is killed. A one-shot bounded watchdog
        // retains hard-stop behavior even if its worker or provider stalls.
        if self.deferred_native_stop {
            if first_cancel && self.process_group.lock().is_ok_and(|group| group.is_some()) {
                let owned = self.process_group.clone();
                thread::spawn(move || {
                    thread::sleep(Duration::from_secs(2));
                    if let Ok(group) = owned.lock()
                        && let Some(pid) = *group
                    {
                        unsafe {
                            libc::kill(-pid, libc::SIGKILL);
                        }
                    }
                });
            }
            return;
        }
        // Group ownership is cleared under this same lock before the child is
        // reaped, preventing a late cancellation from hitting a reused PID.
        if let Ok(group) = self.process_group.lock()
            && let Some(pid) = *group
        {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    /// Cancellation remains observable even when a full queue cannot accept
    /// the final Stopped status. Consumers should consult this on disconnect.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

impl Drop for RunHandle {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn command_for(request: &RunRequest) -> Result<Command> {
    if request.fork_session && (request.agent != AgentKind::Codex || request.resume_id.is_none()) {
        bail!("A Codex source session is required to continue in a new chat");
    }
    validate_images(request)?;
    if let Some(model) = &request.model
        && !valid_model_id(model)
    {
        bail!("Invalid agent model ID");
    }
    let mut command = Command::new(&request.executable);
    crate::process_env::apply(&mut command);
    command.current_dir(&request.project);
    match request.agent {
        AgentKind::Codex => {
            command.args([
                "exec",
                "--sandbox",
                if request.allow_edits {
                    "workspace-write"
                } else {
                    "read-only"
                },
                "--json",
                "--color",
                "never",
                "--skip-git-repo-check",
            ]);
            if let Some(model) = &request.model {
                command.arg("--model").arg(model);
            }
            if let Some(id) = &request.resume_id {
                if id.starts_with('-') || id.is_empty() || id.contains('\0') {
                    bail!("Invalid Codex session ID");
                }
                command
                    .arg(if request.fork_session {
                        "fork"
                    } else {
                        "resume"
                    })
                    .arg(id);
            }
            for image in &request.images {
                command.arg("--image").arg(image);
            }
            command.arg("-");
        }
        AgentKind::Claude => {
            command.args([
                "--print",
                "--output-format",
                "stream-json",
                "--verbose",
                "--include-partial-messages",
                "--permission-mode",
                if request.allow_edits {
                    "acceptEdits"
                } else {
                    "plan"
                },
            ]);
            if !request.images.is_empty() {
                command.args(["--input-format", "stream-json"]);
            }
            if let Some(id) = &request.resume_id {
                if id.starts_with('-') || id.is_empty() {
                    bail!("Invalid Claude session ID");
                }
                command.arg("--resume").arg(id);
            }
        }
        AgentKind::Gemini => {
            if request.trust_workspace {
                command.arg("--skip-trust");
            }
            command.args([
                "--output-format",
                "stream-json",
                "--approval-mode",
                if request.allow_edits {
                    "auto_edit"
                } else {
                    "plan"
                },
            ]);
            resume_arg(&mut command, "--resume", request.resume_id.as_deref())?;
        }
        AgentKind::OpenCode => {
            command.args([
                "run",
                "--format",
                "json",
                "--agent",
                if request.allow_edits { "build" } else { "plan" },
            ]);
            resume_arg(&mut command, "--session", request.resume_id.as_deref())?;
            for image in &request.images {
                command.arg("--file").arg(image);
            }
        }
        AgentKind::Antigravity => {
            command.args([
                "--input-format",
                "stream-json",
                "--output-format",
                "stream-json",
                "--mode",
                if request.allow_edits {
                    "accept-edits"
                } else {
                    "plan"
                },
            ]);
            resume_arg(&mut command, "--conversation", request.resume_id.as_deref())?;
        }
        AgentKind::Cursor => {
            if request.trust_workspace {
                command.arg("--trust");
            }
            command.args([
                "--print",
                "--output-format",
                "stream-json",
                "--stream-partial-output",
            ]);
            if !request.allow_edits {
                command.args(["--mode", "plan"]);
            }
            resume_arg(&mut command, "--resume", request.resume_id.as_deref())?;
        }
        AgentKind::Grok => {
            // Grok reads a file, rather than piped stdin, for a headless prompt.
            // /dev/stdin preserves the bounded pipe and keeps prompts out of argv.
            command.args([
                "--prompt-file",
                "/dev/stdin",
                "--output-format",
                "streaming-messages-json",
                "--include-partial-messages",
                "--permission-mode",
                if request.allow_edits {
                    "acceptEdits"
                } else {
                    "plan"
                },
            ]);
            resume_arg(&mut command, "--resume", request.resume_id.as_deref())?;
        }
    }
    if request.agent != AgentKind::Codex
        && let Some(model) = &request.model
    {
        command.arg("--model").arg(model);
    }
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command.process_group(0);
    Ok(command)
}

/// Native IDs are one argv value, never a flag or control sequence.
/// Brackets and commas support Cursor's native parameterized models.
pub fn valid_model_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_.:/[]@=,+".contains(&b))
}

/// Recognize native ownership conflicts without modifying another app's lock.
pub fn is_active_writer_conflict(diagnostic: &str) -> bool {
    let lower = diagnostic.to_ascii_lowercase();
    lower.contains("already has an active writer")
        || lower.contains("thread-store conflict")
        || lower.contains("thread store conflict")
}

/// A bounded plain headline for UI error cards, excluding startup log noise.
pub fn diagnostic_summary(diagnostic: &str) -> String {
    if is_active_writer_conflict(diagnostic) {
        return "This chat is already active in another app.".into();
    }
    // Normalize only the bounded tail, matching the runtime's stderr budget.
    let mut start = diagnostic.len().saturating_sub(STDERR_LIMIT);
    while !diagnostic.is_char_boundary(start) {
        start += 1;
    }
    let mut plain = String::with_capacity(diagnostic.len() - start);
    let mut chars = diagnostic[start..].chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            match chars.next() {
                Some('[') => {
                    for next in chars.by_ref() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    while let Some(next) = chars.next() {
                        if next == '\u{7}'
                            || (next == '\u{1b}' && chars.next_if_eq(&'\\').is_some())
                        {
                            break;
                        }
                    }
                }
                _ => {}
            }
        } else if !ch.is_control() || ch == '\n' || ch == '\t' {
            plain.push(ch);
        }
    }
    let mut lines = plain.lines().map(str::trim).filter(|line| !line.is_empty());
    let meaningful = |line: &&str| {
        let lower = line.to_ascii_lowercase();
        lower.starts_with("error:")
            || lower.starts_with("fatal:")
            || lower.contains(" error ")
            || lower.contains("failed:")
            || lower.contains("credit balance too low")
            || lower.contains("not trusted")
    };
    let line = lines
        .clone()
        .rfind(meaningful)
        .or_else(|| lines.rfind(|line| !line.to_ascii_lowercase().starts_with("warning:")))
        .unwrap_or("The agent could not complete this turn.");
    let mut summary: String = line.chars().take(280).collect();
    if summary.len() < line.len() {
        summary.push('…');
    }
    summary
}

fn resume_arg(command: &mut Command, flag: &str, id: Option<&str>) -> Result<()> {
    if let Some(id) = id {
        if id.is_empty() || id.starts_with('-') || id.contains('\0') {
            bail!("Invalid agent session ID");
        }
        command.arg(flag).arg(id);
    }
    Ok(())
}

#[cfg(test)]
fn prompt_input(agent: AgentKind, prompt: String) -> String {
    if agent == AgentKind::Antigravity {
        format!(
            "{}\n",
            serde_json::json!({"event":"user", "message":{"content":prompt}})
        )
    } else {
        prompt
    }
}

pub fn supports_images(agent: AgentKind) -> bool {
    matches!(
        agent,
        AgentKind::Codex | AgentKind::Claude | AgentKind::OpenCode | AgentKind::Gemini
    )
}

fn validate_images(request: &RunRequest) -> Result<()> {
    if request.images.is_empty() {
        return Ok(());
    }
    if !supports_images(request.agent) {
        bail!(
            "{} does not expose supported native image input. Choose Codex, Claude Code or OpenCode.",
            request.agent.name()
        );
    }
    if request.images.len() > crate::attachments::MAX_ATTACHMENTS {
        bail!("Attach up to 4 images");
    }
    let mut total = 0_u64;
    for path in &request.images {
        if !path.is_absolute() {
            bail!("Attachment path must be absolute");
        }
        let mut file = std::fs::File::open(path).context("An attached image is unavailable")?;
        let metadata = file.metadata()?;
        if !metadata.is_file()
            || metadata.len() == 0
            || metadata.len() > crate::attachments::MAX_FILE_BYTES
        {
            bail!("An attached image exceeds the 16 MB limit");
        }
        let mut signature = [0_u8; 8];
        file.read_exact(&mut signature)
            .context("An attached image is invalid")?;
        if &signature != b"\x89PNG\r\n\x1a\n" {
            bail!("Use images imported through the composer");
        }
        total = total.saturating_add(metadata.len());
    }
    if total > crate::attachments::MAX_TOTAL_BYTES {
        bail!("Attached images exceed the 32 MB total limit");
    }
    if request.agent == AgentKind::Claude {
        let text_bytes = request
            .prompt
            .bytes()
            .map(|b| match b {
                b'"' | b'\\' | b'\n' | b'\t' | b'\r' | 8 | 12 => 2,
                0..=31 => 6,
                _ => 1,
            })
            .sum::<u64>();
        // Per-file padding plus protocol headers, bounded below Claude's10MB stdin cap.
        if text_bytes + (total + request.images.len() as u64 * 2) / 3 * 4 + 2048 > 10 * 1024 * 1024
        {
            bail!("Claude image input exceeds 10 MB. Use fewer or smaller images.");
        }
    }
    Ok(())
}

struct InputPipe<'a> {
    stdin: &'a mut std::process::ChildStdin,
    done: &'a AtomicBool,
    cancelled: &'a AtomicBool,
}
impl Write for InputPipe<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            if self.done.load(Ordering::Acquire) || self.cancelled.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    // write_all retries Interrupted without returning. A
                    // closed/cancelled input must stop the writer so the
                    // stream worker can join it and publish its terminal state.
                    io::ErrorKind::BrokenPipe,
                    "Agent input closed",
                ));
            }
            match self.stdin.write(bytes) {
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    wait_for_pipe(self.stdin.as_raw_fd(), libc::POLLOUT)?
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => return result,
            }
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        self.stdin.flush()
    }
}

/// Claude's documented SDK user envelope, streamed with constant-size base64
/// buffers. Image contents never enter argv or one large serialized String.
fn write_agent_input(
    agent: AgentKind,
    prompt: &str,
    images: &[PathBuf],
    writer: &mut impl Write,
) -> io::Result<()> {
    if agent == AgentKind::Antigravity {
        writer.write_all(b"{\"event\":\"user\",\"message\":{\"content\":")?;
        serde_json::to_writer(&mut *writer, prompt).map_err(io::Error::other)?;
        return writer.write_all(b"}}\n");
    }
    if agent != AgentKind::Claude || images.is_empty() {
        return writer.write_all(prompt.as_bytes());
    }
    writer.write_all(b"{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":")?;
    serde_json::to_writer(&mut *writer, prompt).map_err(io::Error::other)?;
    writer.write_all(b"}")?;
    for path in images {
        writer.write_all(b",{\"type\":\"image\",\"source\":{\"type\":\"base64\",\"media_type\":\"image/png\",\"data\":\"")?;
        let mut file = std::fs::File::open(path)?.take(crate::attachments::MAX_FILE_BYTES + 1);
        let mut input = [0_u8; 3 * 8192];
        let mut output = [0_u8; 4 * 8192];
        let mut read = 0_u64;
        loop {
            let mut length = 0;
            while length < input.len() {
                let count = file.read(&mut input[length..])?;
                if count == 0 {
                    break;
                }
                length += count;
            }
            if length == 0 {
                break;
            }
            read += length as u64;
            if read > crate::attachments::MAX_FILE_BYTES {
                return Err(io::Error::other("Attached image grew beyond its limit"));
            }
            let encoded = base64_chunk(&input[..length], &mut output);
            writer.write_all(&output[..encoded])?;
        }
        writer.write_all(b"\"}}")?;
    }
    writer.write_all(b"]},\"parent_tool_use_id\":null}\n")
}
fn base64_chunk(input: &[u8], output: &mut [u8]) -> usize {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    for (index, chunk) in input.chunks(3).enumerate() {
        let a = chunk[0];
        let b = chunk.get(1).copied().unwrap_or(0);
        let c = chunk.get(2).copied().unwrap_or(0);
        let out = &mut output[index * 4..index * 4 + 4];
        out[0] = ALPHABET[(a >> 2) as usize];
        out[1] = ALPHABET[(((a & 3) << 4) | (b >> 4)) as usize];
        out[2] = if chunk.len() > 1 {
            ALPHABET[(((b & 15) << 2) | (c >> 6)) as usize]
        } else {
            b'='
        };
        out[3] = if chunk.len() > 2 {
            ALPHABET[(c & 63) as usize]
        } else {
            b'='
        };
    }
    input.len().div_ceil(3) * 4
}

/// Starts a bounded, background stream; no credentials are read by Orbit.
pub fn launch(request: RunRequest) -> Result<RunHandle> {
    launch_mode(request, false)
}
/// Native maintenance uses the existing provider protocol without a prompt turn.
pub fn launch_native_command(
    request: RunRequest,
    method: &str,
    params: Value,
) -> Result<RunHandle> {
    anyhow::ensure!(request.project.is_dir(), "Project folder is unavailable");
    anyhow::ensure!(
        request.agent == AgentKind::Codex,
        "This provider does not expose native maintenance here"
    );
    codex_server::maintenance(request, method, params)
}
pub fn launch_cli_command(request: RunRequest) -> Result<RunHandle> {
    anyhow::ensure!(
        matches!(
            request.agent,
            AgentKind::Claude | AgentKind::Gemini | AgentKind::OpenCode | AgentKind::Cursor
        ) && crate::commands::name(&request.prompt).is_some(),
        "This adapter has no verified headless command dispatch"
    );
    launch_mode(request, true)
}
fn launch_mode(request: RunRequest, cli_command: bool) -> Result<RunHandle> {
    launch_mode_internal(request, cli_command, true)
}
fn launch_mode_internal(request: RunRequest, cli_command: bool, native: bool) -> Result<RunHandle> {
    let (settled_tx, settled) = async_channel::bounded::<()>(1);
    if request.prompt.trim().is_empty() && request.images.is_empty() {
        bail!("Write a message first");
    }
    if request.prompt.len() > PROMPT_LIMIT {
        bail!("Prompt exceeds the 1 MB input limit");
    }
    if !request.project.is_dir() {
        bail!("Project folder is unavailable");
    }
    if native && request.agent == AgentKind::Claude {
        return claude_control::launch(request);
    }
    if native && request.agent == AgentKind::Codex {
        return codex_server::launch(request);
    }
    if native && matches!(request.agent, AgentKind::Gemini | AgentKind::Cursor) {
        return if cli_command {
            gemini_acp::launch_command(request)
        } else {
            gemini_acp::launch(request)
        };
    }
    if native && request.agent == AgentKind::OpenCode {
        return if cli_command {
            opencode_server::launch_command(request)
        } else {
            opencode_server::launch(request)
        };
    }
    let mut native_observer = (request.agent == AgentKind::Codex).then(|| {
        let codex_root = std::env::var_os("CODEX_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|root| {
                if root.is_absolute() {
                    root
                } else {
                    request.project.join(root)
                }
            })
            .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".codex"));
        native_activity::Observer::new(
            codex_root,
            if request.fork_session {
                None
            } else {
                request.resume_id.as_deref()
            },
        )
    });
    let mut child = command_for(&request)?
        .spawn()
        .context("Could not start the agent CLI")?;
    let pid = child.id() as i32;
    let group = Arc::new(Mutex::new(Some(pid)));
    let cancelled = Arc::new(AtomicBool::new(false));
    let (event_tx, events) = mpsc::sync_channel(QUEUED_EVENTS_LIMIT);
    let (wake_tx, wake) = async_channel::bounded(1);
    let budget = Arc::new(QueueBudget::default());
    let events = EventReceiver {
        inner: events,
        budget: budget.clone(),
    };
    let tx = NotifyingSender {
        events: event_tx,
        wake: wake_tx,
        budget,
    };
    let finalized_source = Arc::new(Mutex::new(None));
    let handle = RunHandle {
        settled,
        events,
        wake,
        control: None,
        deferred_native_stop: false,
        cancelled: cancelled.clone(),
        process_group: group.clone(),
        finalized_source: finalized_source.clone(),
    };
    let mut stdout = child.stdout.take().context("Agent stdout is unavailable")?;
    let mut stderr = child.stderr.take().context("Agent stderr is unavailable")?;
    let mut stdin = child.stdin.take().context("Agent stdin is unavailable")?;
    // Pipes are nonblocking so a quiet agent, or a descendant keeping a pipe
    // open, cannot prevent cancellation and child reaping.
    for fd in [stdout.as_raw_fd(), stderr.as_raw_fd(), stdin.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
            let error = io::Error::last_os_error();
            handle.cancel();
            let mut owned_group = group.lock().unwrap_or_else(|e| e.into_inner());
            let _ = child.wait();
            *owned_group = None;
            return Err(error).context("Could not configure agent stream");
        }
    }
    // Writing the input also happens away from the UI thread. The CLI is
    // responsible for reading stdin and applying its existing auth/config.
    let requested_command = cli_command.then(|| {
        request
            .prompt
            .split_whitespace()
            .next()
            .unwrap_or("")
            .chars()
            .take(65)
            .collect()
    });
    let prompt = request.prompt;
    let images = request.images;
    let agent = request.agent;
    let input_done = Arc::new(AtomicBool::new(false));
    let writer_done = input_done.clone();
    let writer_cancelled = cancelled.clone();
    let input_writer = thread::spawn(move || {
        let mut pipe = InputPipe {
            stdin: &mut stdin,
            done: &writer_done,
            cancelled: &writer_cancelled,
        };
        write_agent_input(agent, &prompt, &images, &mut pipe)
    });
    thread::spawn(move || {
        let _settled = settled_tx;
        let mut parser = StreamParser {
            requested_command,
            fork_source_id: request
                .fork_session
                .then(|| request.resume_id.clone())
                .flatten(),
            ..StreamParser::default()
        };
        let mut line = wire::Record::default();
        let mut stderr_tail = Vec::with_capacity(STDERR_LIMIT);
        let mut buffer = [0u8; 8192];
        let mut stdout_eof = false;
        let mut stderr_eof = false;
        let mut exited_at = None;
        let mut failure = None;
        let _ = send(&tx, RuntimeEvent::Status("Connecting".into()), &cancelled);
        'running: loop {
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            // Limit each drain pass so noisy stderr cannot starve stdout or
            // process control. The channel in turn applies UI backpressure.
            for _ in 0..8 {
                match stdout.read(&mut buffer) {
                    Ok(0) => {
                        stdout_eof = true;
                        break;
                    }
                    Ok(count) => {
                        for chunk in buffer[..count].split_inclusive(|&b| b == b'\n') {
                            let complete = chunk.last() == Some(&b'\n');
                            let bytes = if complete {
                                &chunk[..chunk.len() - 1]
                            } else {
                                chunk
                            };
                            let result = line.push(bytes).and_then(|()| {
                                if complete && !line.is_empty() {
                                    dispatch_record(
                                        &mut parser,
                                        request.agent,
                                        &mut line,
                                        &tx,
                                        &cancelled,
                                    )?;
                                    line.clear();
                                }
                                Ok(())
                            });
                            if let Err(error) = result {
                                failure = Some(error.to_string());
                                break 'running;
                            }
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(e) => {
                        failure = Some(format!("Could not read agent output: {e}"));
                        break 'running;
                    }
                }
            }
            for _ in 0..8 {
                match stderr.read(&mut buffer) {
                    Ok(0) => {
                        stderr_eof = true;
                        break;
                    }
                    Ok(count) => {
                        stderr_tail.extend_from_slice(&buffer[..count]);
                        if stderr_tail.len() > STDERR_LIMIT {
                            stderr_tail.drain(..stderr_tail.len() - STDERR_LIMIT);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => {
                        stderr_eof = true;
                        break;
                    }
                }
            }
            if let Some(observer) = native_observer.as_mut() {
                for event in observer.poll(&mut parser, false) {
                    if let Err(error) = send(&tx, event, &cancelled) {
                        failure = Some(error.to_string());
                        break 'running;
                    }
                }
            }
            if exited_at.is_none() {
                // Observe the exited child without reaping it. Its reserved
                // PID keeps group ownership safe while descendants still hold
                // output pipes; Stop can continue to cancel that whole group.
                match child_exit_pending(pid) {
                    Ok(true) => {
                        exited_at = Some(Instant::now());
                    }
                    Ok(false) => {}
                    Err(e) => {
                        failure = Some(format!("Could not wait for agent: {e}"));
                        break;
                    }
                }
            }
            if exited_at.is_some()
                && ((stdout_eof && stderr_eof)
                    || exited_at.is_some_and(|t| t.elapsed() > Duration::from_millis(250)))
            {
                break;
            }
            // Readiness wakes immediately for output and pipe closure. Quiet
            // agents otherwise sleep in the kernel instead of waking 125 times
            // each second, and busy streams avoid an artificial 8 ms delay.
            let mut pipes = [
                libc::pollfd {
                    fd: if stdout_eof { -1 } else { stdout.as_raw_fd() },
                    events: libc::POLLIN,
                    revents: 0,
                },
                libc::pollfd {
                    fd: if stderr_eof { -1 } else { stderr.as_raw_fd() },
                    events: libc::POLLIN,
                    revents: 0,
                },
            ];
            let timeout = if stdout_eof && stderr_eof { 20 } else { 100 };
            if unsafe { libc::poll(pipes.as_mut_ptr(), pipes.len() as libc::nfds_t, timeout) } < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    failure = Some(format!("Could not wait for agent output: {error}"));
                    break;
                }
            }
        }
        let exit = {
            let mut owned_group = group.lock().unwrap_or_else(|e| e.into_inner());
            // The direct child has not been reaped, even on successful exit.
            // Terminate any remaining owned tool/MCP descendants before that
            // PID can be reused and before the handle loses group ownership.
            if let Some(pid) = *owned_group {
                unsafe {
                    libc::kill(-pid, libc::SIGKILL);
                }
            }
            let _ = child.kill();
            let exit = match child.wait() {
                Ok(status) => Some(status),
                Err(error) => {
                    failure.get_or_insert_with(|| format!("Could not reap agent: {error}"));
                    None
                }
            };
            *owned_group = None;
            exit
        };
        drop(stdout);
        drop(stderr);
        // Do not wait on a detached descendant retaining an input pipe.
        input_done.store(true, Ordering::Release);
        let input_result = input_writer.join();
        if !cancelled.load(Ordering::Acquire)
            && failure.is_none()
            && !line.is_empty()
            && let Err(error) =
                dispatch_record(&mut parser, request.agent, &mut line, &tx, &cancelled)
        {
            failure = Some(error.to_string());
        }
        if let Some(observer) = native_observer.as_mut() {
            for event in observer.poll(&mut parser, true) {
                if cancelled.load(Ordering::Acquire) {
                    let _ = tx.try_send(event);
                } else {
                    let _ = send(&tx, event, &cancelled);
                }
            }
        }
        let mut terminal = if cancelled.load(Ordering::Acquire) {
            RuntimeEvent::Status("Stopped".into())
        } else if let Some(error) = failure {
            RuntimeEvent::Failed(error)
        } else if let Some(error) = parser
            .storage_error
            .take()
            .or_else(|| parser.output.error().map(|error| format!("{error:#}")))
        {
            RuntimeEvent::Failed(format!(
                "Could not save the complete agent response: {error}"
            ))
        } else if let Some(error) = parser.codex_pending_error.take() {
            // Exec can emit an error while its own transport is reconnecting.
            // If it exits without turn.completed, retain that useful reason
            // even when partial output exists or the process exits with zero.
            RuntimeEvent::Failed(error)
        } else if exit.is_some_and(|s| s.success()) {
            match input_result {
                Ok(Ok(()))
                    if !parser.output.has_non_whitespace()
                        && parser.interaction_bytes == parser.fork_notice_bytes =>
                {
                    let details = String::from_utf8_lossy(&stderr_tail);
                    let details = details.trim();
                    RuntimeEvent::Failed(if details.is_empty() {
                        "Agent exited successfully without a response. Check its CLI version and configuration, then retry.".into()
                    } else {
                        format!("Agent exited successfully without a response.\n{details}")
                    })
                }
                Ok(Ok(())) => RuntimeEvent::Finished,
                _ => RuntimeEvent::Failed("Could not send the prompt to the agent".into()),
            }
        } else {
            let details = String::from_utf8_lossy(&stderr_tail).trim().to_owned();
            RuntimeEvent::Failed(if details.is_empty() {
                "Agent exited before completing the response".into()
            } else {
                details
            })
        };
        record_terminal(&mut parser, &terminal);
        match parser.rendered_source() {
            Ok(source) => {
                *finalized_source
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Some(source)
            }
            Err(error) => {
                terminal = RuntimeEvent::Failed(format!(
                    "Could not save the complete agent response: {error}"
                ))
            }
        }
        if cancelled.load(Ordering::Acquire) {
            let _ = tx.try_send(terminal);
        } else {
            let _ = send(&tx, terminal, &cancelled);
        }
    });
    Ok(handle)
}

/// Terminal status belongs in the canonical source too, including cancellation
/// when the live event queue is full. The UI independently paints its last event.
fn record_terminal(parser: &mut StreamParser, terminal: &RuntimeEvent) {
    use crate::chat_interactions::{ChatInteraction, InteractionStatus};
    let card = match terminal {
        RuntimeEvent::Failed(error) => Some(ChatInteraction::Status {
            title: if is_active_writer_conflict(error) {
                "Chat open in another app"
            } else {
                "Agent failed"
            }
            .into(),
            details: diagnostic_summary(error),
            status: InteractionStatus::Failed,
        }),
        RuntimeEvent::Status(status) if matches!(status.as_str(), "Stopped" | "Interrupted") => {
            Some(ChatInteraction::Status {
                title: "Stopped".into(),
                details: "You stopped this response.".into(),
                status: InteractionStatus::Cancelled,
            })
        }
        _ => None,
    };
    if let Some(card) = card {
        let _ = parser.interaction(card, "code.runtime.terminal");
    }
}

/// macOS and Linux support WNOWAIT: obtain an exit notification while leaving
/// the child waitable, so its PID cannot be reused before group cleanup.
pub(crate) fn child_exit_pending(pid: i32) -> io::Result<bool> {
    // Zeroed siginfo is valid and distinguishes WNOHANG's no-event result.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    if unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    } < 0
    {
        let error = io::Error::last_os_error();
        if error.kind() == io::ErrorKind::Interrupted {
            return Ok(false);
        }
        return Err(error);
    }
    Ok(info.si_signo == libc::SIGCHLD)
}

fn wait_for_pipe(fd: i32, events: i16) -> io::Result<()> {
    let mut pipe = libc::pollfd {
        fd,
        events,
        revents: 0,
    };
    if unsafe { libc::poll(&mut pipe, 1, 100) } < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}

trait EventSink {
    fn try_send(&self, event: RuntimeEvent) -> std::result::Result<(), TrySendError<RuntimeEvent>>;
    fn wait_for_space(&self, event: &RuntimeEvent, cancelled: &AtomicBool);
}
impl EventSink for SyncSender<RuntimeEvent> {
    fn try_send(&self, event: RuntimeEvent) -> std::result::Result<(), TrySendError<RuntimeEvent>> {
        SyncSender::try_send(self, event)
    }
    fn wait_for_space(&self, _event: &RuntimeEvent, _cancelled: &AtomicBool) {
        // Plain std senders are only used by protocol fixture tests.
        thread::sleep(Duration::from_millis(4));
    }
}
#[derive(Default)]
struct QueueBudget {
    bytes: Mutex<usize>,
    items: AtomicUsize,
    changed: Condvar,
    closed: AtomicBool,
}
/// Same receive operations as std's receiver, with queued payload accounting.
pub struct EventReceiver {
    inner: Receiver<RuntimeEvent>,
    budget: Arc<QueueBudget>,
}
impl EventReceiver {
    fn release(&self, event: &RuntimeEvent) {
        let mut bytes = self.budget.bytes.lock().unwrap_or_else(|e| e.into_inner());
        *bytes = bytes.saturating_sub(event_weight(event));
        self.budget.items.fetch_sub(1, Ordering::Release);
        drop(bytes);
        self.budget.changed.notify_one();
    }
    pub fn try_recv(&self) -> std::result::Result<RuntimeEvent, TryRecvError> {
        let event = self.inner.try_recv()?;
        self.release(&event);
        Ok(event)
    }
    #[cfg(test)]
    pub fn recv_timeout(
        &self,
        timeout: Duration,
    ) -> std::result::Result<RuntimeEvent, std::sync::mpsc::RecvTimeoutError> {
        let event = self.inner.recv_timeout(timeout)?;
        self.release(&event);
        Ok(event)
    }
}
impl Drop for EventReceiver {
    fn drop(&mut self) {
        let bytes = self.budget.bytes.lock().unwrap_or_else(|e| e.into_inner());
        self.budget.closed.store(true, Ordering::Release);
        drop(bytes);
        self.budget.changed.notify_all();
    }
}
fn event_weight(event: &RuntimeEvent) -> usize {
    64 + match event {
        RuntimeEvent::SessionId(s)
        | RuntimeEvent::Text(s)
        | RuntimeEvent::ReplaceText(s)
        | RuntimeEvent::ReplacePreview(s)
        | RuntimeEvent::Status(s)
        | RuntimeEvent::Failed(s) => s.len(),
        RuntimeEvent::Request(request) => native_request_weight(request),
        RuntimeEvent::RequestResolved(id) => id.len(),
        RuntimeEvent::RequestRejected { id, error } => id.len() + error.len(),
        RuntimeEvent::Voice(signal) => signal.weight(),
        RuntimeEvent::Finished => 0,
        RuntimeEvent::Usage(usage) => {
            std::mem::size_of::<crate::usage::Usage>()
                + usage.scope.len()
                + usage.model.as_ref().map_or(0, String::len)
                + usage
                    .limit_windows
                    .iter()
                    .map(|window| {
                        std::mem::size_of::<crate::usage::LimitWindow>() + window.label.len()
                    })
                    .sum::<usize>()
        }
        RuntimeEvent::Models(models) => {
            models.iter().map(|m| m.id.len() + m.label.len() + 64).sum()
        }
        RuntimeEvent::Commands(names) => names.iter().map(|s| s.len() + 24).sum(),
        RuntimeEvent::Interaction(card) => interaction_weight(card),
    }
}
fn native_request_weight(request: &NativeRequest) -> usize {
    let option = |o: &NativeOption| o.id.len() + o.label.len() + 48;
    request.id.len()
        + request.method.len()
        + request.title.len()
        + request.detail.len()
        + 128
        + match &request.kind {
            NativeRequestKind::Approval { choices } => choices.iter().map(option).sum::<usize>(),
            NativeRequestKind::Questions { questions } => questions
                .iter()
                .map(|q| {
                    q.id.len()
                        + q.header.len()
                        + q.question.len()
                        + q.options.iter().map(option).sum::<usize>()
                        + 128
                })
                .sum::<usize>(),
            NativeRequestKind::Unsupported => 0,
        }
}

fn interaction_weight(card: &crate::chat_interactions::ChatInteraction) -> usize {
    use crate::chat_interactions::ChatInteraction;
    match card {
        ChatInteraction::NativeRequest(request) => native_request_weight(request),
        ChatInteraction::Speaker { text, .. } => text.len(),
        ChatInteraction::Delegation {
            title,
            details,
            agent,
            ..
        } => title.len() + details.len() + agent.as_ref().map_or(0, String::len),
        ChatInteraction::Status { title, details, .. } => title.len() + details.len(),
        ChatInteraction::Question(q) => {
            q.id.len()
                + q.header.len()
                + q.prompt.len()
                + q.answer.as_ref().map_or(0, String::len)
                + q.options
                    .iter()
                    .map(|option| option.label.len() + option.description.len() + 64)
                    .sum::<usize>()
        }
        ChatInteraction::Answer { question, answer } => question.len() + answer.len(),
    }
}
struct NotifyingSender {
    events: SyncSender<RuntimeEvent>,
    wake: async_channel::Sender<()>,
    budget: Arc<QueueBudget>,
}
impl EventSink for NotifyingSender {
    fn try_send(&self, event: RuntimeEvent) -> std::result::Result<(), TrySendError<RuntimeEvent>> {
        if self.budget.closed.load(Ordering::Acquire) {
            return Err(TrySendError::Disconnected(event));
        }
        let mut bytes = self.budget.bytes.lock().unwrap_or_else(|e| e.into_inner());
        let weight = event_weight(&event);
        // Permit one oversized replacement when empty, rather than deadlocking
        // an otherwise valid response. Subsequent events wait for its receipt.
        if *bytes > 0 && weight > QUEUED_BYTES_LIMIT.saturating_sub(*bytes) {
            return Err(TrySendError::Full(event));
        }
        self.events.try_send(event)?;
        *bytes += weight;
        self.budget.items.fetch_add(1, Ordering::Release);
        drop(bytes);
        // Capacity one merges burst notifications without affecting the bounded
        // event queue. Never block the producer on the UI's notification drain.
        let _ = self.wake.try_send(());
        Ok(())
    }
    fn wait_for_space(&self, event: &RuntimeEvent, cancelled: &AtomicBool) {
        let bytes = self.budget.bytes.lock().unwrap_or_else(|e| e.into_inner());
        let weight = event_weight(event);
        let _guard = self
            .budget
            .changed
            .wait_timeout_while(bytes, Duration::from_millis(100), |bytes| {
                !self.budget.closed.load(Ordering::Acquire)
                    && !cancelled.load(Ordering::Acquire)
                    && ((*bytes > 0 && weight > QUEUED_BYTES_LIMIT.saturating_sub(*bytes))
                        || self.budget.items.load(Ordering::Acquire) >= QUEUED_EVENTS_LIMIT)
            })
            .unwrap_or_else(|e| e.into_inner());
    }
}

fn send(tx: &impl EventSink, mut event: RuntimeEvent, cancelled: &AtomicBool) -> Result<()> {
    loop {
        if cancelled.load(Ordering::Acquire) {
            bail!("Stopped");
        }
        match tx.try_send(event) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Disconnected(_)) => bail!("Stream consumer disconnected"),
            Err(TrySendError::Full(pending)) => {
                event = pending;
                tx.wait_for_space(&event, cancelled);
            }
        }
    }
}

fn dispatch_line(
    parser: &mut StreamParser,
    agent: AgentKind,
    line: &[u8],
    tx: &impl EventSink,
    cancelled: &AtomicBool,
) -> Result<()> {
    let value: Value = match serde_json::from_slice(line) {
        Ok(value) => value,
        Err(_)
            if agent == AgentKind::OpenCode
                && String::from_utf8_lossy(line).contains("permission requested:") =>
        {
            // The installed OpenCode run command prints permission rejection
            // notices outside its JSON framing. Keep them out of chat prose.
            return send(
                tx,
                RuntimeEvent::Status("Permission denied · configure OpenCode and retry".into()),
                cancelled,
            );
        }
        Err(error) => {
            return Err(error).context("Agent emitted invalid JSON; check its CLI version");
        }
    };
    dispatch_value(parser, agent, &value, tx, cancelled)
}

fn dispatch_record(
    parser: &mut StreamParser,
    agent: AgentKind,
    record: &mut wire::Record,
    tx: &impl EventSink,
    cancelled: &AtomicBool,
) -> Result<()> {
    if let Some(bytes) = record.small() {
        return dispatch_line(parser, agent, bytes, tx, cancelled);
    }
    let (value, shortened) = record
        .projected(cancelled)
        .context("Could not decode a large agent event")?;
    dispatch_value(parser, agent, &value, tx, cancelled)?;
    if shortened && !parser.large_preview_notified {
        parser.large_preview_notified = true;
        send(
            tx,
            RuntimeEvent::Status("Large event received · display preview shortened".into()),
            cancelled,
        )?;
    }
    Ok(())
}

fn dispatch_value(
    parser: &mut StreamParser,
    agent: AgentKind,
    value: &Value,
    tx: &impl EventSink,
    cancelled: &AtomicBool,
) -> Result<()> {
    if agent == AgentKind::Claude
        && value["type"] == "system"
        && value["subtype"] == "init"
        && value["parent_tool_use_id"].is_null()
    {
        let names = crate::commands::advertised(&value["slash_commands"]);
        parser.command_verified = parser.requested_command.as_ref().is_some_and(|requested| {
            names
                .iter()
                .any(|name| requested.strip_prefix('/') == Some(name.as_str()))
        });
        send(tx, RuntimeEvent::Commands(names), cancelled)?;
        if parser.requested_command.is_some() && !parser.command_verified {
            bail!(
                "The installed CLI no longer advertises this headless command. Refresh its command list with a normal turn."
            );
        }
    }
    let mut events = match agent {
        AgentKind::Codex => parser.parse_codex(value),
        AgentKind::Claude => parser.parse_claude(value),
        AgentKind::Grok => parser.parse_grok(value),
        AgentKind::Gemini => parser.parse_gemini(value),
        AgentKind::OpenCode => parser.parse_opencode(value),
        AgentKind::Antigravity => parser.parse_antigravity(value),
        AgentKind::Cursor => parser.parse_cursor(value),
    };
    if let Some(usage) = crate::usage::Usage::observed(agent, value) {
        events.push(RuntimeEvent::Usage(Box::new(usage)));
    }
    if let Some(error) = parser
        .storage_error
        .take()
        .or_else(|| parser.output.error().map(|error| format!("{error:#}")))
    {
        bail!("Could not save the complete agent response: {error}");
    }
    for event in events {
        // Finalize the canonical disk-backed response before publishing failure.
        // A GUI consumer may drop its handle as soon as it sees Failed.
        if let RuntimeEvent::Failed(error) = event {
            bail!("{error}");
        }
        send(tx, event, cancelled)?;
    }
    Ok(())
}

#[derive(Default)]
struct StreamParser {
    large_preview_notified: bool,
    requested_command: Option<String>,
    command_verified: bool,
    output: ResponseBuffer,
    storage_error: Option<String>,
    interactions: interaction_log::InteractionLog,
    interaction_bytes: usize,
    fork_source_id: Option<String>,
    fork_notice_bytes: usize,
    seen_interactions: std::collections::HashSet<u64>,
    recent_interactions: std::collections::VecDeque<u64>,
    session_announced: bool,
    current_id: String,
    current_start: usize,
    current_text_len: usize,
    current_validated: bool,
    failed: bool,
    codex_pending_error: Option<String>,
    child_tasks: std::collections::HashMap<String, String>,
    codex_native_id: Option<String>,
}

impl StreamParser {
    fn rendered_source(&self) -> Result<ResponseSource> {
        if self.interactions.is_empty() {
            return self.output.snapshot();
        }
        let mut output = ResponseBuffer::default();
        let mut cursor = 0;
        self.interactions.for_each(|offset, marker| {
            let offset = offset.min(self.output.len()).max(cursor);
            self.output.copy_range_to(cursor, offset, &mut output)?;
            output.push_str("\n\n");
            output.push_str(marker);
            output.push_str("\n\n");
            cursor = offset;
            Ok(())
        })?;
        self.output
            .copy_range_to(cursor, self.output.len(), &mut output)?;
        output.into_source()
    }
    fn replacement(&mut self) -> RuntimeEvent {
        match self.rendered_source().and_then(|source| {
            let large = source.len() > 64 * 1024;
            source.tail(64 * 1024).map(|text| (text, large))
        }) {
            Ok((text, false)) => RuntimeEvent::ReplaceText(text),
            Ok((text, true)) => RuntimeEvent::ReplacePreview(text),
            Err(error) => {
                self.storage_error = Some(error.to_string());
                RuntimeEvent::Status("Saving response".into())
            }
        }
    }
    #[cfg(test)]
    fn rendered_output(&self) -> String {
        let source = self.rendered_source().unwrap();
        source.read_range(0..source.len()).unwrap()
    }
    fn current_equals(&mut self, text: &str) -> bool {
        self.current_text_len == text.len()
            && self.matches_at(self.output.len() - self.current_text_len, text)
    }
    fn matches_at(&mut self, offset: usize, text: &str) -> bool {
        match self.output.matches_at(offset, text) {
            Ok(matches) => matches,
            Err(error) => {
                self.storage_error = Some(error.to_string());
                false
            }
        }
    }
    fn interaction(
        &mut self,
        card: crate::chat_interactions::ChatInteraction,
        key: &str,
    ) -> Vec<RuntimeEvent> {
        use std::hash::{Hash, Hasher};
        let marker = crate::chat_interactions::serialize_interaction(&card);
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut hash);
        marker.hash(&mut hash);
        let hash = hash.finish();
        if !self.seen_interactions.insert(hash) {
            return Vec::new();
        }
        self.recent_interactions.push_back(hash);
        if self.recent_interactions.len() > 2048
            && let Some(old) = self.recent_interactions.pop_front()
        {
            self.seen_interactions.remove(&old);
        }
        self.interaction_bytes += marker.len() + 4;
        if let Err(error) = self.interactions.push((self.output.len(), marker)) {
            self.storage_error = Some(error.to_string());
        }
        vec![RuntimeEvent::Interaction(Box::new(card))]
    }
    fn tool_interactions(&mut self, name: &str, input: &Value, key: &str) -> Vec<RuntimeEvent> {
        crate::chat_interactions::tool_interactions(name, input, false)
            .into_iter()
            .flat_map(|card| self.interaction(card, key))
            .collect()
    }
    #[cfg(test)]
    fn current_text(&self) -> String {
        self.output
            .read_range(self.output.len() - self.current_text_len..self.output.len())
            .unwrap()
    }

    fn realign_current_interactions(&mut self) {
        // A corrected snapshot replaces this whole message. Old byte offsets
        // cannot safely point into its new text (or split a UTF-8 character).
        // Keep earlier cards anchored, and move cards within this message after
        // its corrected text, preserving their relative ordering.
        if let Err(error) = self
            .interactions
            .reanchor(self.current_start, self.output.len())
        {
            self.storage_error = Some(error.to_string());
        }
    }

    fn begin(&mut self, id: &str) {
        if self.current_id != id {
            self.current_id = id.into();
            self.current_start = self.output.len();
            self.current_text_len = 0;
            self.current_validated = false;
        }
    }

    fn snapshot(&mut self, text: &str) -> Vec<RuntimeEvent> {
        // Completed events often repeat the same already-clean text. Avoid
        // parsing/allocating a sanitized copy when nothing changed.
        if self.current_validated && self.current_equals(text) {
            return Vec::new();
        }
        let Some(text) = presentation::sanitize_transcript_text("assistant", text) else {
            let had_text = self.output.len() > self.current_start;
            self.output.truncate(self.current_start);
            self.current_text_len = 0;
            self.realign_current_interactions();
            let mut events = vec![RuntimeEvent::Status("Processing agent context".into())];
            if had_text {
                events.push(self.replacement());
            }
            return events;
        };
        let text = text.as_str();
        self.current_validated = true;
        if self.current_equals(text) {
            return Vec::new();
        }
        if text.len() >= self.current_text_len
            && text.is_char_boundary(self.current_text_len)
            && self.matches_at(
                self.output.len() - self.current_text_len,
                &text[..self.current_text_len],
            )
        {
            let suffix = &text[self.current_text_len..];
            let separator = if self.current_text_len == 0 && !self.output.is_empty() {
                "\n\n"
            } else {
                ""
            };
            let delta = format!("{separator}{suffix}");
            self.output.push_str(&delta);
            self.current_text_len = text.len();
            vec![RuntimeEvent::Text(delta)]
        } else {
            self.output.truncate(self.current_start);
            if !self.output.is_empty() {
                self.output.push_str("\n\n");
            }
            self.output.push_str(text);
            self.current_text_len = text.len();
            self.realign_current_interactions();
            vec![self.replacement()]
        }
    }

    fn delta(&mut self, text: &str) -> Vec<RuntimeEvent> {
        if text.is_empty() {
            return Vec::new();
        }
        self.current_validated = false;
        let separator = if self.current_text_len == 0 && !self.output.is_empty() {
            "\n\n"
        } else {
            ""
        };
        self.current_text_len += text.len();
        let delta = format!("{separator}{text}");
        self.output.push_str(&delta);
        vec![RuntimeEvent::Text(delta)]
    }

    fn parse_codex(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        if value["type"] == "event_msg" {
            let payload = &value["payload"];
            if let Some(card) = crate::chat_interactions::native_event(payload) {
                return self
                    .interaction(card, payload["call_id"].as_str().unwrap_or("native-event"));
            }
            return self.tool_interactions(
                payload["type"].as_str().unwrap_or(""),
                payload,
                payload["call_id"].as_str().unwrap_or("native-tool"),
            );
        }
        match value["type"].as_str().unwrap_or("") {
            "thread.started" => {
                let Some(id) = value["thread_id"].as_str().filter(|id| !id.is_empty()) else {
                    return Vec::new();
                };
                self.codex_native_id = Some(id.into());
                let mut events = vec![RuntimeEvent::SessionId(id.into())];
                if self
                    .fork_source_id
                    .as_deref()
                    .is_some_and(|source| source != id)
                {
                    self.fork_source_id = None;
                    let before = self.interaction_bytes;
                    events.extend(self.interaction(
                        crate::chat_interactions::ChatInteraction::Status {
                            title: "Continued in Code".into(),
                            details: "The original chat remains in the other app.".into(),
                            status: crate::chat_interactions::InteractionStatus::Completed,
                        },
                        "code.session.forked",
                    ));
                    self.fork_notice_bytes += self.interaction_bytes - before;
                }
                events
            }
            "turn.started" => vec![RuntimeEvent::Status("Thinking".into())],
            "turn.completed" => {
                self.codex_pending_error = None;
                self.interaction(
                    crate::chat_interactions::ChatInteraction::Status {
                        title: "Completed".into(),
                        details: String::new(),
                        status: crate::chat_interactions::InteractionStatus::Completed,
                    },
                    "turn.completed",
                )
            }
            "item.started" | "item.updated" | "item.completed" => {
                let item = &value["item"];
                match item["type"].as_str().unwrap_or("") {
                    "agent_message" => {
                        self.begin(item["id"].as_str().unwrap_or("codex-message"));
                        item["text"]
                            .as_str()
                            .map(|s| self.snapshot(s))
                            .unwrap_or_default()
                    }
                    "command_execution" => vec![RuntimeEvent::Status(
                        if item["status"] == "completed" {
                            "Reading results"
                        } else {
                            "Running a command"
                        }
                        .into(),
                    )],
                    "mcp_tool_call" => {
                        let mut events = self.tool_interactions(
                            item["tool"].as_str().unwrap_or(""),
                            &item["arguments"],
                            item["id"].as_str().unwrap_or("tool"),
                        );
                        // Retain semantic server/tool identity for branded UI
                        // indicators. The UI exposes the provider's display name.
                        let server = item["server"].as_str().unwrap_or("");
                        let tool = item["tool"].as_str().unwrap_or("connected tool");
                        let identity: String =
                            format!("{server}/{tool}").chars().take(180).collect();
                        let stage = match item["status"].as_str() {
                            Some("failed") => "Tool failed",
                            Some("completed") => "Reading results",
                            _ => "Using",
                        };
                        events.push(RuntimeEvent::Status(format!("{stage} {identity}")));
                        events
                    }
                    "collab_tool_call" => self.codex_delegation(item),
                    "web_search" => vec![RuntimeEvent::Status("Searching".into())],
                    "reasoning" => vec![RuntimeEvent::Status("Thinking".into())],
                    _ => Vec::new(),
                }
            }
            "error" => {
                // Codex 0.162.1's exec JSONL emitter maps every app-server
                // error to this event and discards willRetry. The emitter
                // remains Running; only turn.failed or process exit settles
                // failure. Do not kill its reconnect or resubmit the prompt.
                let text = value["error"]["message"]
                    .as_str()
                    .or_else(|| value["message"].as_str())
                    .unwrap_or("Codex reported a connection or service error");
                self.codex_pending_error = Some(text.chars().take(2048).collect());
                vec![RuntimeEvent::Status(text.chars().take(280).collect())]
            }
            "turn.failed" => {
                self.failed = true;
                let text = value["error"]["message"]
                    .as_str()
                    .or_else(|| value["message"].as_str())
                    .or(self.codex_pending_error.as_deref())
                    .unwrap_or("Codex could not complete this turn");
                vec![RuntimeEvent::Failed(text.into())]
            }
            _ => Vec::new(),
        }
    }

    fn parse_claude(&mut self, value: &Value) -> Vec<RuntimeEvent> {
        // Child output stays in its own bounded native card, never the main
        // assistant text or the parent run's failure state.
        if let Some(id) = value["parent_tool_use_id"].as_str() {
            return self.claude_child_event(value, id);
        }
        if value["type"] == "system"
            && matches!(
                value["subtype"].as_str(),
                Some("task_started" | "task_progress" | "task_notification")
            )
        {
            return self.claude_task_event(value);
        }
        match value["type"].as_str().unwrap_or("") {
            "system" if value["subtype"] == "compact_boundary" => self.interaction(
                crate::chat_interactions::ChatInteraction::Status {
                    title: "Context compacted".into(),
                    details: "The CLI compacted this native conversation.".into(),
                    status: crate::chat_interactions::InteractionStatus::Completed,
                },
                "code.command.compact",
            ),
            "system" if value["subtype"] == "init" => {
                let mut events = vec![RuntimeEvent::Status("Thinking".into())];
                if let Some(id) = value["session_id"].as_str() {
                    events.push(RuntimeEvent::SessionId(id.into()));
                }
                events
            }
            "system" if value["subtype"] == "api_retry" => {
                vec![RuntimeEvent::Status("Reconnecting".into())]
            }
            "system" if value["subtype"] == "permission_denied" => vec![RuntimeEvent::Status(
                "Permission required · open the CLI to approve".into(),
            )],
            "stream_event" => {
                let event = &value["event"];
                match event["type"].as_str().unwrap_or("") {
                    "message_start" => {
                        self.begin(event["message"]["id"].as_str().unwrap_or("claude-message"));
                        Vec::new()
                    }
                    "content_block_delta" if event["delta"]["type"] == "text_delta" => {
                        event["delta"]["text"]
                            .as_str()
                            .map(|s| self.delta(s))
                            .unwrap_or_default()
                    }
                    "content_block_start" if event["content_block"]["type"] == "tool_use" => {
                        let name = event["content_block"]["name"].as_str().unwrap_or("tool");
                        self.remember_claude_child(&event["content_block"]);
                        vec![RuntimeEvent::Status(format!("Using {name}"))]
                    }
                    _ => Vec::new(),
                }
            }
            "assistant" => {
                let message = &value["message"];
                let id = message["id"].as_str().unwrap_or("claude-message");
                self.begin(id);
                let text = content_text_view(&message["content"]);
                let mut events = if text.is_empty() {
                    Vec::new()
                } else {
                    self.snapshot(&text)
                };
                if let Some(content) = message["content"].as_array() {
                    for block in content {
                        if block["type"] == "tool_use" {
                            self.remember_claude_child(block);
                            events.extend(self.tool_interactions(
                                block["name"].as_str().unwrap_or(""),
                                &block["input"],
                                block["id"].as_str().unwrap_or(id),
                            ));
                        }
                    }
                }
                events
            }
            "user" => self.claude_child_results(value),
            "result" => {
                if self.requested_command.is_some() && !self.command_verified {
                    self.failed = true;
                    return vec![RuntimeEvent::Failed(
                        "The CLI did not confirm this headless command is available.".into(),
                    )];
                }
                if !value["is_error"].as_bool().unwrap_or(false)
                    && value["subtype"] == "success"
                    && value["result"].as_str().is_none_or(str::is_empty)
                    && let Some(command) = self.requested_command.clone()
                {
                    return self.interaction(
                        crate::chat_interactions::ChatInteraction::Status {
                            title: "Command completed".into(),
                            details: format!("{command} completed in the installed CLI."),
                            status: crate::chat_interactions::InteractionStatus::Completed,
                        },
                        "code.command.completed",
                    );
                }
                if value["is_error"].as_bool().unwrap_or(false) {
                    self.failed = true;
                    let details = value["result"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| content_text(&value["errors"]));
                    vec![RuntimeEvent::Failed(if details.is_empty() {
                        "Claude could not complete this turn".into()
                    } else {
                        details
                    })]
                } else {
                    // `result` repeats the final assistant message after its
                    // streaming deltas and complete assistant event.
                    let text = value["result"].as_str().unwrap_or("");
                    if text.is_empty()
                        || (self.current_validated
                            && (self.current_equals(text) || self.output.ends_with(text)))
                    {
                        Vec::new()
                    } else {
                        self.snapshot(text)
                    }
                }
            }
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TranscriptMessage {
    #[serde(default)]
    pub code_language: Option<String>,
    pub role: String,
    pub text: String,
}

fn content_text_view(content: &Value) -> Cow<'_, str> {
    if let Some(text) = content.as_str() {
        return Cow::Borrowed(text);
    }
    let Some(blocks) = content.as_array() else {
        return Cow::Borrowed("");
    };
    let mut parts = blocks.iter().filter_map(|block| {
        block.as_str().or_else(|| {
            matches!(
                block["type"].as_str(),
                Some("text" | "input_text" | "output_text")
            )
            .then(|| block["text"].as_str())
            .flatten()
        })
    });
    let Some(first) = parts.next() else {
        return Cow::Borrowed("");
    };
    let Some(second) = parts.next() else {
        return Cow::Borrowed(first);
    };
    let mut text = String::with_capacity(first.len() + second.len() + 1);
    text.push_str(first);
    text.push('\n');
    text.push_str(second);
    for part in parts {
        text.push('\n');
        text.push_str(part);
    }
    Cow::Owned(text)
}
fn content_text(content: &Value) -> String {
    content_text_view(content).into_owned()
}

/// Legacy tail reader retained for migration fixture tests. Production uses reverse byte paging.
#[cfg(test)]
pub fn load_transcript(path: &Path, agent: AgentKind) -> Result<Vec<TranscriptMessage>> {
    if !matches!(agent, AgentKind::Codex | AgentKind::Claude) {
        bail!("Transcript import is currently available for Codex and Claude Code");
    }
    let mut file = File::open(path).context("Could not open session transcript")?;
    let size = file.metadata()?.len();
    let start = size.saturating_sub(TRANSCRIPT_LIMIT);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity(size.min(TRANSCRIPT_LIMIT) as usize);
    file.take(TRANSCRIPT_LIMIT).read_to_end(&mut bytes)?;
    let offset = if start > 0 {
        bytes
            .iter()
            .position(|&b| b == b'\n')
            .map(|n| n + 1)
            .unwrap_or(bytes.len())
    } else {
        0
    };
    let mut messages = Vec::new();
    for line in bytes[offset..].split(|&b| b == b'\n') {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            continue;
        };
        messages.extend(transcript_record(&value, agent));
    }
    Ok(messages)
}

fn transcript_record(value: &Value, agent: AgentKind) -> Vec<TranscriptMessage> {
    let mut messages = Vec::new();
    if agent == AgentKind::Codex && value["type"] == "event_msg" {
        let payload = &value["payload"];
        let mut cards = crate::chat_interactions::tool_interactions(
            payload["type"].as_str().unwrap_or(""),
            payload,
            true,
        );
        if let Some(card) = crate::chat_interactions::native_event(payload) {
            cards.push(card);
        }
        return cards
            .into_iter()
            .map(|card| TranscriptMessage {
                role: "tool".into(),
                text: crate::chat_interactions::encode(&card),
                code_language: None,
            })
            .collect();
    }
    if agent == AgentKind::Gemini {
        let role = match value["type"].as_str() {
            Some("user") => "user",
            Some("gemini" | "assistant") => "assistant",
            _ => return messages,
        };
        let text = if let Some(text) = value["content"].as_str() {
            text.to_owned()
        } else {
            value["content"]
                .as_array()
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|part| part["text"].as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .unwrap_or_default()
        };
        if let Some(text) = presentation::sanitize_transcript_text(role, &text) {
            messages.push(TranscriptMessage {
                code_language: None,
                role: role.into(),
                text,
            });
        }
        if let Some(tools) = value["toolCalls"].as_array() {
            for tool in tools {
                messages.extend(tool_messages(
                    tool["name"].as_str().unwrap_or("tool"),
                    &tool["args"],
                ));
            }
        }
        return messages;
    }
    if agent == AgentKind::Codex
        && value["type"] == "response_item"
        && matches!(
            value["payload"]["type"].as_str(),
            Some("function_call" | "custom_tool_call")
        )
    {
        let payload = &value["payload"];
        let arguments = payload["arguments"]
            .as_str()
            .and_then(|s| serde_json::from_str::<Value>(s).ok())
            .unwrap_or_else(|| payload["input"].clone());
        messages.extend(tool_messages(
            payload["name"].as_str().unwrap_or("tool"),
            &arguments,
        ));
        return messages;
    }
    if agent == AgentKind::Codex
        && value["type"] == "response_item"
        && value["payload"]["type"] == "function_call_output"
    {
        if let Some(output) = value["payload"]["output"].as_str() {
            messages.push(TranscriptMessage {
                code_language: None,
                role: "tool".into(),
                text: serde_json::json!({"orbit_tool":true,"label":"Tool output","details":output})
                    .to_string(),
            });
        }
        return messages;
    }
    let message = match agent {
        AgentKind::Codex
            if value["type"] == "response_item"
                && matches!(
                    value["payload"]["type"].as_str(),
                    Some("message" | "agent_message")
                ) =>
        {
            &value["payload"]
        }
        AgentKind::Claude
            if matches!(value["type"].as_str(), Some("user" | "assistant"))
                && !value["isSidechain"].as_bool().unwrap_or(false) =>
        {
            &value["message"]
        }
        _ => return messages,
    };
    let role = message["role"]
        .as_str()
        .or_else(|| {
            (agent == AgentKind::Codex && message["type"] == "agent_message")
                .then(|| message["author"].as_str())
                .flatten()
        })
        .unwrap_or("");
    if !matches!(role, "user" | "assistant") {
        return messages;
    }
    if let Some(text) =
        presentation::sanitize_transcript_text(role, &content_text(&message["content"]))
    {
        messages.push(TranscriptMessage {
            code_language: None,
            role: role.into(),
            text,
        });
    }
    if agent == AgentKind::Claude
        && let Some(content) = message["content"].as_array()
    {
        for block in content {
            if block["type"] == "tool_use" {
                messages.extend(tool_messages(
                    block["name"].as_str().unwrap_or("tool"),
                    &block["input"],
                ));
            }
        }
    }
    messages
}

#[cfg(test)]
pub struct TranscriptReader {
    reader: BufReader<std::io::Take<File>>,
    agent: AgentKind,
    pending: VecDeque<TranscriptMessage>,
    line: Vec<u8>,
    done: bool,
}

/// Streams the complete native history with bounded memory. An oversized or
/// malformed complete record fails the import explicitly instead of silently
/// discarding earlier conversation. A currently-being-written final record is
/// ignored until a later import; it is not yet a complete message.
#[cfg(test)]
pub fn transcript_messages(path: &Path, agent: AgentKind) -> Result<TranscriptReader> {
    if !matches!(agent, AgentKind::Codex | AgentKind::Claude) {
        bail!("Transcript import is currently available for Codex and Claude Code");
    }
    let file = File::open(path).context("Could not open session transcript")?;
    let snapshot_bytes = file.metadata()?.len();
    Ok(TranscriptReader {
        reader: BufReader::new(file.take(snapshot_bytes)),
        agent,
        pending: VecDeque::new(),
        line: Vec::with_capacity(8192),
        done: false,
    })
}

#[cfg(test)]
impl TranscriptReader {
    fn read_record(&mut self) -> Result<Option<Value>> {
        self.line.clear();
        loop {
            let bytes = self.reader.fill_buf()?;
            if bytes.is_empty() {
                self.done = true;
                // Native CLIs can append an incomplete final JSON record.
                return Ok(serde_json::from_slice(&self.line).ok());
            }
            let count = bytes
                .iter()
                .position(|&b| b == b'\n')
                .map(|n| n + 1)
                .unwrap_or(bytes.len());
            if self.line.len() + count > LIVE_OUTPUT_LIMIT {
                bail!(
                    "A native transcript record exceeds the 16 MB import limit. The original history is unchanged."
                );
            }
            let complete = bytes[count - 1] == b'\n';
            self.line.extend_from_slice(&bytes[..count]);
            self.reader.consume(count);
            if complete {
                if self.line.iter().all(u8::is_ascii_whitespace) {
                    self.line.clear();
                    continue;
                }
                return serde_json::from_slice(&self.line).map(Some).context("Native transcript contains an invalid complete JSON record; its history was not imported");
            }
        }
    }
}

#[cfg(test)]
impl Iterator for TranscriptReader {
    type Item = Result<TranscriptMessage>;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(message) = self.pending.pop_front() {
                return Some(Ok(message));
            }
            if self.done {
                return None;
            }
            match self.read_record() {
                Ok(Some(value)) => self.pending.extend(transcript_record(&value, self.agent)),
                Ok(None) => return None,
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
            }
        }
    }
}

fn tool_messages(name: &str, input: &Value) -> Vec<TranscriptMessage> {
    let cards = crate::chat_interactions::tool_interactions(name, input, true);
    if cards.is_empty() {
        vec![tool_message(name, input)]
    } else {
        cards
            .into_iter()
            .map(|card| TranscriptMessage {
                role: "tool".into(),
                text: crate::chat_interactions::encode(&card),
                code_language: None,
            })
            .collect()
    }
}

fn tool_message(name: &str, input: &Value) -> TranscriptMessage {
    let tool = name.rsplit('.').next().unwrap_or(name);
    let label = match tool {
        "exec" => "Ran agent tools".to_owned(),
        "exec_command" | "shell_command" | "Bash" => "Ran a command".to_owned(),
        "Read" | "read_file" => "Read a file".to_owned(),
        "Edit" | "Write" | "apply_patch" => "Updated files".to_owned(),
        "Grep" | "Glob" | "WebSearch" | "web_search" => "Searched".to_owned(),
        _ => format!("Used {}", name.chars().take(80).collect::<String>()),
    };
    let details = input["command"]
        .as_str()
        .or_else(|| input["cmd"].as_str())
        .or_else(|| input["file_path"].as_str())
        .or_else(|| input["description"].as_str())
        .or_else(|| input["code"].as_str())
        .or_else(|| input.as_str())
        .map(str::to_owned)
        .or_else(|| {
            input["command"].as_array().map(|parts| {
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            })
        })
        .unwrap_or_default();
    let mut clipped: String = details.chars().take(2000).collect();
    if clipped.len() < details.len() {
        clipped.push_str("\n… (tool details shortened)");
    }
    let details = clipped;
    TranscriptMessage {
        code_language: None,
        role: "tool".into(),
        text: serde_json::json!({"orbit_tool":true,"label":label,"details":details}).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "uses an installed, authenticated agent and a real model; opt in with CODE_SMOKE_AGENT and CODE_SMOKE_EXECUTABLE"]
    fn installed_agent_prompt_round_trip() {
        let name = std::env::var("CODE_SMOKE_AGENT").expect("set CODE_SMOKE_AGENT");
        let agent = match name.as_str() {
            "codex" => AgentKind::Codex,
            "claude" => AgentKind::Claude,
            "gemini" => AgentKind::Gemini,
            "opencode" => AgentKind::OpenCode,
            "antigravity" => AgentKind::Antigravity,
            "cursor" => AgentKind::Cursor,
            "grok" => AgentKind::Grok,
            _ => panic!("Unknown smoke-test agent"),
        };
        let executable = PathBuf::from(
            std::env::var_os("CODE_SMOKE_EXECUTABLE").expect("set CODE_SMOKE_EXECUTABLE"),
        );
        let project =
            std::env::temp_dir().join(format!("code-live-smoke-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&project).unwrap();
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(project.clone());
        let resume_id = std::env::var("CODE_SMOKE_RESUME").ok();
        let fork_session = std::env::var("CODE_SMOKE_FORK").is_ok_and(|value| value == "1");
        let handle = launch(RunRequest { reasoning_effort:std::env::var("CODE_SMOKE_EFFORT").ok(),
            agent, executable, project,
            prompt: "Reply exactly CODE_SMOKE_OK. Do not use tools, read files, run commands, or edit anything.".into(),
            resume_id: resume_id.clone(),
            fork_session,
            model: std::env::var("CODE_SMOKE_MODEL").ok(),
            images: vec![], allow_edits: false,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: std::env::var("CODE_SMOKE_TRUST").is_ok_and(|value| value == "1"),
        }).unwrap();
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut text = String::new();
        let mut session = None;
        let mut statuses = Vec::new();
        let mut completed = false;
        let mut fork_confirmed = false;
        loop {
            match handle.events.try_recv() {
                Ok(RuntimeEvent::Text(delta)) => text.push_str(&delta),
                Ok(
                    RuntimeEvent::ReplaceText(snapshot) | RuntimeEvent::ReplacePreview(snapshot),
                ) => text = snapshot,
                Ok(RuntimeEvent::SessionId(id)) => session = Some(id),
                Ok(RuntimeEvent::Status(status)) => statuses.push(status),
                Ok(RuntimeEvent::Request(request)) => panic!(
                    "Unexpected native request in a no-tools smoke: {}",
                    request.title
                ),
                Ok(RuntimeEvent::RequestResolved(_) | RuntimeEvent::RequestRejected { .. }) => {}
                Ok(
                    RuntimeEvent::Usage(_)
                    | RuntimeEvent::Commands(_)
                    | RuntimeEvent::Models(_)
                    | RuntimeEvent::Voice(_),
                ) => {}
                Ok(RuntimeEvent::Interaction(card)) => {
                    fork_confirmed |= matches!(card.as_ref(), crate::chat_interactions::ChatInteraction::Status { title, .. } if title == "Continued in Code");
                }
                Ok(RuntimeEvent::Finished) => {
                    completed = true;
                    break;
                }
                Ok(RuntimeEvent::Failed(error)) => panic!("{name} runtime failure: {error}"),
                Err(TryRecvError::Disconnected) => break,
                Err(TryRecvError::Empty) => {
                    if Instant::now() >= deadline {
                        handle.cancel();
                        panic!("{name} timed out; statuses: {statuses:?}");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
            }
        }
        assert!(completed, "{name} never completed; statuses: {statuses:?}");
        assert!(
            text.contains("CODE_SMOKE_OK"),
            "{name} returned no expected response: {text:?}"
        );
        assert!(!statuses.is_empty(), "{name} emitted no status");
        if fork_session {
            assert!(fork_confirmed, "Codex did not confirm the fork");
            assert!(session.is_some());
            assert_ne!(
                session, resume_id,
                "Fork reused the externally owned thread"
            );
        }
        eprintln!(
            "LIVE_SMOKE {name}: completed; session={session:?}; statuses={statuses:?}; text={text:?}"
        );
    }
    use serde_json::json;

    #[test]
    fn codex_snapshots_append_only_the_new_suffix() {
        let mut parser = StreamParser::default();
        assert_eq!(
            parser.parse_codex(&json!({"type":"thread.started","thread_id":"abc"})),
            vec![RuntimeEvent::SessionId("abc".into())]
        );
        assert_eq!(parser.parse_codex(&json!({"type":"item.updated","item":{"id":"1","type":"agent_message","text":"Hello"}})), vec![RuntimeEvent::Text("Hello".into())]);
        assert_eq!(parser.parse_codex(&json!({"type":"item.completed","item":{"id":"1","type":"agent_message","text":"Hello world"}})), vec![RuntimeEvent::Text(" world".into())]);
        assert!(parser.parse_codex(&json!({"type":"item.completed","item":{"id":"1","type":"agent_message","text":"Hello world"}})).is_empty());
        assert_eq!(parser.parse_codex(&json!({"type":"item.completed","item":{"id":"2","type":"agent_message","text":"Done"}})), vec![RuntimeEvent::Text("\n\nDone".into())]);
    }

    #[test]
    fn codex_transport_diagnostics_wait_for_the_authoritative_turn_result() {
        let reconnect = "Reconnecting... 2/5 (stream disconnected before completion: websocket closed by server before response.completed)";
        // Exec flattens app-server errors and discards willRetry. Also accept
        // structured errors without relying on a particular English message.
        for event in [
            json!({"type":"error","message":reconnect}),
            json!({"type":"error","error":{"message":"Temporary service interruption"},"willRetry":true}),
            json!({"type":"error","message":"Falling back from WebSockets to HTTPS transport"}),
        ] {
            let mut parser = StreamParser::default();
            parser.parse_codex(&json!({"type":"item.updated","item":{"id":"reply","type":"agent_message","text":"Before "}}));
            assert!(matches!(
                parser.parse_codex(&event).as_slice(),
                [RuntimeEvent::Status(_)]
            ));
            assert!(!parser.failed);
            assert!(parser.codex_pending_error.is_some());
            assert_eq!(parser.parse_codex(&json!({"type":"item.completed","item":{"id":"reply","type":"agent_message","text":"Before and after"}})), vec![RuntimeEvent::Text(" and after".into())]);
            parser.parse_codex(&json!({"type":"turn.completed","usage":{}}));
            assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Before and after");
            assert!(parser.codex_pending_error.is_none());
            assert!(!parser.failed);
        }
    }

    #[test]
    fn codex_exhausted_reconnect_is_a_real_failure_and_keeps_partial_output() {
        let mut parser = StreamParser::default();
        parser.parse_codex(&json!({"type":"item.updated","item":{"id":"reply","type":"agent_message","text":"Partial answer"}}));
        parser.parse_codex(&json!({"type":"error","message":"Reconnecting... 5/5"}));
        assert_eq!(parser.parse_codex(&json!({"type":"turn.failed","error":{"message":"stream disconnected after all retry attempts"}})), vec![RuntimeEvent::Failed("stream disconnected after all retry attempts".into())]);
        assert!(parser.failed);
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Partial answer");
    }

    #[test]
    fn corrected_snapshot_replaces_output_and_keeps_earlier_messages() {
        let mut parser = StreamParser::default();
        parser.begin("a");
        parser.snapshot("First");
        parser.begin("b");
        parser.snapshot("Draft");
        assert_eq!(
            parser.snapshot("Final"),
            vec![RuntimeEvent::ReplaceText("First\n\nFinal".into())]
        );
    }

    #[test]
    fn claude_deltas_complete_and_result_are_not_duplicated() {
        let mut parser = StreamParser::default();
        parser.parse_claude(
            &json!({"type":"stream_event","event":{"type":"message_start","message":{"id":"m1"}}}),
        );
        assert_eq!(parser.parse_claude(&json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hello"}}})), vec![RuntimeEvent::Text("Hello".into())]);
        assert!(parser.parse_claude(&json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Hello"}]}})).is_empty());
        assert!(
            parser
                .parse_claude(&json!({"type":"result","is_error":false,"result":"Hello"}))
                .is_empty()
        );
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "Hello");
    }

    #[test]
    fn claude_without_partial_events_and_errors_are_supported() {
        let mut parser = StreamParser::default();
        assert_eq!(parser.parse_claude(&json!({"type":"assistant","message":{"id":"m1","content":[{"type":"text","text":"Complete"}]}})), vec![RuntimeEvent::Text("Complete".into())]);
        assert_eq!(
            parser.parse_claude(
                &json!({"type":"result","is_error":true,"errors":["Authentication failed"]})
            ),
            vec![RuntimeEvent::Failed("Authentication failed".into())]
        );
        assert!(parser.failed);
    }

    #[test]
    fn transcript_tail_skips_partial_record_and_tool_blocks() {
        let path =
            std::env::temp_dir().join(format!("orbit-transcript-{}.jsonl", std::process::id()));
        let mut file = File::create(&path).unwrap();
        file.write_all(&vec![b'x'; TRANSCRIPT_LIMIT as usize + 7])
            .unwrap();
        writeln!(file).unwrap();
        writeln!(file, "{}", json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"Question"},{"type":"tool_result","content":"ignored"}]}})).unwrap();
        writeln!(file, "{}", json!({"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"Answer"}]}})).unwrap();
        drop(file);
        let messages = load_transcript(&path, AgentKind::Claude).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(
            messages,
            vec![
                TranscriptMessage {
                    code_language: None,
                    role: "user".into(),
                    text: "Question".into()
                },
                TranscriptMessage {
                    code_language: None,
                    role: "assistant".into(),
                    text: "Answer".into()
                }
            ]
        );
    }

    #[test]
    fn completed_snapshot_still_sanitizes_unvalidated_streamed_tags() {
        let mut parser = StreamParser::default();
        parser.begin("message");
        parser.delta("<final>Visible answer</final>");
        assert_eq!(
            parser.snapshot("<final>Visible answer</final>"),
            vec![RuntimeEvent::ReplaceText("Visible answer".into())]
        );
        assert!(parser.snapshot("Visible answer").is_empty());
        let content = serde_json::json!([{"type":"text","text":"One"},{"type":"tool_use","text":"Hidden"},{"type":"text","text":"Two"}]);
        assert_eq!(content_text_view(&content), "One\nTwo");
        let single = serde_json::json!([{"type":"text","text":"Borrowed"}]);
        assert!(matches!(
            content_text_view(&single),
            Cow::Borrowed("Borrowed")
        ));
    }

    #[test]
    fn empty_stream_chunks_do_not_insert_blank_messages_or_wake_the_ui() {
        let mut parser = StreamParser::default();
        parser.begin("first");
        parser.delta("First");
        parser.begin("second");
        assert!(parser.delta("").is_empty());
        assert_eq!(
            parser.delta("Second"),
            vec![RuntimeEvent::Text("\n\nSecond".into())]
        );
        assert_eq!(parser.output.tail(1024 * 1024).unwrap(), "First\n\nSecond");
    }

    fn notifying_fixture() -> (NotifyingSender, EventReceiver, async_channel::Receiver<()>) {
        let (tx, rx) = mpsc::sync_channel(QUEUED_EVENTS_LIMIT);
        let (wake_tx, wake_rx) = async_channel::bounded(1);
        let budget = Arc::new(QueueBudget::default());
        (
            NotifyingSender {
                events: tx,
                wake: wake_tx,
                budget: budget.clone(),
            },
            EventReceiver { inner: rx, budget },
            wake_rx,
        )
    }

    #[test]
    fn readiness_coalesces_bursts_and_closes_after_the_last_event() {
        let (tx, rx, wake) = notifying_fixture();
        tx.try_send(RuntimeEvent::Text("one".into())).unwrap();
        tx.try_send(RuntimeEvent::Text("two".into())).unwrap();
        assert_eq!(wake.len(), 1);
        wake.try_recv().unwrap();
        assert_eq!(rx.try_recv().unwrap(), RuntimeEvent::Text("one".into()));
        assert_eq!(rx.try_recv().unwrap(), RuntimeEvent::Text("two".into()));
        tx.try_send(RuntimeEvent::Finished).unwrap();
        drop(tx);
        // A pending readiness item remains receivable even after producer close.
        wake.try_recv().unwrap();
        assert_eq!(rx.try_recv().unwrap(), RuntimeEvent::Finished);
        assert!(wake.is_closed());
        assert_eq!(rx.try_recv(), Err(TryRecvError::Disconnected));
    }

    #[test]
    fn queued_payload_budget_releases_on_receive_and_allows_one_large_snapshot() {
        let (tx, rx, _wake) = notifying_fixture();
        let half = QUEUED_BYTES_LIMIT / 2 - 64;
        tx.try_send(RuntimeEvent::Text("a".repeat(half))).unwrap();
        tx.try_send(RuntimeEvent::Text("b".repeat(half))).unwrap();
        assert_eq!(*tx.budget.bytes.lock().unwrap(), QUEUED_BYTES_LIMIT);
        assert!(matches!(
            tx.try_send(RuntimeEvent::Finished),
            Err(TrySendError::Full(_))
        ));
        rx.try_recv().unwrap();
        tx.try_send(RuntimeEvent::Finished).unwrap();
        while rx.try_recv().is_ok() {}
        assert_eq!(*tx.budget.bytes.lock().unwrap(), 0);
        tx.try_send(RuntimeEvent::ReplaceText(
            "x".repeat(QUEUED_BYTES_LIMIT + 1),
        ))
        .unwrap();
        assert_eq!(*tx.budget.bytes.lock().unwrap(), QUEUED_BYTES_LIMIT + 65);
        assert!(matches!(
            tx.try_send(RuntimeEvent::Finished),
            Err(TrySendError::Full(_))
        ));
        rx.try_recv().unwrap();
        assert_eq!(*tx.budget.bytes.lock().unwrap(), 0);
        drop(rx);
        assert!(matches!(
            tx.try_send(RuntimeEvent::Finished),
            Err(TrySendError::Disconnected(_))
        ));
    }

    #[test]
    fn blocked_producers_resume_on_receive_or_exit_when_consumer_drops() {
        for drop_consumer in [false, true] {
            let (tx, rx, _wake) = notifying_fixture();
            for _ in 0..QUEUED_EVENTS_LIMIT {
                tx.try_send(RuntimeEvent::Status("fixture".into())).unwrap();
            }
            let (started_tx, started_rx) = mpsc::channel();
            let (done_tx, done_rx) = mpsc::channel();
            let worker = thread::spawn(move || {
                started_tx.send(()).unwrap();
                let result = send(&tx, RuntimeEvent::Finished, &AtomicBool::new(false));
                done_tx
                    .send(result.map_err(|error| error.to_string()))
                    .unwrap();
            });
            started_rx.recv_timeout(Duration::from_secs(2)).unwrap();
            if drop_consumer {
                drop(rx);
                assert_eq!(
                    done_rx
                        .recv_timeout(Duration::from_secs(2))
                        .unwrap()
                        .unwrap_err(),
                    "Stream consumer disconnected"
                );
            } else {
                rx.try_recv().unwrap();
                done_rx
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap()
                    .unwrap();
                let mut last = None;
                while let Ok(event) = rx.try_recv() {
                    last = Some(event);
                }
                assert_eq!(last, Some(RuntimeEvent::Finished));
            }
            worker.join().unwrap();
        }
    }

    #[test]
    fn legacy_exec_cancellation_exits_a_full_byte_budget_without_a_ui_consumer() {
        let executable = fixture_script(
            "cat >/dev/null\npython3 - <<'PY'\nimport json\nfor i in range(100):\n print(json.dumps({'type':'item.completed','item':{'id':'message','type':'agent_message','text':str(i)+'x'*60000}}),flush=True)\nPY",
        );
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "fixture".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while *handle.events.budget.bytes.lock().unwrap() < QUEUED_BYTES_LIMIT - 70000 {
            assert!(
                Instant::now() < deadline,
                "Fixture did not fill the byte budget"
            );
            thread::sleep(Duration::from_millis(5));
        }
        handle.cancel();
        let deadline = Instant::now() + Duration::from_secs(2);
        while handle.process_group.lock().unwrap().is_some() || !handle.wake.is_closed() {
            assert!(
                Instant::now() < deadline,
                "Budget-blocked producer was not cancelled/reaped"
            );
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn command_flags_preserve_permission_boundaries_and_resume() {
        let mut request = RunRequest { reasoning_effort:None,
            agent: AgentKind::Codex,
            executable: "codex".into(),
            project: "/tmp".into(),
            prompt: "hello".into(),
            resume_id: Some("abc".into()),
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: false,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        let command = command_for(&request).unwrap();
        let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(
            args,
            [
                "exec",
                "--sandbox",
                "read-only",
                "--json",
                "--color",
                "never",
                "--skip-git-repo-check",
                "resume",
                "abc",
                "-"
            ]
        );
        request.allow_edits = true;
        let command = command_for(&request).unwrap();
        let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
        assert_eq!(args[2], "workspace-write");
        assert!(!args.iter().any(|a| a.contains("bypass")));
        request.agent = AgentKind::Claude;
        for (allow_edits, mode) in [(false, "plan"), (true, "acceptEdits")] {
            request.allow_edits = allow_edits;
            let command = command_for(&request).unwrap();
            let args: Vec<_> = command.get_args().map(|a| a.to_str().unwrap()).collect();
            assert_eq!(
                args,
                [
                    "--print",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--include-partial-messages",
                    "--permission-mode",
                    mode,
                    "--resume",
                    "abc"
                ]
            );
            assert!(
                !args
                    .iter()
                    .any(|a| a.contains("bypass") || a.contains("skip-permissions"))
            );
        }
    }

    #[test]
    fn codex_fork_retains_native_permissions_model_images_and_stdin() {
        let image = std::env::temp_dir().join(format!("code-fork-{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&image, b"\x89PNG\r\n\x1a\nfixture").unwrap();
        let mut request = RunRequest { reasoning_effort:None,
            agent: AgentKind::Codex,
            executable: "codex".into(),
            project: std::env::temp_dir(),
            prompt: "private prompt absent from argv".into(),
            resume_id: Some("original-session".into()),
            fork_session: true,
            model: Some("gpt-5.3-codex".into()),
            images: vec![image.clone()],
            allow_edits: false,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        for (edits, mode) in [(false, "read-only"), (true, "workspace-write")] {
            request.allow_edits = edits;
            let command = command_for(&request).unwrap();
            let args: Vec<_> = command
                .get_args()
                .map(|arg| arg.to_str().unwrap())
                .collect();
            assert_eq!(
                args,
                [
                    "exec",
                    "--sandbox",
                    mode,
                    "--json",
                    "--color",
                    "never",
                    "--skip-git-repo-check",
                    "--model",
                    "gpt-5.3-codex",
                    "fork",
                    "original-session",
                    "--image",
                    image.to_str().unwrap(),
                    "-"
                ]
            );
            assert!(
                !args
                    .iter()
                    .any(|arg| arg.contains("bypass") || arg.contains("private prompt"))
            );
        }
        request.resume_id = None;
        assert!(
            command_for(&request)
                .unwrap_err()
                .to_string()
                .contains("source session")
        );
        for id in ["", "--last", "embedded\0null"] {
            request.resume_id = Some(id.into());
            assert!(command_for(&request).is_err());
        }
        request.resume_id = Some("original-session".into());
        request.agent = AgentKind::Claude;
        assert!(
            command_for(&request)
                .unwrap_err()
                .to_string()
                .contains("Codex source")
        );
        std::fs::remove_file(image).unwrap();
    }

    #[test]
    fn fork_notice_requires_a_new_thread_and_survives_corrected_snapshots() {
        let mut parser = StreamParser {
            fork_source_id: Some("original".into()),
            ..StreamParser::default()
        };
        assert_eq!(
            parser.parse_codex(&json!({"type":"thread.started","thread_id":"original"})),
            vec![RuntimeEvent::SessionId("original".into())]
        );
        let events = parser.parse_codex(&json!({"type":"thread.started","thread_id":"owned"}));
        assert_eq!(events[0], RuntimeEvent::SessionId("owned".into()));
        assert!(
            matches!(&events[1], RuntimeEvent::Interaction(card) if matches!(card.as_ref(), crate::chat_interactions::ChatInteraction::Status { title, details, status:crate::chat_interactions::InteractionStatus::Completed } if title=="Continued in Code" && details.contains("original chat")))
        );
        assert_eq!(parser.interaction_bytes, parser.fork_notice_bytes);
        assert_eq!(
            parser.parse_codex(&json!({"type":"thread.started","thread_id":"owned"})),
            vec![RuntimeEvent::SessionId("owned".into())]
        );
        parser.begin("reply");
        parser.snapshot("Draft answer");
        let corrected = parser.snapshot("Correct answer");
        assert!(
            matches!(&corrected[0], RuntimeEvent::ReplaceText(text) if text.contains("Continued in Code") && text.contains("Correct answer") && !text.contains("Draft answer"))
        );
    }

    #[test]
    fn legacy_exec_forked_session_is_resumed_on_the_following_prompt() {
        let executable = fixture_script(
            "mode=''\nwhile [ $# -gt 0 ]; do\n case \"$1\" in\n fork|resume) mode=\"$1\"; session=\"$2\"; break;;\n esac\n shift\ndone\nprompt=$(cat)\nif [ \"$mode:$session:$prompt\" != 'fork:original:first' ] && [ \"$mode:$session:$prompt\" != 'resume:owned:second' ]; then\n printf 'incorrect continuation arguments' >&2; exit 3\nfi\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"owned\"}' '{\"type\":\"item.completed\",\"item\":{\"id\":\"reply\",\"type\":\"agent_message\",\"text\":\"Ready\"}}'",
        );
        let mut id = "original".to_owned();
        for (fork, prompt) in [(true, "first"), (false, "second")] {
            let handle = launch_mode_internal(
                RunRequest { reasoning_effort:None,
                    agent: AgentKind::Codex,
                    executable: executable.clone(),
                    project: std::env::temp_dir(),
                    prompt: prompt.into(),
                    resume_id: Some(id.clone()),
                    fork_session: fork,
                    model: None,
                    images: vec![],
                    allow_edits: false,
                    codex_approvals: crate::runtime::CodexApprovals::default(),
                    trust_workspace: false,
                },
                false,
                false,
            )
            .unwrap();
            let mut saw_notice = false;
            let mut text = String::new();
            loop {
                match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    RuntimeEvent::SessionId(next) => id = next,
                    RuntimeEvent::Text(delta) => text.push_str(&delta),
                    RuntimeEvent::Interaction(card) => {
                        saw_notice |= matches!(card.as_ref(), crate::chat_interactions::ChatInteraction::Status { title, .. } if title == "Continued in Code")
                    }
                    RuntimeEvent::Finished => break,
                    RuntimeEvent::Failed(error) => panic!("{error}"),
                    _ => {}
                }
            }
            assert_eq!(id, "owned");
            assert_eq!(text, "Ready");
            assert_eq!(saw_notice, fork);
        }
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn legacy_exec_fork_announcement_without_a_reply_is_not_success() {
        let executable = fixture_script(
            "cat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"owned\"}'",
        );
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "first".into(),
                resume_id: Some("original".into()),
                fork_session: true,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Failed(error) => {
                    assert!(error.contains("without a response"));
                    break;
                }
                RuntimeEvent::Finished => panic!("Fork metadata is not an agent response"),
                _ => {}
            }
        }
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn native_mcp_status_preserves_brand_and_failure_for_indicators() {
        let mut parser = StreamParser::default();
        for (state, expected) in [
            ("in_progress", "Using"),
            ("completed", "Reading results"),
            ("failed", "Tool failed"),
        ] {
            let events = parser.parse_codex(&serde_json::json!({"type":"item.updated", "item":{"id":"mcp-fixture", "type":"mcp_tool_call", "server":"mcp__codex_apps__github", "tool":"get_pull_request", "status":state, "arguments":{}}}));
            let status = events
                .iter()
                .find_map(|event| {
                    if let RuntimeEvent::Status(status) = event {
                        Some(status)
                    } else {
                        None
                    }
                })
                .unwrap();
            assert!(status.starts_with(expected));
            assert_eq!(
                crate::kai_icons::integration_for_label(status),
                Some("github")
            );
            if state == "failed" {
                assert_eq!(crate::kai_icons::scene_for_status(status), "failure");
            }
        }
    }

    #[test]
    fn diagnostics_hide_log_noise_and_recognize_native_writer_conflicts() {
        for text in [
            "Error: thread-store conflict: already has an active writer",
            "THREAD STORE CONFLICT",
            "This thread already has an active writer",
        ] {
            assert!(is_active_writer_conflict(text));
            assert_eq!(
                diagnostic_summary(text),
                "This chat is already active in another app."
            );
        }
        assert!(!is_active_writer_conflict("permission denied"));
        assert_eq!(
            diagnostic_summary(
                "WARNING: PATH alias failed\n\u{1b}[31mError: model unavailable\u{1b}[0m\n2026 INFO shutdown complete"
            ),
            "Error: model unavailable"
        );
        assert_eq!(
            diagnostic_summary("WARNING: PATH alias failed"),
            "The agent could not complete this turn."
        );
        assert_eq!(
            diagnostic_summary(""),
            "The agent could not complete this turn."
        );
        let long = format!("Error: {}", "🌊".repeat(10000));
        let summary = diagnostic_summary(&long);
        assert!(summary.chars().count() <= 281);
        assert!(summary.ends_with('…'));
    }

    fn fixture_script(body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("orbit-agent-{}-{id}.sh", std::process::id()));
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn codex_reconnect_round_trip_finishes_only_after_recovery_or_final_failure() {
        for (tail, success, expected_error) in [
            (
                "printf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"id\":\"reply\",\"type\":\"agent_message\",\"text\":\"Partial recovered\"}}' '{\"type\":\"turn.completed\",\"usage\":{}}'",
                true,
                "",
            ),
            (
                "printf '%s\\n' '{\"type\":\"turn.failed\",\"error\":{\"message\":\"Retry limit exhausted\"}}'\nexit 1",
                false,
                "Retry limit exhausted",
            ),
            // Some wrappers lose the terminal protocol event. Neither a zero
            // exit nor existing partial text may conceal the pending error.
            ("exit 0", false, "Reconnecting... 2/5"),
            (
                "printf 'unrelated startup warning' >&2\nexit 1",
                false,
                "Reconnecting... 2/5",
            ),
        ] {
            let executable = fixture_script(&format!(
                "cat >/dev/null\nprintf '%s\\n' '{{\"type\":\"turn.started\"}}' '{{\"type\":\"item.updated\",\"item\":{{\"id\":\"reply\",\"type\":\"agent_message\",\"text\":\"Partial\"}}}}' '{{\"type\":\"error\",\"message\":\"Reconnecting... 2/5\"}}'\n{tail}"
            ));
            let handle = launch_mode_internal(
                RunRequest { reasoning_effort:None,
                    agent: AgentKind::Codex,
                    executable: executable.clone(),
                    project: std::env::temp_dir(),
                    prompt: "Hello".into(),
                    resume_id: None,
                    fork_session: false,
                    model: None,
                    images: vec![],
                    allow_edits: false,
                    codex_approvals: crate::runtime::CodexApprovals::default(),
                    trust_workspace: false,
                },
                false,
                false,
            )
            .unwrap();
            let mut output = String::new();
            let mut saw_reconnect = false;
            loop {
                match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    RuntimeEvent::Status(status) => {
                        saw_reconnect |= status == "Reconnecting... 2/5"
                    }
                    RuntimeEvent::Text(text) => output.push_str(&text),
                    RuntimeEvent::Finished => {
                        assert!(success);
                        break;
                    }
                    RuntimeEvent::Failed(error) => {
                        assert!(!success);
                        assert_eq!(error, expected_error);
                        break;
                    }
                    _ => {}
                }
            }
            assert!(saw_reconnect);
            assert_eq!(
                output,
                if success {
                    "Partial recovered"
                } else {
                    "Partial"
                }
            );
            assert!(handle.settled.recv_blocking().is_err());
            std::fs::remove_file(executable).unwrap();
        }
    }

    #[test]
    fn native_commands_require_fresh_advertisement_and_acknowledge_empty_success() {
        for available in [true, false] {
            let commands = if available { "[\"compact\"]" } else { "[]" };
            let body = format!(
                "cat >/dev/null\nprintf '%s\\n' '{{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"native-session\",\"slash_commands\":{commands}}}' '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"\"}}'\n"
            );
            let executable = fixture_script(&body);
            let handle = launch_mode_internal(
                RunRequest { reasoning_effort:None,
                    agent: AgentKind::Claude,
                    executable: executable.clone(),
                    project: std::env::temp_dir(),
                    prompt: "/compact".into(),
                    resume_id: Some("native-session".into()),
                    fork_session: false,
                    model: None,
                    images: vec![],
                    allow_edits: false,
                    codex_approvals: crate::runtime::CodexApprovals::default(),
                    trust_workspace: false,
                },
                true,
                false,
            )
            .unwrap();
            let mut completed = false;
            let mut acknowledged = false;
            loop {
                match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    RuntimeEvent::Interaction(card) => {
                        acknowledged |= matches!(*card, crate::chat_interactions::ChatInteraction::Status {title, ..} if title == "Command completed")
                    }
                    RuntimeEvent::Finished => {
                        completed = true;
                        break;
                    }
                    RuntimeEvent::Failed(error) => {
                        assert!(!available, "{error}");
                        assert!(error.contains("no longer advertises"));
                        break;
                    }
                    _ => {}
                }
            }
            assert_eq!(completed, available);
            assert_eq!(acknowledged, available);
            // A closed settled channel proves owned process cleanup completed.
            let _ = handle.settled.recv_blocking();
            assert!(handle.process_group.lock().unwrap().is_none());
            std::fs::remove_file(executable).unwrap();
        }
    }

    #[test]
    fn exited_cli_wrapper_does_not_leave_a_descendant_holding_output_pipes() {
        let executable = fixture_script(
            "cat >/dev/null\nsleep 30 &\nhelper=$!\nprintf '{\"type\":\"thread.started\",\"thread_id\":\"%s\"}\\n' \"$helper\"\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"id\":\"reply\",\"type\":\"agent_message\",\"text\":\"Ready\"}}'\nexit 0",
        );
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "fixture".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        let parent = handle.process_group.lock().unwrap().unwrap();
        let started = Instant::now();
        let mut descendant = None;
        let mut response = String::new();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(3)).unwrap() {
                RuntimeEvent::SessionId(id) => descendant = Some(id.parse::<i32>().unwrap()),
                RuntimeEvent::Text(text) => response.push_str(&text),
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(error) => panic!("Wrapper failed: {error}"),
                _ => {}
            }
        }
        assert_eq!(response, "Ready");
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(handle.process_group.lock().unwrap().is_none());
        assert_eq!(unsafe { libc::kill(parent, 0) }, -1);
        let descendant = descendant.unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while unsafe { libc::kill(descendant, 0) } == 0 {
            if Instant::now() >= deadline {
                // Clean up a broken fixture before reporting the regression.
                unsafe { libc::kill(descendant, libc::SIGKILL) };
                panic!("The exited CLI left its tool descendant alive");
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn nonreaping_exit_detection_leaves_the_child_waitable() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .spawn()
            .unwrap();
        let pid = child.id() as i32;
        let deadline = Instant::now() + Duration::from_secs(2);
        while !child_exit_pending(pid).unwrap() {
            assert!(Instant::now() < deadline, "Child never reported its exit");
            thread::sleep(Duration::from_millis(5));
        }
        // Repeated observations must neither consume status nor free the PID.
        assert!(child_exit_pending(pid).unwrap());
        assert_eq!(unsafe { libc::kill(pid, 0) }, 0);
        assert_eq!(child.wait().unwrap().code(), Some(7));
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
    }

    #[test]
    fn successful_exit_without_visible_output_reports_diagnostics_instead_of_finished() {
        for output in ["", "printf '%s\\n' '{\"type\":\"unknown_future_record\"}'"] {
            let executable = fixture_script(&format!(
                "cat >/dev/null\n{output}\nprintf 'configuration needs attention' >&2"
            ));
            let handle = launch_mode_internal(
                RunRequest { reasoning_effort:None,
                    agent: AgentKind::Codex,
                    executable: executable.clone(),
                    project: std::env::temp_dir(),
                    prompt: "fixture".into(),
                    resume_id: None,
                    fork_session: false,
                    model: None,
                    images: vec![],
                    allow_edits: false,
                    codex_approvals: crate::runtime::CodexApprovals::default(),
                    trust_workspace: false,
                },
                false,
                false,
            )
            .unwrap();
            loop {
                match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                    RuntimeEvent::Failed(error) => {
                        assert!(error.contains("without a response"));
                        assert!(error.contains("configuration needs attention"));
                        assert!(error.len() <= STDERR_LIMIT + 100);
                        break;
                    }
                    RuntimeEvent::Finished => panic!("An empty stream cannot count as a reply"),
                    RuntimeEvent::Text(_) | RuntimeEvent::ReplaceText(_) => {
                        panic!("Diagnostics must not become assistant prose")
                    }
                    _ => {}
                }
            }
            std::fs::remove_file(executable).unwrap();
        }
    }

    #[test]
    fn tool_only_completed_turn_remains_a_valid_success() {
        let executable =
            fixture_script("cat >/dev/null\nprintf '%s\\n' '{\"type\":\"turn.completed\"}'");
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "fixture".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        let mut saw_interaction = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Interaction(_) => saw_interaction = true,
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(error) => panic!("Completed tool-only turn failed: {error}"),
                _ => {}
            }
        }
        assert!(saw_interaction);
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn full_event_queue_retains_cancellation_state_without_blocking_terminal_delivery() {
        let executable = fixture_script(
            "cat >/dev/null\ni=0\nwhile [ $i -lt 140 ]; do\n printf '%s\\n' '{\"type\":\"turn.started\"}'\n i=$((i+1))\ndone\nsleep 30",
        );
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "fixture".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        assert!(!handle.is_cancelled());
        let deadline = Instant::now() + Duration::from_secs(5);
        while handle.events.budget.items.load(Ordering::Acquire) < QUEUED_EVENTS_LIMIT {
            assert!(
                Instant::now() < deadline,
                "Fixture did not fill the event queue"
            );
            thread::sleep(Duration::from_millis(5));
        }
        handle.cancel();
        let deadline = Instant::now() + Duration::from_secs(3);
        while handle.process_group.lock().unwrap().is_some() || !handle.wake.is_closed() {
            assert!(
                Instant::now() < deadline,
                "Cancellation blocked on terminal delivery"
            );
            thread::sleep(Duration::from_millis(5));
        }
        assert!(handle.is_cancelled());
        while let Ok(event) = handle.events.try_recv() {
            assert_ne!(event, RuntimeEvent::Status("Stopped".into()));
            assert_ne!(event, RuntimeEvent::Finished);
        }
        assert!(matches!(
            handle.events.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn cancellation_finishes_while_the_agent_never_reads_a_large_prompt() {
        let executable = fixture_script(
            "printf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"input-blocked\"}'\nsleep 30",
        );
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "x".repeat(PROMPT_LIMIT),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        loop {
            if matches!(
                handle.events.recv_timeout(Duration::from_secs(5)).unwrap(),
                RuntimeEvent::SessionId(_)
            ) {
                break;
            }
        }
        // The 1 MB prompt exceeds a pipe's capacity. Its writer must remain
        // blocked because this fixture never reads stdin, even while stdout
        // and process cancellation continue to work independently.
        thread::sleep(Duration::from_millis(100));
        let pid = handle.process_group.lock().unwrap().unwrap();
        handle.cancel();
        let deadline = Instant::now() + Duration::from_secs(3);
        while handle.process_group.lock().unwrap().is_some() || !handle.wake.is_closed() {
            assert!(
                Instant::now() < deadline,
                "Cancelled input writer did not join and close the stream"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let mut stopped = false;
        while let Ok(event) = handle.events.try_recv() {
            assert!(!matches!(event, RuntimeEvent::Finished));
            stopped |= event == RuntimeEvent::Status("Stopped".into());
        }
        assert!(stopped, "Cancellation must publish its terminal status");
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn process_streams_and_reaps_after_cancellation() {
        let executable = fixture_script(
            "cat >/dev/null\nprintf '%s\\n' '{\"type\":\"thread.started\",\"thread_id\":\"fixture\"}'\nsleep 30 &\nwait",
        );
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "Hello".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        loop {
            if matches!(
                handle.events.recv_timeout(Duration::from_secs(10)).unwrap(),
                RuntimeEvent::SessionId(_)
            ) {
                break;
            }
        }
        let pid = handle.process_group.lock().unwrap().unwrap();
        handle.cancel();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if handle.process_group.lock().unwrap().is_none() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "Agent was not reaped after cancellation"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn large_wire_events_keep_running_and_preserve_the_full_reply_and_final_status() {
        let executable = fixture_script("cat >/dev/null\ncat \"$0.payload\"");
        let payload = PathBuf::from(format!("{}.payload", executable.display()));
        let expected = "A line 🌊\n".repeat(150_000);
        {
            let mut file = std::io::BufWriter::new(File::create(&payload).unwrap());
            file.write_all(br#"{"type":"item.completed","item":{"aggregated_output":""#)
                .unwrap();
            for _ in 0..320 {
                file.write_all(&[b'x'; 65536]).unwrap();
            }
            file.write_all(b"\",\"type\":\"command_execution\",\"status\":\"completed\"}}\n")
                .unwrap();
            serde_json::to_writer(&mut file, &json!({"type":"item.completed","item":{"type":"agent_message","id":"large-reply","text":expected}})).unwrap();
            file.write_all(b"\n").unwrap();
            // Final unterminated JSONL record is valid, including after a spill.
            file.write_all(br#"{"unused_output":""#).unwrap();
            for _ in 0..32 {
                file.write_all(&[b'x'; 65536]).unwrap();
            }
            file.write_all(
                br#"","type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}"#,
            )
            .unwrap();
            file.flush().unwrap();
        }
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "Hello".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        let mut output = String::new();
        let mut saw_tool_status = false;
        let mut saw_complete = false;
        let mut saw_usage = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(20)).unwrap() {
                RuntimeEvent::Text(text) => output.push_str(&text),
                RuntimeEvent::ReplaceText(text) => output = text,
                RuntimeEvent::Status(text) => saw_tool_status |= text == "Reading results",
                RuntimeEvent::Interaction(card) => {
                    saw_complete |= matches!(card.as_ref(), crate::chat_interactions::ChatInteraction::Status { title, .. } if title == "Completed")
                }
                RuntimeEvent::Usage(usage) => saw_usage |= usage.input == Some(9),
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
        assert_eq!(output, expected.trim());
        assert!(saw_tool_status && saw_complete && saw_usage);
        assert!(handle.settled.recv_blocking().is_err());
        std::fs::remove_file(payload).unwrap();
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn large_projection_retains_each_providers_protocol_and_capability_events() {
        let fixtures = [
            (
                AgentKind::Codex,
                json!({"type":"item.completed","item":{"id":"r","type":"mcp_tool_call","tool":"request_user_input","arguments":{"questions":[{"id":"q","question":"Which?","options":[{"label":"One","description":"First"}]}]},"server":"connected","status":"completed"}}),
            ),
            (
                AgentKind::Claude,
                json!({"type":"system","subtype":"init","session_id":"c","model":"model-c","slash_commands":["compact","cost"]}),
            ),
            (
                AgentKind::Claude,
                json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hi"}}}),
            ),
            (
                AgentKind::Grok,
                json!({"type":"assistant","message":{"id":"r","model":"m","content":[{"type":"text","text":"Hi"}],"usage":{"input_tokens":8}}}),
            ),
            (
                AgentKind::Gemini,
                json!({"type":"message","role":"assistant","content":"Hi","delta":true}),
            ),
            (
                AgentKind::OpenCode,
                json!({"type":"text","sessionID":"oc","part":{"id":"r","text":"Hi"}}),
            ),
            (
                AgentKind::Antigravity,
                json!({"event":"step_update","step_update":{"step_type":"agent_response","step_index":2,"text_delta":"Hi"}}),
            ),
            (
                AgentKind::Cursor,
                json!({"type":"tool_call","subtype":"completed","tool_call":{"readToolCall":{"result":{"error":{"message":"Permission denied"}}}}}),
            ),
            (
                AgentKind::Claude,
                json!({"type":"result","result":"Hi","modelUsage":{"native-model":{"contextWindow":200000}},"usage":{"input_tokens":7}}),
            ),
        ];
        for (agent, value) in fixtures {
            let mut large = value.clone();
            large["unused_output"] = Value::String("x".repeat(2 * wire::MEMORY_BYTES));
            let mut record = wire::Record::default();
            record.push(&serde_json::to_vec(&large).unwrap()).unwrap();
            let (normal_tx, normal_rx) = mpsc::sync_channel(20);
            let (large_tx, large_rx) = mpsc::sync_channel(20);
            let cancelled = AtomicBool::new(false);
            dispatch_value(
                &mut StreamParser::default(),
                agent,
                &value,
                &normal_tx,
                &cancelled,
            )
            .unwrap();
            dispatch_record(
                &mut StreamParser::default(),
                agent,
                &mut record,
                &large_tx,
                &cancelled,
            )
            .unwrap();
            assert_eq!(
                normal_rx.try_iter().collect::<Vec<_>>(),
                large_rx.try_iter().collect::<Vec<_>>(),
                "{agent:?}"
            );
        }
    }

    #[test]
    fn cancellation_reaps_an_agent_in_the_middle_of_a_spilled_event() {
        let executable =
            fixture_script("cat >/dev/null\ncat \"$0.payload\"\ntouch \"$0.ready\"\nsleep 30");
        let payload = PathBuf::from(format!("{}.payload", executable.display()));
        let ready = PathBuf::from(format!("{}.ready", executable.display()));
        {
            let mut file = std::io::BufWriter::new(File::create(&payload).unwrap());
            file.write_all(br#"{"unused_output":""#).unwrap();
            for _ in 0..32 {
                file.write_all(&[b'x'; 65536]).unwrap();
            }
            file.flush().unwrap();
        }
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "Hello".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() {
            assert!(
                Instant::now() < deadline,
                "Fixture never flushed its large partial event"
            );
            thread::sleep(Duration::from_millis(10));
        }
        handle.cancel();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !handle.settled.is_closed() {
            assert!(
                Instant::now() < deadline,
                "Partial event prevented cancellation"
            );
            thread::sleep(Duration::from_millis(10));
        }
        assert!(handle.process_group.lock().unwrap().is_none());
        assert!(handle.is_cancelled());
        assert!(handle.events.try_recv().is_ok());
        while let Ok(event) = handle.events.try_recv() {
            assert!(!matches!(
                event,
                RuntimeEvent::Failed(_) | RuntimeEvent::Finished
            ));
        }
        for path in [payload, ready, executable] {
            std::fs::remove_file(path).unwrap();
        }
    }

    #[test]
    fn stderr_is_drained_without_exposing_it_as_chat_text() {
        let executable = fixture_script(
            "cat >/dev/null\ni=0\nwhile [ $i -lt 10000 ]; do printf '%s\\n' 'diagnostics diagnostics diagnostics diagnostics' >&2; i=$((i + 1)); done\nprintf '%s\\n' '{\"type\":\"item.completed\",\"item\":{\"id\":\"1\",\"type\":\"agent_message\",\"text\":\"Ready\"}}'",
        );
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "Hello".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        let mut output = String::new();
        loop {
            match handle.events.recv_timeout(Duration::from_secs(15)).unwrap() {
                RuntimeEvent::Text(text) => output.push_str(&text),
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
        assert_eq!(output, "Ready");
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn dropping_a_backpressured_stream_cancels_and_reaps_the_child() {
        let executable = fixture_script(
            "cat >/dev/null\ni=0\nwhile [ $i -lt 1000 ]; do printf '%s\\n' '{\"type\":\"turn.started\"}'; i=$((i + 1)); done\nsleep 30",
        );
        let handle = launch_mode_internal(
            RunRequest { reasoning_effort:None,
                agent: AgentKind::Codex,
                executable: executable.clone(),
                project: std::env::temp_dir(),
                prompt: "Hello".into(),
                resume_id: None,
                fork_session: false,
                model: None,
                images: vec![],
                allow_edits: false,
                codex_approvals: crate::runtime::CodexApprovals::default(),
                trust_workspace: false,
            },
            false,
            false,
        )
        .unwrap();
        let group = handle.process_group.clone();
        let pid = group.lock().unwrap().unwrap();
        // Leave the receiver unread until its bounded queue fills.
        thread::sleep(Duration::from_millis(150));
        drop(handle);
        let deadline = Instant::now() + Duration::from_secs(2);
        while group.lock().unwrap().is_some() {
            assert!(Instant::now() < deadline, "Backpressure prevented cleanup");
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        std::fs::remove_file(executable).unwrap();
    }

    #[test]
    fn output_above_sixteen_mb_continues_and_spills_without_losing_text() {
        let mut parser = StreamParser::default();
        let (tx, rx) = mpsc::sync_channel(4);
        let cancelled = AtomicBool::new(false);
        let chunk = "🌊".repeat(16 * 1024);
        for id in 0..320 {
            let line = serde_json::to_vec(&json!({"type":"item.completed","item":{"id":id.to_string(),"type":"agent_message","text":chunk}})).unwrap();
            dispatch_line(&mut parser, AgentKind::Codex, &line, &tx, &cancelled).unwrap();
            assert!(matches!(rx.try_recv().unwrap(), RuntimeEvent::Text(_)));
        }
        let source = parser.rendered_source().unwrap();
        assert!(source.len() > 20 * 1024 * 1024);
        assert!(matches!(&source, ResponseSource::File { .. }));
        assert!(source.matches_at(0, &chunk).unwrap());
        assert!(source.tail(chunk.len()).unwrap().ends_with(&chunk));
        assert!(parser.output.error().is_none());
        assert!(!parser.failed);
    }

    #[test]
    fn codex_transcript_import_ignores_duplicate_event_messages() {
        let path = fixture_script("");
        std::fs::write(&path, format!("{}\n{}\n{}\n",
            json!({"type":"event_msg","payload":{"type":"user_message","message":"Question"}}),
            json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Question"}]}}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Answer"}]}}))).unwrap();
        let messages = load_transcript(&path, AgentKind::Codex).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "Question");
        assert_eq!(messages[1].text, "Answer");
    }

    #[test]
    fn imported_approval_controls_are_hidden_and_real_tool_calls_are_semantic() {
        let path = fixture_script("");
        let record = |role: &str, text: &str| {
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":role,"content":[{"type":"input_text","text":text}]}}).to_string()
        };
        let tool = serde_json::json!({"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}"}});
        std::fs::write(&path, format!("{}\n{}\n{}\n{}\n", record("user", "Planned action JSON:\n{\"tool\":\"exec_command\"}\nAPPROVAL REQUEST END"), record("assistant", r#"{"risk_level":"low","user_authorization":"medium","outcome":"allow","rationale":"Requested"}"#), record("user", "Run the tests."), tool)).unwrap();
        let messages = load_transcript(&path, AgentKind::Codex).unwrap();
        std::fs::remove_file(path).unwrap();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].text, "Run the tests.");
        assert_eq!(
            presentation::message_blocks(&messages[1].role, &messages[1].text),
            vec![presentation::MessageBlock::Activity {
                label: "Ran a command".into(),
                details: "cargo test".into()
            }]
        );
    }

    #[test]
    fn full_transcript_reader_preserves_history_before_the_two_mb_tail() {
        let path = fixture_script("");
        let mut file = File::create(&path).unwrap();
        let record = |text: &str| {
            serde_json::json!({"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":text}]}}).to_string()
        };
        writeln!(file, "{}", record("Earliest question")).unwrap();
        let middle = record(&"x".repeat(1000));
        for _ in 0..2500 {
            writeln!(file, "{middle}").unwrap();
        }
        writeln!(file, "{}", record("Latest question")).unwrap();
        drop(file);
        let mut reader = transcript_messages(&path, AgentKind::Codex).unwrap();
        assert_eq!(reader.next().unwrap().unwrap().text, "Earliest question");
        let mut last = String::new();
        let mut count = 1;
        for message in reader {
            last = message.unwrap().text;
            count += 1;
        }
        assert_eq!(count, 2502);
        assert_eq!(last, "Latest question");
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_complete_record_fails_but_in_progress_tail_is_tolerated() {
        let path = fixture_script("");
        std::fs::write(&path, b"not JSON\n").unwrap();
        let mut reader = transcript_messages(&path, AgentKind::Claude).unwrap();
        assert!(
            reader
                .next()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("invalid complete JSON")
        );
        assert!(reader.next().is_none());
        std::fs::write(&path, b"{\"type\":").unwrap();
        assert!(
            transcript_messages(&path, AgentKind::Claude)
                .unwrap()
                .next()
                .is_none()
        );
        std::fs::remove_file(path).unwrap();
    }
}
