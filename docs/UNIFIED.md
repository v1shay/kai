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
