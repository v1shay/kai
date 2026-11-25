//! Opt-in monotonic startup diagnostics. No background sampler or idle timer.
use std::{sync::OnceLock, time::{Duration,Instant,SystemTime,UNIX_EPOCH}};
static START: OnceLock<Instant> = OnceLock::new();
static PRE_MAIN:OnceLock<Duration>=OnceLock::new();
pub fn start() {
    START.get_or_init(Instant::now);
    PRE_MAIN.get_or_init(||std::env::var("KAI_LAUNCH_ORIGIN_UNIX_NS").ok().and_then(|s|s.parse::<u64>().ok()).and_then(|origin|{
        let delta=SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_nanos().checked_sub(origin as u128)?;
        (delta<60_000_000_000).then(||Duration::from_nanos(delta as u64))
    }).unwrap_or_default());
}
/// Minimal measurement is confined to the existing isolated startup probe.
fn minimal_enabled() -> bool {
    if std::env::var_os("KAI_STARTUP_METRICS").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return false;
    }
    let mut smoke = false;
    let mut startup = false;
    for argument in std::env::args_os() {
        smoke |= argument == "--unified-smoke-test";
        startup |= argument == "--unified-startup-probe";
    }
    smoke && startup
}

pub fn startup_frame_metric_enabled() -> bool {
    std::env::var_os("KAI_PERFORMANCE_LOG").is_some() || minimal_enabled()
}

fn elapsed_milliseconds() -> f64 {
    (START.get_or_init(Instant::now).elapsed()
        + PRE_MAIN.get().copied().unwrap_or_default()).as_secs_f64() * 1000.
}

pub fn mark(name: &str) {
    if std::env::var_os("KAI_PERFORMANCE_LOG").is_some()
        || (name == "main-entered" && minimal_enabled())
    {
        eprintln!("KAI_LAUNCH {} {:.3}ms", name, elapsed_milliseconds());
    }
}

/// Called by the existing post-Submitted frame callback, after the private
/// initial native visibility acknowledgment has been accepted.
pub fn mark_initial_workspace_frame(native_visible_ack: bool) {
    if !native_visible_ack {
        return;
    }
    if std::env::var_os("KAI_PERFORMANCE_LOG").is_some() {
        mark("first-workspace-frame");
    } else if minimal_enabled() {
        eprintln!("KAI_LAUNCH first-workspace-frame {:.3}ms native_visible_ack=1", elapsed_milliseconds());
    }
}
