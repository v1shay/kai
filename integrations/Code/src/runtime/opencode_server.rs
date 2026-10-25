//! Owned, authenticated loopback OpenCode server + native SSE client. We never
//! attach to or stop another application's server. The installed /doc schema is
//! consulted before dispatching optional session operations.
//! Reference: https://dev.opencode.ai/docs/server/ .
use super::{
    NativeAnswer, NativeContext, NativeControl, NativeOption, NativeQuestion, NativeRequest,
    NativeRequestKind, RunHandle, RunRequest, RuntimeEvent, StreamParser, broker, native_handle,
    send, wire,
};
use crate::chat_interactions::{ChatInteraction, InteractionStatus};
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddrV4, TcpStream},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Receiver,
    },
    thread,
    time::{Duration, Instant},
};
const BODY_LIMIT: usize = 2 * 1024 * 1024;

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
        "OpenCode native forks require an explicit fork operation"
    );
    if let Some(service) = broker::checkout(&request)? {
        let (handle, cx) = native_handle(service.pid);
        service.attach(&cx)?;
        let mode = cli_command.then(|| ("opencode.command".into(), Value::Null));
        if let Err(error) = service
            .commands
            .try_send(broker::Command::Turn(Box::new((request, cx, mode))))
        {
            if let broker::Command::Turn(turn) = match error {
                std::sync::mpsc::TrySendError::Full(command)
                | std::sync::mpsc::TrySendError::Disconnected(command) => command,
            } {
                let (_, cx, _) = *turn;
                *cx.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
            }
            service.release();
            bail!("OpenCode did not receive this prompt; retry explicitly");
        }
        service.notify();
        return Ok(handle);
    }
    let password = uuid::Uuid::new_v4().to_string();
    let mut command = Command::new(&request.executable);
    crate::process_env::apply(&mut command);
    command
        .current_dir(&request.project)
        .args(["serve", "--hostname", "127.0.0.1", "--port", "0"])
        .env("OPENCODE_SERVER_PASSWORD", &password)
        .env("OPENCODE_SERVER_USERNAME", "code")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = command
        .spawn()
        .context("Could not start the OpenCode server")?;
    let (handle, cx) = native_handle(child.id() as i32);
    let (service, commands, wake) = match broker::register(&request, child.id() as i32) {
        Ok(registered) => registered,
        Err(error) => {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
            *cx.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
            return Err(error);
        }
    };
    if let Err(error) = service.attach(&cx) {
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
            cli_command,
            password,
            child,
            cx,
            service,
            commands,
            wake,
        )
    });
    Ok(handle)
}

struct Connection {
    api: Api,
    session: Session,
    stream: HttpBody,
    decoder: Sse,
}
#[allow(clippy::too_many_arguments)]
fn service_loop(
    mut request: RunRequest,
    mut cli_command: bool,
    password: String,
    mut child: Child,
    mut cx: NativeContext,
    service: Arc<broker::Service>,
    commands: Receiver<broker::Command>,
    mut wake: std::os::unix::net::UnixStream,
) {
    let mut cached = None;
    'service: loop {
        let mut parser = StreamParser::default();
        let mut active_session = None;
        let result = run(
            &request,
            cli_command,
            &password,
            &mut child,
            &cx,
            &mut parser,
            &mut active_session,
            cached.take(),
            &service,
            &commands,
        );
        let mut terminal = if cx.cancelled.load(Ordering::Acquire) {
            RuntimeEvent::Status("Stopped".into())
        } else {
            match &result {
                Ok(_) => RuntimeEvent::Finished,
                Err(error) => RuntimeEvent::Failed(format!("{error:#}")),
            }
        };
        super::record_terminal(&mut parser, &terminal);
        let mut healthy = result.is_ok() && !cx.cancelled.load(Ordering::Acquire);
        match parser.rendered_source() {
            Ok(source) => {
                *cx.finalized_source
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(source)
            }
            Err(error) => {
                terminal =
                    RuntimeEvent::Failed(format!("Could not retain OpenCode output: {error:#}"));
                healthy = false;
            }
        }
        if !healthy {
            service.begin_cleanup();
            if let Some((api, id)) = &active_session {
                let _ = api.abort_native(id);
                let deadline = Instant::now() + Duration::from_millis(850);
                for child in service.active_children() {
                    if Instant::now() >= deadline {
                        break;
                    }
                    let _ = api.abort_native(&child);
                }
            }
        }
        {
            let mut group = cx.group.lock().unwrap_or_else(|e| e.into_inner());
            if !healthy {
                if let Some(pid) = *group {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
                let _ = child.wait();
            }
            *group = None;
        }
        if healthy {
            service.release();
        } else {
            service.disconnected();
        }
        if cx.cancelled.load(Ordering::Acquire) {
            let _ = super::EventSink::try_send(&cx.tx, terminal);
        } else {
            let _ = send(&cx.tx, terminal, &cx.cancelled);
        }
        service.hold_settled(cx.settled);
        if !healthy {
            return;
        }
        let Ok(mut connection) = result else {
            return;
        };
        let (idle_handle, mut idle_ctx) = native_handle(child.id() as i32);
        *idle_ctx.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
        idle_ctx.cancelled = service.idle_cancelled.clone();
        let mut idle_parser = StreamParser::default();
        let mut buffer = [0u8; 1024];
        loop {
            if service.shutdown.load(Ordering::Acquire) || service.idle_expired() {
                let deadline = Instant::now() + Duration::from_millis(850);
                for id in service.active_children() {
                    if Instant::now() >= deadline {
                        break;
                    }
                    let _ = connection.api.abort_native(&id);
                }
                break 'service;
            }
            while wake.read(&mut buffer).is_ok_and(|count| count > 0) {}
            for _ in 0..32 {
                match commands.try_recv() {
                    Ok(broker::Command::Turn(turn)) => {
                        let (next, next_ctx, mode) = *turn;
                        request = next;
                        cx = next_ctx;
                        cli_command = mode.is_some();
                        cached = Some(connection);
                        continue 'service;
                    }
                    Ok(broker::Command::Child(id, action)) => {
                        child_control(
                            &mut connection.session,
                            &connection.api,
                            &service,
                            &id,
                            action,
                        );
                    }
                    Err(_) => break,
                }
            }
            let bytes = match connection.stream.poll() {
                Ok(bytes) => bytes,
                Err(_) => break 'service,
            };
            if connection
                .decoder
                .feed(&bytes, &idle_ctx.cancelled)
                .is_err()
            {
                break 'service;
            }
            while let Some(event) = connection.decoder.events.pop_front() {
                if connection
                    .session
                    .event(&event, &mut idle_parser, &idle_ctx)
                    .is_err()
                {
                    break 'service;
                }
                while idle_handle.events.try_recv().is_ok() {}
            }
            if connection.stream.done
                || super::child_exit_pending(child.id() as i32).unwrap_or(true)
            {
                break 'service;
            }
            if !bytes.is_empty() {
                continue;
            }
            let mut fds = [
                libc::pollfd {
                    fd: connection.stream.stream.as_raw_fd(),
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
                libc::poll(fds.as_mut_ptr(), 2, service.idle_wait_ms());
            }
        }
    }
    service.begin_cleanup();
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.wait();
    service.disconnected();
}
fn child_control(
    session: &mut Session,
    api: &Api,
    service: &Arc<broker::Service>,
    child: &str,
    action: broker::ChildAction,
) {
    let response = if let broker::ChildAction::Respond { id, .. } = &action {
        Some(id.clone())
    } else {
        None
    };
    let result = (|| -> Result<()> {
        ensure!(
            service.child(child),
            "This child is not owned by this service"
        );
        match action {
            broker::ChildAction::Stop => {
                api.abort_native(child)?;
            }
            broker::ChildAction::Steer(_) => {
                bail!("OpenCode does not expose active-turn child steering")
            }
            broker::ChildAction::Respond { id, answer } => {
                ensure!(
                    session
                        .pending
                        .get(&id)
                        .is_some_and(|pending| pending.session == child),
                    "This approval is not pending for this child"
                );
                let (handle, mut context) = native_handle(0);
                *context.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
                context.cancelled = service.idle_cancelled.clone();
                handle.respond(&id, answer)?;
                let mut parser = StreamParser::default();
                session.controls(api, &mut parser, &context)?;
                while let Ok(event) = handle.events.try_recv() {
                    match event {
                        RuntimeEvent::RequestRejected { id, error } => {
                            service.reject(child, &id, &error)
                        }
                        RuntimeEvent::RequestResolved(id) => service.resolve(&id),
                        _ => {}
                    }
                }
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        service.child_error(child, &format!("{error:#}"));
        if let Some(token) = response {
            service.reject(child, &token, &format!("{error:#}"));
        }
    }
}
fn process_child_controls(
    session: &mut Session,
    api: &Api,
    service: &Arc<broker::Service>,
    commands: &Receiver<broker::Command>,
) {
    for _ in 0..32 {
        match commands.try_recv() {
            Ok(broker::Command::Child(id, action)) => {
                child_control(session, api, service, &id, action)
            }
            Ok(broker::Command::Turn(turn)) => {
                let (_, context, _) = *turn;
                *context.group.lock().unwrap_or_else(|e| e.into_inner()) = None;
                let _=super::EventSink::try_send(&context.tx,RuntimeEvent::Failed("This native chat already has an active turn; the prompt was not dispatched".into()));
            }
            Err(_) => break,
        }
    }
}

#[derive(Clone)]
struct Api {
    address: SocketAddrV4,
    authorization: String,
    directory: String,
    paths: HashSet<String>,
}
impl Api {
    fn open(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        cancelled: &AtomicBool,
    ) -> Result<HttpBody> {
        ensure!(
            path.starts_with('/') && !path.contains(['\r', '\n']),
            "Invalid OpenCode endpoint"
        );
        ensure!(!cancelled.load(Ordering::Acquire), "Stopped");
        let mut stream = TcpStream::connect_timeout(&self.address.into(), Duration::from_secs(2))
            .context("Could not connect to the private OpenCode server")?;
        stream.set_read_timeout(Some(Duration::from_millis(200)))?;
        stream.set_write_timeout(Some(Duration::from_millis(200)))?;
        let body = body
            .map(serde_json::to_vec)
            .transpose()?
            .unwrap_or_default();
        let separator = if path.contains('?') { '&' } else { '?' };
        let headers = format!(
            "{method} {path}{separator}directory={} HTTP/1.1\r\nHost: {}\r\nAuthorization: Basic {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nConnection: close\r\nContent-Length: {}\r\n\r\n",
            self.directory,
            self.address,
            self.authorization,
            body.len()
        );
        write_cancellable(&mut stream, headers.as_bytes(), cancelled)?;
        write_cancellable(&mut stream, &body, cancelled)?;
        let mut headers = vec![];
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut buffer = [0u8; 4096];
        let split = loop {
            ensure!(!cancelled.load(Ordering::Acquire), "Stopped");
            ensure!(
                Instant::now() < deadline,
                "OpenCode did not send HTTP headers"
            );
            if let Some(at) = headers.windows(4).position(|v| v == b"\r\n\r\n") {
                break at + 4;
            }
            ensure!(
                headers.len() <= 64 * 1024,
                "OpenCode HTTP headers exceed the protocol budget"
            );
            match stream.read(&mut buffer) {
                Ok(0) => bail!("OpenCode closed its HTTP response"),
                Ok(n) => headers.extend_from_slice(&buffer[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock
                            | std::io::ErrorKind::TimedOut
                            | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        };
        let text = std::str::from_utf8(&headers[..split])?;
        let status = text
            .lines()
            .next()
            .and_then(|v| v.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u16>().ok())
            .context("Invalid OpenCode HTTP status")?;
        let mut chunked = false;
        let mut length = None;
        for line in text.lines().skip(1) {
            if let Some((name, value)) = line.split_once(':') {
                if name.eq_ignore_ascii_case("transfer-encoding") {
                    chunked = value.trim().eq_ignore_ascii_case("chunked");
                }
                if name.eq_ignore_ascii_case("content-length") {
                    length = value.trim().parse().ok();
                }
            }
        }
        stream.set_nonblocking(true)?;
        let response = HttpBody {
            stream,
            buffer: headers[split..].to_vec(),
            offset: 0,
            chunked,
            remaining: length,
            chunk_remaining: 0,
            chunk_crlf: false,
            done: status == 204,
            status,
        };
        Ok(response)
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        cancelled: &AtomicBool,
    ) -> Result<Value> {
        let mut response = self.open(method, path, body, cancelled)?;
        let status = response.status;
        let mut bytes = vec![];
        let streamed = path.ends_with("/command") || path.ends_with("/shell");
        let mut drained = false;
        let deadline = Instant::now() + Duration::from_secs(60);
        while !response.done {
            ensure!(
                path.ends_with("/command") || path.ends_with("/shell") || Instant::now() < deadline,
                "OpenCode metadata request timed out"
            );
            ensure!(!cancelled.load(Ordering::Acquire), "Stopped");
            let part = response.poll()?;
            if !drained && bytes.len() + part.len() > BODY_LIMIT {
                ensure!(
                    streamed,
                    "OpenCode metadata response exceeds its bounded display budget"
                );
                bytes.clear();
                drained = true;
            }
            if !drained {
                bytes.extend_from_slice(&part);
            }
            if part.is_empty() {
                thread::sleep(Duration::from_millis(10));
            }
        }
        let value = if drained {
            json!({"codeResponseStreamed":true})
        } else if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).context("OpenCode returned invalid JSON")?
        };
        ensure!(
            (200..300).contains(&status),
            "OpenCode request failed ({status}): {}",
            value["message"]
                .as_str()
                .or_else(|| value["data"]["message"].as_str())
                .unwrap_or("native endpoint declined the request")
        );
        Ok(value)
    }
    fn abort_native(&self, id: &str) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(850);
        let mut stream =
            TcpStream::connect_timeout(&self.address.into(), Duration::from_millis(100))
                .context("Could not reach OpenCode to stop this session")?;
        stream.set_nonblocking(true)?;
        let frame = format!(
            "POST /session/{id}/abort?directory={} HTTP/1.1\r\nHost: {}\r\nAuthorization: Basic {}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: 2\r\n\r\n{{}}",
            self.directory, self.address, self.authorization
        );
        let mut at = 0;
        while at < frame.len() && Instant::now() < deadline {
            match stream.write(&frame.as_bytes()[at..]) {
                Ok(0) => bail!("OpenCode closed before receiving the stop request"),
                Ok(count) => at += count,
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    let mut fd = libc::pollfd {
                        fd: stream.as_raw_fd(),
                        events: libc::POLLOUT,
                        revents: 0,
                    };
                    unsafe {
                        libc::poll(&mut fd, 1, 10);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        ensure!(
            at == frame.len(),
            "OpenCode did not accept the stop request in time"
        );
        let mut headers = Vec::with_capacity(4096);
        let mut bytes = [0u8; 1024];
        while Instant::now() < deadline && headers.len() < 16 * 1024 {
            match stream.read(&mut bytes) {
                Ok(0) => bail!("OpenCode closed before confirming the stop request"),
                Ok(count) => {
                    headers.extend_from_slice(&bytes[..count]);
                    if headers.windows(4).any(|window| window == b"\r\n\r\n") {
                        let code = std::str::from_utf8(&headers)?
                            .lines()
                            .next()
                            .and_then(|line| line.split_whitespace().nth(1))
                            .and_then(|code| code.parse::<u16>().ok());
                        ensure!(
                            code.is_some_and(|code| (200..300).contains(&code)),
                            "OpenCode declined the stop request"
                        );
                        return Ok(());
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) =>
                {
                    let mut fd = libc::pollfd {
                        fd: stream.as_raw_fd(),
                        events: libc::POLLIN,
                        revents: 0,
                    };
                    unsafe {
                        libc::poll(&mut fd, 1, 10);
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
        bail!("OpenCode did not confirm the stop request in time")
    }
    fn supports(&self, path: &str) -> bool {
        self.paths.contains(path)
    }
}
fn write_cancellable(stream: &mut TcpStream, bytes: &[u8], cancelled: &AtomicBool) -> Result<()> {
    let mut at = 0;
    let deadline = Instant::now() + Duration::from_secs(30);
    while at < bytes.len() {
        ensure!(!cancelled.load(Ordering::Acquire), "Stopped");
        ensure!(
            Instant::now() < deadline,
            "OpenCode stopped accepting protocol input"
        );
        match stream.write(&bytes[at..]) {
            Ok(0) => bail!("OpenCode connection closed"),
            Ok(n) => at += n,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                        | std::io::ErrorKind::TimedOut
                ) => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
struct HttpBody {
    stream: TcpStream,
    buffer: Vec<u8>,
    offset: usize,
    chunked: bool,
    remaining: Option<usize>,
    chunk_remaining: usize,
    chunk_crlf: bool,
    done: bool,
    status: u16,
}
impl HttpBody {
    fn poll(&mut self) -> Result<Vec<u8>> {
        if self.done {
            return Ok(vec![]);
        }
        if self.offset > 0 {
            self.buffer.drain(..self.offset);
            self.offset = 0;
        }
        let mut temp = [0u8; 8192];
        let mut eof = false;
        match self.stream.read(&mut temp) {
            Ok(0) => eof = true,
            Ok(n) => self.buffer.extend_from_slice(&temp[..n]),
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                ) => {}
            Err(e) => return Err(e.into()),
        }
        let mut out = vec![];
        if self.chunked {
            loop {
                if self.chunk_crlf {
                    if self.buffer.len() - self.offset < 2 {
                        break;
                    }
                    ensure!(
                        &self.buffer[self.offset..self.offset + 2] == b"\r\n",
                        "Invalid OpenCode HTTP chunk terminator"
                    );
                    self.offset += 2;
                    self.chunk_crlf = false;
                }
                if self.chunk_remaining == 0 {
                    let pending = &self.buffer[self.offset..];
                    let Some(end) = pending.windows(2).position(|v| v == b"\r\n") else {
                        ensure!(pending.len() < 1024, "Invalid OpenCode chunk header");
                        break;
                    };
                    let line = std::str::from_utf8(&pending[..end])?
                        .split(';')
                        .next()
                        .unwrap_or("");
                    self.chunk_remaining = usize::from_str_radix(line.trim(), 16)
                        .context("Invalid OpenCode HTTP chunk")?;
                    self.offset += end + 2;
                    if self.chunk_remaining == 0 {
                        self.done = true;
                        break;
                    }
                }
                let length = self.chunk_remaining.min(self.buffer.len() - self.offset);
                if length == 0 {
                    break;
                }
                out.extend_from_slice(&self.buffer[self.offset..self.offset + length]);
                self.offset += length;
                self.chunk_remaining -= length;
                if self.chunk_remaining == 0 {
                    self.chunk_crlf = true;
                }
            }
        } else {
            let available = self.buffer.len() - self.offset;
            let length = self.remaining.unwrap_or(available).min(available);
            out.extend_from_slice(&self.buffer[self.offset..self.offset + length]);
            self.offset += length;
            if let Some(remaining) = &mut self.remaining {
                *remaining -= length;
                if *remaining == 0 {
                    self.done = true;
                }
            } else if eof {
                self.done = true;
            }
        }
        if eof && !self.done {
            bail!("OpenCode event connection closed before completing its HTTP body");
        }
        Ok(out)
    }
}
#[derive(Default)]
struct Sse {
    prefix: Vec<u8>,
    is_data: bool,
    skip_space: bool,
    line_nonempty: bool,
    data: wire::Record,
    events: VecDeque<Value>,
}
impl Sse {
    fn feed(&mut self, bytes: &[u8], cancelled: &AtomicBool) -> Result<()> {
        for chunk in bytes.split_inclusive(|b| *b == b'\n') {
            let complete = chunk.last() == Some(&b'\n');
            let mut body = if complete {
                &chunk[..chunk.len() - 1]
            } else {
                chunk
            };
            if complete && body.last() == Some(&b'\r') {
                body = &body[..body.len() - 1];
            }
            if !body.is_empty() {
                self.line_nonempty = true;
            }
            while !body.is_empty() && !self.is_data && self.prefix.len() < 5 {
                self.prefix.push(body[0]);
                body = &body[1..];
                if self.prefix == b"data:" {
                    self.is_data = true;
                    self.skip_space = true;
                }
            }
            if self.is_data {
                if self.skip_space && !body.is_empty() {
                    if body[0] == b' ' {
                        body = &body[1..];
                    }
                    self.skip_space = false;
                }
                self.data.push(body)?;
            }
            if complete {
                if self.prefix == b"\r" && !self.is_data {
                    self.line_nonempty = false;
                }
                if !self.line_nonempty && !self.data.is_empty() {
                    let value = if let Some(bytes) = self.data.small() {
                        serde_json::from_slice(bytes)
                            .context("OpenCode emitted invalid SSE JSON")?
                    } else {
                        {
                            let (mut value, shortened) = self.data.projected(cancelled)?;
                            if shortened {
                                value["codeProjectionShortened"] = json!(true);
                            }
                            value
                        }
                    };
                    self.events.push_back(value);
                    self.data.clear();
                } else if self.is_data {
                    self.data.push(b"\n")?;
                }
                self.prefix.clear();
                self.is_data = false;
                self.skip_space = false;
                self.line_nonempty = false;
            }
        }
        Ok(())
    }
}
fn start_address(
    child: &mut Child,
    cx: &NativeContext,
    log_cancelled: Arc<AtomicBool>,
) -> Result<SocketAddrV4> {
    let mut out = child
        .stdout
        .take()
        .context("OpenCode server stdout unavailable")?;
    let mut error = child
        .stderr
        .take()
        .context("OpenCode server stderr unavailable")?;
    for fd in [out.as_raw_fd(), error.as_raw_fd()] {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        ensure!(
            flags >= 0 && unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } >= 0,
            "Could not configure OpenCode server output"
        );
    }
    let deadline = Instant::now() + Duration::from_secs(45);
    let mut output = vec![];
    let mut diagnostics = vec![];
    let mut bytes = [0u8; 4096];
    loop {
        ensure!(!cx.cancelled.load(Ordering::Acquire), "Stopped");
        ensure!(
            Instant::now() < deadline,
            "OpenCode server did not publish its private address"
        );
        for (pipe, dest) in [
            (&mut out as &mut dyn Read, &mut output),
            (&mut error as &mut dyn Read, &mut diagnostics),
        ] {
            match pipe.read(&mut bytes) {
                Ok(n) => {
                    dest.extend_from_slice(&bytes[..n]);
                    if dest.len() > super::STDERR_LIMIT {
                        dest.drain(..dest.len() - super::STDERR_LIMIT);
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
                    ) => {}
                Err(e) => return Err(e.into()),
            }
        }
        if let Some(address) = address_from_output(&String::from_utf8_lossy(&output)) {
            // Drain owned logs off the protocol worker. Hard bounded tails; cancellation
            // kills this process group and closes both pipes, ending the drain worker.
            let cancelled = log_cancelled;
            thread::spawn(move || {
                let mut bytes = [0u8; 4096];
                while !cancelled.load(Ordering::Acquire) {
                    let mut live = false;
                    for pipe in [&mut out as &mut dyn Read, &mut error as &mut dyn Read] {
                        match pipe.read(&mut bytes) {
                            Ok(0) => {}
                            Ok(_) => live = true,
                            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => live = true,
                            Err(_) => {}
                        }
                    }
                    if !live {
                        break;
                    }
                    let mut fds = [
                        libc::pollfd {
                            fd: out.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                        libc::pollfd {
                            fd: error.as_raw_fd(),
                            events: libc::POLLIN,
                            revents: 0,
                        },
                    ];
                    unsafe {
                        libc::poll(fds.as_mut_ptr(), 2, -1);
                    }
                }
            });
            return Ok(address);
        }
        if super::child_exit_pending(child.id() as i32)? {
            bail!(
                "OpenCode server stopped: {}",
                String::from_utf8_lossy(&diagnostics).trim()
            );
        }
        thread::sleep(Duration::from_millis(10));
    }
}
fn address_from_output(text: &str) -> Option<SocketAddrV4> {
    let prefix = "http://127.0.0.1:";
    let start = text.find(prefix)? + prefix.len();
    let digits: String = text[start..]
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    let suffix = &text[start + digits.len()..];
    if suffix.is_empty() || !suffix.starts_with(char::is_whitespace) {
        return None;
    }
    let port = digits.parse::<u16>().ok()?;
    (port > 0).then_some(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))
}
fn encode_component(text: &str) -> String {
    let mut result = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            result.push(byte as char);
        } else {
            use std::fmt::Write;
            let _ = write!(result, "%{byte:02X}");
        }
    }
    result
}
/// OpenCode v1.18.3 Identifier.create's ascending format. Counter zero is
/// reserved for client submissions; the harness starts its own counter at one.
/// Native assistant IDs therefore remain ordered after a same-tick user ID.
fn native_message_id() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let clock = millis.wrapping_mul(0x1000) & 0x0000_ffff_ffff_ffff;
    let random = uuid::Uuid::new_v4();
    let chars = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let suffix: String = random
        .as_bytes()
        .iter()
        .take(14)
        .map(|b| chars[*b as usize % 62] as char)
        .collect();
    format!("msg_{clock:012x}{suffix}")
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 256
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
}
fn basic(password: &str) -> String {
    let bytes = format!("code:{password}");
    let mut result = vec![0; bytes.len().div_ceil(3) * 4];
    let n = super::base64_chunk(bytes.as_bytes(), &mut result);
    String::from_utf8(result[..n].to_vec()).unwrap_or_default()
}

struct Pending {
    native_id: String,
    session: String,
    kind: PendingKind,
    v2: bool,
    reviewable: bool,
}
enum PendingKind {
    Permission,
    Question(Vec<String>),
}
struct Session {
    id: String,
    user_message: String,
    root_seen: bool,
    users: HashSet<String>,
    candidate_users: HashSet<String>,
    user_parts: HashSet<u64>,
    compaction_active: bool,
    task_continuation_ready: bool,
    summaries: HashSet<String>,
    children: HashSet<String>,
    assistant: HashSet<String>,
    child_assistant: HashSet<String>,
    parts: HashMap<String, String>,
    child_bytes: HashMap<String, usize>,
    pending: HashMap<String, Pending>,
    started: bool,
    complete: bool,
    error: Option<String>,
    service: Option<Arc<broker::Service>>,
}
impl Session {
    fn new(id: String) -> Self {
        let user_message = native_message_id();
        Self {
            id,
            users: std::iter::once(user_message.clone()).collect(),
            user_message,
            root_seen: false,
            candidate_users: HashSet::new(),
            user_parts: HashSet::new(),
            compaction_active: false,
            task_continuation_ready: false,
            summaries: HashSet::new(),
            children: HashSet::new(),
            assistant: HashSet::new(),
            child_assistant: HashSet::new(),
            parts: HashMap::new(),
            child_bytes: HashMap::new(),
            pending: HashMap::new(),
            started: false,
            complete: false,
            error: None,
            service: None,
        }
    }
    fn admit_user(&mut self, id: &str) {
        if self.users.len() >= 1024
            && let Some(old) = self
                .users
                .iter()
                .find(|old| old.as_str() != self.user_message)
                .cloned()
        {
            self.users.remove(&old);
        }
        self.users.insert(id.into());
        self.candidate_users.remove(id);
    }
    fn user_part(&mut self, part: &Value) {
        let id = part["messageID"].as_str().unwrap_or("");
        if id == self.user_message {
            self.root_seen = true;
        }
        if !self.root_seen {
            return;
        }
        if self.candidate_users.contains(id) {
            if part["type"] == "compaction" && part["auto"] == true {
                self.admit_user(id);
                self.compaction_active = true;
            } else if (part["synthetic"] == true && part["metadata"]["compaction_continue"] == true)
                || (self.compaction_active
                    && part_fingerprint(part).is_some_and(|hash| self.user_parts.contains(&hash)))
            {
                self.admit_user(id);
            } else if self.task_continuation_ready
                && part["synthetic"] == true
                && part["type"] == "text"
            {
                self.admit_user(id);
                self.task_continuation_ready = false;
            }
        }
        if self.users.contains(id)
            && self.user_parts.len() < 2048
            && let Some(hash) = part_fingerprint(part)
        {
            self.user_parts.insert(hash);
        }
    }
    fn event(
        &mut self,
        value: &Value,
        parser: &mut StreamParser,
        cx: &NativeContext,
    ) -> Result<()> {
        let shortened = value["codeProjectionShortened"] == true;
        let value = value.get("payload").unwrap_or(value);
        let kind = value["type"].as_str().unwrap_or("");
        let p = value
            .get("properties")
            .or_else(|| value.get("data"))
            .unwrap_or(&Value::Null);
        if kind == "session.created" {
            let info = &p["info"];
            if info["parentID"]
                .as_str()
                .is_some_and(|id| id == self.id || self.children.contains(id))
                && let Some(id) = info["id"].as_str()
            {
                ensure!(
                    self.children.len() < 128,
                    "OpenCode launched too many child sessions for one live view"
                );
                self.children.insert(id.into());
                if let Some(service) = &self.service {
                    service.observe(
                        id,
                        info["parentID"].as_str().unwrap_or(&self.id),
                        "native/state",
                        &json!({"status":"running"}),
                    )?;
                }
                emit(
                    parser.interaction(
                        ChatInteraction::Delegation {
                            title: info["title"].as_str().unwrap_or("OpenCode subagent").into(),
                            details: format!("Native child session {id}"),
                            status: InteractionStatus::Started,
                            agent: Some(id.into()),
                        },
                        &format!("opencode.child.{id}"),
                    ),
                    cx,
                )?;
            }
            return Ok(());
        }
        let sid = p["sessionID"]
            .as_str()
            .or_else(|| p["part"]["sessionID"].as_str())
            .or_else(|| p["info"]["sessionID"].as_str())
            .unwrap_or("");
        let child = self.children.contains(sid);
        if sid != self.id && !child {
            return Ok(());
        }
        let events = match kind {
            "message.updated" => {
                let info = &p["info"];
                if !child
                    && info["role"] == "user"
                    && let Some(id) = info["id"].as_str()
                {
                    if id == self.user_message {
                        self.root_seen = true;
                    } else if self.root_seen
                        && id > self.user_message.as_str()
                        && self.candidate_users.len() < 1024
                    {
                        self.candidate_users.insert(id.into());
                    }
                }
                if info["role"] == "assistant" {
                    if !child
                        && !info["parentID"]
                            .as_str()
                            .is_some_and(|id| self.users.contains(id))
                    {
                        return Ok(());
                    }
                    if info["summary"] == true
                        && let Some(id) = info["id"].as_str()
                        && self.summaries.len() < 1024
                    {
                        self.summaries.insert(id.into());
                    }

                    if let Some(id) = info["id"].as_str()
                        && self.assistant.len() < 1024
                    {
                        self.assistant.insert(id.into());
                        if child {
                            self.child_assistant.insert(id.into());
                        }
                    }
                    if sid == self.id {
                        self.started = true;
                        if !info["error"].is_null() {
                            self.error = Some(
                                info["error"]["data"]["message"]
                                    .as_str()
                                    .or_else(|| info["error"]["message"].as_str())
                                    .unwrap_or("OpenCode could not complete this turn")
                                    .into(),
                            );
                        }
                    }
                }
                vec![]
            }
            "message.part.delta"
                if p["field"] == "text"
                    && self
                        .assistant
                        .contains(p["messageID"].as_str().unwrap_or(""))
                    && self
                        .parts
                        .get(p["partID"].as_str().unwrap_or(""))
                        .is_some_and(|kind| kind == "text") =>
            {
                let text = p["delta"].as_str().unwrap_or("");
                if self
                    .summaries
                    .contains(p["messageID"].as_str().unwrap_or(""))
                {
                    vec![RuntimeEvent::Status("Compacting context".into())]
                } else if child {
                    let key = p["partID"].as_str().unwrap_or("").to_owned();
                    *self.child_bytes.entry(key).or_default() += text.len();
                    if let Some(service) = &self.service {
                        service.observe(
                            sid,
                            &self.id,
                            "item/agentMessage/delta",
                            &json!({"itemId":p["partID"],"delta":text}),
                        )?;
                        vec![]
                    } else {
                        child_output(parser, sid, text, InteractionStatus::Working)
                    }
                } else {
                    parser.begin(p["partID"].as_str().unwrap_or("opencode-response"));
                    parser.delta(text)
                }
            }
            "message.part.updated" => {
                let part = &p["part"];
                let part_kind = part["type"].as_str().unwrap_or("");
                if !child {
                    self.user_part(part);
                }

                if let Some(id) = part["id"].as_str() {
                    if self.parts.len() >= 4096
                        && !self.parts.contains_key(id)
                        && let Some(old) = self.parts.keys().next().cloned()
                    {
                        self.parts.remove(&old);
                        self.child_bytes.remove(&old);
                    }
                    self.parts.insert(id.into(), part_kind.into());
                }
                let mid = part["messageID"].as_str().unwrap_or("");
                if !self.assistant.contains(mid) {
                    vec![]
                } else if part_kind == "text" && self.summaries.contains(mid) {
                    vec![RuntimeEvent::Status("Compacting context".into())]
                } else if part_kind == "text" {
                    let text = part["text"].as_str().unwrap_or("");
                    if child {
                        let id = part["id"].as_str().unwrap_or("").to_owned();
                        if let Some(service) = &self.service {
                            ensure!(
                                !shortened
                                    || self.child_bytes.get(&id).is_some_and(|seen| *seen > 0),
                                "OpenCode delivered oversized final-only child output; its full native history was not replaced by a partial preview"
                            );
                            if !shortened {
                                service.observe(
                                    sid,
                                    &self.id,
                                    "item/completed",
                                    &json!({"item":{"id":id,"type":"agentMessage","text":text}}),
                                )?;
                            }
                        }
                        let seen = self.child_bytes.entry(id).or_default();
                        if shortened && *seen > 0 {
                            vec![]
                        } else if text.len() > *seen {
                            let start = if text.is_char_boundary(*seen) {
                                *seen
                            } else {
                                0
                            };
                            let delta = &text[start..];
                            *seen = text.len();
                            if self.service.is_some() {
                                vec![]
                            } else {
                                child_output(parser, sid, delta, InteractionStatus::Working)
                            }
                        } else {
                            vec![]
                        }
                    } else {
                        parser.begin(part["id"].as_str().unwrap_or("opencode-response"));
                        if shortened
                            && parser.current_id == part["id"].as_str().unwrap_or("")
                            && parser.current_text_len > 0
                        {
                            vec![]
                        } else {
                            parser.snapshot(text)
                        }
                    }
                } else if part_kind == "tool" {
                    if !child
                        && part["tool"] == "task"
                        && part["state"]["input"]["command"]
                            .as_str()
                            .is_some_and(|command| !command.is_empty())
                    {
                        self.task_continuation_ready = true;
                    }
                    if child {
                        child_output(
                            parser,
                            sid,
                            &format!(
                                "{} · {}\n{}",
                                part["tool"].as_str().unwrap_or("tool"),
                                part["state"]["status"].as_str().unwrap_or("running"),
                                part["state"]["output"].as_str().unwrap_or("")
                            ),
                            InteractionStatus::Working,
                        )
                    } else {
                        let mut events =
                            parser.parse_opencode(&json!({"type":"tool_use","part":part}));
                        events.extend(
                            parser.interaction(
                                ChatInteraction::Status {
                                    title: part["state"]["title"]
                                        .as_str()
                                        .or_else(|| part["tool"].as_str())
                                        .unwrap_or("OpenCode tool")
                                        .into(),
                                    details: bounded(
                                        part["state"]["output"]
                                            .as_str()
                                            .or_else(|| part["state"]["error"].as_str())
                                            .unwrap_or(""),
                                        64 * 1024,
                                    ),
                                    status: match part["state"]["status"].as_str().unwrap_or("") {
                                        "completed" => InteractionStatus::Completed,
                                        "error" => InteractionStatus::Failed,
                                        "pending" => InteractionStatus::Started,
                                        _ => InteractionStatus::Working,
                                    },
                                },
                                &format!("opencode.tool.{}", part["callID"].as_str().unwrap_or("")),
                            ),
                        );
                        events
                    }
                } else if part_kind == "reasoning" {
                    vec![RuntimeEvent::Status(
                        if child {
                            "Subagent thinking"
                        } else {
                            "Thinking"
                        }
                        .into(),
                    )]
                } else {
                    vec![]
                }
            }
            "session.compacted" if !child && self.root_seen => {
                self.compaction_active = false;
                vec![RuntimeEvent::Status("Context compacted".into())]
            }
            "session.status" => {
                let status = p["status"]["type"].as_str().unwrap_or("");
                if child {
                    if let Some(service) = &self.service {
                        service.observe(
                            sid,
                            &self.id,
                            if status == "idle" {
                                "native/idle"
                            } else {
                                "native/state"
                            },
                            &json!({"status":"running"}),
                        )?;
                    }
                    child_output(
                        parser,
                        sid,
                        &format!("Status · {status}"),
                        if status == "idle" {
                            InteractionStatus::Completed
                        } else {
                            InteractionStatus::Working
                        },
                    )
                } else {
                    match status {
                        "busy" => {
                            self.started = true;
                            vec![RuntimeEvent::Status("Working".into())]
                        }
                        "retry" => vec![RuntimeEvent::Status(format!(
                            "Reconnecting · {}",
                            p["status"]["message"]
                                .as_str()
                                .unwrap_or("OpenCode is retrying")
                        ))],
                        "idle" if self.started => {
                            self.complete = true;
                            vec![]
                        }
                        _ => vec![],
                    }
                }
            }
            "session.idle" if !child && self.started => {
                self.complete = true;
                vec![]
            }
            "session.error" => {
                let message = p["error"]["data"]["message"]
                    .as_str()
                    .or_else(|| p["error"]["message"].as_str())
                    .unwrap_or("OpenCode session failed");
                if child {
                    let cancelled = p["error"]["name"] == "MessageAbortedError";
                    if let Some(service) = &self.service {
                        service.observe(sid,&self.id,"turn/completed",&json!({"turn":{"status":if cancelled{"interrupted"}else{"failed"},"error":{"message":message}}}))?;
                    }
                    child_output(
                        parser,
                        sid,
                        message,
                        if cancelled {
                            InteractionStatus::Cancelled
                        } else {
                            InteractionStatus::Failed
                        },
                    )
                } else {
                    self.error = Some(message.into());
                    self.complete = true;
                    vec![]
                }
            }
            "permission.asked" | "permission.v2.asked" | "question.asked" | "question.v2.asked" => {
                ensure!(
                    !shortened,
                    "OpenCode's permission/question details exceed the safe approval display budget; no approval was sent"
                );
                self.request(kind, p, parser, cx)?;
                vec![]
            }
            "permission.replied"
            | "permission.v2.replied"
            | "question.replied"
            | "question.rejected"
            | "question.v2.replied"
            | "question.v2.rejected" => {
                let native = p["requestID"].as_str().unwrap_or("");
                let tokens: Vec<_> = self
                    .pending
                    .iter()
                    .filter(|(_, v)| v.native_id == native)
                    .map(|(k, _)| k.clone())
                    .collect();
                for id in tokens {
                    let child = self
                        .pending
                        .get(&id)
                        .is_some_and(|pending| pending.session != self.id);
                    self.pending.remove(&id);
                    if let Some(service) = &self.service {
                        service.resolve(&id);
                    }
                    if !child {
                        send(&cx.tx, RuntimeEvent::RequestResolved(id), &cx.cancelled)?;
                    }
                }
                vec![]
            }
            "todo.updated" => parser.interaction(
                ChatInteraction::Status {
                    title: if child { "Subagent plan" } else { "Agent plan" }.into(),
                    details: p["todos"]
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
                "opencode.plan",
            ),
            _ => vec![],
        };
        emit(events, cx)?;
        Ok(())
    }
    fn request(
        &mut self,
        kind: &str,
        p: &Value,
        parser: &mut StreamParser,
        cx: &NativeContext,
    ) -> Result<()> {
        let native_id = p["id"]
            .as_str()
            .or_else(|| p["requestID"].as_str())
            .context("OpenCode permission is missing an ID")?;
        ensure!(valid_id(native_id), "Invalid OpenCode request ID");
        if self.pending.values().any(|v| v.native_id == native_id) {
            return Ok(());
        }
        ensure!(
            self.pending.len() < 64,
            "Too many pending OpenCode requests"
        );
        let full_detail = approval_details(p);
        let reviewable = full_detail.is_some()
            && p["questions"].as_array().is_none_or(|questions| {
                questions.len() <= 16
                    && questions.iter().all(|q| {
                        q["options"]
                            .as_array()
                            .is_none_or(|options| options.len() <= 32)
                    })
            });
        let token = uuid::Uuid::new_v4().to_string();
        let v2 = kind.contains(".v2.");
        let session = p["sessionID"].as_str().unwrap_or(&self.id).to_owned();
        let (request_kind, pending_kind, title, detail) = if kind.starts_with("permission") {
            let action = p["permission"]
                .as_str()
                .or_else(|| p["action"].as_str())
                .unwrap_or("Tool permission");
            let resources = p
                .get("patterns")
                .or_else(|| p.get("resources"))
                .unwrap_or(&Value::Null);
            (
                NativeRequestKind::Approval {
                    choices: vec![
                        NativeOption {
                            id: "once".into(),
                            label: "Allow once".into(),
                        },
                        NativeOption {
                            id: "always".into(),
                            label: "Allow matching actions".into(),
                        },
                        NativeOption {
                            id: "reject".into(),
                            label: "Decline".into(),
                        },
                    ],
                },
                PendingKind::Permission,
                format!("Allow {action}?"),
                bounded(&format!("{}\n{}", resources, p["metadata"]), 64 * 1024),
            )
        } else {
            let mut ids = vec![];
            let questions = p["questions"]
                .as_array()
                .into_iter()
                .flatten()
                .take(16)
                .enumerate()
                .map(|(i, q)| {
                    let id = i.to_string();
                    ids.push(id.clone());
                    NativeQuestion {
                        id,
                        header: q["header"].as_str().unwrap_or("Question").into(),
                        question: q["question"].as_str().unwrap_or("").into(),
                        options: q["options"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .take(32)
                            .filter_map(|o| {
                                Some(NativeOption {
                                    id: o["label"].as_str()?.into(),
                                    label: o["label"].as_str()?.into(),
                                })
                            })
                            .collect(),
                        secret: false,
                        multiple: q["multiple"].as_bool().unwrap_or(false),
                        required: true,
                    }
                })
                .collect();
            (
                NativeRequestKind::Questions { questions },
                PendingKind::Question(ids),
                "OpenCode needs your input".into(),
                String::new(),
            )
        };
        let child = self.children.contains(&session);
        self.pending.insert(
            token.clone(),
            Pending {
                native_id: native_id.into(),
                session,
                kind: pending_kind,
                v2,
                reviewable,
            },
        );
        let request = NativeRequest {
            id: token,
            method: kind.into(),
            title,
            detail: full_detail.unwrap_or_else(||format!("{detail}\n\nThis native request is too large to review safely. Decline it and ask for a smaller action. No approval has been sent.")),
            kind: if reviewable{request_kind}else{NativeRequestKind::Unsupported},
        };
        if child && let Some(service) = &self.service {
            let session = self
                .pending
                .get(&request.id)
                .map(|pending| pending.session.as_str())
                .unwrap_or("");
            service.request(session, request);
            return Ok(());
        }
        emit(
            parser.interaction(
                ChatInteraction::NativeRequest(request.clone()),
                "opencode.request",
            ),
            cx,
        )?;
        send(
            &cx.tx,
            RuntimeEvent::Request(Box::new(request)),
            &cx.cancelled,
        )
    }
    fn controls(&mut self, api: &Api, parser: &mut StreamParser, cx: &NativeContext) -> Result<()> {
        while let Ok(control) = cx.control.try_recv() {
            match control {
                NativeControl::Respond { id, answer } => {
                    if let Some(pending) = self.pending.get(&id) {
                        if !pending.reviewable && !matches!(answer, NativeAnswer::Cancel) {
                            send(&cx.tx,RuntimeEvent::RequestRejected{id:id.clone(),error:"This native request cannot be reviewed safely and can only be declined.".into()},&cx.cancelled)?;
                            continue;
                        }
                        let answer = match (&pending.kind, answer) {
                            (
                                PendingKind::Permission | PendingKind::Question(_),
                                NativeAnswer::Cancel,
                            ) => NativeAnswer::Choice("reject".into()),
                            (_, answer) => answer,
                        };
                        let (path, body) = match (&pending.kind, answer) {
                            (PendingKind::Permission, NativeAnswer::Choice(reply))
                                if ["once", "always", "reject"].contains(&reply.as_str()) =>
                            {
                                (
                                    if pending.v2 {
                                        format!(
                                            "/api/session/{}/permission/{}/reply",
                                            pending.session, pending.native_id
                                        )
                                    } else {
                                        format!("/permission/{}/reply", pending.native_id)
                                    },
                                    json!({"reply":reply}),
                                )
                            }
                            (PendingKind::Question(ids), NativeAnswer::Answers(mut answers)) => {
                                let list: Vec<_> = ids
                                    .iter()
                                    .map(|id| answers.remove(id).unwrap_or_default())
                                    .collect();
                                (
                                    if pending.v2 {
                                        format!(
                                            "/api/session/{}/question/{}/reply",
                                            pending.session, pending.native_id
                                        )
                                    } else {
                                        format!("/question/{}/reply", pending.native_id)
                                    },
                                    json!({"answers":list}),
                                )
                            }
                            (PendingKind::Question(_), NativeAnswer::Choice(reply))
                                if reply == "reject" =>
                            {
                                (
                                    if pending.v2 {
                                        format!(
                                            "/api/session/{}/question/{}/reject",
                                            pending.session, pending.native_id
                                        )
                                    } else {
                                        format!("/question/{}/reject", pending.native_id)
                                    },
                                    json!({}),
                                )
                            }
                            _ => {
                                send(&cx.tx,RuntimeEvent::RequestRejected{id:id.clone(),error:"Choose a valid response to this native OpenCode request.".into()},&cx.cancelled)?;
                                continue;
                            }
                        };
                        if let Err(error) = api.request("POST", &path, Some(&body), &cx.cancelled) {
                            send(
                                &cx.tx,
                                RuntimeEvent::RequestRejected {
                                    id: id.clone(),
                                    error: format!("{error:#}"),
                                },
                                &cx.cancelled,
                            )?;
                            continue;
                        }
                        let child = pending.session != self.id;
                        self.pending.remove(&id);
                        if let Some(service) = &self.service {
                            service.resolve(&id);
                        }
                        if child {
                            continue;
                        }
                        send(&cx.tx, RuntimeEvent::RequestResolved(id), &cx.cancelled)?;
                        emit(
                            parser.interaction(
                                ChatInteraction::Status {
                                    title: "Response sent".into(),
                                    details: "Delivered to OpenCode's pending native request."
                                        .into(),
                                    status: InteractionStatus::Completed,
                                },
                                "opencode.resolved",
                            ),
                            cx,
                        )?;
                    }
                }
                NativeControl::Interrupt => {
                    api.request(
                        "POST",
                        &format!("/session/{}/abort", self.id),
                        Some(&json!({})),
                        &cx.cancelled,
                    )?;
                }
                NativeControl::Steer(_) => send(
                    &cx.tx,
                    RuntimeEvent::Status(
                        "OpenCode accepts the next prompt after this turn completes".into(),
                    ),
                    &cx.cancelled,
                )?,
                NativeControl::Rpc { method, params } => {
                    self.operation(&method, params, api, parser, cx)?
                }
            }
        }
        Ok(())
    }
    fn operation(
        &mut self,
        method: &str,
        params: Value,
        api: &Api,
        parser: &mut StreamParser,
        cx: &NativeContext,
    ) -> Result<()> {
        let (verb, template, path) = match method {
            "session/status" => ("GET", "/session/status", "/session/status".into()),
            "session/diff" => (
                "GET",
                "/session/{sessionID}/diff",
                format!("/session/{}/diff", self.id),
            ),
            "session/compact" => (
                "POST",
                "/session/{sessionID}/summarize",
                format!("/session/{}/summarize", self.id),
            ),
            "session/revert" => (
                "POST",
                "/session/{sessionID}/revert",
                format!("/session/{}/revert", self.id),
            ),
            "session/unrevert" => (
                "POST",
                "/session/{sessionID}/unrevert",
                format!("/session/{}/unrevert", self.id),
            ),
            "mcp/status" => ("GET", "/mcp", "/mcp".into()),
            "models/list" => ("GET", "/provider", "/provider".into()),
            "commands/list" => ("GET", "/command", "/command".into()),
            _ => {
                send(
                    &cx.tx,
                    RuntimeEvent::Status(
                        "OpenCode does not expose that session operation here".into(),
                    ),
                    &cx.cancelled,
                )?;
                return Ok(());
            }
        };
        ensure!(
            api.supports(template),
            "Installed OpenCode does not advertise {template}"
        );
        let result = api.request(
            verb,
            &path,
            (verb != "GET").then_some(&params),
            &cx.cancelled,
        )?;
        emit(
            parser.interaction(
                ChatInteraction::Status {
                    title: format!("OpenCode · {method}"),
                    details: bounded(&serde_json::to_string_pretty(&result)?, 64 * 1024),
                    status: InteractionStatus::Completed,
                },
                "opencode.operation",
            ),
            cx,
        )
    }
}
/// Exact native user-part content, excluding reassigned IDs/timestamps. These
/// bounded fingerprints recognize overflow replay without loading old history.
fn part_fingerprint(part: &Value) -> Option<u64> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    match part["type"].as_str()? {
        "text" => {
            "text".hash(&mut hash);
            part["text"].as_str()?.hash(&mut hash);
        }
        "file" => {
            "file".hash(&mut hash);
            part["mime"].as_str().unwrap_or("").hash(&mut hash);
            part["url"].as_str()?.hash(&mut hash);
        }
        _ => return None,
    }
    Some(hash.finish())
}
fn child_output(
    parser: &mut StreamParser,
    id: &str,
    text: &str,
    status: InteractionStatus,
) -> Vec<RuntimeEvent> {
    parser.interaction(
        ChatInteraction::Delegation {
            title: "OpenCode subagent".into(),
            details: bounded(text, 64 * 1024),
            status,
            agent: Some(id.into()),
        },
        &format!("opencode.child.{id}"),
    )
}
fn emit(events: Vec<RuntimeEvent>, cx: &NativeContext) -> Result<()> {
    for event in events {
        send(&cx.tx, event, &cx.cancelled)?;
    }
    Ok(())
}
fn approval_details(value: &Value) -> Option<String> {
    let text = serde_json::to_string_pretty(value).ok()?;
    (text.len() <= 64 * 1024).then_some(text)
}
fn bounded(text: &str, limit: usize) -> String {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}
const POLICY_MARKER: &str = "codeInteractivePermissionV1";
/// Native permission rules are ordered, with the last matching rule winning.
/// Replacing only `allow` with `ask` retains deny rules and their exceptions.
fn stricter_policy(agent: &Value, original: &Value, allow_edits: bool) -> Value {
    let mut rules = vec![json!({"permission":"*","pattern":"*","action":"ask"})];
    for rule in agent
        .as_array()
        .into_iter()
        .flatten()
        .chain(original.as_array().into_iter().flatten())
        .take(4096)
    {
        let mut rule = rule.clone();
        if rule["action"] == "allow" {
            rule["action"] = json!("ask");
        }
        rules.push(rule);
    }
    if !allow_edits {
        for permission in ["edit", "bash"] {
            rules.push(json!({"permission":permission,"pattern":"*","action":"deny"}));
        }
    }
    Value::Array(rules)
}
const OWNER_MARKER: &str = "codeNativeSessionV1";
fn is_owned_session(info: &Value) -> bool {
    info["metadata"][OWNER_MARKER]["version"] == 1
        && info["metadata"][OWNER_MARKER]["createdBy"] == "Code"
}
fn apply_owned_policy(
    api: &Api,
    id: &str,
    info: &Value,
    agent: &Value,
    allow_edits: bool,
    source: Option<&str>,
    cx: &NativeContext,
) -> Result<()> {
    let current = info.get("permission").cloned().unwrap_or_else(|| json!([]));
    let marker = &info["metadata"][OWNER_MARKER];
    // Only newly-created/forked or previously Code-owned sessions reach this
    // function. Shared imported source sessions are never patched.
    let original = if marker["applied"] == current && marker["baseline"].is_array() {
        marker["baseline"].clone()
    } else if info["metadata"][POLICY_MARKER]["applied"] == current
        && info["metadata"][POLICY_MARKER]["original"].is_array()
    {
        info["metadata"][POLICY_MARKER]["original"].clone()
    } else {
        current
    };
    let applied = stricter_policy(&agent["permission"], &original, allow_edits);
    let mut metadata = info
        .get("metadata")
        .filter(|v| v.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(map) = metadata.as_object_mut() {
        map.remove(POLICY_MARKER);
    }
    metadata[OWNER_MARKER] = json!({"version":1,"createdBy":"Code","sourceId":source,"baseline":original,"applied":applied});
    api.request(
        "PATCH",
        &format!("/session/{id}"),
        Some(&json!({"permission":applied,"metadata":metadata})),
        &cx.cancelled,
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn run(
    request: &RunRequest,
    cli_command: bool,
    password: &str,
    child: &mut Child,
    cx: &NativeContext,
    parser: &mut StreamParser,
    active_session: &mut Option<(Api, String)>,
    cached: Option<Connection>,
    service: &Arc<broker::Service>,
    broker_commands: &Receiver<broker::Command>,
) -> Result<Connection> {
    send(
        &cx.tx,
        RuntimeEvent::Status("Connecting to OpenCode".into()),
        &cx.cancelled,
    )?;
    let api = if let Some(connection) = &cached {
        connection.api.clone()
    } else {
        let address = start_address(child, cx, service.idle_cancelled.clone())?;
        let mut api = Api {
            address,
            authorization: basic(password),
            directory: encode_component(&request.project.to_string_lossy()),
            paths: HashSet::new(),
        };
        let schema = api.request("GET", "/doc", None, &cx.cancelled)?;
        api.paths = schema["paths"]
            .as_object()
            .context("Installed OpenCode does not expose its API schema")?
            .keys()
            .cloned()
            .collect();
        api
    };
    ensure!(
        api.supports("/event")
            && api.supports("/permission/{requestID}/reply")
            && api.supports("/session/{sessionID}/prompt_async"),
        "Installed OpenCode does not advertise the required native interactive protocol"
    );
    // Start event delivery before creating/submitting a session so fast model/tool
    // events cannot race subscription. The SSE connection is bound to this project.
    let event_path = if api.supports("/global/event") {
        "/global/event"
    } else {
        "/event"
    };
    let (mut stream, mut decoder, prior_session) = if let Some(connection) = cached {
        (
            connection.stream,
            connection.decoder,
            Some(connection.session),
        )
    } else {
        (
            api.open("GET", event_path, None, &cx.cancelled)?,
            Sse::default(),
            None,
        )
    };
    ensure!(stream.status == 200, "OpenCode event subscription failed");
    let agent_name = if request.allow_edits { "build" } else { "plan" };
    let agents = api.request("GET", "/agent", None, &cx.cancelled)?;
    let agent = agents
        .as_array()
        .into_iter()
        .flatten()
        .find(|v| v["name"] == agent_name)
        .context("OpenCode does not advertise the selected native agent policy")?;
    let (info, source) = if let Some(id) = &request.resume_id {
        ensure!(valid_id(id), "Invalid OpenCode session ID");
        let info = api.request("GET", &format!("/session/{id}"), None, &cx.cancelled)?;
        ensure!(
            info["id"] == *id,
            "OpenCode did not return the requested session"
        );
        if is_owned_session(&info) {
            (info, None)
        } else {
            ensure!(
                api.supports("/session/{sessionID}/fork"),
                "Installed OpenCode cannot safely continue this external session in Code"
            );
            send(
                &cx.tx,
                RuntimeEvent::Status("Continuing the native session in Code".into()),
                &cx.cancelled,
            )?;
            let fork = api.request(
                "POST",
                &format!("/session/{id}/fork"),
                Some(&json!({})),
                &cx.cancelled,
            )?;
            ensure!(
                fork["id"]
                    .as_str()
                    .is_some_and(|fork_id| valid_id(fork_id) && fork_id != id),
                "OpenCode did not return a separate native fork"
            );
            (fork, Some(id.as_str()))
        }
    } else {
        (
            api.request("POST", "/session", Some(&json!({})), &cx.cancelled)?,
            None,
        )
    };
    let id = info["id"]
        .as_str()
        .context("OpenCode did not return a session ID")?;
    ensure!(
        agent["permission"].is_array(),
        "OpenCode did not expose its native permission rules; no prompt was sent"
    );
    apply_owned_policy(&api, id, &info, agent, request.allow_edits, source, cx)?;
    // Fork copies receive fresh native IDs. Advance beyond the completed fork
    // tick before generating this turn's native ascending user message ID.
    if source.is_some() {
        thread::sleep(Duration::from_millis(1));
    }
    let mut session = Session::new(id.into());
    session.service = Some(service.clone());
    service.set_parent(id);
    if let Some(previous) = prior_session {
        session.children = previous.children;
        session.assistant = previous.child_assistant.clone();
        session.child_assistant = previous.child_assistant;
        session.parts = previous.parts;
        session.child_bytes = previous.child_bytes;
        session.pending = previous
            .pending
            .into_iter()
            .filter(|(_, pending)| pending.session != session.id)
            .collect();
    }
    *active_session = Some((api.clone(), session.id.clone()));
    send(
        &cx.tx,
        RuntimeEvent::SessionId(session.id.clone()),
        &cx.cancelled,
    )?;
    if api.supports("/session/{sessionID}/children") {
        let children = api.request(
            "GET",
            &format!("/session/{}/children", session.id),
            None,
            &cx.cancelled,
        )?;
        session.children.extend(
            children
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| {
                    v["id"]
                        .as_str()
                        .filter(|id| valid_id(id))
                        .map(str::to_owned)
                })
                .take(256),
        );
    }
    let commands = api.request("GET", "/command", None, &cx.cancelled)?;
    {
        let names = commands
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v["name"].as_str().map(str::to_owned))
            .take(256)
            .collect();
        send(&cx.tx, RuntimeEvent::Commands(names), &cx.cancelled)?;
    }
    let mut parts = vec![json!({"type":"text","text":request.prompt})];
    for image in &request.images {
        let path = image.to_str().context("An image path is not UTF-8")?;
        parts.push(json!({"type":"file","mime":"image/png","filename":image.file_name().and_then(|v|v.to_str()).unwrap_or("image.png"),"url":format!("file://{}",encode_file_path(path))}));
    }
    let mut body = json!({"messageID":session.user_message,"agent":if request.allow_edits{"build"}else{"plan"},"parts":parts});
    if let Some(model) = &request.model {
        let (provider, model) = model
            .split_once('/')
            .context("OpenCode model must include its provider")?;
        ensure!(
            !provider.is_empty() && !model.is_empty(),
            "Invalid OpenCode model"
        );
        body["model"] = json!({"providerID":provider,"modelID":model});
    }
    // Drain the initial server/session events before prompt submission. A new
    // session's initial idle must never complete the requested turn prematurely.
    for _ in 0..8 {
        let bytes = stream.poll()?;
        if bytes.is_empty() {
            break;
        }
        decoder.feed(&bytes, &cx.cancelled)?;
        while let Some(event) = decoder.events.pop_front() {
            session.event(&event, parser, cx)?;
        }
    }
    let command_result = if cli_command {
        let command = crate::commands::name(&request.prompt)
            .context("A native OpenCode command is required")?;
        ensure!(
            commands
                .as_array()
                .into_iter()
                .flatten()
                .any(|v| v["name"] == command),
            "Installed OpenCode does not advertise /{command}; no model prompt was sent"
        );
        ensure!(
            api.supports("/session/{sessionID}/command"),
            "Installed OpenCode does not advertise native commands"
        );
        let arguments = request
            .prompt
            .trim()
            .split_once(char::is_whitespace)
            .map(|(_, tail)| tail.trim())
            .unwrap_or("");
        let mut command_body = json!({"messageID":session.user_message,"command":command,"arguments":arguments,"agent":if request.allow_edits{"build"}else{"plan"}});
        if let Some(model) = &request.model {
            command_body["model"] = json!(model);
        }
        let api = api.clone();
        let cancelled = cx.cancelled.clone();
        let id = session.id.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        thread::spawn(move || {
            let result = api.request(
                "POST",
                &format!("/session/{id}/command"),
                Some(&command_body),
                &cancelled,
            );
            let _ = tx.send(result);
        });
        Some(rx)
    } else {
        api.request(
            "POST",
            &format!("/session/{}/prompt_async", session.id),
            Some(&body),
            &cx.cancelled,
        )?;
        None
    };
    send(
        &cx.tx,
        RuntimeEvent::Status("Thinking".into()),
        &cx.cancelled,
    )?;
    loop {
        ensure!(!cx.cancelled.load(Ordering::Acquire), "Stopped");
        session.controls(&api, parser, cx)?;
        process_child_controls(&mut session, &api, service, broker_commands);
        let bytes = stream.poll()?;
        decoder.feed(&bytes, &cx.cancelled)?;
        while let Some(event) = decoder.events.pop_front() {
            session.event(&event, parser, cx)?;
        }
        if let Some(result) = command_result.as_ref().and_then(|rx| rx.try_recv().ok()) {
            let result = result?;
            if result["codeResponseStreamed"] != true {
                // The synchronous native response is authoritative. SSE and HTTP
                // are separate sockets, so the final part can arrive on HTTP first.
                // Reconcile full parts before completing, without losing trailing
                // text or duplicating already-streamed content.
                if let Some(info) = result.get("info") {
                    session.event(&json!({"type":"message.updated","properties":{"sessionID":session.id,"info":info}}),parser,cx)?;
                }
                for part in result["parts"].as_array().into_iter().flatten() {
                    if part["type"] == "text" {
                        if let Some(id) = part["id"].as_str() {
                            parser.begin(id);
                        }
                        if result["codeProjectionShortened"] != true || parser.current_text_len == 0
                        {
                            emit(parser.snapshot(part["text"].as_str().unwrap_or("")), cx)?;
                        }
                    } else {
                        session.event(&json!({"type":"message.part.updated","properties":{"sessionID":session.id,"part":part}}),parser,cx)?;
                    }
                }
                session.complete = true;
            }
            // An oversized REST response is deliberately drained without a
            // second giant JSON allocation. Its full canonical text comes from
            // ordered SSE deltas; wait for native idle, including late deltas.
        }
        if session.complete && command_result.is_none() {
            break;
        }
        if session.complete && cli_command {
            // Completion is authoritative only after the command's HTTP request
            // returns; permissions may still be waiting behind an idle event.
            if command_result.as_ref().is_some_and(|rx| {
                matches!(
                    rx.try_recv(),
                    Err(std::sync::mpsc::TryRecvError::Disconnected)
                )
            }) {
                break;
            }
        }
        ensure!(
            !stream.done,
            "OpenCode event stream disconnected before completion"
        );
        if super::child_exit_pending(child.id() as i32)? {
            bail!("OpenCode server exited before completing this turn");
        }
        if bytes.is_empty() {
            thread::sleep(Duration::from_millis(12));
        }
    }
    for (id, pending) in &session.pending {
        if pending.session == session.id {
            send(
                &cx.tx,
                RuntimeEvent::RequestResolved(id.clone()),
                &cx.cancelled,
            )?;
        }
    }
    if let Some(error) = &session.error {
        bail!("{error}");
    }
    if let Some(error) = parser.storage_error.take() {
        bail!("Could not save OpenCode output: {error}");
    }
    Ok(Connection {
        api,
        session,
        stream,
        decoder,
    })
}
fn encode_file_path(path: &str) -> String {
    path.split('/')
        .map(encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(source: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "code-opencode-http-fixture-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let executable = root.join("opencode-fixture");
        std::fs::write(&executable, format!("#!/usr/bin/python3\n{source}")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        (root, executable)
    }
    const NATIVE_FIXTURE: &str = r#"
import os,sys,json,base64,queue,threading,time
from http.server import ThreadingHTTPServer,BaseHTTPRequestHandler
assert sys.argv[1:]==['serve','--hostname','127.0.0.1','--port','0']
events=queue.Queue()
command_done=threading.Event()
stop_done=threading.Event()
session_info={'id':'ses_fixture','permission':[],'metadata':{}}
source_info={'id':'ses_source','permission':[{'permission':'read','pattern':'*.env','action':'deny'}],'metadata':{'user-setting':'untouched'}}
def event(kind,p):events.put({'type':kind,'properties':p})
class Handler(BaseHTTPRequestHandler):
 protocol_version='HTTP/1.1'
 def log_message(self,*a): pass
 def checked(self):
  expected='Basic '+base64.b64encode(('code:'+os.environ['OPENCODE_SERVER_PASSWORD']).encode()).decode()
  assert self.headers.get('Authorization')==expected
 def sendjson(self,v,status=200):
  b=json.dumps(v).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(b)));self.end_headers();self.wfile.write(b)
 def do_GET(self):
  self.checked();path=self.path.split('?')[0]
  if path=='/doc':return self.sendjson({'paths':{p:{} for p in ['/event','/global/event','/permission/{requestID}/reply','/question/{requestID}/reply','/session/{sessionID}/prompt_async','/session/{sessionID}/command','/session/{sessionID}/fork']}})
  if path=='/command':return self.sendjson([{'name':'fixture-command'},{'name':'huge-command'},{'name':'stop-command'}])
  if path=='/agent':return self.sendjson([{'name':'build','permission':[{'permission':'read','pattern':'*.env','action':'deny'}]}])
  if path=='/session/ses_fixture':return self.sendjson(session_info)
  if path=='/session/ses_source':return self.sendjson(source_info)
  if path in ['/event','/global/event']:
   self.send_response(200);self.send_header('Content-Type','text/event-stream');self.send_header('Transfer-Encoding','chunked');self.end_headers()
   try:
    while True:
     try:payload=('data: '+json.dumps({'directory':'/native-other-worktree','payload':events.get(timeout=.2)})+'\n\n').encode()
     except queue.Empty:payload=b': heartbeat\n\n'
     step=13 if len(payload)<2048 else 4096
     for at in range(0,len(payload),step):
      chunk=payload[at:at+step];self.wfile.write(('%x\r\n'%len(chunk)).encode()+chunk+b'\r\n');self.wfile.flush()
   except (BrokenPipeError,ConnectionResetError):pass
  else:self.sendjson({'message':'unknown fixture route'},404)
 def do_POST(self):
  self.checked();path=self.path.split('?')[0];body=json.loads(self.rfile.read(int(self.headers.get('Content-Length',0))) or b'{}')
  if path=='/session':
   assert body=={};return self.sendjson(session_info)
  if path=='/session/ses_source/fork':
   session_info['permission']=source_info['permission'].copy()
   event('message.updated',{'sessionID':'ses_fixture','info':{'id':'msg_copied','role':'assistant','sessionID':'ses_fixture','parentID':'msg_source_user'}})
   event('message.part.updated',{'sessionID':'ses_fixture','part':{'type':'text','id':'prt_copied','messageID':'msg_copied','sessionID':'ses_fixture','text':'OLD_FORK_REPLAY'}})
   return self.sendjson(session_info)
  if path=='/session/ses_fixture/command':
   user=body.pop('messageID');assert user.startswith('msg_') and len(user)==30
   if body['command']=='stop-command':
    event('session.status',{'sessionID':'ses_fixture','status':{'type':'busy'}})
    event('message.updated',{'sessionID':'ses_fixture','info':{'id':'msg_stop','role':'assistant','sessionID':'ses_fixture','parentID':user}})
    event('message.part.updated',{'sessionID':'ses_fixture','part':{'type':'text','id':'prt_stop','messageID':'msg_stop','sessionID':'ses_fixture','text':'RUNNING'}})
    assert stop_done.wait(10)
    try:self.sendjson({'parts':[]})
    except (BrokenPipeError,ConnectionResetError):pass
    return
   if body['command']=='huge-command':
    assert body['arguments']==''
    text='x'*(2*1024*1024+333)+'LATE_END'
    event('session.status',{'sessionID':'ses_fixture','status':{'type':'busy'}})
    event('message.updated',{'sessionID':'ses_fixture','info':{'id':'msg_huge','role':'assistant','sessionID':'ses_fixture','parentID':user}})
    event('message.part.updated',{'sessionID':'ses_fixture','part':{'type':'text','id':'prt_huge','messageID':'msg_huge','sessionID':'ses_fixture','text':''}})
    for at in range(0,1024*1024,8192):event('message.part.delta',{'sessionID':'ses_fixture','messageID':'msg_huge','partID':'prt_huge','field':'text','delta':text[at:at+8192]})
    self.sendjson({'parts':[{'type':'text','id':'prt_huge','text':text}]})
    def late():
     time.sleep(.15)
     for at in range(1024*1024,len(text),8192):event('message.part.delta',{'sessionID':'ses_fixture','messageID':'msg_huge','partID':'prt_huge','field':'text','delta':text[at:at+8192]})
     event('session.status',{'sessionID':'ses_fixture','status':{'type':'idle'}})
    threading.Thread(target=late,daemon=True).start();return
   assert body=={'command':'fixture-command','arguments':'arg one','agent':'build'}
   event('session.status',{'sessionID':'ses_fixture','status':{'type':'busy'}})
   event('message.updated',{'sessionID':'ses_fixture','info':{'id':'msg_command','role':'assistant','sessionID':'ses_fixture','parentID':user}})
   event('message.part.updated',{'sessionID':'ses_fixture','part':{'type':'text','id':'prt_command','messageID':'msg_command','sessionID':'ses_fixture','text':'Command started. '}})
   event('permission.asked',{'id':'per_command','sessionID':'ses_fixture','permission':'bash','patterns':['pwd'],'metadata':{'command':'pwd'},'always':[]})
   assert command_done.wait(10)
   return self.sendjson({'parts':[{'type':'text','text':'Command started. Command completed.'}]})
  if path=='/session/ses_fixture/abort':
   open('abort-received','w').write('native abort received before server termination')
   stop_done.set();return self.sendjson(True)
  if path=='/permission/per_command/reply':
   assert body=={'reply':'once'};self.sendjson(True)
   event('message.part.delta',{'sessionID':'ses_fixture','messageID':'msg_command','partID':'prt_command','field':'text','delta':'Command completed.'})
   event('session.status',{'sessionID':'ses_fixture','status':{'type':'idle'}});command_done.set();return
  if path=='/session/ses_fixture/prompt_async':
   user=body['messageID'];assert user.startswith('msg_') and len(user)==30
   assert body['parts'][0]['text']=='test native interactions';self.send_response(204);self.send_header('Content-Length','0');self.end_headers()
   event('session.status',{'sessionID':'ses_fixture','status':{'type':'busy'}})
   event('message.updated',{'sessionID':'ses_fixture','info':{'id':'msg_parent','role':'assistant','sessionID':'ses_fixture','parentID':user}})
   event('message.part.updated',{'sessionID':'ses_fixture','part':{'type':'text','id':'prt_parent','messageID':'msg_parent','sessionID':'ses_fixture','text':'Before approval. '}})
   event('session.created',{'sessionID':'ses_child','info':{'id':'ses_child','parentID':'ses_fixture','title':'Inspect files'}})
   event('message.updated',{'sessionID':'ses_child','info':{'id':'msg_child','role':'assistant','sessionID':'ses_child'}})
   event('message.part.updated',{'sessionID':'ses_child','part':{'type':'text','id':'prt_child','messageID':'msg_child','sessionID':'ses_child','text':''}})
   event('message.part.delta',{'sessionID':'ses_child','messageID':'msg_child','partID':'prt_child','field':'text','delta':'Child inspected the files.'})
   event('message.part.updated',{'sessionID':'ses_fixture','part':{'type':'reasoning','id':'prt_thought','messageID':'msg_parent','sessionID':'ses_fixture','text':''}})
   event('message.part.delta',{'sessionID':'ses_fixture','messageID':'msg_parent','partID':'prt_thought','field':'text','delta':'PRIVATE_THOUGHT_NOT_ANSWER'})
   event('permission.asked',{'id':'per_fixture','sessionID':'ses_fixture','permission':'bash','patterns':['pwd'],'metadata':{'command':'pwd'},'always':[]});return
  if path=='/permission/per_fixture/reply':
   assert body=={'reply':'once'};self.sendjson(True)
   event('question.asked',{'id':'que_fixture','sessionID':'ses_fixture','questions':[{'header':'Choice','question':'Choose one','options':[{'label':'First','description':'First choice'},{'label':'Second','description':'Second choice'}]},{'header':'Text','question':'Any notes?','options':[]}]});return
  if path=='/question/que_fixture/reply':
   assert body=={'answers':[['Second'],['a custom response']]};self.sendjson(True)
   event('message.part.delta',{'sessionID':'ses_fixture','messageID':'msg_parent','partID':'prt_parent','field':'text','delta':'After approval.'})
   event('session.status',{'sessionID':'ses_child','status':{'type':'idle'}})
   event('session.status',{'sessionID':'ses_fixture','status':{'type':'idle'}});return
  self.sendjson({'message':'unexpected route'},400)
 def do_PATCH(self):
  self.checked();assert self.path.split('?')[0]=='/session/ses_fixture','Shared source permissions must never be modified'
  body=json.loads(self.rfile.read(int(self.headers.get('Content-Length',0))))
  session_info.update(body);self.sendjson(session_info)
server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
print('opencode server listening on http://127.0.0.1:'+str(server.server_port),flush=True)
server.serve_forever()
"#;
    #[test]
    fn owned_server_reuses_sse_and_background_children_keep_scoped_approvals_and_stop() {
        let source=NATIVE_FIXTURE.replace("   event('session.status',{'sessionID':'ses_child','status':{'type':'idle'}})","   event('permission.asked',{'id':'per_child','sessionID':'ses_child','permission':'bash','patterns':['pwd'],'metadata':{'command':'pwd'},'always':[]})")
            .replace("  if path=='/permission/per_fixture/reply':",r#"  if path=='/permission/per_child/reply':
   assert body=={'reply':'once'};self.sendjson(True)
   event('message.part.delta',{'sessionID':'ses_child','messageID':'msg_child','partID':'prt_child','field':'text','delta':' After background approval.'});return
  if path=='/session/ses_child/abort':
   open('child-abort-received','w').write('native child abort')
   self.sendjson(True)
   event('session.error',{'sessionID':'ses_child','error':{'name':'MessageAbortedError','data':{'message':'Child stopped'}}})
   event('session.status',{'sessionID':'ses_child','status':{'type':'idle'}});return
  if path=='/permission/per_fixture/reply':"#);
        let (root, executable) = fixture(&source);
        let mut request = RunRequest {
            agent: crate::discovery::AgentKind::OpenCode,
            executable,
            project: root.clone(),
            prompt: "test native interactions".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        let first = launch(request.clone()).unwrap();
        let pid = first.process_group.lock().unwrap().unwrap();
        loop {
            match first.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                RuntimeEvent::Request(request) => match &request.kind {
                    NativeRequestKind::Approval { .. } => first
                        .respond(&request.id, NativeAnswer::Choice("once".into()))
                        .unwrap(),
                    NativeRequestKind::Questions { questions } => first
                        .respond(
                            &request.id,
                            NativeAnswer::Answers(
                                [
                                    (questions[0].id.clone(), vec!["Second".into()]),
                                    (questions[1].id.clone(), vec!["a custom response".into()]),
                                ]
                                .into_iter()
                                .collect(),
                            ),
                        )
                        .unwrap(),
                    _ => panic!("Unexpected root request"),
                },
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
        assert!(!first.settled.is_closed());
        let deadline = Instant::now() + Duration::from_secs(5);
        let token = loop {
            if let Some(request) = broker::children(&root)
                .iter()
                .find(|child| child.id == "ses_child")
                .and_then(|child| child.requests.first())
                .cloned()
            {
                break request.id;
            }
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        };
        broker::control_child(
            &root,
            crate::discovery::AgentKind::OpenCode,
            "ses_child",
            broker::ChildAction::Respond {
                id: token.clone(),
                answer: NativeAnswer::Choice("invalid".into()),
            },
        )
        .unwrap();
        while broker::take_child_request_error(
            &root,
            crate::discovery::AgentKind::OpenCode,
            "ses_child",
            &token,
        )
        .is_none()
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(broker::has_child_request(
            &root,
            crate::discovery::AgentKind::OpenCode,
            "ses_child",
            &token
        ));
        broker::control_child(
            &root,
            crate::discovery::AgentKind::OpenCode,
            "ses_child",
            broker::ChildAction::Respond {
                id: token.clone(),
                answer: NativeAnswer::Choice("once".into()),
            },
        )
        .unwrap();
        while !broker::children(&root)
            .iter()
            .any(|child| child.preview.contains("After background approval"))
        {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(!first.settled.is_closed());
        assert!(
            broker::control_child(
                &root,
                crate::discovery::AgentKind::OpenCode,
                "ses_child",
                broker::ChildAction::Steer("No fake steering".into())
            )
            .is_err()
        );
        broker::control_child(
            &root,
            crate::discovery::AgentKind::OpenCode,
            "ses_child",
            broker::ChildAction::Stop,
        )
        .unwrap();
        while !first.settled.is_closed() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert!(root.join("child-abort-received").exists());
        assert_eq!(
            broker::children(&root)
                .iter()
                .find(|child| child.id == "ses_child")
                .unwrap()
                .status,
            InteractionStatus::Cancelled
        );
        drop(first);
        request.resume_id = Some("ses_fixture".into());
        request.prompt = "/fixture-command arg one".into();
        let second = launch_command(request).unwrap();
        assert_eq!(*second.process_group.lock().unwrap(), Some(pid));
        loop {
            match second.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                RuntimeEvent::Request(request) => second
                    .respond(&request.id, NativeAnswer::Choice("once".into()))
                    .unwrap(),
                RuntimeEvent::Finished => break,
                RuntimeEvent::Failed(error) => panic!("{error}"),
                _ => {}
            }
        }
        let done =
            broker::shutdown_service(&root, crate::discovery::AgentKind::OpenCode, "ses_fixture");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done.is_closed() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert!(broker::children(&root).iter().all(|child| !child.connected));
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_server_routes_live_permissions_questions_and_child_progress() {
        let (root, executable) = fixture(NATIVE_FIXTURE);
        let request = RunRequest {
            agent: crate::discovery::AgentKind::OpenCode,
            executable,
            project: root.clone(),
            prompt: "test native interactions".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        let handle = launch(request).unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        let mut permission = false;
        let mut question = false;
        let mut resolved = 0;
        loop {
            assert!(Instant::now() < deadline, "OpenCode fixture timed out");
            match handle.events.recv_timeout(Duration::from_secs(5)).unwrap() {
                RuntimeEvent::Request(request) => match &request.kind {
                    NativeRequestKind::Approval { .. } => {
                        permission = true;
                        assert!(request.detail.contains("pwd"));
                        handle
                            .respond(&request.id, NativeAnswer::Choice("once".into()))
                            .unwrap();
                    }
                    NativeRequestKind::Questions { questions } => {
                        question = true;
                        assert_eq!(questions.len(), 2);
                        let answers = [
                            (questions[0].id.clone(), vec!["Second".into()]),
                            (questions[1].id.clone(), vec!["a custom response".into()]),
                        ]
                        .into_iter()
                        .collect();
                        handle
                            .respond(&request.id, NativeAnswer::Answers(answers))
                            .unwrap();
                    }
                    _ => panic!("unexpected native fixture request"),
                },
                RuntimeEvent::RequestResolved(_) => resolved += 1,
                RuntimeEvent::Failed(error) => panic!("OpenCode fixture: {error}"),
                RuntimeEvent::Finished => break,
                _ => {}
            }
        }
        assert!(permission && question);
        assert_eq!(resolved, 2);
        let source = handle.take_response_source().unwrap();
        let output = source.read_range(0..source.len()).unwrap();
        assert!(output.contains("Before approval.") && output.contains("After approval."));
        let child_page = broker::child_transcript_page(
            &root,
            crate::discovery::AgentKind::OpenCode,
            "ses_child",
            0,
        )
        .unwrap();
        assert_eq!(child_page.text, "Child inspected the files.");
        assert!(!output.contains("Child inspected the files."));
        assert!(output.contains("ses_child"));
        assert!(!output.contains("PRIVATE_THOUGHT_NOT_ANSWER"));
        assert!(handle.settled.recv_blocking().is_err());
        drop(handle);
        let done =
            broker::shutdown_service(&root, crate::discovery::AgentKind::OpenCode, "ses_fixture");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done.is_closed() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_commands_use_the_command_endpoint_and_keep_approvals_live() {
        let (root, executable) = fixture(NATIVE_FIXTURE);
        let request = RunRequest {
            agent: crate::discovery::AgentKind::OpenCode,
            executable,
            project: root.clone(),
            prompt: "/fixture-command arg one".into(),
            resume_id: None,
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        let handle = launch_command(request).unwrap();
        let mut approved = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                RuntimeEvent::Request(request) => {
                    assert_eq!(request.method, "permission.asked");
                    handle
                        .respond(&request.id, NativeAnswer::Choice("once".into()))
                        .unwrap();
                    approved = true;
                }
                RuntimeEvent::Failed(error) => panic!("Native command fixture failed: {error}"),
                RuntimeEvent::Finished => break,
                _ => {}
            }
        }
        assert!(approved);
        let source = handle.take_response_source().unwrap();
        let text = source.read_range(0..source.len()).unwrap();
        assert!(text.contains("Command started.") && text.contains("Command completed."));
        assert!(handle.settled.recv_blocking().is_err());
        drop(handle);
        let done =
            broker::shutdown_service(&root, crate::discovery::AgentKind::OpenCode, "ses_fixture");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done.is_closed() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn dropping_the_run_sends_native_abort_before_reaping_the_server() {
        let (root, executable) = fixture(NATIVE_FIXTURE);
        let request = RunRequest {
            agent: crate::discovery::AgentKind::OpenCode,
            executable,
            project: root.clone(),
            prompt: "/stop-command".into(),
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
                RuntimeEvent::Text(text) if text.contains("RUNNING") => break,
                RuntimeEvent::Failed(error) => panic!("Native abort fixture failed: {error}"),
                _ => {}
            }
        }
        let settled = handle.settled.clone();
        let start = Instant::now();
        drop(handle);
        assert!(settled.recv_blocking().is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        assert!(root.join("abort-received").is_file());
        let done =
            broker::shutdown_service(&root, crate::discovery::AgentKind::OpenCode, "ses_fixture");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done.is_closed() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn oversized_command_rest_response_waits_for_every_late_sse_delta() {
        let (root, executable) = fixture(NATIVE_FIXTURE);
        let request = RunRequest {
            agent: crate::discovery::AgentKind::OpenCode,
            executable,
            project: root.clone(),
            prompt: "/huge-command".into(),
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
            match handle.events.recv_timeout(Duration::from_secs(20)).unwrap() {
                RuntimeEvent::Failed(error) => panic!("Large native command failed: {error}"),
                RuntimeEvent::Finished => break,
                _ => {}
            }
        }
        let source = handle.take_response_source().unwrap();
        assert_eq!(source.len(), 2 * 1024 * 1024 + 333 + "LATE_END".len());
        assert!(source.tail(8).unwrap().ends_with("LATE_END"));
        assert!(handle.settled.recv_blocking().is_err());
        drop(handle);
        let done =
            broker::shutdown_service(&root, crate::discovery::AgentKind::OpenCode, "ses_fixture");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done.is_closed() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn external_resume_forks_before_any_permission_changes() {
        let (root, executable) = fixture(NATIVE_FIXTURE);
        let request = RunRequest {
            agent: crate::discovery::AgentKind::OpenCode,
            executable,
            project: root.clone(),
            prompt: "/fixture-command arg one".into(),
            resume_id: Some("ses_source".into()),
            fork_session: false,
            model: None,
            images: vec![],
            allow_edits: true,
            codex_approvals: crate::runtime::CodexApprovals::default(),
            trust_workspace: false,
        };
        let handle = launch_command(request).unwrap();
        let mut forked = false;
        loop {
            match handle.events.recv_timeout(Duration::from_secs(10)).unwrap() {
                RuntimeEvent::SessionId(id) => {
                    assert_eq!(id, "ses_fixture");
                    forked = true;
                }
                RuntimeEvent::Request(request) => {
                    handle
                        .respond(&request.id, NativeAnswer::Choice("once".into()))
                        .unwrap();
                }
                RuntimeEvent::Failed(error) => panic!("Native fork fixture failed: {error}"),
                RuntimeEvent::Finished => break,
                _ => {}
            }
        }
        assert!(forked);
        let source = handle.take_response_source().unwrap();
        let rendered = source.read_range(0..source.len()).unwrap();
        assert!(!rendered.contains("OLD_FORK_REPLAY"));
        assert_eq!(rendered.matches("Command completed.").count(), 1);
        assert!(handle.settled.recv_blocking().is_err());
        drop(handle);
        let done =
            broker::shutdown_service(&root, crate::discovery::AgentKind::OpenCode, "ses_fixture");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done.is_closed() {
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(5));
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn native_permission_order_and_read_only_denials_are_preserved() {
        let source = json!([{"permission":"read","pattern":"*.env","action":"deny"},{"permission":"read","pattern":"example.env","action":"allow"}]);
        let strict = stricter_policy(&source, &json!([]), false);
        let rules = strict.as_array().unwrap();
        assert_eq!(rules[1]["action"], "deny");
        assert_eq!(rules[2]["action"], "ask");
        assert_eq!(
            rules[3],
            json!({"permission":"edit","pattern":"*","action":"deny"})
        );
        assert_eq!(
            rules[4],
            json!({"permission":"bash","pattern":"*","action":"deny"})
        );
        assert_eq!(source[1]["action"], "allow");
    }
    #[test]
    fn approval_preview_never_hides_an_approvable_command_suffix() {
        let command = format!("{} ; rm -rf hidden-target", "echo harmless ".repeat(6000));
        assert!(
            approval_details(
                &json!({"permission":"bash","patterns":["*"],"metadata":{"command":command}})
            )
            .is_none()
        );
        let detail=approval_details(&json!({"permission":"bash","patterns":["pwd"],"metadata":{"command":"pwd","futureSecurityField":"preserved"}})).unwrap();
        assert!(detail.contains("futureSecurityField") && detail.contains("preserved"));
    }
    #[test]
    fn native_compaction_continuations_keep_precise_current_turn_lineage() {
        let (handle, cx) = native_handle(0);
        *cx.group.lock().unwrap() = None;
        let mut session = Session::new("ses_lineage".into());
        let root = session.user_message.clone();
        let mut parser = StreamParser::default();
        let mut feed = |kind: &str, p: Value| {
            session
                .event(&json!({"type":kind,"properties":p}), &mut parser, &cx)
                .unwrap();
        };
        // Copied history is rejected, including historical compaction markers.
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_000000000001old","role":"user"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_old_compact","messageID":"msg_000000000001old","type":"compaction","auto":true}}),
        );
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":root,"role":"user"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_root","messageID":root,"type":"text","text":"Current objective"}}),
        );
        // An actual auto-compaction introduces a native new parent user ID.
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_fffffffffff1compact","role":"user"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_compact","messageID":"msg_fffffffffff1compact","type":"compaction","auto":true}}),
        );
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_summary","role":"assistant","parentID":"msg_fffffffffff1compact","summary":true}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_summary","messageID":"msg_summary","type":"text","text":"PRIVATE_COMPACTION_SUMMARY"}}),
        );
        // Overflow replay has no synthetic flag: match exact current user parts.
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_fffffffffff2replay","role":"user"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_replay","messageID":"msg_fffffffffff2replay","type":"text","text":"Current objective"}}),
        );
        feed("session.compacted", json!({"sessionID":"ses_lineage"}));
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_after_replay","role":"assistant","parentID":"msg_fffffffffff2replay"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_after_replay","messageID":"msg_after_replay","type":"text","text":"After native overflow replay."}}),
        );
        // Tagged native follow-ups retain their own exact parent relationship.
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_fffffffffff3continue","role":"user"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_continue","messageID":"msg_fffffffffff3continue","type":"text","synthetic":true,"metadata":{"compaction_continue":true},"text":"Native continuation"}}),
        );
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_after_continue","role":"assistant","parentID":"msg_fffffffffff3continue"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_after_continue","messageID":"msg_after_continue","type":"text","text":"After tagged continuation."}}),
        );
        // Late old history cannot become a new current parent after root_seen.
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_000000000001old","role":"user"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_old_continue","messageID":"msg_000000000001old","type":"text","synthetic":true,"metadata":{"compaction_continue":true},"text":"Old continuation"}}),
        );
        feed(
            "message.updated",
            json!({"sessionID":"ses_lineage","info":{"id":"msg_old_reply","role":"assistant","parentID":"msg_000000000001old"}}),
        );
        feed(
            "message.part.updated",
            json!({"sessionID":"ses_lineage","part":{"id":"prt_old_reply","messageID":"msg_old_reply","type":"text","text":"OLD_FORK_REPLAY"}}),
        );
        let output = parser.rendered_output();
        assert_eq!(
            output,
            "After native overflow replay.\n\nAfter tagged continuation."
        );
        assert!(
            !output.contains("PRIVATE_COMPACTION_SUMMARY") && !output.contains("OLD_FORK_REPLAY")
        );
        drop(handle);
    }
    #[test]
    fn client_message_ids_preserve_native_ascending_format() {
        let first = native_message_id();
        let second = native_message_id();
        assert_eq!(first.len(), 30);
        assert!(first.starts_with("msg_"));
        assert!(first[4..16].bytes().all(|b| b.is_ascii_hexdigit()));
        assert_ne!(first, second);
        assert!(first[16..].bytes().all(|b| b.is_ascii_alphanumeric()));
    }
    #[test]
    fn private_address_only() {
        assert_eq!(
            address_from_output("opencode server listening on http://127.0.0.1:4321\n")
                .unwrap()
                .port(),
            4321
        );
        assert!(address_from_output("http://0.0.0.0:1234").is_none());
        assert!(address_from_output("http://127.0.0.1:0").is_none());
    }
    #[test]
    fn sse_arbitrary_chunks_and_multiline() {
        let cancelled = AtomicBool::new(false);
        let mut s = Sse::default();
        let bytes=b": heartbeat\n\ndata: {\"type\":\"session.status\",\ndata: \"properties\":{\"sessionID\":\"ses_test\",\"status\":{\"type\":\"busy\"}}}\n\n";
        for chunk in bytes.chunks(3) {
            s.feed(chunk, &cancelled).unwrap();
        }
        let event = s.events.pop_front().unwrap();
        assert_eq!(event["properties"]["status"]["type"], "busy");
        assert!(s.events.is_empty());
    }
    #[test]
    fn url_components_cannot_escape_project_or_ids() {
        assert_eq!(
            encode_component("/A folder/?x=1"),
            "%2FA%20folder%2F%3Fx%3D1"
        );
        assert!(!valid_id("ses_a/../../other"));
        assert_eq!(
            encode_file_path("/A folder/🙂.png"),
            "/A%20folder/%F0%9F%99%82.png"
        );
    }
}
