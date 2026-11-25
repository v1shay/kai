//! Opt-in bounded frame-request counters for repeated surface smoke diagnostics.
use std::{cell::RefCell, hash::{Hash, Hasher}, sync::OnceLock};

const MAX_WINDOWS: usize = 32;
const MAX_CATEGORIES: usize = 32;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Category {
    NinjaEmblem, NinjaName, NinjaStatus, SidebarStatus, HistoryLoading,
    FiniteOther, UnknownRepeat,
    DirectAnimation, DirectBackground, DirectLoadingLogo, DirectIndicator,
    DirectNativeRequest, DirectDiff, DirectImage, DirectOther,
}

#[derive(Clone, Copy, Debug)]
struct Counter {
    window: u64,
    category: Category,
    requests: u64,
    accepted: u64,
    last_view: u64,
}

#[derive(Default)]
struct Counters {
    windows: Vec<u64>,
    rows: Vec<Counter>,
    dropped: u64,
    snapshots: u64,
}

impl Counters {
    fn record(&mut self, window: u64, category: Category, view: u64, accepted: bool) {
        if let Some(row) = self.rows.iter_mut().find(|row| row.window == window && row.category == category) {
            row.requests = row.requests.saturating_add(1);
            row.accepted = row.accepted.saturating_add(u64::from(accepted));
            row.last_view = view;
            return;
        }
        if !self.windows.contains(&window) {
            if self.windows.len() == MAX_WINDOWS {
                self.dropped = self.dropped.saturating_add(1);
                return;
            }
            self.windows.push(window);
        }
        if self.rows.iter().filter(|row| row.window == window).count() == MAX_CATEGORIES {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.rows.push(Counter { window, category, requests: 1, accepted: u64::from(accepted), last_view: view });
    }

    fn snapshot(&mut self) -> (u64, Vec<Counter>, u64) {
        self.snapshots = self.snapshots.saturating_add(1);
        (self.snapshots, self.rows.clone(), self.dropped)
    }
}

thread_local! {
    static COUNTERS: RefCell<Counters> = RefCell::new(Counters::default());
}

fn admitted(flag: bool, arguments: impl FnOnce() -> Vec<std::ffi::OsString>) -> bool {
    if !flag { return false; }
    let arguments = arguments();
    arguments.iter().any(|arg| arg == "--unified-smoke-test")
        && arguments.iter().any(|arg| arg == "--unified-surface-probe")
}

pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| admitted(
        std::env::var_os("KAI_DIAGNOSTIC_FRAME_TIMING").as_deref() == Some(std::ffi::OsStr::new("1")),
        || std::env::args_os().collect(),
    ))
}

pub(crate) fn identifier(value: &impl Hash) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn animation_category(name: Option<&str>, repeats: bool) -> Category {
    if !repeats { return Category::FiniteOther; }
    match name {
        Some(name) if name.starts_with("ninja-emblem-") => Category::NinjaEmblem,
        Some(name) if name.starts_with("ninja-name-") => Category::NinjaName,
        Some(name) if name.starts_with("ninja-status-") => Category::NinjaStatus,
        Some(name) if name.starts_with("sidebar-status-") => Category::SidebarStatus,
        Some("history-loading") => Category::HistoryLoading,
        _ => Category::UnknownRepeat,
    }
}

fn direct_category(file: &str) -> Category {
    match file.rsplit('/').next().unwrap_or(file) {
        "animation.rs" => Category::DirectAnimation,
        "background.rs" => Category::DirectBackground,
        "loading_logo.rs" => Category::DirectLoadingLogo,
        "kai_indicator.rs" => Category::DirectIndicator,
        "native_request_ui.rs" => Category::DirectNativeRequest,
        "diff_view.rs" => Category::DirectDiff,
        "img.rs" => Category::DirectImage,
        _ => Category::DirectOther,
    }
}

pub(crate) fn animation(window: u64, view: u64, name: Option<&str>, repeats: bool) {
    COUNTERS.with(|all| all.borrow_mut().record(window, animation_category(name, repeats), view, false));
}

pub(crate) fn direct(window: u64, view: u64, file: &'static str, accepted: bool) {
    COUNTERS.with(|all| all.borrow_mut().record(window, direct_category(file), view, accepted));
}

pub(crate) fn flush(phase: &'static str, cycle: u32) {
    if !enabled() { return; }
    let (snapshot, rows, dropped) = COUNTERS.with(|all| all.borrow_mut().snapshot());
    // The mutable recorder borrow has ended before formatting or stderr I/O.
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "KAI_FRAME_REQUESTS_BEGIN pid={} phase={} cycle={} snapshot={} records={} dropped={} max_windows={} max_categories={}",
        std::process::id(), phase, cycle, snapshot, rows.len(), dropped, MAX_WINDOWS, MAX_CATEGORIES);
    for row in rows {
        let _ = writeln!(stderr, "KAI_FRAME_REQUESTS window_hash={} category={:?} requests={} accepted={} last_view_hash={}",
            row.window, row.category, row.requests, row.accepted, row.last_view);
    }
    let _ = writeln!(stderr, "KAI_FRAME_REQUESTS_END");
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn false_flag_never_reads_arguments() {
        assert!(!admitted(false, || panic!("disabled argument reader")));
    }
    #[test]
    fn exact_surface_admission_only() {
        use std::ffi::OsString;
        assert!(admitted(true, || vec![OsString::from("--unified-smoke-test"), OsString::from("--unified-surface-probe")]));
        assert!(!admitted(true, || vec![OsString::from("--unified-smoke-test"), OsString::from("--unified-ninja-render-probe")]));
        assert!(!admitted(true, || vec![OsString::from("--unified-surface-probe-extra")]));
    }
    #[cfg(unix)]
    #[test]
    fn non_utf8_arguments_do_not_panic_or_change_exact_admission() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};
        assert!(admitted(true, || vec![OsString::from_vec(vec![255]), "--unified-smoke-test".into(), "--unified-surface-probe".into()]));
    }
    #[test]
    fn static_classes_do_not_retain_dynamic_ids() {
        assert_eq!(animation_category(Some("ninja-emblem-private-name"), true), Category::NinjaEmblem);
        assert_eq!(animation_category(Some("ninja-name-fire"), true), Category::NinjaName);
        assert_eq!(animation_category(Some("ninja-status-ice-working"), true), Category::NinjaStatus);
        assert_eq!(animation_category(Some("sidebar-status-thinking"), true), Category::SidebarStatus);
        assert_eq!(animation_category(Some("history-loading"), true), Category::HistoryLoading);
        assert_eq!(animation_category(Some("/private/user/path"), true), Category::UnknownRepeat);
        assert_eq!(animation_category(Some("ninja-name-fire"), false), Category::FiniteOther);
        assert_eq!(direct_category("/private/source/background.rs"), Category::DirectBackground);
    }
    #[test]
    fn per_window_and_category_counts_preserve_dedupe_information() {
        let mut all = Counters::default();
        all.record(1, Category::DirectAnimation, 10, true);
        all.record(1, Category::DirectAnimation, 11, false);
        all.record(2, Category::DirectAnimation, 20, true);
        assert_eq!(all.rows.len(), 2);
        assert_eq!((all.rows[0].requests, all.rows[0].accepted, all.rows[0].last_view), (2, 1, 11));
    }
    #[test]
    fn window_budget_and_repeated_unknowns_stay_bounded() {
        let mut all = Counters::default();
        for window in 0..1000 { all.record(window, Category::UnknownRepeat, window, false); }
        assert_eq!(all.windows.len(), MAX_WINDOWS);
        assert_eq!(all.rows.len(), MAX_WINDOWS);
        assert_eq!(all.dropped, 1000 - MAX_WINDOWS as u64);
        for view in 0..1000 { all.record(0, Category::UnknownRepeat, view, false); }
        assert_eq!(all.rows.len(), MAX_WINDOWS);
    }
    #[test]
    fn snapshots_are_cumulative_and_owned_after_borrow() {
        let mut all = Counters::default();
        all.record(1, Category::NinjaName, 4, false);
        let (first, rows, _) = all.snapshot();
        all.record(1, Category::NinjaName, 4, false);
        let (second, next, _) = all.snapshot();
        assert_eq!((first, second), (1, 2));
        assert_eq!(rows[0].requests, 1);
        assert_eq!(next[0].requests, 2);
    }
}
