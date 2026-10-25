# Prompt file checkpoints

Code records a unique prompt history entry before launching an agent, a team member, or a native `/terminal` command. A prompt that permits workspace edits first captures its workspace files on a worker. Failure or cancellation stops that launch; an incomplete snapshot is never advertised as Undo. Read-only prompts record metadata without scanning files or claiming file Undo.

Each saved user prompt carries its own app-owned checkpoint UUID, including repeated identical prompts. The marker is retained as attribution metadata without rendering a redundant control. Each **Completed** card opens its native file review, with an integrated **Undo** or **Redo** control when its saved snapshot is eligible. The composer’s **Prompt history** control also lists checkpoints. Native and local history merging strips a valid final user marker for legacy comparisons. New prompts also retain a bounded SHA-256 identity of the exact provider-visible input: context/team/handoff wrappers can differ from the readable prompt without duplicating it on reload. Full native bodies are compared incrementally; matching bookends alone is insufficient.

## Use

1. Click **Completed** to open its native file diff inspector, or choose a saved prompt in **Prompt history**.
2. Review the exact added, deleted, and modified paths; lists are paged 64 at a time.
3. Click **Undo** and confirm **Undo changes** explicitly. Previewing a checkpoint does not change files.
4. The same entry offers **Redo** after a successful Undo.

Code checks the active chat and canonical workspace against the stored checkpoint before previewing and again before restoring. A provider-generated marker cannot execute a restore. Stop agents and close the live terminal first; hidden live terminals also block restores. Starting jobs, pending cleanup, and overlapping workspace checkpoints block restoration.

## What is captured

A mutating prompt captures regular files and symbolic links that Git enumerates as tracked or nonignored untracked paths, plus their ancestor directory modes. Git supplies repository, nested, and global ignore semantics. Nonrepository workspaces use a private bare Git helper outside the workspace; Code never initializes or resets the workspace repository.

Tracked paths are included even when named `build`, `target`, or another usual cache name. Untracked dependency, build, and cache directories are excluded, along with VCS metadata and Code’s private restore names. The exact additional cache names are defined in `hidden()` in `src/checkpoints.rs`. Symlinks are captured and restored as links, never traversed. Empty directories without included files are not independently captured. Extended attributes, ownership, ACLs, hard-link relationships, and file timestamps are not restored.

The final snapshot is captured only after the owned agent process and its sandbox descendants settle. Changes are attributed to the entire before/after interval: another application editing the workspace during the prompt can therefore contribute to its change list. Checkpoints cover workspace files, not network calls, databases, application preferences, processes, files outside the workspace, or other external effects.

## Conflict and crash behavior

Before applying a restore, Code compares affected current contents, permissions, and symlink targets against the expected checkpoint version. Other later edits cause an error instead of being overwritten. Unchanged paths outside the prompt’s change list remain untouched. Path traversal, replaced workspace roots, and symlink parent traversal are rejected.

Atomic exchanges preserve displaced file inodes in private recovery storage. A concurrent edit detected during an exchange is preserved and the operation stops. A disk-backed per-file journal permits an interrupted Undo or Redo to resume from the same explicit action. Do not manually remove checkpoint manifests or recovery directories while recovering. If a later conflicting edit prevents recovery, resolve that conflict before retrying; Code does not offer a destructive force-restore option.

If a window disappears after launching an agent, an orphan cleanup guard waits for process settlement and then seals its checkpoint. After a full process crash, unfinished capture/record/seal entries whose owner PID is dead become **interrupted** at repository initialization. Their file attribution cannot be completed reliably, so they do not offer Undo; **Forget checkpoint…** releases their backup storage. Interrupted applying journals remain resumable. PID checks conservatively treat an existing owner PID as live.

A final snapshot failure is shown as `seal-failed` and does not claim Undo. Its backup data can be explicitly forgotten to release the reservation. Existing complete checkpoints remain available.

Restore recovery storage must be on the same filesystem device as the workspace for atomic exchanges. Code refuses a restore across filesystem devices before changing files; snapshots remain intact. This matters for projects on external volumes when Code’s data directory is on the startup disk.

## Resource limits and retention

All filesystem work runs outside the GPUI thread. Snapshot file bytes stream in 64 KiB buffers; SQLite manifests, WAL, disk-backed temporary tables, indexed cache lookups, and 64-row UI pages avoid a full in-memory filesystem tree. Unchanged files reuse immutable content blobs only after matching inode, device, size, mode, modification time, and change time, with a second validation after cache lookup.

A complete snapshot is bounded to 200,000 included entries and 20 GiB of source bytes. Total retained backup content, recovery reservations, and recorded metadata have a 4 GiB budget with conservative SQLite/metadata headroom. The index and each manifest also have explicit SQLite page limits. At most 512 prompt records are retained; read-only and failed records count toward that limit. A limit stops a new launch with a clear error instead of silently skipping files or discarding previous Undo history.

Metadata cache entries are disposable: completed snapshots prune deleted paths, all roots together retain at most 200,000 cache rows, and forgetting removes cache rows for orphan roots or missing content blobs. This bounds cache retention independently from the prompt timeline. SQLite may retain freed pages for reuse; forgetting does not promise the backing database file shrinks immediately.

Use **Prompt history → Forget checkpoint… → Forget checkpoint** to free old backup data. The confirmation names the prompt and states that Undo is lost. Forget only removes Code-owned backup metadata, recovery files, and content blobs unreferenced by any remaining checkpoint. It never changes workspace files or CLI history. Interrupted garbage collection remains in a retryable `forgetting` state. A chat’s saved prompt stays visible after its backup is forgotten.

The default private repository is `~/Library/Application Support/Orbit/checkpoints` (the retained internal data-directory name), or `$ORBIT_DATA_DIR/checkpoints` when overridden. Do not put it inside a mutating workspace. A read-only prompt can still use a workspace containing that directory because it captures no files.

## Implementation and verification

- `src/checkpoints.rs`: capture/read-only records, immutable content storage, metadata cache, scope and overlap validation, explicit preview/apply, atomic restore journal, orphan cleanup, storage accounting, and reference-aware Forget.
- `src/checkpoint_ui.rs`: `launch_checkpointed`, `seal_checkpoint_run`, stable completion targets, ownership and runtime activity checks, explicit Undo/Redo and Forget confirmation.
- `src/diff_view.rs`: native checkpoint/provider/working-tree file review, bounded worker pages, syntax and line gutters, and compact Undo/Redo confirmation.
- `src/main.rs`: normal prompt launch, persistence marker, completion and window-release hooks.
- `src/team_ui.rs`: team launch and completion hooks.
- `src/native_request_ui.rs`: native `/terminal` capture and launch hooks.
- `src/presentation.rs` and history merger tests: marker rendering and durable prompt associations.

Focused verification: `cargo test --offline -j 2 --bin orbit checkpoint`. Tests exercise modifications/deletions/new binary files, modes, reload and repeated prompts, explicit native UI actions, shared-content GC, ignored and tracked build paths, cache pruning, conflicts and concurrent writes, cancellation, symlink traversal, failed captures, orphan cleanup, storage caps, and a simulated crash between atomic rename and journal commit.

## Exact prompt diffs and slash actions

**Completed** opens the native file diff inspector using the checkpoint associated with that response’s owning user prompt. Its virtualized file list, line gutters, syntax colors and horizontal scrolling are shared with the working-tree review. The tiny marker and user-boundary metadata remain available when decoded history pages are evicted. Click handlers retain the painted checkpoint UUID, chat and execution root, so prepending older history cannot redirect a click to another prompt. Visible completion controls asynchronously check eligibility with at most four metadata jobs and a bounded 256-entry cache; there is no filesystem work during rendering.

If an older response predates checkpoints or its snapshot has been forgotten, Code uses its recorded native agent diff when available. Such a review is labeled **Agent-reported diff** and offers no invented historical Undo. Otherwise the native inspector opens **Current workspace · not an exact prompt diff**, explicitly explaining that this review is the current Git working tree rather than that old response. These fallbacks do not block review with a missing-checkpoint banner. Clicking a still-sealing prompt opens the inspector immediately, then refreshes its exact snapshot when sealing completes.

`/undo` selects the latest eligible completed file-changing checkpoint in the active chat and canonical workspace, and opens its preview. `/redo` selects the latest eligible undone checkpoint. Neither command changes files without the separate explicit confirmation.

Click a changed file in the prompt preview to inspect immutable **before this prompt → after this prompt** unified hunks. Native Rust code reads streamed lines using 24-line alignment lookahead, a 16 KiB per-line cap, and bounded patch pages (160 rows / 256 KiB soft targets plus at most one bounded alignment group). It skips unchanged spans while retaining context and resumes subsequent pages by byte/line cursors, without copying either complete version into memory or temporary source files. Alignment is bounded and need not be the minimal diff. Patch rows are virtualized. Binary/invalid-UTF-8 and oversized individual lines show explicit metadata instead of lossy source text; their original bytes remain covered by file Undo/Redo.
