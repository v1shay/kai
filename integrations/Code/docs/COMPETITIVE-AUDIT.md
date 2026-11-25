# Code: cross-product feature audit

Reviewed October 15, 2026. This inventory combines primary product documentation, public source, installed protocol contracts, and the actual Rust implementation. It is deliberately broader than the examples in the feature requests. It is not an exhaustive inventory of every app in existence, a security certification, or a claim of perfect parity.

**Evidence levels matter:** a provider offering a feature, Code inheriting its configuration, Code rendering its output, Code exposing a working native control, and a successful authenticated end-to-end run are separate things. No account entitlement, cloud service, credential, or completion is invented when it is unavailable.

## Primary sources

| Product / surface | Sources consulted | Distinct areas compared |
|---|---|---|
| ChatGPT desktop / Work | [Feature index](https://learn.chatgpt.com/docs/features), [projects and chats](https://learn.chatgpt.com/docs/projects) | Organization, voice, files, plugins, browser/computer interaction, documents, visualizations, scheduled work, shared Pages |
| Codex | [App Server protocol](https://learn.chatgpt.com/docs/app-server), [open harness architecture](https://developers.openai.com/blog/codex-as-a-platform), installed generated JSON schemas | Native threads/turns, tool approvals, requests, MCP, skills/hooks/apps, reviews, goals, steering, terminal, notifications and realtime |
| Claude Code | [CLI reference](https://code.claude.com/docs/en/cli-reference), [SDK permissions](https://code.claude.com/docs/en/agent-sdk/permissions), [SDK user input](https://code.claude.com/docs/en/agent-sdk/user-input) | Bidirectional requests, questions, agents, context customization, effort/budgets, checkpoints and automation |
| OpenCode | [Server API](https://opencode.ai/docs/server/), [agents](https://opencode.ai/docs/agents/), [tools](https://opencode.ai/docs/tools/), [commands](https://opencode.ai/docs/commands/) | Native sessions/events, primary/subagents, tool policies, PTYs, commands, diagnostics, revert/share, MCP |
| Gemini CLI | [ACP](https://geminicli.com/docs/cli/acp-mode/), [checkpointing](https://geminicli.com/docs/cli/checkpointing/) | Negotiated modes/models/media, permission options, session replay, extensions/hooks, snapshot restoration |
| Cursor | [ACP](https://cursor.com/docs/cli/acp), [documentation](https://cursor.com/docs), installed ACP handshake and implementation | Native permissions, questions, plans, models, child sessions; editor/cloud capabilities remain distinct |
| T3 Code | [Docs index](https://github.com/pingdotgg/t3code/blob/main/docs/README.md), [composer](https://github.com/pingdotgg/t3code/blob/main/docs/user/composer.md), [thread sidebar](https://github.com/pingdotgg/t3code/blob/main/docs/user/thread-sidebar.md), [outside agents](https://github.com/pingdotgg/t3code/blob/main/docs/user/outside-agents.md) | Durable queues, steer, model options, projectless work, organization, remote access and permission-scoped agent-callable MCP |
| Monocode | [Public source and README](https://github.com/hardbeat920/monocode) | Workspace/session panels, visual hierarchy, glass/wallpaper, file/review/terminal surfaces |
| Zed | [Agent panel](https://zed.dev/docs/ai/agent-panel), [tools](https://zed.dev/docs/ai/tools) | Native editor integration, queued prompts, checkpoints, context references, diagnostics, tool profiles, review hunks |
| Conductor | [Review and merge](https://www.conductor.build/docs/guides/review-and-merge), [checkpoints](https://www.conductor.build/docs/reference/checkpoints) | Isolated workspaces, per-turn changes, inline review, CI/PR readiness and destructive restore semantics |
| Amp | [Threads](https://ampcode.com/docs/threads), [tools](https://ampcode.com/docs/tools) | Cross-thread context, handoff, runner/orb lifecycle, organization and model-based tool routing |
| Aider | [Git integration](https://aider.chat/docs/git.html), [documentation](https://aider.chat/docs/) | Repo map, architect/ask/code modes, automatic commits and undo |
| Pi | [Compaction implementation/reference](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/compaction.md) | Branch navigation, cumulative file context, compaction and extension interception |
| AGY and Grok Build | Installed public help/version snapshots under `.build/cli-reference/`; prior [provider coverage](FEATURE-COVERAGE.md) | Agent definitions, permission/sandbox modes, session history, model options, remote and plugin commands. Complete official documentation/source coverage was not established; these remain explicit evidence gaps |

## Implemented by this pass

All runtime/UI changes are Rust. Existing native process management, lazy transcript pages, private owned response storage, image handling, and platform dictation remain in place.

| Feature found missing | Implementation / entry point | Boundary |
|---|---|---|
| Create a project rather than only open an existing folder | Projects `+` → Create project; choose parent and enter a name | Creates one empty folder on a worker; no overwrite, package installation or automatic Git mutation |
| Rename projects and chats | Dots → Rename; animated glass editor | Local display aliases retain filesystem/native session identity |
| Pin projects/chats | Dots → Pin / Unpin | Durable Code-owned order; recency retained within groups |
| Archive and restore conversations | Dots → Archive / Unarchive; Sessions / Archived toggle | Local organization; source CLI histories remain intact |
| Preserve lifecycle choices through immediate normal Quit | Preferences FIFO flush joins history/workspace shutdown | Forced process/OS termination is outside a graceful-save guarantee |
| Cursor tool approvals and questions beyond headless output | Installed `agent acp` transport, exact native request IDs, shared pinned request UI | Existing CLI authentication/entitlements remain authoritative |
| Cursor plan decisions, modes/model catalog and task/child progress | Negotiated ACP capabilities and provider extension handlers | No implicit workspace trust, sign-in or approval-mode escalation |
| Codex's native reviewer | `/review`, `/review base <branch>`, `/review commit <hash>`, `/review <instructions>` → `review/start` | Inline within the same Code-owned conversation; runs under read-only sandbox; review is model work and can consume normal provider usage |
| Native thread goals and reported usage | `/goal`, `/goal set <objective>`, `/goal pause`, `/goal resume`, `/goal complete`, `/goal clear`, `/goal budget <tokens\|none>` | Real goal RPCs, explicit user origin. No fabricated budgets, automatic repeated turns or implied background scheduler |
| Inspect tool details rather than just tool counts | `/tools` → configured MCP tool names/descriptions/input fields | Bounded, demand-driven metadata; listing is not an independent tool-call permission or complete built-in tool catalog |
| Inspect lifecycle hooks and app access | `/hooks`, `/apps` | Read-only native inventory; does not install/run hooks, sign into apps or write provider settings |
| Hook progress, connected-tool progress and model reroutes | Native typed transcript cards and actual reported-model update | Lifecycle output stays separate from assistant prose; notices do not falsely fail successful work |
| Review/file/writing/plan directives | Bounded parser → glass cards, file-range links, draft details and Copy | Unknown/incomplete directives stay literal; rendering grants no execution authority |
| Native plan and task-list output | Authoritative Codex/Claude/OpenCode checklist states | Unknown/pending/cancelled tasks never become fabricated completed work |
| File paths inside inline code and status-card links | Conservative path classification and existing central citation handler | No parsing-time filesystem reads, scheme execution or guessed provider source mapping |
| Dense rich-output parsing cost | Ordered inline-span sweep instead of repeated whole-span scans | Includes a 10,000-link regression; not a frame-rate or energy benchmark |

Details and tests: [project controls](PROJECT-FEATURE-AUDIT.md), [rendering](RENDER-FEATURE-AUDIT.md), [provider protocols](PROVIDER-FEATURE-AUDIT.md), [complete installed Codex surface](FEATURE-COVERAGE.md).

## Verified baseline retained

Code already has seven provider launch integrations; native Codex/Claude/Gemini/OpenCode protocols; CLI-owned auth; dynamic model choices; same-visible-chat provider handoff; native requests and per-request decisions; secret field protection; bounded streamed output; source-owned lazy history including compressed archives; virtualized lists; file/media previews; image attachments; code/diff inspection; Apple Speech dictation; glass controls; configurable art/effects; finite/reduced-motion animations; and Stop/Quit cleanup. This list describes the implemented surfaces, not every provider's complete feature parity or a guarantee that its account can run inference.

## Original remaining local functionality, prioritized

The queue, single native terminal, explicit mixed-provider teams, cross-project chat context and per-prompt file checkpoints have since been implemented in the next pass. The table below is the original audit scope; the current [36-family status](ROADMAP-36.md) and [native workflow details](NATIVE-WORKFLOWS.md) distinguish those controls from the substantial remaining work.

These are ordinary feature gaps rather than a reason to ask for credentials or turn unsafe defaults on. They need additional implementation and verification; they are not declared done by this pass.

| Priority | Missing / partial family | Concrete work and constraints |
|---|---|---|
| 1 | Persistent native broker and background children | Keep an owned provider service alive across turns, route child output/approvals independently, define idle unloading and restart recovery; current worker lifetime ends with its invocation |
| 1 | Durable follow-up queue | Edit/remove/reorder queued prompts and attachments, hold after failures/restart, dispatch once on authoritative completion, prevent duplicate billing; `/steer` exists for Codex but is not a persistent queue |
| 1 | Full terminal environment | PTY rendering/input/resize, multiple handles, task lifecycle and reconnect; sandbox `/terminal` is a single command rather than a persistent terminal emulator |
| 1 | Full editor and language tools | Conflict-safe file saving/undo, diagnostics, LSP, symbols/references, tests/build tasks, terminal selection context and inline completion; file preview is not an editor |
| 1 | Agent-callable Code workspace/chat tools | Scoped local bridge with project/read/write permissions and stable IDs; UI project controls are not automatically model tools. Unknown dynamic tool requests are safely rejected today |
| 2 | Fine-grained model/run options | Provider-advertised reasoning effort, service tiers, context/turn/output budgets, structured schemas and primary/subagent definitions; retain native defaults unless explicitly changed |
| 2 | Context UI | Native `@` file/folder/symbol/skill/thread picker, file selection ranges, document attachments and large-paste handling without materializing entire histories |
| 2 | Per-turn review and checkpoints | Durable original/revised file snapshots, compare and resolve each finding/hunk, conflict detection with user/concurrent-agent edits. Current Changes view is against Git HEAD, not historical turn snapshots |
| 2 | Git/worktree workflow | Create/remove/switch worktrees, branch/commit context, staging, commit/PR/CI actions, review comments and safe merge. Do not treat a working directory as an isolated environment |
| 2 | Project organization extensions | Scratch projects, multi-root projects, model defaults, templates, drag ordering/groups, bulk selection and local streaming export |
| 2 | Full remaining provider adapters | Grok/AGY interactive requests, complete provider-native history and custom agent settings; Pi/Amp/Aider providers require their own authenticated protocol adapters, not just new SVGs |
| 2 | Local notifications and accessibility | Opt-in native completion/approval notices, keyboard command discovery, VoiceOver semantics and focus audit across all custom controls |
| 3 | Rich artifacts | Editable documents/tables, mathematical layout, diagram rendering, safe previews, artifact versions/export. Source fences remain readable where no safe renderer exists |
| 3 | Configuration management | Skills/hooks/MCP/plugin editing, enable/disable, OAuth and reload with validation/rollback; inspection and CLI-owned reuse do not equal a complete settings manager |
| 3 | Session trees and rollback | Native per-provider fork/revert/branch visualization, source ownership and exact conversation/file checkpoints; do not simulate history rollback by merely hiding messages |

## Features needing a deliberate product/security choice

No remote exposure, paid API subscription, unattended schedule, credential migration, broad tool grant, destructive restore, or cloud publication was enabled as part of an audit. Before implementing these flows, ask for the applicable choice at a concrete reviewable step:

- **Remote/mobile or outside-agent MCP access:** local-only broker vs authenticated network service; permissions, pairing, revocation, retention and hosting need to be chosen.
- **Scheduled/autonomous work and background launch at login:** select wake conditions, spend/permission boundaries, failure behavior and whether the app must remain alive. A native goal setting alone is not authorization to run an indefinite billing loop.
- **Snapshots/revert or source-control publishing:** review exact file/Git changes and destination; concurrent user edits must be detected rather than overwritten.
- **Cloud voice/image/research/document services:** choose actual available API/provider/account, pricing and data routing. CLI access does not establish access to every ChatGPT product service or private conversation history.
- **Browser/computer control and screen context:** explicit OS permissions and visible capture/operation boundaries; never silently collect other apps or browser profiles.
- **Multi-account pooling and shared project memory:** explicit account separation and data scope. Reusing one CLI's auth is not permission to copy credentials or merge unrelated private chats.

## Verification policy

Protocol fixtures must inspect real request methods/IDs/values and UI state, not just string snapshots. Installed initialize/metadata checks can establish negotiation without inference; they do not establish paid tool execution. Full test/Clippy/release results are recorded after source integration. No UI responsiveness, zero-bug, all-app parity or account availability claim follows from an automated test count.


### October 15 integrated result

**584 checks passed** (493 app, 91 parser/benchmark-target), with 10 explicitly opt-in checks excluded. All-target strict Clippy, formatting and whitespace checks passed. Native Cursor handoff preserves the same visible conversation; new project/archive persistence, source fidelity and native review/goal controls have regressions. The desktop control service failed to start, so live visual inspection was not performed; native GPUI paint/layout/input tests were exercised. Account-dependent live model execution and remaining feature gaps retain their separate evidence levels above. Reproducible log names and scope are in [FEATURE-COVERAGE.md](FEATURE-COVERAGE.md).

The release bundle passes strict signature verification and was reopened without interrupting an active agent. The existing release state/parser benchmark completed; workload timings and their measurement limits are recorded in [PERFORMANCE.md](../PERFORMANCE.md). These measurements do not establish all-app parity or interactive frame/energy targets.
