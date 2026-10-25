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
  again in the workspace to return to the mini chat. Releasing cancels the hold.
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
  forms, including multiple selections and secure text fields.
- Existing Command/Fn/Option dictation shortcuts remain available. Image drops
  and screen context use Code's attachment import and native submission path.
  The voice button uses Code's single Codex call. Hold-to-voice passes the
  recorded first prompt to that same call; voice screen captures are supplied
  as local image references for the native agent's image-viewing tools.

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
per block. Projects and recent chats are bounded too. The full conversation,
editing, terminal, files, diffs, checkpoints, queues, teams, delegated-child
inspector, and provider settings remain in the real Code workspace. No second
copy of the IDE is embedded into a web view.

The pet list currently follows Code's top-level run handles, including team
member sessions. Nested provider children remain available in Code's native
child inspector; their live permission requests use the shared notch review
path. They do not yet receive individual floating pets. Mini-view back/forward and
explicit per-turn reasoning-effort selection are also not wired to Code yet;
use the workspace navigation and provider configuration for now. These controls
remain available in the standalone Kai target.

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
first while the contour retracts into the notch. Revision checks retire stale
work after interruptions; Reduced Motion uses a short fade. The original
compact-to-extended notch animation remains intact, and its conversation does
not receive the added workspace text overlay.

Idle provider logos transition into Kai's native thinking scene. A done mark is
shown only for a confirmed successful run. Sidebar labels share a consistent
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
