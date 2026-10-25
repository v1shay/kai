# Project and conversation feature audit

Reviewed 2026-10-15. This is the project/chat-lifecycle part of the broader provider audit. A documented competitor feature is evidence of a product capability, not proof that every backend API is publicly available to Code.

## Primary-source comparison

| Feature family | Evidence | Code before this pass | Result of this pass |
| --- | --- | --- | --- |
| Rename projects and chats | [OpenAI Projects and chats](https://learn.chatgpt.com/docs/projects) | Source-generated titles; no user-facing rename | Native compact glass rename editor. Local aliases survive discovery and restarts, appear in rows/tabs/header/search, and never rename the filesystem or rewrite CLI transcripts. |
| Pin projects/chats | [OpenAI organization](https://learn.chatgpt.com/docs/projects), [T3 Code thread sidebar](https://github.com/pingdotgg/t3code/blob/main/docs/user/thread-sidebar.md) | Recency only | Persistent project/chat pins sort first; remaining chats stay ordered by last activity. Pin glyphs identify pinned entries. |
| Archive/restore chats | [OpenAI organization](https://learn.chatgpt.com/docs/projects), [T3 Code thread sidebar](https://github.com/pingdotgg/t3code/blob/main/docs/user/thread-sidebar.md) | Remove/undo only | Separate persistent archive state; Sessions/Archived toggle; per-chat archive/unarchive in the existing animated dots menu. Archived summaries are retrieved by saved ID in explicit 128-item pages beyond recent/project caps, with retry and stale-result cancellation. Running/finishing chats cannot be archived. |
| Create local projects | [ChatGPT projects](https://learn.chatgpt.com/docs/projects), [MonoCode README](https://github.com/hardbeat920/monocode) | Open an existing folder only | Create project action chooses a parent, then explicitly creates a named empty folder on a worker. Existing paths/symlinks are never overwritten. No Git initialization, dependencies, commands, or model calls run. |
| Find conversations | [T3 Code thread search](https://github.com/pingdotgg/t3code/blob/main/docs/user/thread-sidebar.md), [OpenCode TUI](https://opencode.ai/docs/tui/) | Bounded indexed source-title/message search | Existing search preserved; locally renamed titles now participate too, including archived chats. |
| Remove without destroying files | [T3 Code projectless threads](https://github.com/pingdotgg/t3code/blob/main/docs/user/thread-sidebar.md) | Project removal and chat hiding with undo | Preserved. Removal is Code-local; source files and CLI sessions remain. |
| Resume and branch sessions | [OpenCode sessions](https://opencode.ai/docs/tui/), [MonoCode README](https://github.com/hardbeat920/monocode) | Existing native session resume/fork and same-chat provider handoff | Preserved; aliases and archive/pin state belong to Code's visible chat identity. |

## Efficiency and ownership

- Metadata contains names/IDs/paths/sets only; it never loads a conversation body or decodes media.
- SQLite history queries and transcript pagination remain on the existing history worker. Sidebar rows remain virtualized.
- Preference writes are emitted only on explicit lifecycle changes and run on the existing dedicated preferences worker; no writes run during animation or rendering. Normal Quit now waits for the preference write queue along with history/workspace saves and in-flight folder creation, including when its editor was dismissed. Unsaved preference errors keep the app open until a successful retry; successful renames return focus to the composer.
- A folder name is limited to 256 UTF-8 bytes and rejects traversal, separators, control characters and empty names. `create_dir` refuses an existing target; no recursive filesystem deletion is added.
- Archive/pin/rename modify only Code-owned metadata. Renaming a project does **not** change its working directory, Git path, native provider session title, authentication or source ownership. There is no new cloud sync or data upload.
- Rename/create transitions use the existing spring and stop requesting frames after 320 ms; reduced-motion mode skips them. Menus retain their existing short close animation and no shadow.

## Verified behavior

Focused tests cover creating a Unicode-named directory, rejecting traversal and existing paths, preserving file contents, name validation, persistent aliases/pins/archive after worker restart, pinned order after refresh, archive filtering and restoration, native editor rendering, name validation/focus, unsaved write failure/retry, pending folder work during Quit, archived ID paging/retry/stale results, and summaries beyond discovery caps. The application-wide suite remains required after integrating all parallel changes.

## Remaining lifecycle/product gaps

These are recorded, not presented as implemented:

- Manual drag ordering, multi-select/bulk actions, folders/groups, settled/snoozed states and scheduled wake-up. T3 Code's server-owned rules and synchronization need a deliberately designed scheduler/broker; silently running future prompts would change the current execution model.
- Projectless scratch workspaces, multiple roots per project, per-project model defaults and workspace-template creation.
- Worktree create/remove/merge controls and task-to-branch/PR linking. These change Git state and need explicit UI choices; existing code/diff viewing is not equivalent.
- Side-by-side split session panes, durable background terminals and server-owned sessions after app closure. MonoCode/T3 document these independently from tab navigation.
- Cross-app rename/archive sync. This pass intentionally stores aliases locally, because source sessions can be actively owned by another CLI. A separate authenticated broker with well-defined ownership would be needed for safe multi-client writes.
- Chat/project sharing, cloud conversation sync, provider-specific export, and provider-independent streamed Markdown export. Cloud ChatGPT projects/files/memory and local CLI projects are different services.
- Agent-callable project/chat management tools comparable to MonoCode's `/operator` surface or T3's server-owned metadata operations. These need a scoped tool bridge, explicit per-project access and durable execution ownership; native UI lifecycle controls alone do not expose them to a model.
- Restoring a project hidden in a prior app session requires opening its existing folder again; removed chat recovery currently has immediate Undo, while archived chats have a persistent recovery list.

The comprehensive transport/security/agent inventory is in [FEATURE-COVERAGE.md](FEATURE-COVERAGE.md). A complete feature-parity or zero-bug claim would be unsupported.
