# Quick Chat, Codex voice, and appearance

Quick Chat defaults to Control–Option–Space. Settings → Quick Chat shortcut
records a replacement chord. Registration is event driven through macOS Carbon;
conflicts leave the previous shortcut registered. No Accessibility event tap or
background polling is needed.

The floating panel shares the main Rust workspace, running agents, session
history and drafts. Its text input and scroll geometry belong to its own window.
It renders at most eight already loaded transcript blocks and shares the cached
wallpaper image. It does not scan transcripts or resize/reprocess wallpaper on
each opening. Escape hides it; hiding preserves ordinary agent work.

Codex's audio-document icon starts conversational voice. It reuses the Kai
workflow from `v1shay/kai`: an authenticated Codex App Server session signals a
WebRTC connection, with native Rust chat rendering, approvals and tool statuses.
Code's microphone permission is requested only on starting voice. Mute, speech
interruption, ending the call and voice selection are inline controls. Typed
text during a call uses realtime text input instead of waiting behind the call.

The small Apple media dylib is loaded on demand. It hosts only the WebRTC media
bridge, never the application interface or Codex credentials. Ending voice
immediately closes microphone and speaker tracks while allowing Codex's final
transcript and any backing coding task to finish. Closed/stale sessions cannot
reactivate an old microphone. Protocol, media and runtime queues are bounded.

Voice uses Codex's experimental realtime interface and therefore requires a
compatible signed-in Codex CLI/account. Voice-delegated coding retains native
sandbox and approvals. It does not yet create Code's per-typed-prompt file undo
checkpoint. Live microphone, speaker and authenticated backend round trips must
be checked by starting voice in the app; automated checks do not record audio.

Appearance settings include system/dark/light schemes, eleven paired Monocode
palettes, custom accent/background/foreground/sidebar/surface colors, installed
interface and code fonts, independent sizes, sidebar/main glass, opaque mode,
high contrast, diff palettes, reset, and versioned theme import/export including
blur and opacity. Wallpaper settings remain separate, retaining existing image,
effects and per-project/global scope controls.

Theme resolution and font metrics are cached. Settings changes repaint existing
views without reparsing transcripts or reprocessing wallpaper. Terminal painting,
hit testing, IME and sizing use the same monospace metrics. Theme imports are
bounded to 64 KB and file I/O runs off the UI thread.

## Verification — October 22, 2026

- Rust regression suite, serialized: 690 passed, 0 failed, 11 opt-in tests skipped.
- `cargo clippy --all-targets -- -D warnings` and formatting: passed.
- Kai media lifecycle regressions: 11 passed.
- Apple media shim: Swift typecheck passed.

The Rust suite includes real native layout/input interactions, clipboard image
decoding, temporary provider servers, approval/question round trips, same-chat
WebRTC signaling, lazy large transcripts, and wallpaper/font cache invariants.
It does not establish live microphone/speaker or account-specific realtime
availability, frame-rate benchmarks, or a guarantee of zero bugs.
