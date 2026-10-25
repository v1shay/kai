//! Apple Speech dictation. Native capture exists only during an explicit session.
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    mpsc,
};

/// Four normalized listening levels, stored as f32 bits. Readers use Relaxed:
/// these independent visual samples do not synchronize capture ownership.
pub type MeterLevels = Arc<[AtomicU32; 4]>;

fn new_levels() -> MeterLevels {
    Arc::new(std::array::from_fn(|_| AtomicU32::new(0f32.to_bits())))
}

fn clear_levels(levels: &MeterLevels) {
    for level in levels.iter() {
        level.store(0f32.to_bits(), Ordering::Relaxed);
    }
}

/// Sample four temporal bands without copying a microphone buffer. The native
/// tap supplies a first-channel reader that respects AVAudioPCMBuffer.stride.
/// Work is capped at 256 samples even when the callback contains many frames.
fn meter_bands(frames: usize, mut sample: impl FnMut(usize) -> f32) -> [f32; 4] {
    let count = frames.min(256);
    if count == 0 {
        return [0.; 4];
    }
    let mut sums = [0f32; 4];
    let mut counts = [0u32; 4];
    for index in 0..count {
        let value = sample(index * frames / count);
        let value = if value.is_finite() {
            value.clamp(-1., 1.)
        } else {
            0.
        };
        let band = index * 4 / count;
        sums[band] += value * value;
        counts[band] += 1;
    }
    std::array::from_fn(|band| {
        // Kai's microphone meter uses a fixed gain of 18 before its visual
        // compression/smoothing. Keep that gain separate from animation timing.
        ((sums[band] / counts[band].max(1) as f32).sqrt() * 18.).clamp(0., 1.)
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpeechEvent {
    Authorizing,
    Listening { on_device: bool },
    Transcript { text: String, final_result: bool },
    Error(String),
    Stopped,
}

/// Only accepted spoken material is published. Unstable final words stay local
/// until another word arrives or the phrase ends; Apple corrections never edit
/// already published text.
#[derive(Default)]
struct TranscriptState {
    generation: u64,
    committed: String,
    emitted_phrase: String,
    partial: String,
    finalized: bool,
    window: Option<(f64, f64)>,
}
impl TranscriptState {
    fn advance(&mut self, generation: u64) {
        self.generation = generation;
        self.finalized = false;
        self.partial.clear();
        self.emitted_phrase.clear();
        self.window = None;
    }
    fn revise(
        &mut self,
        generation: u64,
        text: &str,
        final_result: bool,
    ) -> Result<Option<String>, ()> {
        self.revise_timed(generation, text, final_result, None)
    }
    fn revise_timed(
        &mut self,
        generation: u64,
        text: &str,
        final_result: bool,
        window: Option<(f64, f64)>,
    ) -> Result<Option<String>, ()> {
        if generation != self.generation || self.finalized {
            return Ok(None);
        }
        if text.len() > crate::input::MAX_DRAFT_BYTES {
            return Err(());
        }
        if let Some((start, end)) = window.filter(|(start, end)| {
            start.is_finite() && end.is_finite() && *start >= 0. && end >= start
        }) {
            // Some Apple on-device tasks start a fresh transcription window
            // after a pause without first emitting isFinal. Segment timestamps
            // are relative to this task's audio, so a disjoint later window is
            // evidence of a new phrase rather than an ASR prefix correction.
            if let Some((previous_start, previous_end)) = self.window
                && start > previous_start + 0.25
                && start >= previous_end - 0.02
                && !self.partial.is_empty()
            {
                self.revise(generation, "", true)?;
                self.finalized = false;
                self.partial.clear();
                self.emitted_phrase.clear();
            }
            self.window = Some((start, end));
        }
        let phrase = if text.is_empty() {
            self.partial.as_str()
        } else {
            text
        };
        let candidate = if final_result {
            phrase
        } else {
            // The last whitespace-delimited word is most susceptible to ASR
            // revisions. Holding it avoids streaming fragments such as 'hel'.
            let shared = phrase
                .bytes()
                .zip(self.partial.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            let words = word_ranges(phrase);
            let stable_end = words
                .iter()
                .take(words.len().saturating_sub(1))
                .rfind(|range| range.end <= shared)
                .map_or(0, |range| range.end);
            &phrase[..stable_end]
        };
        let delta = unseen_spoken_suffix(&self.emitted_phrase, candidate);
        let separator = !delta.is_empty()
            && !self.committed.is_empty()
            && !self.committed.ends_with(char::is_whitespace)
            && !delta.starts_with(char::is_whitespace);
        let len = self
            .committed
            .len()
            .saturating_add(delta.len())
            .saturating_add(usize::from(separator));
        if len > crate::input::MAX_DRAFT_BYTES {
            return Err(());
        }
        if !delta.is_empty() {
            if separator {
                self.committed.push(' ');
            }
            self.committed.push_str(delta);
            if !self.emitted_phrase.is_empty()
                && !self.emitted_phrase.ends_with(char::is_whitespace)
                && !delta.starts_with(char::is_whitespace)
            {
                self.emitted_phrase.push(' ');
            }
            self.emitted_phrase.push_str(delta);
        }
        if final_result {
            self.finalized = true;
            self.partial.clear();
        } else if !text.is_empty() {
            self.partial.clear();
            self.partial.push_str(text);
        }
        Ok(Some(self.committed.clone()))
    }
}

fn word_ranges(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = None;
    for (index, ch) in text.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = start.take() {
                ranges.push(start..index);
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(start) = start {
        ranges.push(start..text.len());
    }
    ranges
}
fn same_spoken_word(a: &str, b: &str) -> bool {
    let a = a.trim_matches(|ch: char| !ch.is_alphanumeric());
    let b = b.trim_matches(|ch: char| !ch.is_alphanumeric());
    !a.is_empty() && a.eq_ignore_ascii_case(b)
}
fn unseen_spoken_suffix<'a>(emitted: &str, candidate: &'a str) -> &'a str {
    if let Some(delta) = candidate.strip_prefix(emitted) {
        return delta;
    }
    let old = word_ranges(emitted);
    let new = word_ranges(candidate);
    if new.len() < old.len() || old.is_empty() {
        return "";
    }
    // Match the longest surviving emitted tail. Changed prefixes are ignored,
    // rather than duplicated; only material after its anchor can be appended.
    for length in (1..=old.len().min(8)).rev() {
        if length > new.len() {
            continue;
        }
        let old_tail = &old[old.len() - length..];
        for start in (0..=new.len() - length).rev() {
            if old_tail
                .iter()
                .zip(&new[start..start + length])
                .all(|(a, b)| same_spoken_word(&emitted[a.clone()], &candidate[b.clone()]))
            {
                return &candidate[new[start + length - 1].end..];
            }
        }
    }
    ""
}

fn restart_allowed(cancelled: &AtomicBool, current: &AtomicU64, finished: u64) -> bool {
    !cancelled.load(Ordering::Acquire) && current.load(Ordering::Acquire) == finished
}

fn silent_phrase_end(domain: &str, code: isize) -> bool {
    // Apple's documented "Failed to recognize any speech" means this request
    // ended during silence; it is not a microphone/authorization failure.
    domain == "kAFAssistantErrorDomain" && code == 1110
}

/// A small, replaceable queue prevents a delayed UI from accumulating partials.
/// All transcripts are cumulative snapshots, including committed phrases. A
/// delayed reader needs just the latest snapshot; errors/stops remain terminal.
#[derive(Clone)]
struct Publisher {
    sender: async_channel::Sender<SpeechEvent>,
    receiver: async_channel::Receiver<SpeechEvent>,
    terminal: Arc<Mutex<bool>>,
}
impl Publisher {
    fn new() -> Self {
        let (sender, receiver) = async_channel::bounded(4);
        Self {
            sender,
            receiver,
            terminal: Arc::new(Mutex::new(false)),
        }
    }
    fn publish(&self, event: SpeechEvent) {
        let mut terminal = self.terminal.lock().unwrap_or_else(|e| e.into_inner());
        if *terminal && !matches!(event, SpeechEvent::Stopped) {
            return;
        }
        if matches!(event, SpeechEvent::Error(_) | SpeechEvent::Stopped) {
            *terminal = true;
        }
        // New snapshots include every earlier committed final. Replacing even
        // a final snapshot does not lose text when recognition restarts.
        let mut pending = Vec::with_capacity(4);
        while let Ok(old) = self.receiver.try_recv() {
            if !matches!(event, SpeechEvent::Transcript { .. })
                || !matches!(old, SpeechEvent::Transcript { .. })
            {
                pending.push(old);
            }
        }
        if pending.len() == 4
            && let Some(index) = pending.iter().position(|old| {
                matches!(
                    old,
                    SpeechEvent::Authorizing | SpeechEvent::Listening { .. }
                )
            })
        {
            pending.remove(index);
        }
        for old in pending {
            let _ = self.sender.try_send(old);
        }
        let _ = self.sender.try_send(event);
    }
}

enum Command {
    SpeechPermission(isize),
    MicrophonePermission(bool),
    Restart(u64),
    QuietRestart(u64),
    Finish,
    Stop,
}

pub struct SpeechHandle {
    pub events: async_channel::Receiver<SpeechEvent>,
    pub levels: MeterLevels,
    commands: mpsc::Sender<Command>,
    cancelled: Arc<AtomicBool>,
    publisher: Publisher,
    transcript: Arc<Mutex<TranscriptState>>,
}
impl SpeechHandle {
    #[cfg(test)]
    pub(crate) fn test_pending(text: &str) -> Self {
        let publisher = Publisher::new();
        publisher.publish(SpeechEvent::Transcript {
            text: text.into(),
            final_result: false,
        });
        let (commands, _) = mpsc::channel();
        Self {
            events: publisher.receiver.clone(),
            levels: new_levels(),
            commands,
            cancelled: Arc::new(AtomicBool::new(false)),
            publisher,
            transcript: Arc::new(Mutex::new(TranscriptState {
                committed: text.into(),
                finalized: true,
                ..TranscriptState::default()
            })),
        }
    }

    /// Returns immediately. Permission and capture readiness arrive as events.
    /// This must be called only in response to the user's dictation action.
    pub fn start() -> anyhow::Result<Self> {
        #[cfg(target_os = "macos")]
        {
            native::start()
        }
        #[cfg(not(target_os = "macos"))]
        {
            anyhow::bail!("Apple Speech dictation is available only on macOS.")
        }
    }
    pub fn stop(&self) {
        self.cancelled.store(true, Ordering::Release);
        // Serialize with the last callback publication before the UI drains
        // pending text. No later partial can arrive after an explicit stop.
        let mut state = self.transcript.lock().unwrap_or_else(|e| e.into_inner());
        let generation = state.generation;
        match state.revise(generation, "", true) {
            Ok(Some(text)) => self.publisher.publish(SpeechEvent::Transcript {
                text,
                final_result: true,
            }),
            Err(()) => self.publisher.publish(SpeechEvent::Error(
                "Dictation reached the message size limit. Start a new message to continue.".into(),
            )),
            Ok(None) => {}
        }
        *self
            .publisher
            .terminal
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = true;
        drop(state);
        clear_levels(&self.levels);
        let _ = self.commands.send(Command::Stop);
    }

    /// Called after stop, while the composer still owns its dictation prefix.
    /// At most four events are pending; snapshots include all finalized words.
    pub fn take_pending(&self) -> (Option<String>, Option<String>) {
        let mut latest = None;
        let mut error = None;
        while let Ok(event) = self.events.try_recv() {
            match event {
                SpeechEvent::Transcript { text, .. } => latest = Some(text),
                SpeechEvent::Error(reason) => error = Some(reason),
                _ => {}
            }
        }
        (latest, error)
    }
}
impl Drop for SpeechHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use block2::RcBlock;
    use objc2::{
        msg_send,
        rc::{Retained, autoreleasepool},
        runtime::{AnyClass, AnyObject, Bool},
    };
    use objc2_foundation::NSString;
    use std::{ffi::CStr, ptr};

    #[link(name = "Speech", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "AVFoundation", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "AVFAudio", kind = "framework")]
    unsafe extern "C" {}

    pub(super) fn start() -> anyhow::Result<SpeechHandle> {
        // Privacy keys are required by macOS; without them a native authorization
        // request terminates the process. Validate before requesting anything.
        unsafe {
            let bundle_class = AnyClass::get(c"NSBundle")
                .ok_or_else(|| anyhow::anyhow!("macOS bundle services are unavailable."))?;
            let bundle: *mut AnyObject = msg_send![bundle_class, mainBundle];
            for key in [
                "NSMicrophoneUsageDescription",
                "NSSpeechRecognitionUsageDescription",
            ] {
                let value: *mut AnyObject =
                    msg_send![bundle, objectForInfoDictionaryKey: &*NSString::from_str(key)];
                anyhow::ensure!(
                    !value.is_null(),
                    "Open the packaged Code.app to use dictation (missing {key})."
                );
            }
            anyhow::ensure!(
                AnyClass::get(c"SFSpeechRecognizer").is_some(),
                "Apple Speech is unavailable on this Mac."
            );
            anyhow::ensure!(
                AnyClass::get(c"AVAudioEngine").is_some(),
                "macOS audio capture is unavailable."
            );
            anyhow::ensure!(
                AnyClass::get(c"AVCaptureDevice").is_some(),
                "macOS microphone permission services are unavailable."
            );
        }
        let publisher = Publisher::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let levels = new_levels();
        let transcript = Arc::new(Mutex::new(TranscriptState::default()));
        let (commands, receiver) = mpsc::channel();
        let handle = SpeechHandle {
            events: publisher.receiver.clone(),
            commands: commands.clone(),
            cancelled: cancelled.clone(),
            levels: levels.clone(),
            publisher: publisher.clone(),
            transcript: transcript.clone(),
        };
        std::thread::Builder::new()
            .name("orbit-apple-speech".into())
            .spawn(move || {
                autoreleasepool(|_| {
                    run(receiver, commands, cancelled, publisher, levels, transcript)
                });
            })?;
        Ok(handle)
    }

    fn run(
        receiver: mpsc::Receiver<Command>,
        commands: mpsc::Sender<Command>,
        cancelled: Arc<AtomicBool>,
        publisher: Publisher,
        levels: MeterLevels,
        transcript: Arc<Mutex<TranscriptState>>,
    ) {
        publisher.publish(SpeechEvent::Authorizing);
        let mut audio: Option<Audio> = None;
        // All lifecycle mutations happen on this worker. Native callbacks send
        // commands only; no native object or borrowed block crosses ownership.
        if !cancelled.load(Ordering::Acquire) {
            unsafe {
                request_speech(&commands);
            }
        } else {
            publisher.publish(SpeechEvent::Stopped);
            publisher.sender.close();
            return;
        }
        while let Ok(command) = receiver.recv() {
            if cancelled.load(Ordering::Acquire) {
                break;
            }
            match command {
                Command::SpeechPermission(3) => unsafe {
                    request_microphone(&commands);
                },
                Command::SpeechPermission(status) => {
                    publisher.publish(SpeechEvent::Error(match status {
                        2 => "Speech recognition is restricted by your Mac's settings.",
                        _ => "Allow Code in System Settings → Privacy & Security → Speech Recognition to use dictation.",
                    }.into()));
                    break;
                }
                Command::MicrophonePermission(true) => {
                    if audio.is_some() {
                        continue;
                    }
                    match unsafe {
                        Audio::start(&commands, &cancelled, &publisher, &levels, &transcript)
                    } {
                        Ok(session) => {
                            if cancelled.load(Ordering::Acquire) {
                                drop(session);
                                break;
                            }
                            publisher.publish(SpeechEvent::Listening {
                                on_device: session.on_device,
                            });
                            audio = Some(session);
                        }
                        Err(error) => {
                            if !cancelled.load(Ordering::Acquire) {
                                publisher.publish(SpeechEvent::Error(error.to_string()));
                            }
                            break;
                        }
                    }
                }
                Command::MicrophonePermission(false) => {
                    publisher.publish(SpeechEvent::Error("Allow Code in System Settings → Privacy & Security → Microphone to use dictation.".into()));
                    break;
                }
                Command::Restart(generation) | Command::QuietRestart(generation) => {
                    if let Some(audio) = &mut audio
                        && let Err(error) = unsafe {
                            audio.restart(
                                generation,
                                matches!(command, Command::QuietRestart(_)),
                                &commands,
                                &cancelled,
                                &publisher,
                            )
                        }
                    {
                        publisher.publish(SpeechEvent::Error(error.to_string()));
                        break;
                    }
                }
                Command::Finish | Command::Stop => break,
            }
        }
        // Prevent further capture callbacks before stopping the engine and tap.
        cancelled.store(true, Ordering::Release);
        drop(audio);
        clear_levels(&levels);
        publisher.publish(SpeechEvent::Stopped);
        publisher.sender.close();
    }

    unsafe fn request_speech(commands: &mpsc::Sender<Command>) {
        let class = AnyClass::get(c"SFSpeechRecognizer").expect("validated Speech class");
        let status: isize = unsafe { msg_send![class, authorizationStatus] };
        if status != 0 {
            let _ = commands.send(Command::SpeechPermission(status));
            return;
        }
        let commands = commands.clone();
        let block = RcBlock::new(move |status: isize| {
            let _ = commands.send(Command::SpeechPermission(status));
        });
        // Apple copies this completion block; the local RcBlock may be dropped.
        unsafe {
            let _: () = msg_send![class, requestAuthorization: &*block];
        }
    }

    unsafe fn request_microphone(commands: &mpsc::Sender<Command>) {
        let Some(class) = AnyClass::get(c"AVCaptureDevice") else {
            let _ = commands.send(Command::MicrophonePermission(false));
            return;
        };
        let media = NSString::from_str("soun"); // AVMediaTypeAudio
        let status: isize = unsafe { msg_send![class, authorizationStatusForMediaType: &*media] };
        if status != 0 {
            let _ = commands.send(Command::MicrophonePermission(status == 3));
            return;
        }
        let commands = commands.clone();
        let block = RcBlock::new(move |granted: Bool| {
            let _ = commands.send(Command::MicrophonePermission(granted.as_bool()));
        });
        unsafe {
            let _: () =
                msg_send![class, requestAccessForMediaType: &*media, completionHandler: &*block];
        }
    }

    /// These objects and the retained blocks are owned by the worker. AVFAudio
    /// invokes the tap concurrently; Apple permits appendAudioPCMBuffer there.
    struct CaptureRequest(Retained<AnyObject>);
    // Only SFSpeechAudioBufferRecognitionRequest is stored here. Apple's audio
    // tap is explicitly a valid append site; the mutex serializes append/end
    // and replacement. No other AnyObject is transferred by this wrapper.
    unsafe impl Send for CaptureRequest {}

    struct Audio {
        engine: Retained<AnyObject>,
        input: Retained<AnyObject>,
        request: Retained<AnyObject>,
        task: Option<Retained<AnyObject>>,
        _recognizer: Retained<AnyObject>,
        _tap: RcBlock<dyn Fn(*mut AnyObject, *mut AnyObject)>,
        _results: RcBlock<dyn Fn(*mut AnyObject, *mut AnyObject)>,
        request_slot: Arc<Mutex<CaptureRequest>>,
        generation: Arc<AtomicU64>,
        transcript: Arc<Mutex<TranscriptState>>,
        tap_installed: bool,
        on_device: bool,
        last_restart: std::time::Instant,
    }
    impl Audio {
        unsafe fn start(
            commands: &mpsc::Sender<Command>,
            cancelled: &Arc<AtomicBool>,
            publisher: &Publisher,
            levels: &MeterLevels,
            transcript: &Arc<Mutex<TranscriptState>>,
        ) -> anyhow::Result<Self> {
            // Signatures/selectors are taken directly from the macOS SDK Speech
            // and AVFAudio headers. Retained owns every +new/recognition object.
            let recognizer = unsafe { new(c"SFSpeechRecognizer") }?;
            // Apple defaults result callbacks to the main queue. Keep native
            // recognition/cumulative transcript work off the render thread and
            // serialize callbacks without allocating a new queue per result.
            let queue = unsafe { new(c"NSOperationQueue") }?;
            unsafe {
                let _: () = msg_send![&*queue, setMaxConcurrentOperationCount: 1isize];
                let _: () = msg_send![&*recognizer, setQueue: &*queue];
            }
            let available: Bool = unsafe { msg_send![&*recognizer, isAvailable] };
            anyhow::ensure!(
                available.as_bool(),
                "Apple Speech is currently unavailable for your language. Try again when your Mac is online."
            );
            let on_device: Bool = unsafe { msg_send![&*recognizer, supportsOnDeviceRecognition] };
            let on_device = on_device.as_bool();
            let request = unsafe { new(c"SFSpeechAudioBufferRecognitionRequest") }?;
            unsafe {
                let _: () = msg_send![&*request, setShouldReportPartialResults: Bool::YES];
                let _: () = msg_send![&*request, setTaskHint: 1isize]; // Dictation
                if on_device {
                    let _: () = msg_send![&*request, setRequiresOnDeviceRecognition: Bool::YES];
                }
            }
            let engine = unsafe { new(c"AVAudioEngine") }?;
            let input: Retained<AnyObject> = unsafe { msg_send![&*engine, inputNode] };
            let format: Retained<AnyObject> =
                unsafe { msg_send![&*input, outputFormatForBus: 0usize] };
            let sample_rate: f64 = unsafe { msg_send![&*format, sampleRate] };
            let channels: u32 = unsafe { msg_send![&*format, channelCount] };
            anyhow::ensure!(
                sample_rate > 0. && channels > 0,
                "No microphone input is available. Connect or select a microphone in System Settings → Sound."
            );
            let request_slot = Arc::new(Mutex::new(CaptureRequest(request.clone())));
            let tap_request = request_slot.clone();
            let generation = Arc::new(AtomicU64::new(1));
            let transcript = transcript.clone();
            transcript
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .advance(1);
            let tap_cancelled = cancelled.clone();
            let tap_levels = levels.clone();
            let tap = RcBlock::new(move |buffer: *mut AnyObject, _: *mut AnyObject| {
                if !buffer.is_null() && !tap_cancelled.load(Ordering::Acquire) {
                    // The block owns its retained request; buffer is valid for the
                    // duration of Apple's callback. No Rust audio copy/allocation.
                    unsafe {
                        let measured = buffer_levels(buffer);
                        if !tap_cancelled.load(Ordering::Acquire) {
                            for (level, value) in tap_levels.iter().zip(measured) {
                                level.store(value.to_bits(), Ordering::Relaxed);
                            }
                        }
                        // The capture thread never waits on a recognizer restart.
                        // A request swap may skip one buffer, but never copies or
                        // queues PCM data or opens another microphone session.
                        if let Ok(request) = tap_request.try_lock() {
                            let _: () = msg_send![&*request.0, appendAudioPCMBuffer: buffer];
                        }
                    }
                }
            });
            let results = result_handler(commands, cancelled, publisher, &generation, &transcript);
            let mut session = Self {
                engine,
                input,
                request,
                task: None,
                _recognizer: recognizer,
                _tap: tap,
                _results: results,
                request_slot,
                generation,
                transcript,
                tap_installed: false,
                on_device,
                last_restart: std::time::Instant::now(),
            };
            // The complete guard exists before installing a tap: every subsequent
            // error path stops capture, removes the tap and cancels recognition.
            unsafe {
                let _: () = msg_send![&*session.input, installTapOnBus: 0usize, bufferSize: 1024u32, format: &*format, block: &*session._tap];
                session.tap_installed = true;
                let task: Retained<AnyObject> = msg_send![&*session._recognizer, recognitionTaskWithRequest: &*session.request, resultHandler: &*session._results];
                session.task = Some(task);
                if cancelled.load(Ordering::Acquire) {
                    anyhow::bail!("Dictation was cancelled.");
                }
                let _: () = msg_send![&*session.engine, prepare];
                let mut error: *mut AnyObject = ptr::null_mut();
                let started: Bool = msg_send![&*session.engine, startAndReturnError: &mut error];
                if !started.as_bool() {
                    let reason = if error.is_null() {
                        "Microphone capture could not start.".to_owned()
                    } else {
                        let description: *mut AnyObject = msg_send![error, localizedDescription];
                        string_value(description)
                    };
                    anyhow::bail!("Microphone: {reason}");
                }
            }
            Ok(session)
        }
    }
    /// Final recognition ends only that phrase's request, not the microphone.
    /// The worker swaps its request while the same engine/tap remains active.
    impl Audio {
        unsafe fn restart(
            &mut self,
            finished_generation: u64,
            quiet_pause: bool,
            commands: &mpsc::Sender<Command>,
            cancelled: &Arc<AtomicBool>,
            publisher: &Publisher,
        ) -> anyhow::Result<()> {
            if !restart_allowed(cancelled, &self.generation, finished_generation) {
                return Ok(());
            }
            if cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            let next = finished_generation.wrapping_add(1);
            self.generation.store(next, Ordering::Release);
            self.transcript
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .advance(next);
            let request = unsafe { new(c"SFSpeechAudioBufferRecognitionRequest") }?;
            unsafe {
                let _: () = msg_send![&*request, setShouldReportPartialResults: Bool::YES];
                let _: () = msg_send![&*request, setTaskHint: 1isize]; // Dictation
                if self.on_device {
                    let _: () = msg_send![&*request, setRequiresOnDeviceRecognition: Bool::YES];
                }
            }
            let results = result_handler(
                commands,
                cancelled,
                publisher,
                &self.generation,
                &self.transcript,
            );
            // Cancel the completed request with its generation already invalid.
            // Late native callbacks cannot replace the newly committed prefix.
            unsafe {
                let mut slot = self.request_slot.lock().unwrap_or_else(|e| e.into_inner());
                let _: () = msg_send![&*slot.0, endAudio];
                *slot = CaptureRequest(request.clone());
            }
            unsafe {
                if let Some(task) = self.task.take() {
                    let _: () = msg_send![&*task, cancel];
                    // isFinal can be delivered before the task reaches its
                    // completed state. Starting another task on that recognizer
                    // immediately can fail with Apple's active-task error 1100.
                    let deadline =
                        std::time::Instant::now() + std::time::Duration::from_millis(400);
                    while std::time::Instant::now() < deadline && !cancelled.load(Ordering::Acquire)
                    {
                        let state: isize = msg_send![&*task, state];
                        if state == 4 {
                            break;
                        }
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                }
            }
            self.request = request;
            self._results = results;
            if quiet_pause {
                // A service that repeatedly reports no speech must not spin
                // tasks. The tap already points at the new request, so audio
                // arriving during this bounded delay is retained for it.
                while self.last_restart.elapsed() < std::time::Duration::from_millis(500)
                    && !cancelled.load(Ordering::Acquire)
                {
                    std::thread::sleep(std::time::Duration::from_millis(10));
                }
            }
            if cancelled.load(Ordering::Acquire) {
                // Stop may arrive while the worker prepares a new request.
                // Leave capture cleanup to run's common exit path.
                return Ok(());
            }
            let task: Retained<AnyObject> = unsafe {
                msg_send![&*self._recognizer, recognitionTaskWithRequest: &*self.request, resultHandler: &*self._results]
            };
            self.task = Some(task);
            self.last_restart = std::time::Instant::now();
            Ok(())
        }
    }

    fn result_handler(
        commands: &mpsc::Sender<Command>,
        cancelled: &Arc<AtomicBool>,
        publisher: &Publisher,
        generation: &Arc<AtomicU64>,
        transcript: &Arc<Mutex<TranscriptState>>,
    ) -> RcBlock<dyn Fn(*mut AnyObject, *mut AnyObject)> {
        let publisher = publisher.clone();
        let commands = commands.clone();
        let cancelled = cancelled.clone();
        let epoch = generation.load(Ordering::Acquire);
        let generation = generation.clone();
        let transcript = transcript.clone();
        RcBlock::new(move |result: *mut AnyObject, error: *mut AnyObject| {
            if cancelled.load(Ordering::Acquire) || generation.load(Ordering::Acquire) != epoch {
                return;
            }
            autoreleasepool(|_| unsafe {
                if !result.is_null() {
                    let transcription: *mut AnyObject = msg_send![result, bestTranscription];
                    let string: *mut AnyObject = msg_send![transcription, formattedString];
                    let text = string_value(string);
                    let final_result: Bool = msg_send![result, isFinal];
                    let final_result = final_result.as_bool();
                    let mut state = transcript.lock().unwrap_or_else(|e| e.into_inner());
                    match state.revise_timed(
                        epoch,
                        &text,
                        final_result,
                        transcription_window(transcription),
                    ) {
                        Ok(Some(text)) => {
                            // Keep serialization through publishing: concurrent
                            // callbacks cannot publish an older revision last.
                            publisher.publish(SpeechEvent::Transcript { text, final_result });
                            if final_result {
                                let _ = commands.send(Command::Restart(epoch));
                                return;
                            }
                        }
                        Ok(None) => return,
                        Err(()) => {
                            publisher.publish(SpeechEvent::Error("Dictation reached the message size limit. Start a new message to continue.".into()));
                            let _ = commands.send(Command::Finish);
                            return;
                        }
                    }
                }
                if !error.is_null() {
                    // Apple may send a cancellation/error callback after a
                    // request finalized but before the worker replaces it.
                    let mut state = transcript.lock().unwrap_or_else(|e| e.into_inner());
                    if state.generation != epoch || state.finalized {
                        return;
                    }
                    let domain: *mut AnyObject = msg_send![error, domain];
                    let code: isize = msg_send![error, code];
                    if silent_phrase_end(&string_value(domain), code) {
                        match state.revise(epoch, "", true) {
                            Ok(Some(text)) => publisher.publish(SpeechEvent::Transcript {
                                text,
                                final_result: true,
                            }),
                            Err(()) => {
                                publisher.publish(SpeechEvent::Error("Dictation reached the message size limit. Start a new message to continue.".into()));
                                let _ = commands.send(Command::Finish);
                                return;
                            }
                            Ok(None) => {}
                        }
                        let _ = commands.send(Command::QuietRestart(epoch));
                        return;
                    }
                    let description: *mut AnyObject = msg_send![error, localizedDescription];
                    publisher.publish(SpeechEvent::Error(format!(
                        "Apple Speech: {}",
                        string_value(description)
                    )));
                    let _ = commands.send(Command::Finish);
                }
            });
        })
    }

    unsafe fn transcription_window(transcription: *mut AnyObject) -> Option<(f64, f64)> {
        if transcription.is_null() {
            return None;
        }
        let segments: *mut AnyObject = unsafe { msg_send![transcription, segments] };
        if segments.is_null() {
            return None;
        }
        let first: *mut AnyObject = unsafe { msg_send![segments, firstObject] };
        let last: *mut AnyObject = unsafe { msg_send![segments, lastObject] };
        if first.is_null() || last.is_null() {
            return None;
        }
        let start: f64 = unsafe { msg_send![first, timestamp] };
        let last_start: f64 = unsafe { msg_send![last, timestamp] };
        let duration: f64 = unsafe { msg_send![last, duration] };
        Some((start, last_start + duration))
    }

    /// SDK AVAudioBuffer.h: floatChannelData is nullable for non-Float32
    /// formats; frameLength/frameCapacity are AVAudioFrameCount (u32), and
    /// stride is NSUInteger. The first pointer addresses frames separated by
    /// stride samples, including interleaved multichannel capture.
    unsafe fn buffer_levels(buffer: *mut AnyObject) -> [f32; 4] {
        let channels: *const *const f32 = unsafe { msg_send![buffer, floatChannelData] };
        if channels.is_null() {
            return [0.; 4];
        }
        let samples = unsafe { *channels };
        if samples.is_null() {
            return [0.; 4];
        }
        let frames: u32 = unsafe { msg_send![buffer, frameLength] };
        let capacity: u32 = unsafe { msg_send![buffer, frameCapacity] };
        let stride: usize = unsafe { msg_send![buffer, stride] };
        let frames = frames.min(capacity) as usize;
        if stride == 0
            || frames
                .saturating_sub(1)
                .checked_mul(stride)
                .is_none_or(|offset| offset > isize::MAX as usize / std::mem::size_of::<f32>())
        {
            return [0.; 4];
        }
        meter_bands(frames, |frame| unsafe { *samples.add(frame * stride) })
    }
    impl Drop for Audio {
        fn drop(&mut self) {
            unsafe {
                let _: () = msg_send![&*self.engine, stop];
                if self.tap_installed {
                    let _: () = msg_send![&*self.input, removeTapOnBus: 0usize];
                }
                let slot = self.request_slot.lock().unwrap_or_else(|e| e.into_inner());
                let _: () = msg_send![&*slot.0, endAudio];
                if let Some(task) = &self.task {
                    let _: () = msg_send![&**task, cancel];
                }
            }
        }
    }
    unsafe fn new(name: &CStr) -> anyhow::Result<Retained<AnyObject>> {
        let class = AnyClass::get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "Apple Speech component {} is unavailable.",
                name.to_string_lossy()
            )
        })?;
        let object: Option<Retained<AnyObject>> = unsafe { msg_send![class, new] };
        object.ok_or_else(|| {
            anyhow::anyhow!(
                "Apple Speech does not support your current language or audio configuration."
            )
        })
    }
    unsafe fn string_value(value: *mut AnyObject) -> String {
        if value.is_null() {
            return String::new();
        }
        let text: *const std::ffi::c_char = unsafe { msg_send![value, UTF8String] };
        if text.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(text) }
                .to_string_lossy()
                .into_owned()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn native_no_speech_callback_flushes_pending_words_and_requests_restart() {
            autoreleasepool(|_| unsafe {
                let publisher = Publisher::new();
                let (commands, receiver) = mpsc::channel();
                let cancelled = Arc::new(AtomicBool::new(false));
                let generation = Arc::new(AtomicU64::new(1));
                let transcript = Arc::new(Mutex::new(TranscriptState::default()));
                {
                    let mut state = transcript.lock().unwrap();
                    state.advance(1);
                    state.revise(1, "First line pending 🦀", false).unwrap();
                }
                let callback =
                    result_handler(&commands, &cancelled, &publisher, &generation, &transcript);
                let error_class = AnyClass::get(c"NSError").unwrap();
                let error: Retained<AnyObject> = msg_send![error_class, errorWithDomain: &*NSString::from_str("kAFAssistantErrorDomain"), code: 1110isize, userInfo: ptr::null::<AnyObject>()];
                callback.call((ptr::null_mut(), (&*error as *const AnyObject).cast_mut()));
                assert!(matches!(
                    receiver.try_recv().unwrap(),
                    Command::QuietRestart(1)
                ));
                assert!(
                    matches!(publisher.receiver.try_recv().unwrap(),SpeechEvent::Transcript {text,..} if text=="First line pending 🦀")
                );
                assert!(!*publisher.terminal.lock().unwrap());
                // A replacement request can continue under the same capture
                // session; the old callback cannot overwrite it after Stop.
                generation.store(2, Ordering::Release);
                transcript.lock().unwrap().advance(2);
                callback.call((ptr::null_mut(), (&*error as *const AnyObject).cast_mut()));
                assert!(receiver.try_recv().is_err());
                assert_eq!(
                    transcript
                        .lock()
                        .unwrap()
                        .revise(2, "Second line", true)
                        .unwrap()
                        .unwrap(),
                    "First line pending 🦀 Second line"
                );
            });
        }

        #[test]
        fn native_pcm_meter_respects_interleaved_stride_and_nonfloat_formats() {
            // These are memory-only PCM buffers. No engine, microphone,
            // recognition request or privacy permission is created.
            autoreleasepool(|_| unsafe {
                let format_class = AnyClass::get(c"AVAudioFormat").unwrap();
                let buffer_class = AnyClass::get(c"AVAudioPCMBuffer").unwrap();
                for common_format in [1usize, 3usize] {
                    // Float32, Int16
                    let allocated: objc2::rc::Allocated<AnyObject> = msg_send![format_class, alloc];
                    let format: Retained<AnyObject> = msg_send![allocated, initWithCommonFormat: common_format, sampleRate: 48000f64, channels: 2u32, interleaved: Bool::YES];
                    let allocated: objc2::rc::Allocated<AnyObject> = msg_send![buffer_class, alloc];
                    let buffer: Retained<AnyObject> =
                        msg_send![allocated, initWithPCMFormat: &*format, frameCapacity: 1024u32];
                    let _: () = msg_send![&*buffer, setFrameLength: 1024u32];
                    let channels: *const *mut f32 = msg_send![&*buffer, floatChannelData];
                    if common_format == 1 {
                        let stride: usize = msg_send![&*buffer, stride];
                        assert_eq!(stride, 2);
                        let samples = *channels;
                        for frame in 0..1024 {
                            *samples.add(frame * stride) = 0.01;
                            *samples.add(frame * stride + 1) = 1.;
                        }
                        let measured = buffer_levels((&*buffer as *const AnyObject).cast_mut());
                        assert!(
                            measured
                                .into_iter()
                                .all(|level| (level - 0.18).abs() < 0.0001)
                        );
                    } else {
                        assert!(channels.is_null());
                        assert_eq!(
                            buffer_levels((&*buffer as *const AnyObject).cast_mut()),
                            [0.; 4]
                        );
                    }
                }
            });
        }

        #[test]
        fn cancelled_worker_ignores_late_permission_without_native_requests() {
            let publisher = Publisher::new();
            let events = publisher.receiver.clone();
            let (commands, receiver) = mpsc::channel();
            // Simulate authorization finishing after the handle was dropped.
            commands.send(Command::SpeechPermission(3)).unwrap();
            commands.send(Command::MicrophonePermission(true)).unwrap();
            commands.send(Command::Restart(1)).unwrap();
            run(
                receiver,
                commands,
                Arc::new(AtomicBool::new(true)),
                publisher,
                new_levels(),
                Arc::new(Mutex::new(TranscriptState::default())),
            );
            assert_eq!(events.try_recv().unwrap(), SpeechEvent::Authorizing);
            assert_eq!(events.try_recv().unwrap(), SpeechEvent::Stopped);
            assert!(events.try_recv().is_err());
            assert!(events.is_closed());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disjoint_native_utterance_windows_keep_collecting_after_first_phrase() {
        let mut state = TranscriptState::default();
        state.advance(1);
        state
            .revise_timed(1, "First line", false, Some((0., 1.)))
            .unwrap();
        state
            .revise_timed(1, "First line ends", false, Some((0., 1.5)))
            .unwrap();
        assert_eq!(state.committed, "First line");
        // Apple's next window contains only the new sentence, is shorter than
        // the old one, and never finalized the previous recognition request.
        let first_new = state
            .revise_timed(1, "Next", false, Some((2., 2.4)))
            .unwrap()
            .unwrap();
        assert_eq!(first_new, "First line ends");
        state
            .revise_timed(1, "Next sentence", false, Some((2., 2.8)))
            .unwrap();
        state
            .revise_timed(1, "Next sentence continues", false, Some((2., 3.4)))
            .unwrap();
        assert_eq!(
            state.revise(1, "", true).unwrap().unwrap(),
            "First line ends Next sentence continues"
        );
    }

    #[test]
    fn native_timestamp_corrections_do_not_create_duplicate_phrases() {
        let mut state = TranscriptState::default();
        state.advance(1);
        for (text, window) in [
            ("I like", (0., 1.)),
            ("I like cats", (0., 1.5)),
            ("I really like cats", (0.08, 1.6)),
            ("I really like cats today", (0.07, 2.)),
        ] {
            state.revise_timed(1, text, false, Some(window)).unwrap();
        }
        let final_text = state.revise(1, "", true).unwrap().unwrap();
        assert_eq!(final_text, "I like cats today");
        assert!(!silent_phrase_end("kLSRErrorDomain", 1110));
        assert!(!silent_phrase_end("kAFAssistantErrorDomain", 1700));
        assert!(!silent_phrase_end("kAFAssistantErrorDomain", 1101));
        assert!(silent_phrase_end("kAFAssistantErrorDomain", 1110));
    }
    #[test]
    fn listening_meter_is_silent_bounded_and_ignores_nonfinite_samples() {
        assert_eq!(meter_bands(0, |_| panic!("empty input read")), [0.; 4]);
        assert_eq!(meter_bands(1024, |_| 0.), [0.; 4]);
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(meter_bands(1024, |_| value), [0.; 4]);
        }
        let mut reads = 0;
        let levels = meter_bands(1_000_000, |frame| {
            reads += 1;
            assert!(frame < 1_000_000);
            ((frame as f32 * 0.03).sin()) * 0.025
        });
        assert_eq!(reads, 256);
        assert!(levels.into_iter().all(|value| value > 0. && value <= 1.));
        assert_eq!(meter_bands(1024, |_| f32::MAX), [1.; 4]);
    }

    #[test]
    fn listening_meter_samples_first_channel_and_resets_without_capture() {
        let interleaved = [0.01f32, 1., 0.01, 1., 0.01, 1., 0.01, 1.];
        let bands = meter_bands(4, |frame| interleaved[frame * 2]);
        assert!(bands.into_iter().all(|value| (value - 0.18).abs() < 0.0001));
        let levels = new_levels();
        for (level, value) in levels.iter().zip(bands) {
            level.store(value.to_bits(), Ordering::Relaxed);
        }
        clear_levels(&levels);
        assert!(
            levels
                .iter()
                .all(|level| level.load(Ordering::Relaxed) == 0)
        );
    }

    #[test]
    fn delayed_reader_gets_cumulative_phrases_then_stop() {
        let queue = Publisher::new();
        queue.publish(SpeechEvent::Authorizing);
        queue.publish(SpeechEvent::Listening { on_device: true });
        for text in ["h", "hé", "hé🦀"] {
            queue.publish(SpeechEvent::Transcript {
                text: text.into(),
                final_result: false,
            });
        }
        assert_eq!(queue.receiver.len(), 3);
        queue.publish(SpeechEvent::Transcript {
            text: "hé🦀!".into(),
            final_result: true,
        });
        queue.publish(SpeechEvent::Transcript {
            text: "hé🦀! next phrase".into(),
            final_result: false,
        });
        queue.publish(SpeechEvent::Stopped);
        let events: Vec<_> = std::iter::from_fn(|| queue.receiver.try_recv().ok()).collect();
        assert_eq!(
            events,
            vec![
                SpeechEvent::Authorizing,
                SpeechEvent::Listening { on_device: true },
                SpeechEvent::Transcript {
                    text: "hé🦀! next phrase".into(),
                    final_result: false
                },
                SpeechEvent::Stopped
            ]
        );
    }
    #[test]
    fn automatic_snapshots_are_monotonic_across_corrections_and_pauses() {
        let mut state = TranscriptState::default();
        state.advance(1);
        let mut previous = String::new();
        for (text, final_result) in [
            ("hel", false),
            ("Hello", false),
            ("Hello 日本", false),
            ("Hello 日本語 today", false),
            ("Hello", false),
            ("Hello 日本語 today more", false),
            ("Hello 日本語 today more words", true),
        ] {
            let snapshot = state.revise(1, text, final_result).unwrap().unwrap();
            assert!(
                snapshot.starts_with(&previous),
                "automatic replacement: {previous:?} -> {snapshot:?}"
            );
            previous = snapshot;
        }
        assert_eq!(previous, "Hello 日本語 today more words");
        assert!(state.revise(1, "late reset", false).unwrap().is_none());
        state.advance(2);
        assert!(state.revise(1, "old request", true).unwrap().is_none());
        assert_eq!(
            state.revise(2, "After silence", false).unwrap().unwrap(),
            previous
        );
        let snapshot = state
            .revise(2, "After silence continue", false)
            .unwrap()
            .unwrap();
        assert!(snapshot.starts_with(&previous));
        let final_snapshot = state.revise(2, "", true).unwrap().unwrap();
        assert!(final_snapshot.starts_with(&snapshot));
        assert_eq!(
            final_snapshot,
            "Hello 日本語 today more words After silence continue"
        );
    }

    #[test]
    fn revised_prefixes_do_not_duplicate_words_or_erase_published_text() {
        let mut state = TranscriptState::default();
        state.advance(1);
        for text in ["I like", "I like cats", "I like cats today"] {
            state.revise(1, text, false).unwrap();
        }
        assert_eq!(state.committed, "I like cats");
        state.revise(1, "I really like cats today", false).unwrap();
        state
            .revise(1, "I really like cats today tomorrow", false)
            .unwrap();
        assert_eq!(state.committed, "I like cats today");
        let final_text = state
            .revise(1, "I really like cats today tomorrow", true)
            .unwrap()
            .unwrap();
        assert_eq!(final_text, "I like cats today tomorrow");
    }

    #[test]
    fn shorter_asr_correction_replaces_held_words_before_stop_flushes() {
        let mut state = TranscriptState::default();
        state.advance(7);
        state
            .revise_timed(7, "Keep these words", false, Some((0., 1.)))
            .unwrap();
        state
            .revise_timed(7, "Keep these words tomorrow", false, Some((0., 1.5)))
            .unwrap();
        assert_eq!(state.committed, "Keep these words");
        state
            .revise_timed(7, "Keep these words", false, Some((0., 1.4)))
            .unwrap();
        assert_eq!(
            state.revise(7, "", true).unwrap().unwrap(),
            "Keep these words"
        );
        state.advance(8);
        state.revise(8, "日本語 deleted ending", false).unwrap();
        state.revise(8, "日本語", false).unwrap();
        assert_eq!(
            state.revise(8, "", true).unwrap().unwrap(),
            "Keep these words 日本語"
        );
    }

    #[test]
    fn empty_final_preserves_latest_partial_through_next_phrase() {
        let mut state = TranscriptState::default();
        state.advance(1);
        assert_eq!(state.revise(1, "Hello 日本語", false).unwrap().unwrap(), "");
        assert_eq!(state.revise(1, "", true).unwrap().unwrap(), "Hello 日本語");
        state.advance(2);
        assert_eq!(
            state.revise(2, "next phrase", false).unwrap().unwrap(),
            "Hello 日本語"
        );
        assert_eq!(
            state.revise(2, "", true).unwrap().unwrap(),
            "Hello 日本語 next phrase"
        );
        state.advance(3);
        assert_eq!(
            state.revise(3, "", true).unwrap().unwrap(),
            "Hello 日本語 next phrase"
        );
    }

    #[test]
    fn explicit_stop_retains_queued_snapshot_and_rejects_late_publication() {
        let publisher = Publisher::new();
        let (commands, _receiver) = mpsc::channel();
        let transcript = Arc::new(Mutex::new(TranscriptState::default()));
        transcript.lock().unwrap().finalized = true;
        let handle = SpeechHandle {
            events: publisher.receiver.clone(),
            levels: new_levels(),
            commands,
            cancelled: Arc::new(AtomicBool::new(false)),
            publisher: publisher.clone(),
            transcript: transcript.clone(),
        };
        publisher.publish(SpeechEvent::Transcript {
            text: "Earlier words".into(),
            final_result: true,
        });
        // Simulate an already displayed callback.
        assert!(matches!(
            handle.events.try_recv().unwrap(),
            SpeechEvent::Transcript { .. }
        ));
        publisher.publish(SpeechEvent::Transcript {
            text: "Earlier words and queued 日本語 🦀".into(),
            final_result: false,
        });
        handle.stop();
        publisher.publish(SpeechEvent::Transcript {
            text: "late overwrite".into(),
            final_result: false,
        });
        publisher.publish(SpeechEvent::Stopped);
        assert_eq!(
            handle.take_pending().0.as_deref(),
            Some("Earlier words and queued 日本語 🦀")
        );
        assert!(handle.take_pending().0.is_none());
        assert!(handle.cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn explicit_stop_flushes_unstable_tail_and_preserves_pending_error() {
        let publisher = Publisher::new();
        let (commands, _receiver) = mpsc::channel();
        let transcript = Arc::new(Mutex::new(TranscriptState::default()));
        {
            let mut state = transcript.lock().unwrap();
            state.advance(1);
            state.revise(1, "Final 日本語 🦀", false).unwrap();
            assert!(state.committed.is_empty());
        }
        let handle = SpeechHandle {
            events: publisher.receiver.clone(),
            levels: new_levels(),
            commands,
            cancelled: Arc::new(AtomicBool::new(false)),
            publisher: publisher.clone(),
            transcript,
        };
        handle.stop();
        assert_eq!(handle.take_pending().0.as_deref(), Some("Final 日本語 🦀"));

        let publisher = Publisher::new();
        let (commands, _receiver) = mpsc::channel();
        let handle = SpeechHandle {
            events: publisher.receiver.clone(),
            levels: new_levels(),
            commands,
            cancelled: Arc::new(AtomicBool::new(false)),
            publisher: publisher.clone(),
            transcript: Arc::new(Mutex::new(TranscriptState::default())),
        };
        publisher.publish(SpeechEvent::Error("Microphone unavailable".into()));
        handle.stop();
        assert_eq!(
            handle.take_pending().1.as_deref(),
            Some("Microphone unavailable")
        );
    }

    #[test]
    fn explicit_stop_and_stale_completion_never_restart_recognition() {
        let cancelled = AtomicBool::new(false);
        let current = AtomicU64::new(2);
        assert!(restart_allowed(&cancelled, &current, 2));
        assert!(!restart_allowed(&cancelled, &current, 1));
        cancelled.store(true, Ordering::Release);
        assert!(!restart_allowed(&cancelled, &current, 2));
    }

    #[test]
    fn bounded_queue_keeps_every_committed_phrase_in_latest_snapshot() {
        let queue = Publisher::new();
        let mut state = TranscriptState::default();
        queue.publish(SpeechEvent::Authorizing);
        queue.publish(SpeechEvent::Listening { on_device: true });
        for generation in 1..=20 {
            state.advance(generation);
            let text = state
                .revise(generation, "phrase 🦀", true)
                .unwrap()
                .unwrap();
            queue.publish(SpeechEvent::Transcript {
                text,
                final_result: true,
            });
            assert!(queue.receiver.len() <= 4);
        }
        queue.publish(SpeechEvent::Error("provider failed".into()));
        queue.publish(SpeechEvent::Stopped);
        let events: Vec<_> = std::iter::from_fn(|| queue.receiver.try_recv().ok()).collect();
        assert!(events.iter().any(|event| matches!(event, SpeechEvent::Transcript { text, .. } if text.matches("phrase 🦀").count() == 20)));
        assert!(events.contains(&SpeechEvent::Error("provider failed".into())));
        assert_eq!(events.last(), Some(&SpeechEvent::Stopped));
    }

    #[test]
    fn cumulative_speech_limit_is_bounded_without_truncating_unicode() {
        let mut state = TranscriptState::default();
        state.advance(1);
        let first = "🦀".repeat(crate::input::MAX_DRAFT_BYTES / 4);
        assert_eq!(state.revise(1, &first, true).unwrap().unwrap(), first);
        state.advance(2);
        assert!(state.revise(2, "é", true).is_err());
        assert_eq!(state.committed, first);
    }
    #[test]
    fn terminal_error_survives_late_native_callbacks() {
        let queue = Publisher::new();
        queue.publish(SpeechEvent::Error("denied".into()));
        queue.publish(SpeechEvent::Listening { on_device: true });
        queue.publish(SpeechEvent::Stopped);
        assert_eq!(
            queue.receiver.try_recv().unwrap(),
            SpeechEvent::Error("denied".into())
        );
        assert_eq!(queue.receiver.try_recv().unwrap(), SpeechEvent::Stopped);
    }
}
