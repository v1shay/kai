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
send, and normal/Ninja owner routing. The native state harness separately exercises
approval actions, background status, history and image-only drafts.

Rust tests exercise the actual shared task owner, Mini Code/menu controller, queued
follow-up submission without cancellation, provider-pinned navigation, host failures,
and a delayed HTTP submission reply arriving after chat navigation. Existing tests
also send two prompts through a fixture provider and wait for native runtime completion.

The release gate now includes the native continuity harness. It bounds Rust fixture
concurrency to two by default because these tests create their own database/runtime
workers. No timeout was increased and no failing test was disabled.

These checks cover scripted workflows, not every real provider or remote device.
