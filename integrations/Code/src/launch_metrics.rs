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
pub fn mark(name: &str) {
    if std::env::var_os("KAI_PERFORMANCE_LOG").is_some() {
        eprintln!(
            "KAI_LAUNCH {} {:.3}ms",
            name,
            (START.get_or_init(Instant::now).elapsed()+PRE_MAIN.get().copied().unwrap_or_default()).as_secs_f64() * 1000.
        );
    }
}
