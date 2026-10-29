# Native experience pass — request of October 28, 2026

Baseline: `405e115`, Kai Unified 0.10.1 (32), clean source tree. Work remains on
`v1shay/kai-code-unified`. Preserve the running installed session and back up the
bundle before installing the finished build. Implementation and the checks available on this host are complete; the macOS 26
visual-verification limit is recorded below. No subagent delegation was requested.

## Requested outcomes

- [x] Rework glass toward the supplied MonoCode references: legible native blur,
  restrained surface overlays, consistent typography/spacing and animated controls.
  Pixel-identical visual matching remains unverified; see host limits below.
- [x] Separate classic AppKit glass and real public macOS 26 Liquid Glass modes,
  selectable and persisted. Respect accessibility and older system availability.
- [x] Default launch opens the large Code workspace through the existing morph;
  settings select workspace / extended notch / compact notch on later launches.
- [x] Measure fresh-process presentation and discovery separately on a warm
  filesystem; improve startup,
  chat/tab switching, resource use and build iteration with evidence.
- [x] Terminal can be dragged and docked left/right/top/bottom, resized and saved.
- [x] Comprehensive simple keyboard commands: Ctrl-letter, Ctrl-Shift-letter or
  Cmd-Shift-letter; preserve normal editing, IME, terminal input and Kai gestures.
  Discoverable command list/help, no awkward modifier combinations.
- [x] Animated project folding and conversation-sidebar folding, including rapid
  reversal and Reduced Motion. Fix UI issues found in these paths.
- [x] Small actual context-window usage control, independent of quota estimates.
- [x] Real subagent events appear reliably; individual pet/color/task/transcript
  tabs in the child viewer; compact-notch cycling; nested small pets in floating
  parent-pet groups. Preserve owner-scoped approvals and stop/steer routing.
- [x] Generate semantic chat titles without altering the user's conversation;
  animate placeholder-to-generated names; manual rename wins; bounded work.
- [x] Orchestrator discovers actual provider/model catalogs and can explicitly
  choose provider, model and thinking effort for new workers across providers.
- [x] Test new features and connected flows, inspect actual native UI, diagnose
  hangs/crashes, package/sign/install with backup and commit the finished changes.

## Reference evidence and constraints

- Existing full checkout: `.build/reference-monocode`, reference commit `26465c6`.
- Current upstream inspected using GitHub connector, pinned commit:
  `fc01ae25afefa4d591eda2d4b03558b99f8ab2d8`.
  Selected sources saved under `.build/monocode-current/`. MIT license is already
  retained in `integrations/Code/LICENSES/Monocode.txt`; retain attribution for ports.
- Supplied five screenshots are in the user's latest message. They show classic
  blur, crisp foregrounds, low-opacity reading panes and quiet inset borders.
- Current upstream GPUI theme uses a 0.28 glass pane and transparent inset panes;
  our current main pane composes to ~0.70 at default opacity, plus reading cards.
- Current Mac: macOS 14.2, Xcode SDK 14.5. No macOS 26 connected test host established.
- Apple NSGlassEffectView docs: use real `contentView`, not an arbitrary sibling.
  NSGlassEffectContainerView batches nearby glass. New classes must be availability
  checked; never link an unavailable selector unconditionally on macOS 14.
  https://developer.apple.com/documentation/appkit/nsglasseffectview
  https://developer.apple.com/videos/play/wwdc2025/310/
- User also asks to inspect creator screenshots on X. Creator discovered as
  `@nikolay_dp`; direct X access returned HTTP 403. Supplied screenshots and the
  pinned open-source implementation were used as references.

## Useful implementation map

- Glass/theme: `integrations/Code/src/native_backdrop.rs`, `native.rs`, `theme.rs`,
  `theme_controls.rs`, `personalization_ui.rs`, native `ComposerGlassPopover.swift`.
- Startup: `main.rs` boot, `unified.rs::attach`, `workspace_state.rs`,
  `workspace_ui.rs`, native `EmbeddedCodeRuntime.swift`, `App.swift`, WorkspaceMorph.
- Dock: `terminal_ui.rs`, main conversation layout near terminal_visible render,
  workspace state. PTY already isolated, visible-cell rendering.
- Keys: main `actions!`, `bind_app_keys`, on_action handlers, `input.rs` bindings,
  Quick Chat action handlers; native App shortcut system must stay compatible.
- Children: `runtime/codex_server.rs` thread/started and collab events,
  `runtime/broker*`, `broker_ui.rs`, `task_pets.rs`, `unified.rs`, native
  FloatingTaskPets/TaskPetAssignments/App. Existing child paths and tests already
  exist; reproduce missing live events before replacing working behavior.
- Titles: current upstream `monocode-core/src/session_title.rs` and
  GPUI `conversation/title.rs`: bounded separate generation, compare-and-replace
  persistence; never append title prompts to the user's chat.
- Orchestrator: `orchestrator.rs` already supports installed providers but no
  explicit model/effort arguments or catalog action; teams scheduler captures
  permissions and serializes overlapping edits. Keep this normal runtime.
- Existing tests: Rust test target `.build/unified-test-cargo` in the original
  checkout; Swift `.build/feature-tests`; `scripts/test-unified-native.sh`.
  Build/sign `scripts/build-unified-app.sh`. Live probes use copied unique app IDs,
  `--unified-smoke-test` isolated data, exact test-chat cleanup via native archive.

## Progress / validation

Implemented and installed as Kai Unified 0.11.0 (33):

- Native classic / public Liquid Glass preference, native host reparenting and
  independent tint; lighter role-specific pane fills; native dark/light material.
- Workspace-first launch plus persisted three-surface startup preference, without
  waiting for discovery or provider metadata. Opt-in monotonic launch metrics.
- Four-edge terminal drag/dock, resizing, persisted dimensions; same live Entity
  retained. Three real drag/resize tests pass, including minimum chat space.
- One 30-binding command catalog shared by GPUI, native notch and settings help.
  Terminal Ctrl input and marked text remain local. Native event/draft tests pass.
- Real accordion group geometry with virtualized child rows, finite folding and
  reversible conversation sidebar. Fresh project branches reuse metadata briefly.
- Independent context-window ring from actual used/limit telemetry in both UIs;
  native ring/unknown-state checks pass separately from quota values.
- Exact provider/model/effort validation and captured durable worker settings;
  list_models reads catalogs and selected-model capabilities off the UI thread.
- Separate, bounded, ephemeral read-only Codex title generation after first reply;
  manual rename/deletion wins. Native conversation history is not used for titles.
- Per-child pet/gradient tabs, native nickname/model/effort/workspace/scope,
  nested floating child pets and active-only compact pet cycling.
- Critical live discovery finding: current installed Codex emits subAgentActivity
  with agentThreadId even when no spawn collabAgentToolCall is emitted. That path
  now registers/subscribes children; a bounded initial item tail covers late
  subscription. Parent-thread relation is retained, including nested children.
- Current internal Codex child activity can expose scope/model but omit plaintext
  delegated prompt. Show the actual scope; retain the full prompt when supplied
  by collabAgentToolCall. Do not fabricate the hidden assignment text.
- Coalesced native status publication (one per display interval); command replies
  remain immediate. Completed child retention prunes old terminal records rather
  than permanently exhausting the live registry after 128 historical children.
- Content-addressed Swift library build caching; final release stays optimized.

## Final validation

- Rust: **845 passed, zero failures, 19 deliberately ignored live/system tests**.
  Command: `cargo test --offline --manifest-path integrations/Code/Cargo.toml --bin orbit`.
  Native pasteboard and loopback fixture servers require normal OS access; the
  execution sandbox blocks those facilities. Final run used that access.
- Swift: **208 tests, one skipped, zero failures**. Includes 10 children sharing
  two native pet windows, bounded visible companions, family paging and lifecycle.
- Native composer: actual key events, draft/context preservation, independent
  context ring, owner-scoped approvals, click dismissal, typography and controls.
- Native morph matrix: workspace/compact/extended/Quick Chat, Control–Option,
  double-Command, rapid reversal, Reduced Motion, retained background workspace,
  shared chat/restored input, and native window levels.
- Final signed isolated bundle: classic material/tint/opacity/hit testing and
  working-area geometry; all three startup selections; exclusive Quick Chat;
  continuous morph reversal. All passed.
- Real Codex child: exact parent, actual model/scope, live full output, completion
  and verified archive with the isolated parent. Completed in 31.3 seconds.
- Real ephemeral title: selected an available `gpt-5.6-luna`, generated the
  requested title in 4.02 seconds without modifying user conversation history.
- Provider/model/effort orchestration: exact catalog validation and captured
  worker configuration pass protocol/fixture tests across supported adapters.
  This is not a claim of a live authenticated run on every external provider.
- Source whitespace check and both Python helper syntax checks passed.

## Measurements

Single-host measurements, fresh isolated app processes with a warm filesystem;
not cold OS caches or an all-provider inference benchmark:

| Measurement | Result |
| --- | --- |
| Default workspace first GPUI frame | 249–341 ms in final signed probes |
| Native owner ready | 211–307 ms in workspace probes |
| Workspace discovery ready | 1.04–1.06 s in workspace probes |
| Idle app CPU, motion enabled, process CPU-time delta | 1.71% of one core |
| Idle process resident memory | 219.2 MiB |
| Idle process threads | about 12 |
| Repackage/re-sign with all source inputs cached | 0.854 s |
| Actual store sidebar, 6,000 summaries | 6.39 ms |
| Actual title/FTS searches | 2.12 / 2.11 ms |
| Batch discovery insert, 10,000 sessions | 31.97 ms |

The performance probe exposed a real idle loop (~24–29% CPU): hidden session and
transcript reveals kept the welcome logo busy. The repaired visibility lifecycle
measured 1.71% with the original animations enabled. A new GPUI regression checks
that hidden reveals let the logo settle, then still animate when made visible.
Completed-child eviction also now protects ancestors of retained descendants;
its regression fills and rotates the registry beyond its 128-entry limit.

The database timings are state-engine workloads only: they do not measure
on-screen tab latency, scrolling FPS, startup, network latency or agent inference.
A changed optimized Rust build still takes minutes; the sub-second number is an
unchanged-input cached build. Incremental debug development is available through
`KAI_UNIFIED_CONFIGURATION=debug`.

Reproducible packaged probes: `python3 scripts/test-unified-packaged.py <mode>`,
where mode is `glass`, `startup-workspace`, `startup-extended`, `startup-compact`,
or `performance`. Raw reports are under `.build/validation/`.

## Installation and verification limits

Installed signed bundle:
`/Users/vishayagarwal/Documents/ChatGPT/notchdex/.build/Kai Unified.app`.
Backup:
`/Users/vishayagarwal/Documents/ChatGPT/notchdex/.build/Kai Unified-0.10.1-32-backup-20261028.app`.
The running 0.10.1 process was preserved; relaunching loads the new bundle.
No active user chat, voice call or provider process was stopped for installation.

The real public Liquid Glass path is implemented and availability-gated, but this
macOS 14 host cannot execute it. Its visual appearance still requires macOS 26.
Native desktop automation repeatedly failed to start (Sky Computer Use service);
behavior/geometry/materials were verified through native integration probes, not
pixel-identical screenshot comparison. This release does not claim proof that
arbitrary future workloads cannot crash or that every external provider is online.

## 0.11.2 — adaptive composer follow-up

Replaced the wrapping composer row with one measured shared implementation for
Code and Quick Chat. Permission/voice labels compact first, followed by optional
mode labels; smaller layouts move secondary actions into the click-only overflow
menu. Model access stays present, and microphone/send remain one fixed trailing
group. Width, label opacity and glyph details reverse smoothly through a finite
240 ms transition. The overflow trigger remains available while its menu is open.
No timer, process or background polling loop was added.

Verified after compiler contention ended: 28 composer checks, all 15 project-tree
checks, and the hidden-reveal/large-history regressions pass. The broader suite
exposed wall-clock-sensitive assertions under concurrent release compilation;
the accordion geometry test now paints an explicit test-only 25 ms frame. The
other load-sensitive checks passed when rerun sequentially. This is not a claim
that every broad run was failure-free.

Signed 0.11.2 startup and native glass/morph probes passed. The fresh-process
workspace frame measured 288–340 ms; the idle probe measured 1.02% of one core,
219.1 MiB resident memory and about 12 threads with motion enabled. Installed
build 35 with the 0.11.1 (34) bundle backed up; the running user session was kept.
