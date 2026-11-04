# One live conversation, four presentations — 0.12.59 (96)

This supersedes the editor-handoff design described in
`SURFACE_CONTINUITY_2026_11_03.md`. Sharing a backend while copying between
independent editors was not sufficient. Kai Unified now presents the original
GPUI window and original Orbit entity in every interactive form.

## Ownership

`conversation_surface::Form` selects workspace, expanded notch, Mini Code, or
menu-bar geometry. The full application and compact presentations render the
same conversation subtree, including personal Ninja conversations. Compact forms
hide outer navigation and inspectors; they do not construct a second editor,
copy a draft, reload history, resume a provider, or transfer a running task.
Opening an inspector expands the original window. The compact sidebar exposes
the same project and Ninja navigation.

The original composer entity, transcript list state, attachments, permission
controls, provider selection, voice session, task handles, and queued follow-ups
remain owned by Orbit. A surface command changes presentation only and does not
consume a prompt or approval acknowledgement. Native shortcuts use that owner's
draft rather than publishing a mirrored editor first.

Swift receives the requested form after the owner applies layout, then changes
only the original NSWindow's geometry and level. Native geometry work is deferred
out of the GPUI borrow to avoid AppKit callback reentrancy. Presentation revisions
reject delayed shows after dismissal. The shared morph restores interrupted
visual state before applying the destination level, so rapid reversals cannot
restore the previous form's window level.

The physical compact notch remains a read-only activity indicator with explicit
approval controls. It observes authoritative task state. It is not another chat
editor. When the conversation is foreground, approvals remain in that same
conversation. Background approvals can appear at the physical notch.

The standalone Code build retains its separate QuickChat fallback. Legacy native
editor helpers remain, but Kai Unified's interactive routes no longer enter those
editors. This is not a
claim that every legacy presentation helper has been deleted.

## Regression coverage

- Rust owner tests keep the exact composer identity, selected owner/model,
  running handle, draft, Ninja history, and pending approval through every form.
  The regular send path still queues a follow-up without cancelling the run.
- Native continuity tests keep the exact NSWindow and content view, forbid
  draft/send/navigation commands during presentation, reject a stale show after
  dismissal, and bound geometry on displays with positive and negative origins.
- The native morph matrix exercises all 16 form pairs, rapid reversals, reduced
  motion, window level and input restoration, the same voice host, background
  approval routing, and menu dismissal on focus loss. This matrix now runs in the
  release quality gate.
- Signed packaged probes assert one GPUI window, the same native window number,
  Orbit entity, editor entity and conversation owner in every form. A separate
  personal Ninja fixture exercises the same forms with a 24-turn conversation
  and eight PNG/SVG artifact downloads through the actual artifact queue.

These are controlled local regression workflows. They do not establish that
every live provider, remote host, accessibility permission or clean Mac works.
No worker migration is required for this presentation change.

## Evidence

Logs, package hashes and screenshots are retained locally in
`.build/validation/one-conversation-20261103/`. Final results and installation are
recorded there. The first surface probe seeded its synthetic draft before startup
restore and was corrected to wait for restore. The first transition matrix found
a wrong window level after rapid reversal; that implementation defect was fixed
in WorkspaceMorph and the matrix passed unchanged.

## Installed result

Installed and relaunched **0.12.59 (96)**. The executable and native library hashes
match the signed candidate. The existing persistent worker remained PID 823.

- Rust: 995 passed, 27 ignored, no failures.
- Swift: 216 executed, one skipped, no failures.
- Native continuity and final morph matrix: passed.
- Final signed package: surface identity, actual menu-bar toggle, and all three
  startup modes passed.
- After the user unlocked the Mac, the installed build passed the final Ninja
  fixture: same owner/editor/window across all four forms, all eight inline
  PNG/SVG previews loaded, and 120 frame samples at **16.63 ms median / 17.64 ms
  p95**. Screenshots of all four forms were saved and the compact views inspected.
  The installed executable/library hashes still match the signed candidate.
- Earlier locked-screen attempts were rejected rather than counted as performance
  evidence. The runner now reports that precondition explicitly. Measurements
  above are from the controlled 24-turn fixture, not every provider workload.

The repeated-click package check also caught a focus race: a status-item click
could temporarily remove key focus while the menu was opening. Toggling now uses
visibility, and both the native regression and final packaged menu test pass.
