# October 16 verification

This pass fixes reported status/quota layout, per-prompt diffs and undo commands, and adds owned Codex/OpenCode child services, bounded native file editing, file/folder/range context, and a durable prompt library. The [36-family matrix](ROADMAP-36.md) still contains required work; this is not a complete-parity or zero-bug claim.

## Completed checks

- `cargo test --all-targets -- --test-threads=4`: **601 app tests and 93 supporting tests passed**, zero failures. Ten opt-in checks are excluded by the ordinary suite.
- Six opt-in checks also passed: three installed Codex sandbox execution/read-only/cancellation checks; installed Codex account/model/quota RPC inspection; installed Claude bidirectional initialization; and a read-only audit of existing Code/Codex history. No model inference was invoked.
- The history audit resolved **272** old transcript locations through native metadata and loaded bounded pages from **three** existing compressed conversations. Source file stamps remained unchanged. This does not assert that every conversation's entire body was read.
- `cargo clippy --all-targets -- -D warnings`, `cargo fmt --all --check`, and `git diff --check` passed.
- `scripts/bundle.sh` produced `dist/Code.app`; strict code-signature verification passed. Its executable is native ARM64 Mach-O, **18,024,992 bytes**.

The regression suite includes separate immutable diffs for two consecutive prompts; undo/redo ordering and conflicts; actual native editor paste/Undo/Redo/Cmd-S/discard; draft restart conflicts; pinned quotas with separate reset rows; sparse and stale quota reports; parent-completed/background-child approvals; nested decision ownership; service reuse and old-handle cleanup; stop/reaping; same-length output corrections; Unicode page boundaries; long-line horizontal scrolling and long project labels; and full preservation of a child response above 16 MiB. The bulk-output fixture keeps exact byte-count assertions. Its bulk-I/O deadline is separate from tight decision/stop deadlines.

## Efficiency properties verified by implementation and tests

No browser or JavaScript frontend was added, and this pass adds no Cargo dependencies. Child prose spills after the existing bounded in-memory response threshold and is read in 64 KiB pages. Metadata events coalesce; a closed child inspector does not request text-delta redraws. Owned services have bounded admission, two idle cache slots and an idle unload deadline. File editing has one bounded buffer; hidden Files releases heavy previews, media, directory cursors and lexer caches. Context and library work runs only on demand on workers, with explicit storage/read budgets. These properties are not measurements of frame rate, idle memory, startup or energy use.

## Unverified and remaining

The computer-use service failed to start, so the release window was not inspected live. Native GPUI layout/input tests passed, but full visual, VoiceOver and acoustic dictation QA remain open. Authenticated paid prompt/delegation checks across every provider were not run. Provider accounts, supported models, network access and external tools remain real dependencies; errors are not hidden or converted into success.

Persistent child history/decision recovery across a complete app restart, broader provider broker parity, LSP/debugging, fuller Git/PR/CI workflows, remote/cloud/scheduling and other unfinished families remain listed in the matrix. See [checkpoints](CHECKPOINTS.md), [native services](NATIVE-SERVICES.md), and [file editing](FILE-EDITOR.md) for exact behavior and limits.
