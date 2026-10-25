# Native file editing

Open **Files**, select a UTF-8 text/code/data file, and choose **Edit**. The native Rust editor supports multiline input, normal selection/clipboard actions, **⌘Z / ⇧⌘Z**, a visible dirty indicator, **Save / ⌘S**, **Copy draft**, and explicit draft discard. Edits do not become file changes until Save is clicked or invoked.

The editor is bounded to 64 KiB per file. Larger files keep the existing lazy paged preview and explicitly show the editor limit. Binary, non-UTF-8, symlink, and hard-linked files remain in preview; they are not silently converted. This is a local editing buffer, not an LSP/diagnostics/completion implementation.

The Files pane retains one editing buffer across project navigation and inspector hiding. The buffer labels its own original project and file; opening another file for editing requires saving or explicitly discarding a dirty buffer first. Hiding Files releases decoded preview images/assets, audio, preview jobs, directory cursors/rows, and lexer caches. Only the bounded editor and lightweight selection survive. Opening Files rehydrates the tree and preview on demand.

Draft text and its original file baseline are persisted through the existing coalesced background workspace store. The baseline includes the original full byte hash, file/root/parent identities, and permissions. Restart restores the draft without substituting a new baseline from the current file. If the file changed while Code was closed, the draft remains visible and copyable; Save refuses to overwrite the newer file. Normal Quit waits for pending editor IO and for the workspace flush; a workspace write failure keeps the app open.

Saving uses rooted no-follow file descriptors, full-byte conflict checks before and after staging, preserved regular file permissions, fsync, and an atomic exchange. Replaced parent paths and external changes are refused. A concurrent version detected after an exchange is preserved at the named `.code-restore-editor-…tmp` recovery path and the error explains which versions to compare. Code does not force-save a conflicting buffer or delete its draft. A file explicitly saved by another application may need to be reopened after the old draft is copied/discarded.

**Close** on a dirty buffer asks whether to keep editing or explicitly discard the draft. Discard changes only the buffer; it never restores or deletes the file on disk. A clean buffer closes immediately. Text that cannot be saved (for example an inserted NUL) remains in the bounded draft so the user can copy or repair it.

Implementation: `editor.rs` contains bounded file loading, persisted baseline validation and conflict-safe saving. `editor_ui.rs` contains the native buffer and explicit controls. `files.rs` retains/suspends the pane. `workspace_state.rs` and `workspace_ui.rs` persist and restore the draft through the existing background state worker.

Metadata scope: saves preserve basic Unix permission bits and UTF-8 file bytes. They do not preserve ownership, extended attributes, ACLs, resource forks, file timestamps, or hard-link relationships. Hard-linked files are refused rather than silently breaking their relationship. Use the paged preview and an appropriate external editor when that richer metadata must be preserved.
