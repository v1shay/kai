# Kai Unified quality pass — 0.12.55 (92)

This pass fixes shared state and navigation defects, reduces work in Ninja rendering,
and adds a repeatable release check. It does not certify that the application has no bugs.

## Shared behavior

- Desktop and native Ninja surfaces now use one task-merge function. A state-only
  approval-to-stopped update takes effect; a delayed running snapshot cannot override
  an already completed history record. Completed summaries preserve the full answer.
- The desktop renderer compares borrowed task records and retains its cached history.
  It no longer clones every full transcript and every profile's summary on every paint.
  Background animation activity follows the selected chat's tasks.
- Ninja message bodies continue to use the normal desktop message-block renderer.
  Native surfaces use the same typed presentation data, with their AppKit renderer.
  This pass does not add another text formatter or special-case the reported examples.
- Project headers select the project in the double-sidebar layout. The unified tree
  retains its folding behavior. Chat and project order retain the documented prompt-recency
  policy, with pins breaking equal-time ties; a stale lifecycle test now checks both cases.
- A completion label remains clickable after its file summary arrives. Diff review,
  undo, redo, and closing the inspector are exercised together.
- Long Ninja titles now yield space to the corner controls at narrow window widths.
  Static Ninja artwork preserves its face and colors instead of using a monochrome mask.
- Removing a local imported pet no longer resurrects its packaged migration copy.
  Runtime pet icons no longer modify the signed application bundle.

## Verification

Run `python3 scripts/verify-unified-quality.py` from the checkout. It runs Rust,
Swift and worker tests, builds a signed release, verifies its signature, checks
approval transport, then runs isolated native application probes sequentially.
It stops on failure and records commands, exit codes, timings, logs and probe JSON
under `.build/validation/quality-<timestamp>/`. `--bundle /absolute/path/App.app`
validates an existing bundle instead of rebuilding. It never installs or restarts
a user's worker. The chat probe now fails if frame p95 exceeds its 34 ms budget.

Final evidence: `.build/validation/quality-20261103-161438/`.

| Check | Result |
| --- | --- |
| Full Rust suite | 986 passed, 27 explicitly opt-in tests ignored |
| Full Swift suite | 216 executed, 1 optional audio test skipped, 0 failures |
| Python inference, worker and installer suites | 26 passed |
| Installed harness checks | 5 passed separately |
| Packaged approval transport | Empty form, false-valued form, rejection and resolution-before-ack passed; max ack 49 ms |
| Packaged native workflows | Chat rendering, menu bar, three startup surfaces, navigation latency, visible idle and background idle passed |
| Inline host images | All 8 PNG/SVG previews loaded |
| Narrow header layout | Controls remain visible at 620, 900 and 1180 px |
| Signing and installation | Atomic bundle replacement; component hashes match; deep/strict signature valid after relaunch; existing worker preserved |

Final package measurements on this Mac (scripted workloads, no build running):

| Measurement | Result |
| --- | --- |
| Ninja chat frame interval | median 16.64 ms, p95 17.55 ms |
| Warm chat switch | p95 23.60 ms, max 25.70 ms; 24 samples |
| Cold fixture chat switch | max 739.44 ms; 4 samples |
| Warm surface reveal | p95 93.83 ms; 20 samples |
| Workspace first frame from launch | 920 ms |
| Visible workspace idle | 4.44% of one CPU core, 267.7 MiB mean RSS |
| Compact background idle | 1.50% of one CPU core, 219.7 MiB mean RSS |

CPU is calculated from process CPU-time deltas over the sampled interval. RSS is
resident memory, not peak physical footprint. These short samples vary with system
load and do not establish a battery-life or competitor performance advantage.

The packaged `ninja-wide.png` and `ninja-compact.png` screenshots were inspected for
inline images, message/action layout, intact static artwork, and visible corner controls.
The glass configuration and existing adaptive-text preference were preserved.

An intermediate run overlapped compilation and exposed a wall-clock-sensitive fold
reversal assertion and a five-second history probe timeout. Those failure logs are
retained in the earlier validation folder. The fold test now uses its existing
controlled elapsed-time hook. The complete suites and unchanged navigation timeout
passed on the final sequential run; no timeout was increased.

Additional installed-harness checks use disposable workspaces and no model inference:
Codex terminal execution, read-only writes being blocked, workspace/full-access and
network boundaries, cancellation reaping the owned command, and Claude's bidirectional
initialization. These are separate from simulated-provider protocol tests.

The initial suites exposed six Rust and three Swift failures. Product defects were
fixed; obsolete UI test control locations and ambiguous tied-timestamp fixtures were
updated to exercise the current interface. The completion click test waits for the
asynchronous file summary before sampling the real control's coordinates. No failing
test was disabled, and no timeout was increased.

## Scope of the evidence

Packaged probes render production widgets in real native windows with isolated data.
Their scripted workloads do not prove every real provider, every machine, long-session
stability, remote screen control, microphone/device switching, or overnight completion.
Opt-in integration tests remain explicitly identified instead of being counted as passes.
Native computer-use automation could not start in this environment; automated layout,
input and lifecycle tests do not substitute for a complete manual usability review.
No comparative speed claim against ChatGPT or MonoCode is made.
