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
- Code's Quick Chat shortcut opens the mini chat. **↗ Notch** in the conversation
  header, **Command–Option–N**, and the **海** menu also return to the notch.
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

`WorkspaceMorph` expands one temporary native glass mask from the visible notch
rectangle into the saved workspace rectangle. Thirty-two fragments share one
snapshot texture and dissolve during the expansion. The real Code window lays
out once at its destination and fades through the glass; its renderer is never
resized per frame. The notch, drop panel, and floating pets are hidden while
Code owns the surface. The same Kai sprite layer moves into the workspace header.

Transitions are finite and reversible. Generation checks cancel stale work;
an independent settling timer prevents an omitted AppKit animation callback
from leaving the surface stuck. Display changes constrain the saved rectangle.
Reduced Motion uses a short fade. Code's existing top-down transcript entrance
is combined with a bounded text decode that settles to the original text and
preserves UTF-8/link byte offsets. Idle text needs no animation frames.

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

Recorded checks on 2026-10-24 for 0.2.0:

- Swift: 178 tests, one optional local-audio test skipped, zero failures.
- Code: 714 checks verified, 11 ignored. The restricted full run passed 707;
  seven clipboard/loopback-server checks passed when rerun with native access.
- Two consecutive notch submissions reached a local Claude-protocol fixture,
  produced native replies, and recorded confirmed successful outcomes.
- Regression coverage includes project-filtered chats, provider model catalogs,
  draft handoff/conflicts, exact run cancellation, background approvals,
  native question forms, long-hold timing, and interrupted morph reversal.
- Release bundle: signature and all eight public native ABI exports verified.
- Packaged 0.2.0 probe: owner handshake, notch-only launch, interrupted reversal,
  only one visible surface, shared pet layer, repeat-open behavior, return to
  notch, and visible compact indicator passed.

Automated verification cannot establish subjective animation quality, actual
microphone/TCC behavior, or every installed provider's live behavior. Those need
an interactive desktop pass. UI automation was unavailable during development;
the native tests, ABI checks, and packaged-app probe are recorded separately
from that remaining visual and live-provider validation.
