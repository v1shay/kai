use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    io::{Read, Write},
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
const LIMIT: u64 = 8 * 1024 * 1024;
pub fn validate_endpoint(value: &str) -> Result<()> {
    ensure!(
        value.len() <= 512
            && !value.chars().any(|c| c.is_control() || c.is_whitespace())
            && !value.contains(['@', '?', '#', '\\']),
        "Invalid inference endpoint"
    );
    let (rest, secure) = if let Some(v) = value.strip_prefix("https://") {
        (v, true)
    } else if let Some(v) = value.strip_prefix("http://") {
        (v, false)
    } else {
        anyhow::bail!("Use an HTTP or HTTPS inference endpoint")
    };
    let authority = rest.split('/').next().unwrap_or("");
    let host = if authority.starts_with('[') {
        authority
            .split(']')
            .next()
            .unwrap_or("")
            .trim_start_matches('[')
    } else {
        authority.split(':').next().unwrap_or("")
    };
    let ip = host.parse::<std::net::IpAddr>().ok();
    let private = ip.is_some_and(|ip| match ip {
        std::net::IpAddr::V4(v) => {
            let b = v.octets();
            v.is_loopback() || v.is_private() || (b[0] == 100 && (64..=127).contains(&b[1]))
        }
        std::net::IpAddr::V6(v) => v.is_loopback() || v.segments()[0] == 0xfd7a,
    });
    ensure!(
        private || host == "localhost" || (secure && host.ends_with(".ts.net")),
        "Ninja inference must use a local, LAN, or authenticated tailnet endpoint"
    );
    ensure!(
        !rest.split('/').any(|part| part == ".."),
        "Invalid endpoint path"
    );
    Ok(())
}
pub fn json(method: &str, url: &str, body: Option<&Value>, timeout: Duration) -> Result<Value> {
    validate_endpoint(url)?;
    let mut command = Command::new("/usr/bin/curl");
    command.args([
        "--noproxy",
        "*",
        "--silent",
        "--show-error",
        "--fail",
        "--connect-timeout",
        "2",
        "--max-time",
        &format!("{}", timeout.as_secs_f32()),
        "--max-filesize",
        &LIMIT.to_string(),
        "-X",
        method,
        "-H",
        "Content-Type: application/json",
    ]);
    if body.is_some() {
        command.args(["--data-binary", "@-"]);
    }
    command.arg(url);
    let bytes = run(
        command,
        body.map(serde_json::to_vec).transpose()?.as_deref(),
        timeout,
    )?;
    serde_json::from_slice(&bytes).context("Inference server returned invalid JSON")
}
pub fn public(url: &str) -> Result<Vec<u8>> {
    ensure!(
        url.starts_with("https://ollama.com/") || url.starts_with("https://registry.ollama.ai/"),
        "Untrusted model registry"
    );
    let mut command = Command::new("/usr/bin/curl");
    if url.contains("/search?") {
        command.args(["-H", "HX-Request: true"]);
    }
    command.args([
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--silent",
        "--show-error",
        "--fail",
        "--location",
        "--connect-timeout",
        "5",
        "--max-time",
        "20",
        "--max-filesize",
        &LIMIT.to_string(),
        url,
    ]);
    run(command, None, Duration::from_secs(21))
}
pub fn command_json(path: &Path, args: &[&str], timeout: Duration) -> Result<Value> {
    let mut c = Command::new(path);
    crate::process_env::apply(&mut c);
    c.args(args);
    serde_json::from_slice(&run(c, None, timeout)?).context("Invalid device inventory")
}
pub(crate) fn run(mut command: Command, input: Option<&[u8]>, timeout: Duration) -> Result<Vec<u8>> {
    let mut child = command
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("Could not start inference connection")?;
    if let Some(input) = input {
        if let Some(mut stdin) = child.stdin.take() {
            if let Err(e) = stdin.write_all(input) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.into());
            }
        }
    }
    let stdout = child.stdout.take().context("Missing connection output")?;
    // Read concurrently so both large inventories and a stalled local CLI are bounded.
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout
            .take(LIMIT + 1)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = tx.send(result);
    });
    let at = Instant::now();
    loop {
        match child.try_wait()? {
            Some(status) => {
                ensure!(
                    status.success(),
                    "The inference endpoint could not be reached or rejected the request"
                );
                let bytes = rx.recv_timeout(Duration::from_secs(1))??;
                ensure!(
                    bytes.len() as u64 <= LIMIT,
                    "Inference metadata exceeds the size limit"
                );
                return Ok(bytes);
            }
            None if at.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!("Inference connection timed out")
            }
            None => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}
