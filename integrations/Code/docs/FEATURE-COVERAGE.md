# Native provider feature coverage

Audited 2026-10-14; updated 2026-10-16. See [cross-product audit](COMPETITIVE-AUDIT.md) for the broader comparison and this pass’s added controls. This is a gap inventory, not a claim of complete parity or zero bugs. An installed CLI feature, a transport feature, an implemented native control, and a live-tested account are different evidence levels. Provider auth, entitlement, operating-system restrictions, and service availability still apply.

## Reproducible sources

Read-only installed `--help` snapshots: `.build/cli-reference/`; actual binaries inspected with `--version`: Codex 0.162.1, Claude Code 2.1.70, Gemini 0.42.0, OpenCode 1.18.3, AGY 1.3.4, Cursor 2026.10.14-941492e, Grok 1.0.41. Codex's generated JSON schema is the authoritative method/field contract for this installation; new CLI releases require rechecking it.

Official references consulted: [Codex App Server](https://learn.chatgpt.com/docs/app-server), [Claude CLI](https://code.claude.com/docs/en/cli-reference), [Gemini headless mode](https://geminicli.com/docs/cli/headless/), [OpenCode server API](https://opencode.ai/docs/server/), [OpenCode commands](https://opencode.ai/docs/commands/), [Cursor parameters](https://cursor.com/docs/cli/reference/parameters). AGY and Grok findings below use their installed help; a Grok official documentation URL did not resolve, so source gaps remain explicit.

## Bridge coverage and remaining work

| Feature family | Current native behavior | Remaining scope / verification |
|---|---|---|
| Prompt, resume, model override | Seven CLI adapters; owned native IDs kept per chat/provider; native Codex forks preserve external ownership | Each provider's options, model effort/service tiers, schema output, budgets and custom agent definitions need independent controls |
| Streaming and completion | Bounded event queues; large response backing files; real completion/failure statuses; transient Codex reconnect is not terminal failure | Provider account/network failures remain visible; all event variants require fixture coverage |
| Historical conversations | Lazy native Codex/Claude/Gemini/OpenCode import; compressed Codex sources supported; local history for all adapters | Cursor/AGY/Grok native history import parity, server-side deletion/archive/revert policies |
| Agent switching | Same visible chat, bounded portable context and per-provider resume binding; failed preparation rolls back | This does not transfer provider hidden reasoning or a native provider cache |
| Tool approvals and questions | Codex App Server, Claude bidirectional stream-json, Gemini/Cursor ACP and OpenCode server bridges track real request IDs and keep responses with the owning worker | Claude installed initialization and protocol fixtures are verified; live Claude model/tool execution remains blocked by the current account credits. AGY/Grok interactive approval parity remains open; Cursor now has native ACP requests (see [provider audit](PROVIDER-FEATURE-AUDIT.md)). Headless permission flags alone are not an approval UI |
| Sandbox and terminal | Codex native sandbox-backed `/terminal` command execution; provider read-only/plan vs workspace-edit policy; owned process groups and cancellation | Demand-created local Rust PTY dock (`/shell`) now has ANSI/Unicode input, resize, bounded scrollback and owned cleanup. It preserves its shell when hidden; it is not a VM/container or a provider-owned terminal. Multiple terminals and remote environment lifecycle remain open |
| Subagents | Owned persistent Codex/OpenCode native services, paged child output, background decisions after parent completion, nested ownership, native Stop and supported Codex steering | Durable child recovery/history after app restart, nested tree presentation, arbitrary child definitions and full cross-provider parity remain open; see [native services](NATIVE-SERVICES.md) |
| Codex status | Demand-driven account/model/quota RPCs plus genuine rolling quota notifications; readable provider/bucket names and percentage left sit above scrolling details, with each reported reset on its own line. Unreported account/quota/metric rows are absent | Partial RPC successes survive missing/malformed later records; sparse updates retain known resets without making older unrelated buckets fresh. Actual token reports remain distinct from entitlement or estimated usage |
| Other status/model discovery | Claude/Cursor native auth status, Claude SDK model catalog, OpenCode credentials/catalog, Grok model list; Gemini negotiated model catalog plus configured fallback, AGY configured fallback | Gemini/AGY unknown authentication is not shown as signed out; no fabricated quota/context values |
| Native slash commands | Code UI controls are explicitly labeled; Claude, Gemini and OpenCode advertise actual native commands from initialization/server metadata | No blanket forwarding of unknown terminal slash commands as model prose. CLI terminal commands are not automatically headless commands |
| Codex review / goal | Native inline `/review` targets and explicit `/goal` read/set/pause/resume/clear/budget RPC controls; paged review cards retain complete bounded wire output | Goals do not add an automatic persistent execution loop; imported source sessions must first be continued into Code ownership |
| Codex compact | Genuine `thread/compact/start` maintenance bridge, not a summarization prompt | Must preserve writer ownership and wait for actual compaction completion |
| Codex skills/MCP/plugins/hooks/apps/tools | `/skills`, `/mcp`, `/plugins`, `/hooks`, `/apps`, `/tools` read genuine project-scoped RPC inventories on demand, including configured MCP tool descriptions and input names; local plugins only, no installation/account mutation | Management, OAuth, enable/disable/install/update flows are separate; MCP inspection may initialize configured servers |
| File and media UI | Native file tree, text/code rendering, file citations, default browser links, image attachments with drag/drop and composer clipboard image paste | File editor/LSP/diagnostics, arbitrary media input, audio/video provider transport and complete accessibility need dedicated work |
| Local context selection | Native file/folder browse and line-range inputs; worker-created bounded UTF-8 preview, explicit Copy/Attach, file-type SVG chips, saved-snapshot inspection; cross-project chat context remains available | Canonical checkout confinement; bounded folder/line scans and omitted-content indicators. Symbol/skill/review/terminal selection, document extraction and remote source contexts remain separate |
| Prompt stash and templates | Private local text/context stash with durable acknowledgment before clearing; reusable text templates, exact preview/copy/recall, rename/remove/reload. Recall preserves current provider/model/permission settings and never sends a prompt | Image drafts are explicitly refused and preserved; use the existing image-capable queue. Library is bounded and local; remote sync, template variables and full multimodal stashes are not implemented |
| Voice | Apple Speech composer dictation, pause/restart and multiline tail-following regressions | Signed-app acoustic recognition/permission QA remains manual. Native Codex realtime voice and spoken conversation are not implemented |
| Prompt recovery / timeline | New workspace-edit prompts capture streamed deduplicated before/after snapshots, native preview/Undo/Redo and conflict detection; read-only prompts retain cheap timeline records | Existing prompts have no retroactive snapshot. External side effects are outside file Undo. See [checkpoint limits](CHECKPOINTS.md) |
| Cross-provider teams | Explicit two-to-four-member assignments, native per-member transcript/requests, concurrency cap, failure/restart holds; shared-folder editing serializes | Automatic dependencies/coordinator, isolated competing worktrees and independently persistent native children remain open |
| Context and queue | Cross-project indexed chat excerpts with chips; bounded durable editable/reorderable follow-ups with images and explicit restart/failure review | Not hidden-reasoning transfer or live subscriptions. Queue dispatch waits owned cleanup; external acceptance/local acknowledgement cannot be atomically committed |
| Provider customization | Existing CLI-owned configuration/auth/hooks/MCP are reused | Hooks/skills/plugin installation or editing, memory, custom output schemas and application integrations are not all editable natively |
| Cloud / remote / mobile / automation | Local agent process integration | Cloud environments, remote workers, scheduling, remote-control daemons, notification delivery, review sharing, source-control publishing and mobile interfaces are not automatically preserved |

Codex and OpenCode now retain owned native services with bounded idle eviction and independently running child controls. Other adapters retain their invocation-based transports. Native chat history/resume IDs persist; live child decisions and transcripts are not yet recovered after a complete application restart. The local PTY dock is an independently owned demand-driven shell, separate from provider RPC services. See [native workflows](NATIVE-WORKFLOWS.md) and the [36-family status](ROADMAP-36.md).

### Codex questions and command approval controls

Code explicitly enables `features.default_mode_request_user_input` for its Codex start/resume/fork requests. The installed CLI reports this feature disabled by default; without it, ordinary-mode agents can ask a question as prose rather than emitting a structured request. Live structured questions use the existing glass request tray, selectable answers, optional typed responses, and native Submit. Replies travel through the owning JSON-RPC request and never become a substitute prompt. Both the installed `item/tool/requestUserInput` method and the documented short form are recognized. Imported or prose-only questions cannot acquire a live native request ID retroactively.

The composer's Read only / Build button now opens the glass Permissions menu instead of silently toggling a boolean. Codex command review is a separate, saved workspace setting:

- **Ask me:** `approvalPolicy: on-request`, `approvalsReviewer: user`.
- **Automatic review:** `approvalPolicy: on-request`, `approvalsReviewer: auto_review`.
- **Never ask:** `approvalPolicy: never`; blocked commands stay blocked by the selected sandbox.

The setting applies to new turns, including reused native services. Read-only/workspace-write sandbox selection stays independent. Native question answers are never supplied automatically by these command settings. Existing in-flight approvals retain the provider's offered session/rule decisions; Code filters modern approval buttons against `availableDecisions` when supplied. Code does not rewrite the user's global Codex configuration or relax managed requirements. Other providers keep their existing native approval flows.

The setting adds a small per-request enum and reuses the existing workspace snapshot worker and finite popover animation. It adds no dependency, polling task, worker thread, transcript scan, or global input monitor. Protocol semantics were checked against the installed generated schema and the official [App Server](https://learn.chatgpt.com/docs/app-server) and [configuration reference](https://learn.chatgpt.com/docs/config-file/config-reference). Account-dependent automatic review and model tool selection still depend on the installed provider.

Verification for this pass: 657 application tests and 97 benchmark-target checks pass (754 active checks; 12 opt-ins ignored), with strict all-target Clippy, formatting, and whitespace checks passing. Disposable native-process fixtures cover both question method names, required answer delivery, command decisions filtered by the provider's advertised choices, and per-thread/per-turn reviewer policies without automatically answering questions. Native UI checks cover selection/Submit exactly once, workspace persistence/default migration, and opening the actual permissions picker from the composer. The full suite required localhost test-server access outside the filesystem/network sandbox. Authenticated model/automatic-review account smoke tests were not run.

The existing `dist/Code.app` was rebuilt and signed without stopping or relaunching the running instance. Strict code-signature verification passes, and its executable UUID matches `target/release/orbit` (`4A160EA7-55EA-3C6A-A236-454432C91C73`).

## Full installed Codex client request surface

The installed schema declares **105 client methods**. The list below is intentionally complete for this generated schema; existence here does not imply Code has a UI for the method. Native account/model/limits and read-only catalogs above are implemented separately from settings mutations, remote services or filesystem APIs. The transport must reject unknown tool requests safely rather than hanging or accepting them.

- `initialize`
- `thread/start`
- `thread/resume`
- `thread/fork`
- `thread/archive`
- `thread/delete`
- `thread/unsubscribe`
- `thread/name/set`
- `thread/goal/set`
- `thread/goal/get`
- `thread/goal/clear`
- `thread/metadata/update`
- `thread/attachment/add`
- `thread/attachment/list`
- `thread/attachmentOwner/list`
- `thread/attachment/remove`
- `thread/section/move`
- `thread/unarchive`
- `thread/compact/start`
- `thread/shellCommand`
- `thread/approveGuardianDeniedAction`
- `thread/revert`
- `thread/list`
- `threadSection/list`
- `threadSection/create`
- `threadSection/update`
- `threadSection/delete`
- `thread/loaded/list`
- `thread/read`
- `thread/turns/list`
- `thread/items/list`
- `thread/inject_items`
- `skills/list`
- `skills/extraRoots/set`
- `hooks/list`
- `marketplace/add`
- `marketplace/remove`
- `marketplace/upgrade`
- `plugin/list`
- `plugin/installed`
- `plugin/reconcile`
- `plugin/read`
- `plugin/skill/read`
- `plugin/share/save`
- `plugin/share/updateTargets`
- `plugin/share/list`
- `plugin/share/checkout`
- `plugin/share/delete`
- `app/read`
- `app/list`
- `app/installed`
- `fs/readFile`
- `fs/writeFile`
- `fs/createDirectory`
- `fs/getMetadata`
- `fs/readDirectory`
- `fs/remove`
- `fs/copy`
- `fs/watch`
- `fs/unwatch`
- `skills/config/write`
- `plugin/install`
- `plugin/uninstall`
- `turn/start`
- `turn/steer`
- `turn/interrupt`
- `review/start`
- `model/list`
- `account/gatewayOAuth/read`
- `account/gatewayOAuth/login`
- `account/gatewayOAuth/cancel`
- `modelProvider/capabilities/read`
- `experimentalFeature/list`
- `permissionProfile/list`
- `experimentalFeature/enablement/set`
- `mcpServer/oauth/login`
- `config/mcpServer/reload`
- `mcpServerStatus/list`
- `mcpServer/resource/read`
- `mcpServer/tool/call`
- `windowsSandbox/setupStart`
- `windowsSandbox/readiness`
- `account/login/start`
- `account/login/cancel`
- `account/logout`
- `account/rateLimits/read`
- `account/rateLimitResetCredit/consume`
- `account/usage/read`
- `account/workspaceMessages/read`
- `account/sendAddCreditsNudgeEmail`
- `feedback/upload`
- `command/exec`
- `command/exec/write`
- `command/exec/terminate`
- `command/exec/resize`
- `config/read`
- `externalAgentConfig/detect`
- `externalAgentConfig/import`
- `externalAgentConfig/import/recordHistory`
- `externalAgentConfig/import/readHistories`
- `config/value/write`
- `config/batchWrite`
- `configRequirements/read`
- `account/read`
- `fuzzyFileSearch`

## Installed Codex server-to-client surface

Native clients also need to handle server requests and notifications. Unsupported requests must receive an explicit safe error/decline; silently dropping a request leaves a session blocked. Unknown informational notifications may be ignored while preserving authoritative turn/session lifecycle events. Realtime audio/video and attestation are distinct integrations; displaying a transcript is not equivalent to supporting these transports.

### Server requests (10)

- `item/commandExecution/requestApproval`
- `item/fileChange/requestApproval`
- `item/tool/requestUserInput`
- `mcpServer/elicitation/request`
- `item/permissions/requestApproval`
- `item/tool/call`
- `account/chatgptAuthTokens/refresh`
- `attestation/generate`
- `applyPatchApproval`
- `execCommandApproval`

### Server notifications (84)

- `error`
- `thread/started`
- `thread/status/changed`
- `thread/archived`
- `thread/deleted`
- `thread/unarchived`
- `thread/closed`
- `thread/reverted`
- `skills/changed`
- `thread/name/updated`
- `thread/attachment/updated`
- `thread/goal/updated`
- `thread/prediction/updated`
- `thread/goal/cleared`
- `thread/queue/changed`
- `project/changed`
- `thread/project/updated`
- `thread/environment/connected`
- `thread/environment/disconnected`
- `thread/settings/updated`
- `thread/tokenUsage/updated`
- `turn/started`
- `hook/started`
- `turn/completed`
- `hook/completed`
- `turn/diff/updated`
- `turn/plan/updated`
- `item/started`
- `item/autoApprovalReview/started`
- `item/autoApprovalReview/completed`
- `autoApprovalReview/strictReviewRequired`
- `item/completed`
- `item/agentMessage/delta`
- `item/plan/delta`
- `command/exec/outputDelta`
- `process/outputDelta`
- `process/exited`
- `item/commandExecution/outputDelta`
- `item/commandExecution/terminalInteraction`
- `item/fileChange/outputDelta`
- `item/fileChange/patchUpdated`
- `serverRequest/resolved`
- `item/mcpToolCall/progress`
- `mcpServer/oauthLogin/completed`
- `mcpServer/startupStatus/updated`
- `mcpServer/event/stream/notification`
- `account/updated`
- `account/gatewayOAuth/changed`
- `account/rateLimits/updated`
- `app/list/updated`
- `remoteControl/status/changed`
- `externalAgentConfig/import/progress`
- `externalAgentConfig/import/completed`
- `fs/changed`
- `item/reasoning/summaryTextDelta`
- `item/reasoning/summaryPartAdded`
- `item/reasoning/textDelta`
- `thread/compacted`
- `model/rerouted`
- `model/verification`
- `modelProvider/authRecoveryStarted`
- `modelProvider/authRecoveryCompleted`
- `turn/moderationMetadata`
- `model/safetyBuffering/updated`
- `warning`
- `guardianWarning`
- `deprecationNotice`
- `configWarning`
- `fuzzyFileSearch/sessionUpdated`
- `fuzzyFileSearch/sessionCompleted`
- `thread/realtime/started`
- `thread/realtime/itemAdded`
- `thread/realtime/item/started`
- `thread/realtime/item/transcript/delta`
- `thread/realtime/item/completed`
- `thread/realtime/transcript/delta`
- `thread/realtime/transcript/done`
- `thread/realtime/outputAudio/delta`
- `thread/realtime/sdp`
- `thread/realtime/error`
- `thread/realtime/closed`
- `windows/worldWritableWarning`
- `windowsSandbox/setupCompleted`
- `account/login/completed`

## Other provider surface areas that must not disappear from the audit

- **Claude:** native bidirectional stream-json is now implemented, with SDK initialization, exact allow-once/deny responses, structured AskUserQuestion answers, request cancellation and interrupt; resume/fork; model/effort; agent definitions and child events; system/context prompts; structured output and budgets; directory allowances; MCP; plugins; skills/hooks; Chrome and IDE integration; worktrees; output stream partials; account management. Code does not claim all of these have native controls.
- **OpenCode:** ACP plus server API/session events; permissions/questions; model/provider/auth; sessions/children/messages/parts; abort/fork/revert/unrevert/share; prompt and command execution; summarize/compact; files/search/diagnostics; terminal PTYs; tools, agents and slash command catalogs; MCP OAuth/resources; configuration/plugins; project/worktree; global event subscriptions; update/shutdown. Server endpoints are not equivalent to CLI `run` events.
- **Gemini:** ACP permission/session updates; headless JSON/stream-json; models and approval modes; sandbox/trust; resume/list/delete sessions; image/media and include-directory support; extensions; skills; hooks; MCP; routing/local Gemma; interactive slash commands. Interactive-only terminal behavior needs an explicit bridge, not an assumption.
- **Cursor:** ACP; plan/ask/agent; sandbox enabled/disabled and `sandbox run` path/network policies; permission allowlists; workspace trust; partial output; model catalog; sessions/worktrees; MCP tools/auth; plugins, rules/skills/hooks; cloud worker management; remote auth and account inspection.
- **AGY:** stream-json input/output; multiple turns; plan/accept-edits; explicit sandbox flag; model/effort; project/session selection; MCP/plugin management; skills/slash expansion; auth; remote-control daemon. Account/model probing must not trigger automatic sign-in.
- **Grok:** ACP-native streaming-json; subagent definitions/dashboard; allow/deny rules and permission modes; sandbox profiles; resume/fork/session IDs; reasoning effort; worktrees/repository snapshot restore; output schema; memory; MCP; plugins/marketplaces; usage; trace/export; remote sessions and clipboard wrapper. These are provider features, not all native Code features.

## Efficiency contract

Metadata discovery runs only on explicit controls/status/catalog opening or refresh, off the UI thread. Status results cache for 60 seconds; active CLI controls probe sequentially, never in a burst. Codex inspection has an 8-second deadline and a 2 MiB cumulative wire bound; catalogs retain at most 128 display entries and metadata does not retain account email, tokens or raw diagnostics. Each probe owns a separate process group, kills it before reaping, does not resume a chat, and never submits inference. Unknown values stay unknown. No recurring metadata polling or idle animation loop is required.

## Verification added with the native controls pass

- Codex metadata fixtures cover real RPC envelopes, catalog IDs, hidden models, account plan/type, privacy, separate primary/secondary quota windows, multi-bucket preference, partial endpoint failures and invalid quota values. The opt-in installed-binary test passed with 7 real model choices, account state and 1 provider-returned limit window, without starting a thread or inference.
- Claude protocol fixtures exercise a complete child process round trip: initialize, user envelope, partial text, actual pending tool request, explicit UI decision, matching control response and completed output. Native slash commands must be advertised before any prompt is written. Pure tests cover decline/cancel, exact original input, read-only decisions and structured question answers. The installed 2.1.70 binary accepted SDK initialization and advertised 14 commands and 5 model choices without inference. [Official SDK wire reference](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/query.py), [permission semantics](https://code.claude.com/docs/en/agent-sdk/permissions), [user questions](https://code.claude.com/docs/en/agent-sdk/user-input). Native provider allow rules still apply before a callback; Code does not claim to intercept every tool unconditionally. Read-only Claude starts in plan mode and disables mutating built-ins (including Bash) because the installed older CLI predates strengthened plan-mode shell-write checks. Native model events populate the picker, and metadata refresh reads its genuine initialize catalog without inference.
- Secret request fields shape one bullet per extended grapheme, map cursor/mouse/selection/IME bounds to source offsets, disable clipboard copy/cut, avoid exposing raw contents to macOS text retrieval, and stay out of draft persistence. Unicode and native GPUI tests cover these behaviors; normal composer input allocates no secret mapping.

## OpenCode and Gemini native transport verification

Verified against installed OpenCode **1.18.3** and Gemini CLI **0.42.0** on 2026-10-14. The OpenCode server's installed `/doc` advertised 162 paths. Relevant primary contracts are the [OpenCode server API](https://dev.opencode.ai/docs/server/), its [native message-ID implementation](https://github.com/anomalyco/opencode/blob/v1.18.3/packages/opencode/src/id/id.ts), [native compaction lifecycle](https://github.com/anomalyco/opencode/blob/v1.18.3/packages/opencode/src/session/compaction.ts), [Gemini ACP](https://geminicli.com/docs/cli/acp-mode/), and [ACP tool permissions](https://agentclientprotocol.com/protocol/v1/tool-calls).

- **OpenCode: 12 focused checks pass.** Authenticated loopback HTTP/SSE fixtures cover native approval transport and decline safeguards, multiple questions and custom answers, child progress, advertised command execution while approvals remain live, external-session fork isolation, arbitrary SSE framing, native user-message identity, automatic compaction/overflow replay/continuation lineage, and native abort delivery before process cleanup when the run handle is dropped. Copied fork history and reasoning/compaction summaries do not become new assistant prose. Oversized synchronous command responses drain within a bounded metadata buffer while the complete canonical reply continues through SSE, including late final deltas.
- **OpenCode: real native prompt and continuation pass.** A disposable no-tools read-only prompt completed through the Rust server bridge; a subsequent external-session continuation forked into a Code-owned native session and returned exactly one `CODE_SMOKE_OK`. The earlier smoke helper removed its temporary workspace; recreating that owned empty directory was necessary because OpenCode itself retains the original workspace path. This is a connectivity/streaming check, not a large coding-task or performance benchmark.
- **Gemini: 6 focused checks pass.** Bidirectional ACP fixtures verify the actual advertised permission option IDs and responses, streamed tools and text, advertised native slash commands, complete-review safeguards, and native session cancellation before process cleanup. A real installed `initialize` handshake confirmed ACP v1, native resume, and advertised image/audio capabilities. Gemini prompt/model entitlement has not been reverified through a paid ACP inference call; prior Google account/client eligibility problems are not claimed resolved. Negotiated model IDs and names populate the native model picker; image submissions also require the live advertised image capability.
- **Bounded Stop cleanup:** OpenCode sends its native session abort request and Gemini sends ACP `session/cancel` before the owned process is terminated and reaped. Cleanup protocol IO has an independent 850 ms deadline; a shared 2-second native-process watchdog remains the fallback for an unresponsive worker. Cancellation immediately releases backpressure waits. Protocol fixtures cover both explicit Stop and dropping the run handle; they do not claim that every third-party tool/plugin has been proven to terminate correctly.
- **Permission isolation:** imported OpenCode sessions are forked before Code applies its per-run policy. Code never patches the shared source session's permissions. Code-owned native sessions retain stable IDs across subsequent turns. Ordered native deny rules are preserved, native allow rules become individual asks, and read-only mode denies native edit/bash permissions. This is a harness permission policy, not an independent operating-system sandbox or container.
- **Complete review:** an oversized permission context becomes decline-only. Both the displayed options and the owning worker enforce this, so hidden command suffixes cannot be accepted through a clipped preview. Request responses use provider-native IDs and stay with the owning process. Gemini advertises no client filesystem/terminal proxy; native CLI tools keep their own policy and execution behavior.

Evidence logs are local build artifacts: `.build/opencode-native-tests.log`, `.build/gemini-acp-tests.log`, `.build/opencode-native-live-smoke.log`, and `.build/opencode-native-resume-smoke.log`. Remaining gaps above still apply: OpenCode PTY emulation, complete provider/plugin management, richer MCP OAuth controls, and arbitrary audio/video input have not acquired full native UI parity merely because a transport exists.

## Stop and normal Quit verification

An installed Codex no-inference regression demonstrated that killing App Server alone left its sandbox command alive. Native Stop now terminates the owned command/turn through App Server before reaping the server; the same exact-PID regression passes. Claude process fixtures also prove detached-tool cleanup, full-queue cancellation and a bounded stop against an unresponsive peer. Legacy adapters retain direct process-group cancellation.

Normal Quit blocks new prompt launches, cancels runs including handles returned by an already-starting launch, waits for native cleanup, drains final queued response sources and only then flushes history/workspace state. Native UI regressions verify the full final event queue and the launch/quit race. The short system-shutdown hook starts cancellation too; force-killing the application is not a graceful-shutdown guarantee. Persistent terminals and background children beyond a completed parent invocation remain the separate broker scope described above.

## October 14 automated verification

The final complete offline suite passed **538 checks** (458 app checks and 80 benchmark/parser checks), with 10 explicitly opt-in installed-agent checks excluded from the ordinary run. Selected installed checks were exercised separately as described above. All-target Clippy passed with warnings denied; formatting and whitespace checks passed. Source evidence: `.build/native-release-tests.log`, `.build/native-release-clippy.log`, and the provider-specific logs above. These results are bounded verification, not a claim of complete feature parity or absence of all bugs.

## October 15 cross-product integration

The [cross-product inventory](COMPETITIVE-AUDIT.md) now links project lifecycle, rendering and provider audits. Cursor ACP adds exact native approvals/questions/plan decisions, negotiated subagent streaming and reported usage; AGY/Grok remain headless adapters. Codex review/goal/catalog and hook/tool/reroute notifications now have native controls/cards. Project create/rename/pin/archive and structured review/draft/plan rendering were added without rewriting source CLI histories or adding a web runtime. See the final integrated verification below; earlier counts above describe their dated source revisions.

## October 15 final integrated verification

The complete offline suite passes **584 checks**: **493 application checks** and **91 parser/benchmark-target checks**, with **10 explicitly opt-in installed-agent/image workload checks excluded**. All-target Clippy passes with warnings denied; formatting and whitespace checks pass. The full suite uses disposable local fixtures, including authenticated-loopback-shaped OpenCode servers; it does not submit paid model prompts. Installed Cursor ACP negotiation is separately verified, while this machine's Cursor account requires authentication, so live inference is not claimed.

New verification includes native inline review and goal RPCs without fallback model prompts, complete multi-megabyte review findings retained through paged cards and saved history, hook/tool/model events, goal-state truthfulness, Cursor permissions/questions/plans/children, literal tool-output fidelity, percent-containing file references, dense 10,000-link output, lifecycle persistence/focus, failed-save Quit recovery, folder-creation/Quit races, and explicit archive-ID paging beyond both recent-list caps. A fixture readiness marker was made atomic; actual Stop deadlines and orphan-PID assertions were retained.

Logs: `.build/feature-audit-final-tests.log`, `.build/feature-audit-final-clippy.log`, `.build/cursor-acp-tests.log`, `.build/cursor-handoff-final.log`, and `.build/claude-stop-fixture-tests.log`. Native GPUI layout/paint/input tests pass. Live visual inspection remains unavailable because the desktop computer-use service failed to start. The release benchmark remains a state/parser workload, not an FPS, typing-latency, startup or energy measurement. No complete parity or zero-bug claim follows from these results.

## Recoverable workflow verification, October 15

The final integrated source passes **650 checks**: 559 application checks plus 91 shared parser/state benchmark-target checks, with 10 opt-in checks excluded from the ordinary suite. Strict all-target Clippy, formatting and whitespace checks pass. Logs: `.build/workflows-final-tests.log`, `.build/workflows-final-clippy.log`. Commands: `cargo test --offline -j2 --all-targets -- --test-threads=4` and `cargo clippy --offline -j2 --all-targets -- -D warnings`.

Three additional explicit installed Codex App Server checks pass: sandbox command output, read-only write rejection, and owned command cleanup after Stop. They use no model inference; log `.build/workflows-installed-sandbox-tests.log`. Actual PTY, two-provider concurrent team, exact approval routing, transformed native-prompt history/restart, checkpoint conflict/crash/GC, escaped persistence budgets, queued context/settings races, clipboard image safety and multiline dictation states have disposable fixtures. This is not a claim of new authenticated model execution for every provider.

The computer-use service could not start, so live release-window appearance and actual acoustic dictation were not verified. The native layout/paint/input tests run separately from that service. All 36 broad feature families are **not complete**; [ROADMAP-36.md](ROADMAP-36.md) records the remaining broker, editor/LSP, provider, remote and cloud work. [NATIVE-WORKFLOWS.md](NATIVE-WORKFLOWS.md) describes the controls actually implemented by this pass.

## October 16 context, composer and status additions

The compact status dropdown keeps provider-reported quota windows above a separate details scroller, so a long tool/skill inventory cannot bury the limits. A reset appears below its window value, and unreported account, quota, token and cost rows consume no space. The shared parser covers modern named quota buckets, legacy primary/secondary reports and live App Server updates. Sparse updates preserve previously reported optional reset/duration fields; per-window observation times prevent a fresh update for one bucket from making unrelated old buckets override a newer account read. Lifecycle labels reflect current run epochs, pending native requests, startup, saving and cached owned-child activity rather than treating every gap as idle.

The local-context control provides native file/folder browsing and optional line ranges. IO runs on a worker; reads remain inside the canonical checkout and use descriptor-relative no-follow traversal to prevent symlink redirection. The user sees and can copy the exact bounded snapshot before attaching it. Attached chips reopen their saved bytes without rereading changing files. Snapshots are at most 24 KiB; folder reads cover at most 256 entries to depth 5, omit hidden/generated subtrees, cap individual folder-file excerpts at 4 KiB and state that content was omitted. Line-range scans stop after 8 MiB. These are context snapshots, not a full-folder ingestion or document-understanding claim.

The local prompt library loads only when opened. Text/context stashes clear the original composer only after a private atomic save is acknowledged and only if that draft still matches. Changed drafts and save failures retain the current text. Recall refuses to overwrite another draft, preserves current execution settings and does not start an agent. Reusable templates store text only. Rename, remove, reload and copying are explicit native interactions. Persistence uses revision checks to avoid concurrent lost updates, 32 saved entries, 256 KiB of text/context payload and a 1 MiB encoded-file cap. Image drafts are preserved and directed to the existing queue rather than silently dropping attachments.

Focused disposable tests verify actual RPC and rolling quota shapes, missing fields, separate reset geometry, long inventory positioning, line-range/UTF-8 bounds, path escapes, stale previews, private atomic persistence, stale revisions, save/recall policy preservation and draft/save races. Evidence: `.build/status-*-final-verified.log`, `.build/file_context--final-verified.log`, `.build/file_context_ui--final-verified.log`, `.build/prompt_library--final-verified.log`, `.build/prompt_library_ui--final-verified.log`. Complete live account/visual/acoustic coverage and feature-family parity remain separate verification; this section does not replace the dated full-suite results above.


## 2026-10-16 Completed review, remaining quotas and Send-time handoff

Completed opens the native virtualized diff inspector. Saved checkpoints use immutable per-prompt before/after contents and expose confirmed Undo/Redo; legacy responses use a real recorded provider patch or an explicitly labeled current-workspace fallback. Painted card actions retain checkpoint/chat/root ownership across history prepend. Eligibility checks are worker-only, capped at four concurrent metadata jobs with a 256-entry cache. File and patch pages remain bounded.

All reported quota cards display percentage left, readable provider identities and separate reset lines. Missing data stays hidden. Staging a different provider cannot relabel the previous provider's counters or context.

Agent selection now only stages the composer. Explicit Send captures the instruction, attachments, context, model and policy; asynchronous history/handoff preparation resumes that immutable submission and launches one destination invocation in the same visible chat. New typing stays an unsent draft. Preparation failures and Stop hold queued work instead of replaying it. No browser renderer, new dependency or recurring account polling was added.

Verification: `cargo test --all-targets -- --test-threads=4` passed **706 tests** (94 support/benchmark-target tests plus 612 application tests), with **10 opt-in checks ignored** and zero failures. `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check` and `git diff --check` passed. Logs: `.build/completed-send-quota-all-tests.log` and `.build/completed-send-quota-clippy.log`. This pass used disposable local transport/UI/filesystem fixtures and did not make paid inference requests. These checks establish the tested behavior, not complete provider parity, zero defects or measured power/FPS guarantees.

Live visual inspection was attempted, but the computer-use service failed to start. Native GPUI layout/paint/input tests passed; the running release window was not visually verified through that service.


## 2026-10-16 Provider identity after composer selection

Legacy response headers and speaker cards now resolve their fallback provider from the active chat/run, independently of the selected composer destination. Every normal, native-command and team run captures its launch provider; freshly repainted live rows and clipped-tail notices use that immutable identity. Explicit provider attribution from mixed-agent history remains authoritative. Selection does not rename chat titles or mutate unrelated chat bindings. This adds one enum to each active run without background polling, history scanning or new dependencies.

The native regression test switches through all seven providers while Codex streams and checks painted historical headers, legacy/explicit speaker cards, live row identities and unrelated chat metadata. The complete suite passed on the lower-concurrency rerun: **707 tests** (94 support/benchmark-target plus 613 application), **10 opt-in checks ignored**. Strict Clippy, formatting and whitespace checks passed. An unchanged Cursor signed-out fixture returned Unknown in the initial four-worker run; it passed in isolation and in the full two-worker rerun. Logs: `.build/provider-identity-regression.log`, `.build/provider-identity-all-tests.log`, `.build/provider-identity-cursor-check.log`, `.build/provider-identity-all-tests-recheck.log`, `.build/provider-identity-clippy.log`.
