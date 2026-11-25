# Kai Unified — native Code experiment

This branch builds **Kai Unified.app** from Kai and the pinned native source of
[v1shay/code](https://github.com/v1shay/code), commit
`b497c10b0e175513e8426fb38cba891dd9ebbc7a`. The source is in
`integrations/Code`, with provenance in `UPSTREAM.json` and the original asset
and dependency notices retained. The original Kai target still builds normally.

## Try it

```sh
./scripts/build-unified-app.sh
open ".build/unified/Kai Unified.app"
```

Local builds use a persistent certificate-backed signing identity in a private
`~/Library/Application Support/Kai Build` keychain. This gives macOS a stable app
identity for microphone and other permission prompts across rebuilds. The build
trusts that identity only for code signing in your user account; it does not
grant privacy permissions or change system-wide or browser/TLS trust. Set
`KAI_SIGNING_IDENTITY` (and optionally `KAI_SIGNING_KEYCHAIN`) to use an existing
development or distribution identity. An explicit `KAI_SIGNING_IDENTITY=-`
retains ad-hoc signing for isolated test bundles.

Saved projects/chat summaries and installed providers are published before the
full native-history scan. The scan runs outside the SQLite/history command
worker, and a cached selected chat can restore while unrelated histories are
still being indexed. Run `python3 scripts/test-unified-packaged.py discovery`
to measure cached navigation, providers, and full discovery separately using an
isolated copy of summary metadata.

The build uses cached Cargo dependencies and checks every Swift ABI export
before signing the bundle. On a fresh development machine, first run
`cargo fetch --manifest-path integrations/Code/Cargo.toml`.

- Hold **Control–Option** for one second to open the mini chat. Continue the
  same hold to 1.8 seconds to morph the notch into the Code workspace. Hold it
  again in the foreground workspace to fold to the mini chat. With Code open
  behind another application, the normal hold overlays the extended notch while
  retaining Code’s existing window and selected chat. The next hold brings that
  same window forward through the original morph and staged UI entrance.
  Releasing cancels the hold.
- Code's Quick Chat shortcut opens only Code's floating mini chat. Its show/hide
  lifecycle suppresses the notch and cancels competing Control–Option holds.
  **↗ Notch**, **Command–Option–N**, and the **海** menu return to the extended notch.
- Double-Command dismisses the extended notch, floating Quick Chat, or large
  workspace. The large view reverses its component/shape choreography into the
  compact notch. Agent work continues while these views are dismissed.
- The native glass transition hides the notch while Code is expanded. Kai's
  existing pet animation moves into the workspace header, sharing the same
  sprite layer. Reduced Motion uses a short fade.
- Choose a chat, agent, or model in the notch. Sending and dictation go through
  Code's normal submission path. Switching providers retains Code's handoff and
  confirmation behavior. A staged confirmation remains in the Code workspace.
- Running chats supply Kai's activity indicators and floating task pets. Native
  approvals can be approved once, declined, or stopped in the compact popout.
  **Review…** opens native controls for the provider's exact choices and question
  forms, including multiple selections and secure text fields. Pending requests
  automatically open Kai’s native controls while the Code workspace is in the
  background. Foreground Code retains its native review UI. Window/application
  focus notifications route an already-pending request without waiting for a
  new agent event. The inbox’s Next control cycles requests without changing chats.
- Existing Command/Fn/Option dictation shortcuts remain available. Image drops
  and screen context use Code's attachment import and native submission path.
  The voice button uses Code's single Codex call. Hold-to-voice passes the
  recorded first prompt to that same call. Voice appshots send the actual image
  to a native context turn in the same chat before realtime starts. Short taps
  preserve the current surface; a held silent release creates no chat or call.

Kai Unified has a separate bundle ID (`com.kai.unified.experimental`) and data
store (`~/Library/Application Support/Kai Unified`). On first launch it seeds
bounded Code workspace/project preference files and Kai's appearance/input
preferences into its own stores. It does not copy a live SQLite database or
provider credentials. CLI transcripts and native session IDs are still shared
with the installed providers; use an idle conversation for initial testing.
For shortcut testing, quit the stable Kai first so both apps do not handle the
same modifier holds. If the stable Code is using the same Quick Chat shortcut,
close that app or change one shortcut. The existing Kai and Code bundles and
their checkouts are not replaced.
Because this is a separate app identity, enable its own macOS Accessibility and
Input Monitoring permissions for modifier-hold shortcuts, and grant microphone,
speech recognition, or screen capture access when using those features.
An explicit `ORBIT_DATA_DIR` still overrides the experimental store.

## Ownership and resource use

GPUI remains the application lifecycle owner, renderer for the IDE, and sole
owner of sessions, agents, persistence, drafts, consent gates, and native run
handles. `libKaiNotch.dylib` installs Kai's existing AppKit/Core Animation views
inside that same process without replacing GPUI's application delegate.

The embedded `KaiBridge` does not launch Python, a socket bridge, a Codex app
server, a second CLI, or a history monitor. Owner notifications push bounded
state directly into the native notch. Commands enter a bounded async queue and
execute on Code's UI context. Voice levels use a separate four-value callback,
so microphone updates do not resend the conversation to Swift. The media shim
is loaded only when Code starts a voice call. Optional local speech playback
retains its existing separate speech helper.

The mini transcript contains at most 100 rendered blocks, with 4,000 characters
per block and a 512 KiB total encoded JSON budget, including speech text and JSON
escapes. The newest complete blocks are retained. Load more pages through the earlier
blocks within that same budget; Latest returns to the live/native tail. An unchanged transcript is
omitted from subsequent owner updates and retained by Swift for that exact chat;
switching owners clears it. Voice levels still use their separate callback.
Projects and recent chats are bounded too. The full conversation,
editing, terminal, files, diffs, checkpoints, queues, teams, delegated-child
inspector, and provider settings remain in the real Code workspace. No second
copy of the IDE is embedded into a web view.

The pet list includes Code’s top-level runs, team members, and live provider
children, preserving each child’s parent, provider, and native turn epoch. Clicking
a child opens its existing native inspector. Child permissions use the same
request inbox and broker response channel as the Code workspace.

Mini-view back/forward uses the owner’s session navigation and draft storage.
Model effort options come from the installed provider’s catalog and reach Codex’s
native `turn/start.effort`; the Code picker and Kai share the same setting. Effort,
workspace motion, and text decode preferences are persisted by Code. Kai’s text
morph menu controls workspace text decode without adding a second mini-notch
entrance. Code-owned attachment previews and image-only drafts roundtrip to Kai;
canonical images are retained without reimporting their thumbnail pixels.

Conversational realtime voice uses Code’s Codex WebRTC session. Apple Speech
input can submit to the other providers through their normal native turn path.

## Safe handoff

- The owner token includes the selected session, project, draft generation,
  and agent. Composer edits use a separate exact-base check,
  so typing cannot invalidate unrelated navigation commands.
- A handoff compares the current IDE draft with the notch's last observed base.
  A divergent IDE draft is preserved and the action is rejected with a visible
  explanation. Draft transfer is guarded before navigation. Expanding the workspace is immediate;
  a divergent hidden notch draft remains intact if its transfer is rejected.
- Send is gated before calling Code's native send function, whose active-run
  behavior is otherwise cancellation. A send from the notch cannot become Stop.
- Stop checks the exact native run generation. Approvals bind the owning session,
  opaque request ID, and invocation epoch, including background delegated work.
- Provider choices are transmitted by exact ID. Generic Approve selects only a
  recognized one-time choice. Broader choices require their explicit labelled
  button in Review. Required and single-choice answers are validated again by
  the owner before reaching the normal runtime response channel.
- Request IDs are deduplicated in a bounded cache of SHA-256 fingerprints;
  recorded audio and prompt bodies are not retained in that cache. Commands are never retried
  after an ambiguous acknowledgement. Import failures retain the input.

## Window continuity

`WorkspaceMorph` clips the actual native Code window with a continuous contour
whose initial shoulders exactly match `NotchSizing.path`. Both endpoint paths
have corresponding segments, so the original notch outline interpolates into
normal app corners. One glass backdrop bridges the surfaces behind the live UI.
There are no fragmented snapshots. Code lays out once at `NSScreen.visibleFrame`,
filling the working area between the menu bar and Dock without entering a
separate fullscreen Space.

The real header, sidebar, transcript, inspectors, and composer enter in sequence
using finite opacity and paint-offset transitions. Existing list entrances and
text decode run inside that sequence. Dismissal retires the lower components
first while the contour retracts into the notch. Reversals capture the exact presentation-layer bezier contour and each real
component’s current progress. Swift and Rust use the same system monotonic epoch,
so command-queue delay does not restart component timing. Revision checks retire stale
work after interruptions; Reduced Motion uses a short fade. The original
compact-to-extended notch animation remains intact, and its conversation does
not receive the added workspace text overlay.

Idle provider logos transition into Kai's native thinking scene. A done mark is
shown only for a confirmed successful run, including late confirmations or runs
that finish between snapshots. Failure and cancellation never infer success
from a run disappearing. Quick Chat keeps task pets and automatic chat reviews
suppressed across every owner notification. Sidebar labels share a consistent
gutter and baseline; file-citation underlines cover the filename, leaving the
icon, gutter, and file-action buttons clear.

Voice shortcuts record first, then pass the exact audio/context through the
matching navigation acknowledgement to Code's native voice session. Recorder
resets do not publish an intermediate off state before recording or handoff.
The compact surface retains ownership during backend startup. Refused startup
returns an error acknowledgement and visible feedback rather than an empty notch.

Project rows show their own chats in the notch. Model discovery runs off-thread
for the selected available provider without requiring Code's picker to be open;
the provider default remains usable while discovery finishes. A saved default
from another provider does not block new-chat creation. Completion notices come
from confirmed runtime outcomes, including turns that finish between snapshots.

## Verification

```sh
swift test --disable-sandbox
cargo test --manifest-path integrations/Code/Cargo.toml --bin orbit -- --test-threads=1
python3 -m unittest discover -v
scripts/test-unified-native.sh
```

The packaged build also has a provider-free startup/morph probe:

```sh
open -n ".build/unified/Kai Unified.app" --args --unified-smoke-test
```

That probe uses a temporary data directory, verifies that Swift decoded Code's
initial state, expands the real native workspace, reverses a transition in flight, returns it
to the notch, and
quits. Its log reports the owner handshake and morph result. It does not submit
an agent prompt or answer a real approval.

Recorded checks on 2026-10-24 for 0.3.0:

- Swift: 183 tests, one optional local-audio test skipped, zero failures.
- Code: complete serial run, 718 passed and 11 ignored.
- Every voice-shortcut action is covered through recording, context capture,
  exact chat-selection acknowledgement, and first-audio handoff.
- Runtime checks cover refused voice startup, two consecutive local-provider
  submissions, draft conflicts, approvals, and native question forms.
- Rendered native sidebar/citation fixtures verify icon gutters and filename-only
  underlines using the production link attributes.
- Release bundle: signature and all eight public native ABI exports verified.
- Packaged probe: morph reversal, maximized working area, shared pet,
  repeat-open behavior, extended-notch return, the double-Command dismissal
  handler, and exclusive native Quick Chat passed.

Automated verification cannot establish subjective animation quality, actual
microphone/TCC behavior, or every installed provider's live behavior. Those need
an interactive desktop pass. UI automation was unavailable during development;
the native tests, ABI checks, and packaged-app probe are recorded separately
from that remaining visual and live-provider validation.

### 0.4.0 verification (2026-10-25)

The permanent `Tests/UnifiedNative/StateHarness.swift` harness compiles the actual
Kai AppKit sources with the in-process runtime. It runs under its own bundle ID,
using synthetic owner events and no microphone/provider calls. It verifies
failure/cancellation, late confirmed completion, exclusive Quick Chat, clickable
approval controls over both workspace and Quick Chat, exact approval token
submission, inbox cycling, incremental-history retention/isolation, and Code-owned
image-only drafts without reimporting thumbnails.

The owner regression suite verifies Unicode/escaped JSON budgets, shared effort
choices, draft-preserving back/forward, delegated child identity, stale approval
rejection, form validation, and two consecutive native-provider fixture turns.
Installed-provider checks additionally verify Codex model/account metadata, a real
minimal Codex turn with an explicit effort, native sandbox commands/write blocking/
owned-command cancellation, and Claude’s bidirectional initialization/catalog.
These do not establish physical modifier delivery, live microphone audio, or
subjective animation quality. The desktop automation service was unavailable.

Final 0.4.0 checks: 185 Swift tests (one optional local-audio fixture skipped),
727 Code tests (11 opt-in tests excluded), the isolated native event harness,
and the signed release morph/Quick Chat probe. The three installed native sandbox
checks and scoped installed Codex/Claude checks passed separately.

### 0.4.1 background continuity

Opening the extended notch from another application retains the actual Code
window in place. It does not order Code out, activate it, reload the selected
chat, or rebuild its workspace. Dismissing the overlay leaves Code open.
Returning uses the same live window and existing continuous contour, with real
components replaying their entrance from top to bottom. Foreground folding and
double-Command dismissal retain their original animations.

Automatic Kai approval popouts follow workspace keyboard/application focus.
Foreground Code keeps its own review surface; switching away presents a pending
request in Kai, even with no new owner event. Returning to Code retires the
popout. Review forms retain their local input if reopened for the same request.
The isolated native harness verifies retained window identity/frame, overlay
dismissal, replayed morph, foreground approval suppression, background approval
routing through activation notifications, and repeated focus changes for the
same pending request. Swift: 187 tests, one optional audio fixture skipped.
Rust: six motion regressions and the native owner regression suite.

### 0.4.2 selected-chat continuity

Ordinary compact/extended/workspace expansion preserves Code’s selected chat and
composer draft, including an idle selected chat while another agent is running.
Activity focus does not implicitly navigate. Explicit pet/chat selections still
use normal owner navigation. Launch uses Code’s normal saved workspace restore
or its new-chat state. The native event harness covers the competing-running-chat
case and verifies that expansion emits no chat-navigation command.

### 0.5.0 held voice restoration

Voice shortcuts record from modifier-down, including the configured hold interval.
Activation and appshot capture still require that hold; a short tap restores the
prior surface. Releasing a silent recording discards it without creating a chat
or opening a voice session. Releasing speech starts the selected-chat or new-chat
action only after the exact navigation acknowledgement, preserving its audio and
context. Holding the assigned shortcut again stops the native call.

New standalone voice chats now create their validated, application-owned working
directory before launching Codex. Existing user project directories are never
recreated. Voice media attaches to the actual visible notch for a shortcut or the Code
workspace for its own voice controls. A direct native view handoff prevents
starting WebKit capture in an ordered-out IDE, without another media window. Appshots use real
`localImage` inputs in a native Codex context turn, followed by realtime on that
same thread. Voice shutdown is excluded from ordinary coding-completion popouts. Listening
and speaking both retain Kai's audio waveform. Terminal Stop also closes its
PTY before waiting for the owned shell; the previous order repeatedly stalled
that shutdown regression in the full test suite.

Reproducible checks:

```sh
scripts/test-unified-native.sh voice
python3 scripts/diagnose-unified-voice.py --backend /absolute/path/to/orbit
```

The shortcut harness invokes the actual AppKit modifier handler with in-memory
NSEvents; it does not post keys to the desktop. It replaces hardware recording,
context capture, and owner acknowledgements with controlled fixtures. The live
diagnostic uses the real native Code runtime, signed Apple media shim, installed
Codex, generated speech, and a generated image. Earlier in this repair, both new-chat-with-image and
same-chat resume completed with peer connection, transmitted first prompt,
user/assistant transcripts, and received audio while the IDE window was hidden.
Its microphone track is disabled and its speaker muted. Its temporary app uses
the original Kai microphone identity; it does not establish physical key delivery
or the experimental app's own macOS permission grant.

Full suites: 188 Swift tests (one optional local-audio test skipped), 730 Code
tests passed and 11 opt-in tests ignored. The final modifier harness tests every
voice action on Command, Option and Fn, including compact indicator ownership,
actual native host selection, speaking waveforms, ordinary Command-key
combinations, short taps, and double-Command in the big form.
The native state harness passed separately. The release signature, all nine ABI
exports, and owner startup handshake passed; installed executable/library hashes
match the signed build.

The final awake-display run passed both strengthened spoken-image checks using
the release Code executable and production Apple media shim: a new chat read the
generated blue image and answered its color in voice; resuming the same native
chat answered the color again with user/assistant transcripts and received audio.
The signed packaged probe passed continuous morph reversal, maximized working
area, shared pet, repeat open, extended-notch return, double-Command dismissal,
and exclusive Quick Chat. Earlier locked-display attempts were invalid for
these checks: macOS deferred capture and animation completion until unlock.
Native desktop automation remained unavailable, so physical key delivery and the
experimental bundle's first macOS permission prompt were not automated.

### 0.6.0 voice state, effective models, and windowed collapse

A native voice call launched from an empty draft now retains ownership after
that draft receives its real chat ID. The call's live caption updates the shared
run status, including Listening, Thinking and Speaking. Busy text input is
reported separately as `composerBusy`; it no longer makes an interactive voice
conversation claim to be read only. Historical views retain their actual input
restriction. The voice header also identifies the realtime model used by the
same request.

Kai, Code and Quick Chat display effective model IDs and reasoning effort from
native runtime reports, project-scoped Codex config, native catalog defaults,
Claude's resolved aliases, ACP model/config updates, and configured provider
settings. Explicit model overrides remain separate: resolving a CLI default does
not pin a model or change the user's settings. The model menu identifies its
resolved default even when another model is selected. Configured global effort
continues to apply when changing models; native per-model effort is used when
there is no configured override. Explicit Claude effort is sent through its
native `--effort` option, and Codex effort reaches both thread config and turns.

Gemini's installed default is resolved from its settings or the installed build's
own selected default constant. An Auto router remains its actual Auto ID until
the provider reports a concrete selection; no Flash/Pro model is inferred from
catalog order. This discovery does not authenticate or send a prompt. Native
runtime metadata updates the selected identity when a chat runs.

The continuous workspace contour now renders above the menu bar during a morph,
then restores the window's previous level on completion or interruption. A resized
workspace therefore collapses into the screen's physical notch instead of ending
at a clipped shape underneath it. The packaged double-Command probe now resizes
the real workspace before dismissal. The animation timing/choreography is kept.

Verification: 736 Code tests passed (12 opt-in tests excluded), 189 Swift tests
(one optional audio fixture skipped), and the production native view harness.
The harness asserts live voice header text, actual model/effort settings labels,
composer-busy semantics, resized-window contour position above the menu bar, and
restoration of the original window level. Read-only installed metadata checks
resolved Codex's configured model/effort, Claude's concrete default alias, and
Gemini's installed Auto selection. A generated-audio packaged owner probe is
available with `--unified-smoke-test --unified-voice-probe`,
`KAI_CODE_VOICE_DIAGNOSTIC=1`, and a bounded JSON fixture at
`KAI_UNIFIED_VOICE_FIXTURE`; it verifies the actual draft-to-live voice ownership,
transcripts, header state, resolved model/effort, unchanged default selection,
and native stop in one process. It sends no ambient microphone audio.

Final 0.6.0 verification repeated the full Code suite serially: 736 passed and
12 opt-in tests excluded; the 98 benchmark-binary tests also passed with one
opt-in exclusion. The shared idle-service pool allows concurrent fixture tests
to evict each other's completed services, so the release regression run uses
`--test-threads=1`. The refreshed model-picker fixture carries its actual project
scope. Swift again passed 189 tests with one optional fixture skipped, and the
native view harness passed again.

Installed native metadata also confirms Cursor's combined `(current, default)`
marker resolves to `auto`, and Grok resolves to `grok-4.6`. OpenCode and Antigravity
may omit a pre-run default when there is no configured model. Their published
runtime model identity updates the shared display; unpublished routing decisions
and effort levels are never fabricated. Cursor metadata uses its native model
catalog without creating an extra ACP session.

The latest packaged live voice retry stalled while macOS reported both the
screen locked and the display asleep. It is not recorded as a successful live
0.6.0 verification. The generated-audio probe and resized-window packaged morph
probe are ready for an unlocked display. Native state and geometry assertions
passed independently. Installation preserves the previous experimental bundle
and does not terminate the running user application or its agents.

### 0.6.1 native workspace-to-notch handoff

Foreground Control–Option now retires the workspace into the compact notch
contour, then opens the actual mini interface using Kai's existing notch morph.
Previously `setOpen(mini: true)` ran before the workspace collapse: the mini
animation played while its panel was hidden, Code's contour ended at the mini
chat's height, and the completed mini panel appeared abruptly at handoff.
The workspace now keeps its full-size frame throughout retirement. Mini chat
begins only after the workspace is hidden, so its live native mask and content
animate together. Preparing the hidden notch also leaves the shared pet in
Code's coordinates until it is returned to the notch. Double-Command's existing
dismissal path and background-workspace overlays are unchanged.

The production native view harness passed with new assertions for the compact
collapse target, unchanged Code window frame, preserved shared-pet frame, hidden
mini during retirement, visible original mini mask animation after handoff, and
preserved chat draft. The signed packaged smoke probe passed on an awake display:
continuous reversal, workspace working-area sizing, shared pet, repeat open,
extended-notch return, resized-window double-Command, and exclusive Quick Chat.
The 0.6.1 (9) installation retains a 0.6.0 rollback bundle; its signature and all
three executable/library hashes were checked. No running user process was stopped.
Native desktop automation remains unavailable (Sky service startup failure), so
this does not establish physical key delivery or subjective visual inspection.

The separate previously pending generated-audio voice probe was retried while
awake. WebKit permission, WebRTC connection and first-prompt transmission passed,
but the native voice session closed with reason `requested` before the full
transcript assertions completed. It is not counted as a passing voice regression;
no cause is inferred from that reason alone and this release changes no voice
runtime behavior.

### 0.6.2 direct extended-notch swap and uninterrupted shortcut morph

This supersedes 0.6.1's extra compact-notch stop. Control–Option now targets the
extended notch's full rectangle directly with the existing continuous workspace
contour. Code's real content fades away over 140 ms; after 160 ms, while that
same contour is still shrinking, the actual native mini chat runs Kai's original
280 ms notch morph above the glass. Code is never resized or projected onto the
mini surface. Both complete within the original 600 ms collapse. The incoming
panel temporarily sits above the retiring workspace, then regains its original
level. Reversals cancel the pending handoff, restore Code's opacity, and retire
the incoming panel. Interrupting that handoff with another collapse keeps Code's
content hidden. Ordinary double-Command keeps its existing dismissal path.

Notch opening now commits layout and the shape animation together, installs the
animation before ordering its panel front, and uses an explicit start time with
backwards fill. Same-size layout refreshes preserve the active shape animation
and its original start time; they no longer remove it and expose the final mask.
The original curve and duration are retained for shortcut expansion.

Verification passed: 189 Swift tests (one optional audio fixture skipped), the
native view harness and the production modifier harness. New native assertions
check a direct extended-size target, no Code pixels at the visible notch handoff,
overlapping native/outer animations, unchanged workspace frame, restored levels
and opacity after reversal, no delayed second expansion, and preserved draft.
The actual Control–Option handler retains the original morph through a layout
notification and owner refresh without restarting it. The signed packaged smoke
probe passed reversal, direct notch return, shared pet, repeated open, resized
window double-Command and exclusive Quick Chat. Installation of 0.6.2 (10)
verified the signature and executable/library hashes and retained a 0.6.1 backup.
Running user chats were not stopped. Physical desktop input and subjective visual
review remain outside these checks because native desktop automation is unavailable.

### 0.6.3 restore native text unjumbling in the extended notch

The native renderer no longer skips text reveal merely because Code owns its
state. Streaming assistant suffixes use the existing bounded Kai reveal engine.
Opening the extended notch explicitly replays its visible text, including an
unchanged cached chat and the direct Code-to-notch handoff. The opening reveal
is prepared after the actual panel is ordered front, while the same atomic
shape/layout transaction is still active. The underlying transcript stays exact
for selection, copying, links and accessibility; temporary glyph tiles settle
back to native rendering. Shared Text Morph preferences and Reduce Motion still
control the effect. The large Code view keeps its own existing decode effect.

Six focused text-animation tests passed, covering changed suffixes, Unicode,
viewport budgets, settings, bounded snapshots and cleanup. The native view
harness passed with assertions for visible tiles on new text, reopening the
same chat, the overlapping Code handoff, and exact settled native text. Its
text scenario explicitly closes Quick Chat first, preserving exclusive surfaces.

Opt-in self-tests now omit physical keyboard/mouse monitors and invoke the real
handlers directly, preventing simultaneous user input from arming another voice
or notch shortcut in the test process. Normal launches retain those monitors.
This does not establish physical shortcut delivery. Two earlier packaged runs
left a visible surface after the simulated double-Command check; the final
isolated packaged retry ended with SIGTERM before completion. No successful full
packaged smoke run is claimed for this release, and no cause is inferred from
that termination. The native text regression passed independently.

The installed 0.6.3 (11) bundle's signature and executable/library hashes were
verified through a clean staging copy. Previous bundles are retained, and no
running user chat or agent was stopped. Relaunch to load the new native library.


### 0.7.0 restore independent launches and add native chat timing

Agent startup now belongs to the submitted chat, including checkpoint preparation,
cancellation, native maintenance operations and the notch task snapshot. Navigating
to another chat or a new draft no longer carries the first chat's busy controls.
Returning while a checkpoint is being prepared preserves the submitted prompt;
a late history load cannot erase the active prompt or attach its response to the
wrong conversation. Asynchronous sends retain their original explicit model
selection even if the composer changes while a handoff is preparing.

The owned Codex App Server reads its effective project configuration before
starting or resuming a thread. Built-in OpenAI threads select an app-local
first-party HTTPS/SSE provider with native OpenAI authentication and WebSockets
disabled. This avoids waiting for WebSocket retries before ordinary agent work.
Configured custom providers retain their native routing. Realtime voice retains
its separate WebRTC media transport. No global Codex configuration or credentials
are edited. The supported provider fields are defined in
[Codex's provider configuration](https://github.com/openai/codex/blob/main/codex-rs/model-provider-info/src/lib.rs).
Startup acknowledgments are bounded; the deadline does not limit an acknowledged
coding turn, sandbox command, compaction or review.

Existing Codex chats recover the actual model and reasoning effort from their
latest native turn-context record, read on the background engine with a bounded
tail. User prose cannot supply that metadata. New drafts can show the configured
provider default; existing chats no longer substitute it for their own model.
Explicit reasoning choices belong to the chat and model, and resumed requests
preserve the native model and effort unless the user chooses another value.

New submissions record their actual submit time, first output, completion and
monotonic elapsed duration. The composer and notch show live elapsed work; the
transcript retains a small monospace completed/stopped/failed duration per turn.
Horizontal swipes over the transcript reveal local message times and release
fades them away without changing the message layout. Up to 1,024 timing records
survive restart. Historical imported turns without recorded timing are not given
invented times or durations. Voice retains its live call clock; voice speaker rows
do not inherit the preceding coding turn's timestamp or duration.

The opt-in `--unified-agent-probe` exercises the actual composer send handler,
background checkpoint preparation, independent new draft, two native edit turns
(new and resumed), the native model report, and persisted durations in its own
`/private/tmp/kai-agent-ui-*` workspace. With generated speech provided through
`KAI_AGENT_VOICE_FIXTURE` and ambient microphone disabled by
`KAI_CODE_VOICE_DIAGNOSTIC=1`, it also invokes the workspace voice button handler,
waits for both real speaker transcripts, and stops the call. It requires isolated
`--unified-smoke-test` state and never uses a user project or chat.


Verification for 0.7.0: the full Code suite passed 743 tests (12 opt-in tests
ignored), with another 99 benchmark-library tests passing (one opt-in test
ignored). The native state harness and production voice shortcut harness passed.
The signed packaged animation probe passed morph reversal, maximum working-area
sizing, shared pet, repeated open, direct extended-notch return, double-Command
and exclusive Quick Chat. Timestamp verification dispatches a horizontal scroll
through the actual rendered transcript list and checks the real duration footer.
The tests invoke production UI handlers; physical keyboard/trackpad delivery and
subjective visual review are separate from these automated checks.

The final signed 0.7.0 release passed the live agent/voice probe in 32.4 seconds:
the new chat created the proof file, the resumed chat appended the requested
second line, both native model reporting and on-disk timing records were
verified, and the workspace voice button exchanged real user/assistant speech
transcripts before a clean stop. Generated speech was used; ambient microphone
capture was disabled. The final model-menu refinement additionally passed all
20 unified-interface regressions. Installation retained the prior 0.6.3 bundle,
verified the new signature and executable/library hashes, and left the user's
running process untouched. Quit and relaunch Kai Unified to load 0.7.0 (12).


### 0.7.1 automatically restore status over other applications

The native app/window activation handler now shows the compact status notch as
soon as the expanded Code workspace loses focus. It retains the same workspace
window, frame, selected chat and draft in the background; the nonactivating,
click-through status panel does not claim keyboard focus. Returning to Code hides
the overlay and returns the shared pet to its workspace slot. Repeated focus
notifications leave an explicitly opened extended notch intact. Idle/completion
updates cannot let an older auto-compact timer hide the background status panel.
Quick Chat remains exclusive, and background approvals keep their native review
routing.

Workspace reveal completion now reconciles focus again, covering an app switch
that happened during the morph. An inactive application's animation completion
no longer orders its workspace front. The direct extended-notch and compact
collapse animations retain their existing contours and timing.


Verification: 189 Swift tests ran with one optional test skipped and no failures.
The native state harness passed automatic background status via both application
and workspace activation notifications, repeated return/leave, nonactivating
panel ordering, preserved workspace geometry/draft, idle persistence, and focus
loss during reveal, alongside its existing approval and handoff checks. Its
foreground provider is controlled for deterministic focus tests. The final signed
package passed the existing morph/double-Command/Quick Chat smoke probe. Installed
0.7.1 (13), verified its signature and executable/library hashes, and retained the
0.7.0 bundle without stopping the user's running app. Quit and relaunch to load
the updated native library.

### 0.8.0 clean agent history, reasoning, model controls and status transitions

Imported tool envelopes are decoded before native code-fence continuation metadata.
Large tool output retains its exact source, including literal backslashes and code
fences, inside bounded, animated disclosures instead of becoming escaped Markdown
in the chat. Historical tool inputs retain a neutral Details label; authoritative
native success/failure cards keep their actual status. File-change events render
paths and literal diffs. Attachment-prefixed prompts use the actual request for
newly discovered titles. Owned temporary verification projects are omitted from
the daily project/session lists without deleting their native records.

Codex reasoning summary fragments update one stable card per native item, grouped
by summary section. The authoritative completed item replaces streamed text;
interrupted summaries retire as stopped/failed. The original ordered interaction
log supports replacement after spilling, and completed updates return to its
bounded storage. The card provides a readable preview, animated disclosure and
copying. Imported native reasoning summaries use the same presentation. These are
provider-supplied readable summaries, never generated approximations or decrypted
private reasoning.

First-party Codex turns explicitly request detailed summaries, following the
[T3 Code adapter](https://github.com/pingdotgg/t3code/blob/main/apps/server/src/orchestration-v2/Adapters/CodexAdapterV2.ts).
The [official App Server contract](https://learn.chatgpt.com/docs/app-server)
defines summary deltas and authoritative completed items. Existing authentication,
HTTPS routing, approvals and coding model choices are retained. Custom provider
requests preserve their own configuration. An experimental concurrent-summary
flag was evaluated and removed after it did not improve delivery.

The model picker adds local catalog search, separates the active model from a
selection for the next turn, and keeps pending effort choices visible. Native
notch menus prioritize the selected model's effort catalog over the current
run's model. Continuing a chat preserves its reported native model/effort; new
drafts resolve the configured provider setting. Selecting a model preserves the
last actual report and the current draft.

Background status is activity-driven. Idle Code does not leave a permanent
notch. Work, voice and approvals animate the compact notch out of its original
physical contour, accompanied by one short, nonactivating contour echo from the
retained workspace. Returning to Code uses the original notch-to-workspace morph.
Results settle briefly before retiring, and new work during that closing animation
reverses the dismissal. Repeated focus notifications do not restart animations.
Explicit mini chat and Quick Chat retain their own surface ownership. The contour
echo is released after half a second and respects Reduce Motion; no workspace
snapshot, extra agent service or persistent animation loop is introduced.

The direct workspace-to-extended-notch handoff also explicitly retires Code's
content fade before revealing Kai's native UI. A delayed render-server frame
cannot leave miniature workspace pixels on the extended-notch surface. Native
settlement checks poll for the actual completion callback with a bounded deadline;
the tests still inspect the overlapping morph and original text animation.

Live reasoning verification with the installed Codex 0.164.0 and existing ChatGPT
subscription succeeded on GPT-5.6 Terra. GPT-6.1 Sol returned empty summary items
with explicit detailed requests through both the direct HTTPS and built-in OpenAI
routes. The RPC audit confirmed the requested settings and empty incoming summary
items; Kai does not substitute another model or invent thinking text. The opt-in
native reasoning test intentionally fails when the service emits no readable
summary, keeping that limitation visible in verification rather than hiding it
behind a fallback. The public viewer still handles summaries whenever supplied.

Verification for 0.8.0: 849 Rust tests passed across the application and support
library (15 opt-in tests ignored); the Swift suite ran 189 tests with one skipped
and no failures. A read-only replay of the screenshot's actual native history
decoded 117 typed tool/interaction cards without escaped envelopes. The native
state harness passed activity-only status, no focus theft, retained workspace,
return/reversal morphs, delayed-frame pixel retirement, original text unjumbling,
approval ownership, selected chat/draft continuity, Quick Chat and image-only
submission. The separate production modifier-handler harness passed voice holds,
silence cancellation, context/pre-roll ownership and Double-Command dismissal.

The signed app's real UI probe created and resumed separate native coding turns,
edited an isolated fixture, received readable reasoning, reported the native
model, retained per-chat startup state and persisted both working durations.
Its real workspace voice-button path connected, exchanged both speaker transcripts
using generated diagnostic audio, and stopped cleanly (48.4 seconds total).
Native desktop automation could not start (Sky Computer Use service startup
failure), so these are production native-handler and signed-app checks rather
than a claim of subjective visual or physical-key/microphone review.

The final signed 0.8.0 (14) bundle also passed the packaged morph probe: continuous
reversal, maximized visible working area, shared pet, repeated open, direct
extended-notch return, Double-Command dismissal and exclusive Quick Chat. It was
installed atomically at the existing experimental `.build/Kai Unified.app`
destination, with signatures and all executable/library hashes verified. The
prior 0.7.1 bundle is retained as `Kai Unified 0.7.1 backup 20261026-162120.app`.
No running app or agent was stopped. Quit and relaunch the experimental app after
ongoing runs finish to load the update.

### 0.8.1 existing saved replies and native tool summaries

The 0.8.0 native-import fix missed large replies already saved in Kai's SQLite
store. Its 64 KiB byte pages could split an app-owned interaction envelope,
exposing escaped JSON/diff fragments as ordinary Markdown. Separately, the native
notch adapter flattened status details into an italic activity label.

The sparse local-history index now records app-owned card boundaries on disk.
Each physical slice reconstructs a complete typed fragment, with one decoded
card cached during paging. Source strings, escaped backslashes, Unicode and
fenced literal examples retain their content. Valid legacy file-change envelopes
render paths and literal diffs. The original SQLite messages and native history
are unchanged. Version-one saved viewports replay their original source
checkpoint through the corrected pager, retaining their selected chat/page;
version-two snapshots retain the normal direct restoration path.

The native notch exports concise status labels and bounded prose previews.
Clickable activity titles open their exact details in the shared Code workspace
through the existing morph. The command checks both chat ownership and a
content-derived card identifier before expanding and scrolling to the row.
It preserves drafts and rejects stale card links.

Verification: the full Code suite passed 751 tests (15 ignored); the subsequently
added legacy-viewport migration regression also passed. The Swift suite ran 190
tests with one skipped and no failures. A read-only copy of the user's exact
874,101-byte saved reply passed every page with 273 typed cards and no escaped
envelope spill. Its actual Code snapshot exported 100 compact native-notch rows;
the native AppKit harness rendered those rows, verified clean visible text,
clicked the exact detail link and entered the original workspace morph.
Synthetic large Unicode/escaped diffs retain every source byte across pages;
fenced examples stay literal. The new regressions are in `store.rs`, `engine.rs`,
`unified.rs`, `ConversationRendererTests.swift` and `SavedHistoryHarness.swift`.

The migration regression also removes the new boundary footer from an older
page's sparse index, then verifies source replay restores the same typed page
instead of its deliberately stale cached fragment. The final signed 0.8.1 (15)
bundle passed the packaged morph/reversal/Double-Command/Quick Chat probe and was
installed at the existing experimental destination. Signatures and all three
executable/library hashes match the tested bundle. Backup:
`Kai Unified 0.8.0 backup 20261026-170402.app`. The running app and agents were left
alone; relaunch to load this update and reopen the same existing chat.

### 0.8.2 voice lifetime and surface consistency

Surface dismissal no longer sends a voice-stop command. Double-Command, folding
Code into the extended notch, and hiding Quick Chat retain the shared call;
the voice button and the configured voice hold remain explicit hang-up actions.
Compact status follows live work/voice ownership and retires after that ownership
ends. Voice's shutdown tail cannot reopen the coding-completion popout; ordinary
coding completion keeps its existing behavior. Quick Chat dismissal restores
compact live status without creating a new voice session.

Workspace focus reconciliation records actual foreground boundaries. Arming a
voice shortcut over an already foreground Code window does not replay Code's
entrance on subsequent owner updates. The selected chat, window and unsent draft
are retained. Existing-chat voice can start while its draft remains unsent.
A dispatched voice request keeps its real acknowledgement deadline when its UI
is dismissed. Its connecting indicator and the input listening/thinking morphs
retain their animation across repeated state updates.

The new-chat content wrapper supplies the flex column needed by the centered
welcome. A native GPUI geometry regression verifies centering in the full content
area at two window sizes.

Verification includes 753 passing Code tests (16 opt-in tests ignored), the
190-test Swift suite with one skip and no failures, and the native state harness
for focus, approval routing, retained window/chat/draft, morph reversal and Quick
Chat ownership. The production modifier harness covers all voice actions on
Command/Option/Fn, silent release, pre-roll/context retention, foreground startup
with an existing draft, no repeated workspace entrance, UI voice start, direct
extended-notch handoff, Double-Command without hang-up, explicit hang-up with no
ghost provider indicator, delayed acknowledgement after dismissal and stable
thinking animation through repeated owner updates.

The final native suite ran 190 tests with one skip and no failures. The signed
0.8.2 (16) candidate is installed at the existing experimental destination;
signatures and all executable/library hashes were verified. The prior build is
retained as `Kai Unified 0.8.1 backup 20261026-173638.app`. No running app or agent
was stopped.

Live verification remains pending desktop unlock. macOS reported
`IOConsoleLocked = Yes`; WebKit reported a visible but occluded host and deferred
its microphone request until the startup deadline. The signed foreground-window
probe also cannot assume Code is frontmost while the console is locked. Neither
attempt is recorded as a pass. After unlock, rerun both the packaged morph probe
and `--unified-voice-probe` in isolated smoke-test mode. The latter retains and
checks the same voice session/owner through workspace expansion, Double-Command,
extended-notch return and Quick Chat dismissal, then asks that session to speak
again and requires new incoming WebRTC audio bytes before explicit hang-up. It
also checks that hang-up leaves no ghost notch. Its media remains diagnostic
generated audio; ambient microphone tracks are disabled.

## 0.8.3 — automatic voice handoff and explicit workspace access

The workspace voice button and Kai’s voice shortcuts now share one routing path.
New voice chats always use Codex, even when the empty composer has Gemini or
another provider selected. An existing non-Codex conversation transfers through
Code’s transactional handoff: its local chat ID, project and execution directory
stay the same, while its original native session and history remain preserved.
The bounded saved conversation packet is persisted into a real destination
Codex turn before realtime starts. Captured screen context and images travel
with that preparation; the held recording remains attached to the same media
session. Ordinary dictation remains text input for the selected agent.

The voice call captures the destination model, effort and permissions at start.
Context acknowledgements cannot launch a different chat, and stopped calls reject
late media callbacks. Early voice requests resolve the Codex executable directly
through the normal provider-discovery function instead of waiting for project
and saved-history discovery. Recordings arriving during microphone permission,
after document startup, or after activation are retained and played once. Muting
during playback remains in effect when live microphone input resumes.

The composer’s mode menu now separates **workspace access** from **command
approvals**, displays the actual execution directory, and offers Codex’s real
**Full access** mode. Full access permits filesystem and network access outside
the workspace. **Never ask** changes approval prompts only; it does not change
the selected workspace boundary. Select Full access and Never ask together for
runs without a Codex sandbox or command approval prompts. Read-only intent always retains a
read-only sandbox, including when a previous Full access preference exists.

Access is persisted independently and captured in each invocation. It reaches
new/resumed native threads, subsequent turns, terminal commands, voice tools and
team launches. Queued prompts retain their access choice and are held for review
if access changes before dispatch. Permission changes affect the next invocation;
ongoing work retains its captured policy. The application does not rewrite the
user’s global Codex configuration. Approval requests continue using the existing
opaque per-run token and focus-aware Code/notch review surfaces.

Verification: 756 Code tests passed with no failures (17 opt-in checks ignored in
the ordinary suite), and all 14 WebRTC media regressions passed. The installed
Codex command/exec check ran separately: read-only blocked writes, workspace
access permitted only the project write, and Full access permitted the separate
outside-project write, without raising an approval prompt. Native state and
production shortcut harnesses passed, including focus-aware exact-token reviews,
all Command/Option/Fn voice actions, silence, held audio/context, and continuity.
The actual Code UI also created and resumed a real Codex agent, edited only its
disposable project, reported the native model and saved both turn durations.

The final signed bundle passed all four generated-speech routes: saved Gemini
chat and new Gemini-selected draft, each through the shortcut command and the
workspace voice button. Existing-chat replies recalled the transferred “cobalt
blue” fact. Every route preserved its single voice identity across workspace,
Double-Command, extended notch and Quick Chat transitions, received new WebRTC
audio afterward, and closed its media on explicit stop. The teardown check waits
for authoritative backing coding work to finish before checking notch retirement;
real remaining work keeps its status. No stale notch remained after quiescence.
The separate signed morph check passed reversal, visible working-area bounds,
shared pet, repeat opening, extended return, Double-Command and exclusive Quick
Chat. These unlocked-desktop results complete the live checks left pending in
0.8.2. Ambient microphone audio was disabled in these generated-speech runs.

Installed experimental build: **0.8.3 (17)**. The prior bundle is retained as
`Kai Unified 0.8.2 backup 20261026-185027.app`. Installation verified the strict
bundle signature and all three executable/library hashes. Running apps and
agents were not stopped; relaunch the experimental app to load this build.

## 0.8.4 — unified composer and native model intensity controls

The extended-notch composer has separate editing and control rows, neutral glass,
a symbol-only permission control, a model-family glyph, a compact animated effort
preview, and a native waveform voice button. The permission symbol transitions
from white for read-only, through white/orange for workspace edits, to white/red
for Full access. Its menu exposes workspace access and approvals independently,
shows the actual working directory, and explains that changes affect subsequent
invocations. Its owner token prevents a stale menu from changing another chat.
The Command keycap and shortcut labels include **⌘**.

Four original SVG glyphs identify OpenAI model families in both interfaces and
model menus: Luna/moon, Terra/earth, Sol/sun, Astra/star. Other models keep their
provider identity. Known family labels are formatted readably while native IDs
remain unchanged in requests and saved settings.

Both interfaces use a discrete thinking slider with provider/model-specific
stops. Its thumb and fill slide between stops while a shared blue, violet, pink,
and warm-red palette interpolates intensity. The native control supports dragging,
keyboard arrows, and accessibility increment/decrement; the Code control supports
dragging and keyboard navigation. Animations retire after their short transition
and respect Reduce Motion. Existing drafts and model selections survive changes.

The slider reads Codex model capabilities, Claude SDK effort levels, ACP thinking
selectors, and OpenCode variants. ACP configuration uses exact advertised selector
IDs and confirms the resulting effort before a prompt is sent. Modern config-only
ACP catalogs are supported alongside legacy model/mode fields. OpenCode sends the
selected native variant without changing its approval flow. Plugin-free OpenCode
metadata and prompt-free ACP metadata provide capabilities for fresh selections;
these probes are bounded, cached, and do not send model prompts or change global
provider configuration. Unknown or unsupported effort choices are not invented.
Selection remains scoped to each chat/model, and native live capabilities take
precedence over older cached catalogs.

Verification: 767 Rust tests passed (17 opt-in tests ignored); the Swift suite
ran 190 tests with one skip and no failures; all 14 WebRTC media tests passed.
Native composer, state, and production modifier-shortcut harnesses passed,
including exact effort dispatch, stale-owner rejection, narrow layouts, typed
draft controls, and independent workspace/approval settings. The signed build
passed its morph check and live generated-speech tests for an existing Gemini
chat through the workspace voice button and a new Gemini-selected draft through
the shortcut. Both kept one voice session and fresh incoming audio across
workspace, notch, Double-Command, and Quick Chat transitions, then retired after
explicit stop. New and resumed Codex agents edited only their disposable project
and persisted actual model and duration. These checks used Codex authentication;
provider-specific ACP/OpenCode effort exchanges were verified with native wire
fixtures, and the installed OpenCode verbose catalog was checked separately.

Installed experimental build: **0.8.4 (18)**. The previous bundle is retained as
`Kai Unified 0.8.3 backup 20261026-223343.app`. Strict signature and all three
executable/library hashes match the tested build. Running apps and agents were
not stopped; relaunch the experimental app to load this version.

## 0.8.5 — bottom conversation alignment and shared Code wallpaper

Short extended-notch conversations now sit above the composer. A native text
origin offset disappears as history grows beyond the viewport, preserving normal
scrolling, selection, links, and text reveal. A separate reproduced Code bug pinned
long first submissions to their first line; submissions now anchor to their tail.
The notch host, composer, and conversation accept the first click while another
application is active. The native window test sends actual mouse events to focus
the editor and dispatch New.

**Appearance → Code wallpaper at bottom** replaces the glass with the current
Code wallpaper, effect, and empty/chat visibility. **Bottom appearance → Fade
height** controls its upward feather. The existing background worker produces the
effect once and retains a secondary unfaded raster capped at 512 pixels per side.
The in-process ABI passes these pixels directly to a native layer; no extra
decoder, web view, background renderer, or playback timer is introduced. GIF
playback stays active for the visible notch and stops when neither surface needs
it. Project changes, empty-only visibility, removal, and reduced motion use the
existing background policy. Glass and wallpaper are mutually exclusive choices.

Verification: 770 Rust tests passed with 17 opt-in tests ignored; the Swift suite
ran 191 tests with one skip and no failures. All 26 wallpaper tests passed with
one opt-in test ignored, including exact shared effect pixels, transparency, and
bounded native rasters. Wallpaper-enabled native composer/input checks passed;
the bottom-alignment changes also passed two-minute idle audits before wallpaper
sharing was added. The final signed morph probe confirmed that real Code wallpaper
pixels reach the extended notch during the workspace-to-notch handoff.

The reported intermittent freeze remains unresolved. Live samples taken while
the user reported it frozen showed the main event loop processing timers, and
Accessibility trust was enabled. A first-click rejection was verified and fixed;
this has not established the full cause of the reported menu/notch freeze. The
new live generated-speech check failed before connection with “Microphone
disconnected” both with wallpaper enabled and disabled. The voice media library
is byte-identical to 0.8.4; the current failure is still open and is not counted
as a passing voice verification. Earlier successful live checks remain recorded
under their actual release versions.

Installed and reopened **0.8.5 (19)** through LaunchServices, with Code wallpaper
enabled. The prior bundle is retained as `Kai Unified 0.8.4 backup
20261026-234612.app`. Strict bundle signature and all three executable/library
hashes were verified. No active agent subprocesses were present in the previous
observed instance when it exited.

## 0.8.6 — native tool questions and commands at the caret

Tool elicitation forms no longer go through the approval-only Yes/No route.
Single boolean questions send their exact question ID and true/false answer;
other forms open Kai's native answer controls. Empty confirmation forms retain
their Continue control. Cancel answers the pending request without stopping the
agent; Stop remains an explicit separate action. Background owner and invocation
tokens remain mandatory, with stale and duplicate responses rejected. The
compact command preview also uses the available height when no status is shown.

Slash completion follows the caret inside a sentence and retains surrounding
Unicode text, existing suffixes, and undo. Choosing a local control executes that
control and preserves the remaining draft. Native CLI entries complete into the
prompt without discarding its context. Paths and URLs remain ordinary text.
Quick Chat now mirrors caret and IME state as well as the draft and enables the
same keyboard navigation. The extended notch displays the same provider-scoped
catalog inside its existing window and sends command choices through the shared
picker, including UTF-16 caret validation, owner guards, draft conflict checks,
and explicit acknowledgement before adopting the resulting draft.

Verification: the full Rust suite passed 775 tests with 17 opt-in tests
ignored; the additional final native-notch command bridge regression passed
separately. The Swift suite ran 194 tests with one skip and no failures. These
checks include tool form typing, request cancellation, duplicate/stale rejection,
inline completion, draft preservation, Quick Chat keyboard input, and undo.
The real native-window check exercised Unicode-caret command dispatch, remaining
draft acknowledgement, and a background tool's typed No answer. Its wallpaper-
enabled idle audit processed 579 status deliveries over 120 seconds without
losing its surface. The signed animation/shortcut probe also passed.

Both signed live voice routes passed with generated speech and muted ambient
input: an existing Gemini chat using the app button, and a new Gemini draft using
the shortcut. The existing-chat route verified the transferred cobalt-blue fact.
Both produced user/assistant transcripts and new incoming audio after switching
through the workspace, extended/compact notch, double-Command and Quick Chat,
retained one session identity, and ended through explicit native Stop.

The intermittent freeze is still not established as fixed. The latest read-only
live sample showed the normal event loop rather than a deadlock. An isolated
generated-speech media test connected, exchanged both transcripts, and received
audio without the earlier microphone-disconnection error, but its older appshot
context assertion did not pass. That older diagnostic is not counted as a passing
context check; the separate signed product probes above did pass. The earlier
intermittent microphone failure was not reproduced in these runs, and its cause
has not been established.

Installed **0.8.6 (20)** with a timestamped 0.8.5 backup, strict signature
verification, and matching executable/library hashes. The running app was kept
open because it hosts the active conversation; the update loads on relaunch.

## 0.8.7 — local libraries and one continuous mini workspace

**Appearance → Fonts** discovers the Mac's installed font families on demand,
including Font Book additions such as Google Sans and Geist. Interface and code
fonts are shared with Code's existing personalization settings, persist through
the same owner, and repaint the notch without replacing its draft or viewport.
Code's font picker now searches the complete, virtualized installed-family list.

Pets are discovered from `~/.codex/pets` and
`~/Library/Application Support/Kai Unified/Pets`. **Pet → Add pet folder…** selects
another local library; **Rescan local pets** reloads edits without rebuilding the
app. Valid v1/v2 manifests load their actual external sprites, replacing packaged
copies of installed custom pets while keeping built-in pets. Selection migrates
from the previous packaged ID when a local manifest has a different ID. Scanning,
palette extraction, and image decoding are bounded and run off the UI thread.

Code's configured mini-window shortcut, including Command–\`, now participates
in the same continuous contour transition as the workspace and notch. The real
mini window paints at its final size, the outgoing UI retires before the native
notch appears, and the one shared pet moves between headers. Closing mini Code
returns to the originating surface; Control–Option returns directly to extended
chat, and double-Command dismisses into compact status only while needed. A
background workspace remains open. Interrupted transitions preserve the current
contour and restore window levels, input, masks, and opacity. Reduced Motion
uses a short fade. Stale queued dismissals cannot close a newly reopened mini
window, and the consumed Command–\` chord cannot become voice or double-Command.
Voice input uses the visible mini window as its media host. Foreground mini Code
uses its own request tray; approvals still route to Kai while it is backgrounded.

Held stop shortcuts also honor their key-event deadline when a busy main queue
processes key release before the timer. A stop armed for a call that has already
ended cannot accidentally start a replacement call. Deliberately starting voice
from mini Code remains separate from consuming its window shortcut.

The historical OS crash report identified eager `AVAudioPlayerNode` construction
during speech-player initialization. Audio nodes are now created only for actual
PCM playback and released afterward. This does not establish the cause of the
reported intermittent freeze. A bounded, sleep-aware main-loop watchdog now
captures up to three local stack samples per process after a sustained stall;
Rust panic diagnostics retain a bounded location/backtrace file without the
panic payload. Evidence is stored under the experimental app's `Diagnostics`
folder. No ongoing chat content is logged by these diagnostics.

Native state, presentation, and media-host callbacks now release Rust's owner
borrow before entering AppKit, which can synchronously reenter Code on focus or
window changes. Shared immutable state bytes remain alive across that reentry;
native delivery shares the cached allocation. A regression replaces the cache from inside the
native callback and verifies both reentry and byte lifetime; it passed, alongside
24 owner-bridge tests with one opt-in test ignored. This removes a concrete
borrow-panic risk; it does not prove the cause of the reported freeze.

Verification: the full Rust suite passed 778 tests with 17 opt-in tests ignored;
the final mini-Code regressions passed 13 tests separately. The Swift suite ran
197 tests with one skip and no failures, including actual Google Sans/Geist
resolution and six local pet manifests. The native modifier handler passed all
voice destinations and context variants, silence discard, blocked-timer release,
double-Command, Command–\` chords, and deliberate voice from mini Code. The native
morph matrix passed all four surfaces, eight rapid reversal sequences, Reduced
Motion, shared pet/media host, restored input/window levels, retained background
Code, and controlled foreground/background approval events. Actual OS focus could
not be acquired in the automation session; that manual focus check remains open.
The final wallpaper-enabled native composer audit ran for 120 seconds and 572 owner
deliveries, including live font repaint and external pet image sources.

Signed product checks passed the real morph and both generated-speech routes:
an existing Gemini context handed off through the mini-Code voice button and a
new Gemini draft launched through the voice shortcut. Each received real
user/assistant transcripts and fresh audio after the surface sequence, preserved
one call identity, and stopped through explicit Stop. The transferred cobalt-blue
fact was verified on the existing-chat route. Ambient input was disabled in these
tests. These checks do not establish that every reported intermittent hang has
been fixed.

Installed **0.8.7 (21)** after the final signed morph and both live voice routes
passed. Strict signature and all three executable/library hashes were verified.
The previous bundle is `Kai Unified 0.8.6 backup 20261027-172534.app`. The running
app was retained to preserve the active conversation; relaunch after this reply
to load the update.

## 0.8.8 — one glass backdrop during mini/full Code handoffs

Mini Code and full Code previously each animated a separate glass shell during
their handoff. The overlapping blur looked doubled until both animations settled.
The incoming surface now owns the single backdrop; the outgoing surface retains
its contour and content fade without creating another blur layer. An interrupted
mini entrance also retires its old backdrop immediately when full Code starts.
The same ownership rule applies in reverse. Notch transitions retain their
existing glass and animation. The native morph matrix checks that at most one
transition backdrop is visible throughout the handoffs and rapid reversals.

The overlap matrix and signed product animation probe passed. Installed
**0.8.8 (22)** with strict signature/hash verification and a timestamped 0.8.7
backup. The running app was retained; relaunch to load the glass fix.

## 0.8.9 — live voice text with a short suffix unjumble

Realtime transcript deltas now update one native voice message in the ordinary
run transcript before the segment completes. Its final recognition replaces the
same interaction, including corrections, shortening, or retracted text. Replayed
canonical events do not duplicate the segment; legacy flat events retain a
separate identity for each utterance. Finalized segments release their live keys.
The existing owner path carries these updates into full Code, mini Code, and Kai.

Assistant voice rows use a 160 ms paint-only unjumble on newly arriving text.
Earlier words stay resolved when another delta arrives. User speech remains
readable immediately. The existing text-animation preference and Reduced Motion
policy still apply; UTF-8 lengths and citation offsets remain unchanged.

Verification: 23 voice regressions and all three text-decode tests passed,
including partial-before-final delivery, Unicode corrections, retractions,
duplicate prevention, and stable earlier words. The signed animation probe and
both real generated-speech voice routes passed. Installed **0.8.9 (23)** with
strict signature/hash verification and a timestamped 0.8.8 backup. The running
conversation was retained; relaunch to load the update.

## 0.9.0 — conversational transcript and unified voice rendering

User messages align right; assistant prose aligns left without enclosing every
activity in a card. Commands group into a compact disclosure, and completed
turns fold intermediate work into a reopenable summary above the final answer.
Active runs retain earlier work for on-demand paging. Mini Code uses its own
virtual transcript list rather than showing only the last eight rows.

Written output and its spoken relay render as one answer in both live and saved
history. Suppressing a redundant relay also preserves Markdown split by that
event. Independent voice-only replies remain visible. Live user voice and
dictation use the short suffix unjumble without changing draft content, caret,
selection, or undo; motion preferences still apply.

Live history snapshots use a read-only worker channel and bounded resident
pages. They do not steer or restart the running agent. Chat switches and run
epochs guard asynchronous results against entering another conversation.

Verification: 792 Rust tests passed (17 ignored), and 198 Swift tests completed
with one skipped and no failures. The native composer probe passed actual mouse
focus, mid-sentence commands, Unicode caret handling, synchronized drafts,
background tool responses, model controls, and permission controls.

Explicit Code folds now bypass the global shortcut's background-overlay choice.
This prevents a focus change or an interrupted reveal from leaving the workspace
open when Code requested a fold. Global shortcuts still preserve the existing
background workspace when opening the notch over another app.

The native morph matrix and signed animation probe passed after this routing
fix. Both signed generated-speech voice routes passed: existing Gemini context
through the mini Code voice button, and a new-chat voice shortcut starting in
Codex. Each retained the live session and incoming audio across surface changes
and verified explicit stop. Release **0.9.0 (24)** retains the running app until
the user relaunches.

## 0.9.1 — thinking shimmer, provider usage, and Fast Mode

Thinking controls show the current level in a colored chip. Hover opens a custom
glass panel of provider-advertised choices. Each choice uses MonoCode's 32×5
deterministic tile mask, distance stagger, and three 1.4-second fill pulses, with
a five-second fade and its own intensity palette. The native implementation
uses Core Animation; Code paints only the hovered effect. Reduced Motion keeps
a static highlight. No background animation timer runs for a closed picker.
The source reference is `hardbeat920/monocode` at
`26465c620977f3951d0a532a0f7db32c3c179007`; its MIT license is bundled.

An animated bolt enables the exact fast service tier advertised by the selected
Codex model, including `priority` on the installed CLI. The same setting reaches
new/resumed turns and voice requests. It persists across relaunches, leaves
permissions and reasoning independent, and is absent for unsupported providers
and models.

Full Code, mini Code, and the extended notch expose a compact usage footer.
Quota windows are sorted by remaining capacity, with bars showing remaining
amounts. The glass details include reported account/plan, quota resets, context
usage, refresh, and Codex banked resets where supplied. Reset redemption uses
the native idempotent RPC and validates the current credit before dispatch.
Codex quotas come from App Server. Claude OAuth, Gemini Code Assist, and OpenCode
Go quotas use their own signed-in provider endpoints; credentials stay in the
worker, never in logs, arguments, or UI. Other harnesses retain their actual
native context-usage reporting; their context capacity is labeled as context,
never presented as an account allowance or borrowed from Codex.

Codex's explicit `final_answer` phase now marks the complete final response.
Completed commentary folds above that response even for a turn with no tools.
The extended notch can reopen and fold work in place. Retained source and lazy
history paging keep earlier work accessible without rendering every past item.

Release checks exposed an inactive-window GPUI key-focus deadlock. Native morphs
now request app activation first and assign key focus only while the application
is active; the contour can appear immediately without waiting for focus. The
native transition matrix guards this ordering, including rapid reversals and
background workspace retention.

Opening mini Code during a workspace collapse now captures the logical retiring
state rather than treating its still-visible window as the return destination.
The incoming surface retires the outgoing glass backdrop, and the old fold
completion cannot steal focus from mini Code. The native matrix exercises this
overlap and verifies return to the extended notch.

Verification: the full Rust run passed 794 tests; its one outdated picker-close
assertion was corrected and all five composer regressions passed on rerun.
All 15 account-control tests and three provider-quota parsing tests passed.
The native composer checks exercised the custom hover/usage panels, real bolt
dispatch, thinking selection, stale-owner rejection, and narrow layouts. The
final native morph matrix, signed animation probe, and both generated-speech
voice routes passed. Installed **0.9.1 (25)** with signature/hash verification
and a timestamped 0.9.0 backup; the running conversation was retained.

## 0.9.2 — reliable completed conversations and inline review

Completed turns preserve each prompt and the full final answer while folding
intermediate work into an expandable summary. Imported Codex final-answer
boundaries survive paging and restored history. Both Code surfaces and the
notch share the projection; list measurements and the visible-history budget
follow the folded rows so earlier answers do not disappear or leave empty gaps.
Pending requests and failed actions remain accessible.

The usage footer is vertically aligned. The selected thinking level animates
on the control and in the glass panel; hovering another choice previews that
choice and leaving returns to the selection. The approval's fourth animated
review glyph expands its actual request inline, with the notch and action row
moving together. Long reviews scroll within the available screen height.

Verification: 807 Rust tests passed (17 ignored), 201 Swift tests completed
with one skipped, and the native composer and approval/state checks passed.
The signed packaged morph probe passed, including reversal, extended-notch
return, double-Command dismissal, and exclusive Quick Chat. Both signed
generated-speech voice handoff and new-chat shortcut checks passed. Installed
**0.9.2 (26)** with a verified 0.9.1 backup, leaving the running app open.

## 0.9.3 — fill folded history across source pages

Large completed responses can occupy an entire byte-limited history page,
excluding the prompt and earlier turns. Folding that page can remove its scroll
extent, preventing the previous scroll-triggered loader from ever retrieving
those messages. Code and mini Code now fill an underfilled viewport from the
existing bounded history cursor. Scrolling near the first visible messages
loads earlier pages even when hundreds of hidden work rows precede them.

Only one page request runs at a time. Automatic requests stop at the same cursor
after an I/O failure; a deliberate retry remains available. Earlier source
pages retain lazy hydration and bounded decoded residency.

Verification: 808 application regressions passed (17 ignored). The final three
folded-history checks passed, including a real asynchronous saved-history load
with two responses larger than 600KB each. It recovered both prompts and final
answers without opening Worked, traversed more than eight pages, and retained
at most eight decoded pages. The failure/retry test also exercises a visible
top edge after 302 raw rows.

The signed packaged transition probe passed. Installed **0.9.3 (27)** with
signature and binary-hash verification and a timestamped 0.9.2 backup; the
running conversation was retained.

## 0.9.6 — completion and submissions share one owner across all surfaces

Runtime completion immediately schedules a bounded parse of the immutable final
response source, independent of the persistence/history worker. The retired
live parse can no longer leave old streaming text on screen until a later input.
The result checks chat, generation and response-start identity before publishing;
it never replaces a newer chat or turn. A persisted file-backed history refresh
keeps the visible final in place, then replaces its pages without replaying the
entire conversation entrance or showing an empty loading state.

Code, mini Code and the extended notch call the same submission method. The
notch no longer rejects a valid input using an old busy/read-only snapshot.
Active/finishing work uses the existing follow-up queue, and historical sends
restore the live tail using Code's existing captured-submission path. Pending
loads and provider handoffs are acknowledged as accepted submissions. Exact
owner/draft-base checks still protect edits made on another surface.

The owner-notification handler advances voice/handoff/queue/team work before
exporting its state, so hiding both GPUI windows does not suspend chat progress.
Repeated dismissal of an already hidden mini editor cannot overwrite the shared
draft being edited in Code or the notch. Rendering and reveal effects remain
owned by the visible surface.

Verification: 817 applicable Rust regression cases passed across the full run
and the updated history-commit follow-up check; 17 opt-in cases remain ignored.
205 Swift tests ran with one skipped and no failures. The native composer
harness verifies stale-busy submission and final-text arrival without typing.
The signed isolated app's launch and complete surface-transition probe passed.
Installed **0.9.6 (30)** with signature/hash verification and a timestamped 0.9.5
backup; the current running session is left open for the user to relaunch.

## 0.9.5 — centered typing cursor and a single approval action row

The extended composer's AppKit text cell now centers its drawing and live field
editor in the same text row, correcting the cursor that appeared above the
placeholder. Expanded approval review moves the provider's exact SVG choices to
the existing bottom action row instead of drawing a second set inside the body.
Forms retain their input controls and submit action; Boolean questions retain
direct Yes/No controls. Closing review restores the compact action row without
resolving the request. Provider choice IDs and background owner tokens are preserved.

Verification: 204 Swift tests, one skipped, zero failures. The native composer
harness verifies the focused cursor geometry, a single bounded SVG review row,
exact provider choice dispatch, review close/reopen, and background tool answers.
Installed the signed **0.9.5 (29)** bundle with three-binary hash verification and
a timestamped 0.9.4 backup. The existing running session is left open.

## 0.9.4 — SVG approvals, shared pet identities, and composer alignment

Notch approval choices, review, and inbox cycling use animated SVG controls.
The extended notch retains approve/reject/stop/review actions; opening review
does not claim a submission or consume the pending request. Closing and
reopening the same request restores its selectable command and enabled glyphs.
Presentation updates preserve pending prompt and decision acknowledgements.

The usage footer is centered in the physical gap between the composer and the
bottom of the notch, accounting for the scaled content inset. The editor has
space for wrapped input, and thinking intensity sits immediately after the
model's measured name. Code and mini Code use the notch's circular arrow and
the selected pet's radial palette, retaining their existing send/stop actions.

Subagents retain distinct Kai task pets in the native sidebar, inline activity,
and Code's viewer. Native inline sprites use shared bounded image decoding and
Core Animation; Code animates visible sprites and respects Reduced Motion.
Native parent/child IDs establish ownership across worktrees. Viewer reads and
steering target the selected child's actual workspace. Child IDs reported in
collaboration state maps are observed even without a receiver ID array.
The installed protocol schema and [OpenAI Docs](https://learn.chatgpt.com/docs/app-server)
were checked while reviewing these events.

Verification: 811 Code regressions passed (17 ignored), 202 Swift tests ran
with one skipped and no failures, and the focused acknowledgement test passed.
Native composer checks exercise the physical footer center, model/intensity
spacing, and opening review then returning to the extended notch to answer an
owned question with its SVG. Native state checks exercise distinct inline child
pets and the existing window, approval, and draft continuity matrix.

The native visual check confirmed that both children create visible compositor
layers. The final signed launch/morph probe passed. Installed **0.9.4 (28)** with
signature and three-binary hash verification and a timestamped 0.9.3 backup,
leaving the running conversation open.

## 0.10.0 — shared Fast shimmer, native glass, and Code orchestration

Fast Mode now uses the same finite, distance-staggered tile shimmer as thinking
intensity, in a yellow palette. The native notch uses the same EffortTileLayer;
Code uses the same shimmer painter. Hover and selection restart the burst,
Reduce Motion is respected, and no idle animation timer is introduced.

The workspace backdrop is now an AppKit-owned NSVisualEffectView at full
opacity, with its own tint sibling beneath GPUI. MonoCode's MIT filter handling
preserves native Gaussian blur, replaces copies when the radius changes,
removes native saturation/coloration, and reapplies after AppKit material
updates. The rectangular WindowServer blur and 1% material alpha are removed.
GPUI keeps its Metal renderer and input host. Theme composition accounts for
native tint rather than painting it twice; sidebar and reading surfaces have
different opacity curves. Opaque, sidebar-only and main-only options remain.

Explicit work folding shares the same reversible disclosure state in Code and
native commands. The chevron rotates continuously. The extended notch fades
between its actual visible conversation contents, preserving scroll position;
the temporary, bounded snapshot never receives mouse input and is retired on
completion, cancellation, or a chat switch.

Choose Orchestrate in a new composer or run /orchestrate. This creates a new
Codex orchestrator with a packaged code-orchestrator skill and an opt-in,
client-handled code_workspace dynamic tool. It can page projects and chat
metadata, read bounded saved/native chat context, open projects/chats, create
projects and chats, assign prompts to existing or new worker chats, and wait
for real outcomes. Worker chats run through the existing durable scheduler,
checkpoints, native approvals, history and status paths. Existing providers and
models are preserved. New Codex workers inherit the parent's selected model.

Task permissions are captured from the parent invocation, independently of
later composer settings. Shared-directory edits serialize; the orchestration
parent does not occupy a worker's edit slot. Task receipts are explicitly queued
until actual completion. Returned context is treated as conversation data, and
the skill requires reading worker results and reporting a brief as each task
finishes. Restart holds unfinished tasks rather than replaying prompts. No
external control server, additional renderer or global Codex configuration is
installed. Creating project folders outside the current workspace requires the
existing Full access setting.

Validation for this build:

- Rust: 829 application tests passed (17 opt-in integration tests ignored), plus
  103 benchmark checks passed (2 ignored). Native clipboard/local fixture tests
  ran with macOS services available; large-history checks used four test threads.
- Swift: 206 tests executed, one skipped, zero failures. Native mouse/composer
  harness also verified the shared shimmer and worker completion ownership.
- Signed-app glass/surface probe: one full-opacity backdrop, independent tint,
  radius/opacity toggles, pass-through input, continuous morph reversals,
  extended-notch return, double-Command dismissal and exclusive Quick Chat.
- Authenticated Codex workflow: discovery of two disposable projects, two worker
  chats, independently verified file edits, an existing-chat follow-up using the
  same native thread, saved worker results and the parent completion brief.
  Navigation traces confirmed the parent remained selected before the explicit
  open-chat check. Disposable native test chats were archived afterwards.

Startup restoration now runs in the shared chat owner while Code is hidden, so
opening the workspace cannot replace work begun from the notch. Orchestrated
worker status/completions carry their parent ID; worker completion updates pets
and status without adding an independent automatic chat review.

Live microphone orchestration was not exercised in this pass. Realtime launch
uses the same opt-in tool registry and captured permissions; existing voice
routing/protocol regressions passed. Dictated text uses the ordinary shared
composer submission path.

Installed 0.10.0 (31) at the existing `.build/Kai Unified.app` location. The prior
0.9.6 bundle is preserved as `Kai Unified 0.9.6 backup-20261028-172243.app`.
Code and both native libraries match the signed staging bundle byte-for-byte.
The running user session was left open; relaunch loads the new build.

## 0.10.1 — consistent click-operated controls

Thinking controls in Code and the extended notch open only on click. Hover
retains the existing visual feedback and never opens a panel or changes a
setting. Repeating a trigger click closes its panel; clicking another control
switches panels. Clicking elsewhere, pressing Escape, or changing apps dismisses
the transient controls while preserving the draft and selected settings.

GPUI remembers which picker owned pointer-down before outside-dismiss handlers
run, sharing the same rule across Code and Quick Chat. Native thinking/usage
panels identify their trigger and retire only the outgoing panel after event
dispatch; a queued outside-click cannot close a newly opened replacement.
Native model/permission menus also dismiss the custom panel before tracking.

Validation: seven composer regressions passed, including actual pointer events
with motion enabled/disabled, model/intensity switching, outside dismissal,
window deactivation and draft preservation. The existing dropdown-toggle test
also passed. Native mouse-event checks verified thinking/usage toggles, panel
replacement and clicking back into the editor. Swift ran 206 tests with one
skip and zero failures. The signed release passed the full surface/morph/Quick
Chat smoke probe and all eleven native ABI exports.

Installed 0.10.1 (32); app and native library hashes match the signed staging
bundle. The previous app is preserved as `Kai Unified 0.10.0 backup-20261028-180516.app`.
The running session was preserved; relaunch loads the interaction fix.
