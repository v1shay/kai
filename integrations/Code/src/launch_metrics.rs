//! Opt-in monotonic startup diagnostics. No background sampler or idle timer.
use std::{sync::OnceLock, time::Instant};
static START: OnceLock<Instant> = OnceLock::new();
pub fn start() {
    START.get_or_init(Instant::now);
}
pub fn mark(name: &str) {
    if std::env::var_os("KAI_PERFORMANCE_LOG").is_some() {
        eprintln!(
            "KAI_LAUNCH {} {:.3}ms",
            name,
            START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.
        );
    }
}
