# Feature-family implementation status

This tracks the 36 families in the October 15 comparison. A family containing a working feature is not thereby complete. “Partial” identifies specific implemented pieces; unchecked work remains required before claiming full parity. The user has authorized ordinary local implementation, but cloud accounts, remote exposure, unattended billing, OS capture and publishing still require the applicable concrete choices.

| # | Family | Implemented foundation / this pass | Remaining |
|---|---|---|---|
| 1 | Agent lifetime | Owned persistent Codex stdio/OpenCode server+SSE services, native IDs, child approvals/control/lifetime, scoped shutdown | Durable recovery of independent child transcripts/decisions after full app restart, broader provider broker parity |
| 2 | Prompt queues | Durable bounded queue, edit/remove/reorder, image/context retention, failure/restart holds, explicit steering | Provider-native queue synchronization and remote dispatch |
| 3 | Subagent workspace | Native launch/progress cards, owned Codex/OpenCode child workspace with paged output and native decisions; Codex steering and native Stop | Full nested tree, durable child history/recovery and per-child settings across all providers |
| 4 | Team orchestration | Mixed-provider tasks, native approvals/transcripts, app concurrency cap, serialized shared writes, stop/resume | Dependencies, coordinator, isolated parallel writes, automatic result synthesis |
| 5 | Context picker | Cross-project chat picker, file/folder UTF-8 snapshots with preview/copy/attach and line ranges, excerpt chips, citations/images | Symbol/skill picker, terminal/review selection context |
| 6 | Composer tools | Native editor input, dictation, paste/drop images, editable queue, durable text/context stash/recall/templates | Selected-response quoting, large-paste files, image prompt-library entries |
| 7 | Conversation branches | Native resume/protective source forks | Branch from specific message, edit-and-resend tree, native rewind UI |
| 8 | Real editor | Bounded paged previews plus 64 KiB UTF-8 native editing, conflict-safe Save/Cmd-S, Undo/Redo, durable drafts | Find/replace, multiple cursors, encoding controls, larger editable documents |
| 9 | Language intelligence | Syntax highlighting | LSP lifecycle, completion, references, refactoring, diagnostics |
| 10 | Terminal workspace | Demand-created real PTY, ANSI/Unicode, resize, selection, bounded scrollback, hidden suspension | Multiple/split terminals, search, shell integration, reconnect |
| 11 | Development tasks | Native tool commands and terminal exit status | Build/test runner, explorer/debugger, rerun failed checks |
| 12 | Change review | Git diff, native reviewer/finding cards, checkpoint changed-file previews | Per-hunk accept/reject, finding resolution, inline review feedback |
| 13 | Checkpoints | Per-prompt immutable colored paged diffs, /undo and /redo previews, conflict-safe restore, bounded dedup storage, forget/recovery | Selective hunk/path restore, conversation rewind integration |
| 14 | Git/worktrees | Existing checkout/diff context | Worktree/branch/stage/commit/conflict controls, safe merge/cleanup |
| 15 | PRs and CI | CLI tools can operate under native permissions | Native PR/review/CI interfaces, explicit publish/merge workflows |
| 16 | Project organization | Create/open, local rename, pin/remove, chat archive/restore | Scratch/multi-root/templates/defaults/groups, bulk/drag/split panes |
| 17 | Search/export | Indexed bounded history, cross-project chat context search | Tool/date/provider filters, in-chat matches, streaming export/import |
| 18 | Model controls | Real model catalogs, provider/model selection, reported usage | All negotiated effort/tier/schema/budget options, project defaults |
| 19 | Provider compatibility | Seven adapters, capability checks, versioned protocol fixtures | Complete provider-native history/options, visible version matrix/diagnostics |
| 20 | Permissions/environments | Native scoped requests and decisions, provider run modes, sandbox commands | Named profiles/grant viewer/revocation, explicit container/VM lifecycle |
| 21 | Tools/MCP/plugins | CLI configuration reused; native catalogs/schema inspection | Add/edit/OAuth/disable/reload, health and rollback |
| 22 | Skills/hooks/rules | Inherited CLI rules; read-only inventory and hook progress | Effective-rule UI, editing/validation, scope/reload controls |
| 23 | Agent definitions | Provider choices, native delegation, explicit team roles | Reusable custom definitions and tool/model/isolation controls |
| 24 | Memory | Native provider behavior; explicit bounded chat sharing | Code memory/provenance/editor/exclusions, approved persistent sharing |
| 25 | Documents/media | File/media previews, image attachments and paste | Document extraction/OCR, broader provider media input, gallery/annotation |
| 26 | Rich responses | Markdown/code/tables, typed plans/review/directives/citations | Math/diagrams/footnotes, CSV export, full artifact interactions |
| 27 | Editable artifacts | Read/copy draft cards | Editable/versioned canvas, documents/tables/LaTeX/isolated previews |
| 28 | Voice | Apple Speech dictation and microphone UI | Signed-app acoustic QA; spoken replies/realtime deliberately excluded by user |
| 29 | Browser/visual testing | Default-browser links | Authorized preview/picking/screenshots, errors/device controls, automation |
| 30 | Desktop integration | Native window/platform services | Quick composer/share/deep links, optional notifications/menu bar |
| 31 | Scheduling | Native explicit goal controls | Wakeups/triggers/history/budgets, unattended-run policy and login choice |
| 32 | Remote/collaboration | Local application | SSH/device pairing, routing/approvals/revocation/sharing |
| 33 | Accessibility/layout | Native input/focus, finite/reduced-motion glass controls | Complete VoiceOver/keyboard/text-scaling audit, remappable shortcuts |
| 34 | Reliability/diagnostics | Bounded queues, durable workspace/history, process cleanup, checkpoint journal, regression fixtures | Persistent request recovery, support bundle, rollback dashboard, broader fault injection |
| 35 | Resource controls | Virtualization, lazy history, bounded IO/decoding/storage/concurrency, hidden terminal suspension | Machine/battery-aware admission, per-worker telemetry, interactive energy/frame benchmarks |
| 36 | Usage/accountability | Genuine provider quotas/token reports with pinned reset cards and sparse-update freshness, background-child status, per-prompt changes | Durable approval/event audit, duration/cost trends, export |

See [native workflows](NATIVE-WORKFLOWS.md) for entry points and exact limits. Passing automated fixtures does not imply every authenticated provider account or external service was exercised.
