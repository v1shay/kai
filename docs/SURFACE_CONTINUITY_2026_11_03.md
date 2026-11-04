# Conversation continuity — Kai Unified 0.12.56 (93)

The desktop, Mini Code, menu bar and native notch continue to use the same Orbit
conversation owner. This change repairs the editors and acknowledgements around
that owner; it does not create another runtime or copy a running task between windows.

## Fixed ownership paths

- Native typing and dictation publish draft edits to the owner. Consecutive writes
  compare against the preceding queued edit instead of an older display snapshot.
  Switching to Mini Code or the menu bar also flushes the native editor.
- A retiring Mini Code editor no longer writes its mirrored text back over newer
  owner input. Its input events already write through while the editor is active.
- Send and slash-command controls use their own recorded request result, including
  results followed by another command. Repeated identical failures release Send
  and retain its draft. Successful sends retain text typed while awaiting the reply.
- Navigation, history, files, provider selection and voice setup also resolve their
  own request results. Unrelated command errors cannot dismiss a pending provider
  picker or strand a loading control. Image sends wait for their own import and
  keep failed imports unsent; navigation cancels an unfinished local image import
  before it can attach files to a different chat.
- A menu-bar shortcut for the already selected provider preserves its current chat,
  instead of selecting whichever other chat has the newest background activity.
- Native Ninja sends wait for the host response before acknowledging success.
  Busy or unavailable hosts return an error while preserving the draft. A late
  submission response cannot navigate away from a subsequently selected chat.
  Successfully submitted saved drafts are retired; orchestrator-enriched wire text
  is compared against the original editor text when clearing the submitted draft.

## Regression coverage

The native continuity harness exercises real AppKit editors and presentation
transitions with a controlled owner transport: rapid edits, intermediate snapshots,
repeated transitions, delayed acknowledgements, repeated failures, new typing during
send, image-import readiness and navigation cancellation, and normal/Ninja owner
routing. The native state harness separately exercises approval actions, background
status, history and image-only drafts.

Rust tests exercise the actual shared task owner, Mini Code/menu controller, queued
follow-up submission without cancellation, provider-pinned navigation, host failures,
and a delayed HTTP submission reply arriving after chat navigation. Existing tests
also send two prompts through a fixture provider and wait for native runtime completion.

The release gate now includes the native continuity and provider-picker harnesses. It bounds Rust fixture
concurrency to two by default because these tests create their own database/runtime
workers. No timeout was increased and no failing test was disabled.

These checks cover scripted workflows, not every real provider or remote device.

## Verified release

Installed version: **0.12.56 (93)**. Source commits: `4579c27` and `804b28c`.

- Broad Rust suite: 990 passed, 27 ignored. After the final Ninja settlement
  cleanup, the focused Ninja suite passed 58 tests with 3 ignored.
- Final Swift suite: 217 passed, 1 optional audio test skipped, no failures.
- Final native continuity, provider picker and state harnesses passed.
- Final signed package passed all eight isolated modes: Ninja chat, menu bar,
  workspace/extended/compact startup, navigation latency, visible idle and
  background idle. All eight image previews loaded in the Ninja chat check.
- Ninja chat frames: median 16.63 ms, p95 17.61 ms. Warm saved-chat switches:
  median 25.13 ms, p95 33.05 ms. Reopening to the next workspace frame:
  median 52.79 ms, p95 80.97 ms (20 cycles). These are measured fixture workflows,
  not provider inference latency or full animation completion times.
- Visible idle: 0.64% of one CPU core from process CPU-time deltas, about 262 MiB
  resident memory. This does not measure battery life or GPU power.

Evidence: `.build/validation/quality-20261103-164757/` contains command logs,
31 passing check records, exact package hashes and the installation record.
An earlier package latency attempt exited without its success marker; it was
rejected, retained in `prior-package-attempts.json`, and not installed. The final
package completed 28 chat switches and all 20 reopen cycles. The first unrestricted
Rust run hit two fixture deadlines; both passed unchanged alone and the complete
suite passed with bounded concurrency. No failing result is treated as success.

The persistent worker is intentionally retained: this update changes the desktop
conversation bridge and UI settlement paths, with no worker migration required.
