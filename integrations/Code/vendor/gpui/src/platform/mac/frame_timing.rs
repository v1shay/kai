//! Bounded main-thread diagnostics for the explicitly opted-in Ninja smoke probe.
use std::{
    cell::RefCell,
    ffi::{OsStr, OsString},
    sync::OnceLock,
};

const CAPACITY: usize = 32768;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Scope {
    PrepareFrame,
    WakeFrameClock,
    ParkFrameClock,
    StopDisplayLink,
    DisplayLinkStart,
    DisplayLinkStop,
    CvStart,
    CvStop,
    MainQueue,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Event {
    Begin,
    End,
    StateLocked,
    CocoaQueriesDone,
    DrawableSizeUpdated,
    RendererPrepareBegin,
    RendererReadiness,
    RunningState,
    VisibleState,
    LinkPresent,
    NoOpRunning,
    NoOpHidden,
    NoOpMissingLink,
    CallReturned,
    Handled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Record {
    sequence: u64,
    scope: Scope,
    event: Event,
    object: usize,
    time: f64,
    value: i64,
}

struct Buffer {
    records: Vec<Record>,
    dropped: usize,
    next_sequence: u64,
    open_spans: usize,
}

impl Buffer {
    fn new() -> Self {
        Self {
            records: Vec::with_capacity(CAPACITY),
            dropped: 0,
            next_sequence: 1,
            open_spans: 0,
        }
    }

    fn push(&mut self, record: Record) {
        if self.records.len() < CAPACITY {
            self.records.push(record);
        } else {
            self.dropped = self.dropped.saturating_add(1);
        }
    }

    fn begin(&mut self, scope: Scope, object: usize, time: f64) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.open_spans = self.open_spans.saturating_add(1);
        self.push(Record {
            sequence,
            scope,
            event: Event::Begin,
            object,
            time,
            value: 0,
        });
        sequence
    }

    fn instant(&mut self, scope: Scope, event: Event, object: usize, time: f64, value: i64) {
        let sequence = self.next_sequence;
        self.next_sequence = self.next_sequence.saturating_add(1);
        self.push(Record {
            sequence,
            scope,
            event,
            object,
            time,
            value,
        });
    }

    fn finish(&mut self, sequence: u64, scope: Scope, object: usize, time: f64) {
        self.open_spans = self.open_spans.saturating_sub(1);
        self.push(Record {
            sequence,
            scope,
            event: Event::End,
            object,
            time,
            value: 0,
        });
    }
}

#[derive(Default)]
struct Recorder {
    buffer: Option<Buffer>,
    flushed: bool,
}

impl Recorder {
    fn begin(&mut self, scope: Scope, object: usize, time: f64) -> Option<u64> {
        if self.flushed {
            return None;
        }
        Some(
            self.buffer
                .get_or_insert_with(Buffer::new)
                .begin(scope, object, time),
        )
    }

    fn mark(&mut self, record: Record) {
        if !self.flushed {
            if let Some(buffer) = &mut self.buffer {
                buffer.push(record);
            }
        }
    }

    fn finish(&mut self, sequence: u64, scope: Scope, object: usize, time: f64) {
        if !self.flushed {
            if let Some(buffer) = &mut self.buffer {
                buffer.finish(sequence, scope, object, time);
            }
        }
    }

    fn handled(&mut self, sequence: u32, time: f64) {
        if !self.flushed {
            self.buffer.get_or_insert_with(Buffer::new).instant(
                Scope::MainQueue,
                Event::Handled,
                0,
                time,
                i64::from(sequence),
            );
        }
    }

    fn take_once(&mut self) -> Option<Buffer> {
        if self.flushed {
            return None;
        }
        self.flushed = true;
        Some(self.buffer.take().unwrap_or_else(Buffer::new))
    }
}

thread_local! {
    static RECORDER: RefCell<Recorder> = RefCell::new(Recorder::default());
}

fn admitted(flag: bool, arguments: impl IntoIterator<Item = OsString>) -> bool {
    if !flag {
        return false;
    }
    let mut smoke = false;
    let mut ninja = false;
    for argument in arguments {
        smoke |= argument.as_os_str() == OsStr::new("--unified-smoke-test");
        ninja |= argument.as_os_str() == OsStr::new("--unified-ninja-render-probe");
    }
    smoke && ninja
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        if std::env::var_os("KAI_DIAGNOSTIC_FRAME_TIMING").as_deref() != Some(OsStr::new("1")) {
            return false;
        }
        admitted(true, std::env::args_os())
    })
}

#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {
    fn CACurrentMediaTime() -> f64;
}
unsafe extern "C" {
    fn pthread_main_np() -> std::ffi::c_int;
}

fn main_thread() -> bool {
    unsafe { pthread_main_np() != 0 }
}

fn now() -> f64 {
    unsafe { CACurrentMediaTime() }
}

pub(super) struct Span {
    identity: Option<(u64, Scope, usize)>,
}

impl Span {
    pub(super) fn begin(scope: Scope, object: usize) -> Self {
        if !enabled() || !main_thread() {
            return Self { identity: None };
        }
        // No RefCell borrow spans a clock query or any caller's native operation.
        let time = now();
        let sequence = RECORDER.with(|cell| cell.borrow_mut().begin(scope, object, time));
        Self {
            identity: sequence.map(|sequence| (sequence, scope, object)),
        }
    }

    pub(super) fn mark(&self, event: Event, value: i64) {
        let Some((sequence, scope, object)) = self.identity else {
            return;
        };
        let time = now();
        RECORDER.with(|cell| {
            cell.borrow_mut().mark(Record {
                sequence,
                scope,
                event,
                object,
                time,
                value,
            })
        });
    }
}

impl Drop for Span {
    fn drop(&mut self) {
        if let Some((sequence, scope, object)) = self.identity {
            let time = now();
            RECORDER.with(|cell| cell.borrow_mut().finish(sequence, scope, object, time));
        }
    }
}

pub(super) fn main_queue_handled(sequence: u32) {
    if !enabled() || !main_thread() {
        return;
    }
    let time = now();
    RECORDER.with(|cell| cell.borrow_mut().handled(sequence, time));
}

pub(super) fn flush() {
    if !enabled() || !main_thread() {
        return;
    }
    let dump = RECORDER.with(|cell| cell.borrow_mut().take_once());
    let Some(buffer) = dump else { return };
    // Formatting and I/O begin only after releasing the thread-local borrow.
    eprintln!(
        "KAI_FRAME_TIMING_BEGIN pid={} clock=CACurrentMediaTime_seconds_since_boot records={} capacity={} dropped={} open_spans={}",
        std::process::id(),
        buffer.records.len(),
        CAPACITY,
        buffer.dropped,
        buffer.open_spans
    );
    for record in buffer.records {
        eprintln!(
            "KAI_FRAME_TIMING sequence={} scope={:?} event={:?} object={} media_time={:.9} value={}",
            record.sequence, record.scope, record.event, record.object, record.time, record.value
        );
    }
    eprintln!("KAI_FRAME_TIMING_END");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opt_in_requires_all_three_conditions() {
        for bits in 0..8 {
            let mut arguments = Vec::new();
            if bits & 2 != 0 {
                arguments.push(OsString::from("--unified-smoke-test"));
            }
            if bits & 4 != 0 {
                arguments.push(OsString::from("--unified-ninja-render-probe"));
            }
            assert_eq!(admitted(bits & 1 != 0, arguments), bits == 7);
        }
    }

    #[test]
    fn false_flag_does_not_consume_arguments() {
        let arguments = std::iter::from_fn(|| -> Option<OsString> {
            panic!("disabled admission must not scan arguments")
        });
        assert!(!admitted(false, arguments));
    }

    #[test]
    #[cfg(unix)]
    fn non_utf8_argument_preserves_exact_probe_admission() {
        use std::os::unix::ffi::OsStringExt;
        let invalid = OsString::from_vec(vec![0xff, 0xfe]);
        assert!(admitted(
            true,
            [
                invalid.clone(),
                OsString::from("--unified-smoke-test"),
                OsString::from("--unified-ninja-render-probe")
            ]
        ));
        assert!(!admitted(
            true,
            [invalid, OsString::from("--unified-ninja-render-probe")]
        ));
    }

    #[test]
    fn disabled_span_and_flush_leave_storage_unallocated() {
        // A unit test executable has neither smoke nor Ninja probe arguments.
        assert!(!enabled());
        let span = Span::begin(Scope::PrepareFrame, 1);
        span.mark(Event::StateLocked, 1);
        drop(span);
        flush();
        RECORDER.with(|cell| {
            assert!(cell.borrow().buffer.is_none());
            assert!(!cell.borrow().flushed);
        });
    }

    #[test]
    fn nested_spans_keep_distinct_sequences_and_event_order() {
        let mut recorder = Recorder::default();
        let outer = recorder.begin(Scope::PrepareFrame, 10, 1.0).unwrap();
        let inner = recorder.begin(Scope::CvStart, 20, 2.0).unwrap();
        recorder.finish(inner, Scope::CvStart, 20, 3.0);
        recorder.finish(outer, Scope::PrepareFrame, 10, 4.0);
        let buffer = recorder.take_once().unwrap();
        assert_eq!(buffer.open_spans, 0);
        assert_eq!(
            buffer
                .records
                .iter()
                .map(|r| (r.sequence, r.event, r.time))
                .collect::<Vec<_>>(),
            vec![
                (outer, Event::Begin, 1.0),
                (inner, Event::Begin, 2.0),
                (inner, Event::End, 3.0),
                (outer, Event::End, 4.0)
            ]
        );
        assert_ne!(outer, inner);
    }

    #[test]
    fn scalar_record_cap_counts_overflow_without_growth() {
        let mut buffer = Buffer::new();
        let capacity = buffer.records.capacity();
        for index in 0..CAPACITY + 9 {
            buffer.push(Record {
                sequence: 1,
                scope: Scope::PrepareFrame,
                event: Event::StateLocked,
                object: 2,
                time: index as f64,
                value: 0,
            });
        }
        assert_eq!(buffer.records.len(), CAPACITY);
        assert_eq!(buffer.records.capacity(), capacity);
        assert_eq!(buffer.dropped, 9);
    }

    #[test]
    fn one_flush_closes_recorder_and_reports_open_span() {
        let mut recorder = Recorder::default();
        let sequence = recorder.begin(Scope::PrepareFrame, 10, 1.0).unwrap();
        let dump = recorder.take_once().unwrap();
        assert_eq!(dump.open_spans, 1);
        assert_eq!(dump.records.len(), 1);
        assert!(recorder.take_once().is_none());
        assert!(recorder.begin(Scope::PrepareFrame, 11, 2.0).is_none());
        recorder.finish(sequence, Scope::PrepareFrame, 10, 3.0);
        assert!(recorder.buffer.is_none());
    }

    #[test]
    fn empty_flush_is_still_single_use() {
        let mut recorder = Recorder::default();
        let dump = recorder.take_once().unwrap();
        assert!(dump.records.is_empty());
        assert!(recorder.take_once().is_none());
    }
    #[test]
    fn queue_ack_carries_original_sequence_with_distinct_trace_sequence() {
        let mut recorder = Recorder::default();
        let outer = recorder.begin(Scope::PrepareFrame, 10, 1.0).unwrap();
        recorder.handled(42, 2.0);
        recorder.finish(outer, Scope::PrepareFrame, 10, 3.0);
        let dump = recorder.take_once().unwrap();
        assert_eq!(dump.records[1].scope, Scope::MainQueue);
        assert_eq!(dump.records[1].event, Event::Handled);
        assert_eq!(dump.records[1].value, 42);
        assert_ne!(dump.records[1].sequence, outer);
        assert_eq!(dump.open_spans, 0);
    }
}
