# Native workflows

These controls run in the Rust/GPUI application. They reuse each CLI's authentication, native session identity and request/approval bridge. Installing a CLI is not evidence that its account can perform paid inference; real provider errors remain visible.

## Prompt changes and timeline

Every new normal prompt receives a Code-owned timeline record. Workspace-edit prompts capture a bounded before/after snapshot; read-only prompts record their policy without copying the checkout or claiming file changes can be undone. Click Completed to open the native diff inspector, or use the clock control for the chat timeline. Completed includes Undo/Redo for eligible snapshots; inspect changed paths and confirm before restoring. Legacy responses show recorded provider changes or an explicitly labeled current-workspace fallback. Existing conversations from before snapshots were enabled cannot acquire retroactive before-images.

File restoration rejects conflicting current contents, modes, substituted roots and symlink traversal. It waits for owned agents and checkpoint finalization, and is blocked while the local terminal remains active. Only included local workspace files are covered: external services, published changes, ignored caches, detached processes and other outside effects are not reversed. See [checkpoint implementation and recovery](CHECKPOINTS.md).

## Agent selection and status

Choosing an agent stages the next message without starting a handoff or interrupting current work. Send captures the message and settings, prepares bounded prior context on a worker, and launches a single destination invocation with both context and instruction in the same visible chat. New typing during preparation stays in the draft. Historical labels use their recorded provider or the chat binding, while each live run retains its launch provider independently of the composer selection. Selecting a different destination cannot rename earlier response headers or relabel fresh output from an already-running agent.

Status quotas show percentage left, readable provider names and separate reported reset times. Missing values are hidden; staging a provider does not relabel another provider's quota or context.

## Mixed-provider teams

Use the team control or `/team`; assign a concrete task to two through four installed agents. Each task retains its provider, model, native requests, responses and chat identity. Open a task to read its normal transcript and answer its real native approval or question. Stop cancels starting and active members. Dismiss removes the completed team panel while preserving its chats.

Teams default to read-only. Read-only tasks can overlap, with at most four agents starting/running across the app. Tasks that edit an overlapping checkout serialize; this is not worktree isolation. A failed or stopped member holds pending members for explicit review/resume. Restored work is held, never silently resubmitted. A previously attempted task without a recovered native session is not automatically started again.

This is explicit user task assignment and progress collection. Automatic task dependencies, an autonomous coordinator, isolated competing implementations and automatic merge are not implemented. Separately, owned Codex/OpenCode native children can continue after their parent turn finishes; their live controls and lifetime limits are described in [native services](NATIVE-SERVICES.md).

## Attach another chat

Use the chat-context control or `/attach-chat`. Search indexed conversations across projects and providers, then attach a bounded snapshot to the current draft. Chips identify and remove attachments. The target agent receives quoted context with source/project attribution; it receives no new permission to modify the source project. The original conversation remains unchanged.

Each draft accepts up to four excerpts, each at most 24 KiB. The excerpt retains the objective and recent tail and marks omitted earlier material. It is a snapshot, not an automatically refreshed subscription or a transfer of provider hidden reasoning. Excerpts stay with their originating draft if loading finishes after switching chats.

## Follow-up queue

Enter with a nonempty draft while its chat is running queues the next prompt. Empty Send remains Stop. `/steer` remains an explicit native immediate instruction where supported. Queue entries retain provider, model, policy, normalized image paths and attached-chat excerpts. They can be edited, reordered, removed or held/resumed.

Dispatch waits for authoritative completion, process cleanup and checkpoint finalization. It operates on the foreground chat and does not replace new typing. Provider changes, failed image restoration, failures, Stop and restart hold affected work for review. A queued workspace-edit request never silently escalates a composer subsequently switched to read-only. The queue is bounded to 16 entries with an 8 KiB text limit per entry. All workflow metadata shares an encoded persistence budget; additions beyond it preserve the existing work and draft.

Before launch, team/queue intent is flushed to local storage; native success removes the queued entry. A crash between the external provider accepting work and the local acknowledgement cannot be made transactionally atomic with that provider. Restart therefore requires explicit review rather than automatic re-billing.

## Terminal, image paste and dictation

The terminal control or `/shell` creates a real native PTY dock only when requested. It preserves its shell and bounded scrollback when hidden, suspends hidden viewport work and closes owned processes explicitly. It uses ordinary local shell permissions. Codex `/terminal <command>` remains a separate provider sandbox operation. See [terminal behavior and limits](NATIVE-TERMINAL.md).

Paste an image into the composer using Cmd-V, or use the existing drag/drop and attachment controls. Native screenshot shortcuts are supported through PNG/TIFF pasteboard representations, including uncompressed Retina TIFF captures up to 64 MiB before compression. Clipboard decoding/resizing runs on the existing bounded worker path; normalized images remain at most 2048 pixels per side and retain the 16 MiB per-image and 32 MiB prompt budgets. File uploads retain their existing source limit. A native regression uses a private test pasteboard and an over-16-MiB TIFF to exercise the real Cmd-V composer flow without replacing the user's clipboard. The native fixture needs access to the macOS pasteboard service. Image preparation and thumbnail decoding run outside rendering. Queue restoration reads only managed normalized image files and rejects substituted paths. Ordinary text fields retain normal text paste.

The microphone uses Apple Speech dictation, with pause/task restart handling, retained final words and multiline tail following without selecting the dictated text. It starts only after explicit microphone activation. Automated state-machine and input tests do not establish acoustic recognition quality or OS permission behavior on a signed application; test a long spoken paragraph, a pause, another paragraph and Stop on the actual Mac. Conversational voice, spoken replies and Codex realtime voice transport are not implemented by this workflow.

## Efficiency and evidence

No browser renderer or JavaScript service was added. The terminal engine runs on demand; transcript/context access stays paged and bounded; snapshots stream and deduplicate file contents with cached stat signatures; serialization and durable writes run on workers; animations stop after their transitions. Snapshotting an uncached large checkout still performs real disk work. These choices bound work; they do not establish a measured frame-rate, energy, memory or zero-bug guarantee.

The original 36-family comparison is a roadmap, not a completed parity claim. [Coverage](FEATURE-COVERAGE.md) and the [remaining-family checklist](ROADMAP-36.md) distinguish implemented controls from remaining editor, service, provider and cloud work.

## Tool-card disclosure motion

Expandable tool/status cards use one monotonic height curve for painting and direction reversal. The unpadded outer clip shrinks while the bounded inner scroll viewport keeps its dimensions and scroll position, preventing collapse from reaching zero and rebounding or reflowing padding at the end. Rounded card clipping contains hover backgrounds. The transition retains the existing 320 ms lifetime and reduced-motion behavior; it adds no polling or persistent layout cache.

The two user recordings were inspected for the collapse jitter. Verification: four focused interaction tests plus 91 native chat UI tests passed, as did strict all-target Clippy, formatting and whitespace checks. The collapse regression covers short details, long scrolled output, sampled frame-by-frame height/stack movement, exact final geometry and animation cleanup. Logs: `.build/collapse-regression-tests.log`, `.build/collapse-ui-tests.log`, `.build/collapse-clippy.log`. The test accounts for GPUI 0.2.2 retaining stale debug-selector entries by checking current row geometry and reveal state instead of selector absence.

## Internal record framing

Versioned `orbit_interaction` JSON payloads are opaque to the Markdown inline-code scanner. Unmatched backticks in terminal output cannot pair with a later command's backticks and hide native record boundaries. Opening and closing markers still respect surrounding literal inline/fenced examples and ordinary JSON. Payload contents are preserved for the normal typed tool/status card; no blanket tag stripping or transcript rewrite is performed. Raw history pages are parsed again when loaded, so existing conversations use the correction after reopening. The additional payload-range scan is linear and stays on the existing bounded parsing workers.

Verification: the synthetic backtick regression fails before the change and passes afterward. All 37 presentation tests pass, including literal examples, partial live envelopes, Unicode and bounded chunks. A read-only audit of the reported saved response recovers all 163 encoded interaction blocks. No private transcript fixture is committed. The optional audit accepts a local UTF-8 export with `CODE_TRANSCRIPT_AUDIT_FILE` and runs `cargo test --bin bench presentation::tests::recorded_internal_interaction_transcript_audit -- --ignored --exact --nocapture`. Evidence: `.build/interaction-leak-red.log`, `.build/interaction-leak-parser-tests.log`, `.build/interaction-leak-recorded-audit.log`. The 92 native chat UI tests pass on the isolated rerun (`.build/interaction-leak-ui-recheck.log`). The initial concurrent-compiler UI run hit the delegation fixture's five-second startup deadline; that fixture passed alone and in the full rerun.
