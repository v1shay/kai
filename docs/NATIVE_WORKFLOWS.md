# Native workspace actions

- **Finder:** use Open With → Kai Unified for a folder/source file, or Services
  → Open in Kai Unified for selected files. Kai chooses the deepest registered
  project containing the selection. Otherwise it finds the repository root or
  adds the selected folder. A mixed-project selection is rejected. Existing
  chat drafts are preserved.
- **Quick Look:** use Quick Look in the file menu or attachment preview. It
  opens a lazy native preview and does not create an embedded browser at launch.
- **Command palette:** press Control-K or Command-K. Search commands, projects,
  chats, workflows and recent results. Type `>` for the command catalog. Use
  arrows and Return; Escape restores message focus. Empty search keeps recent
  chats accessible.
- **Named workflows:** choose the project/provider/model/effort, then send:

  ```text
  /workflow save Check build
  Run the build and explain any failures without changing files.
  ```

  Run it with `/workflow run Check build`, the command palette, the
  `kai://workflow/Check%20build` URL, an external workspace command, or the
  **Run Kai Workflow** action in macOS Shortcuts. Workflows retain their project,
  provider, exact optional model and effort. They use current permissions;
  approval policy does not expand filesystem access. Provider capabilities are
  inspected before launching a workflow with explicit model settings.
- **Branch:** `/fork` creates a separate chat. Codex's native thread branch is
  created before the new chat is editable, so restarting Kai cannot accidentally
  append to the source thread. Other providers receive a portable context
  packet. The source and its files are preserved.
- **Documents:** `/artifacts` finds saved documents in the current project.
  Codex receives scoped `code_artifacts` and `code_history` tools on new and
  resumed sessions. Documents persist independently of transcript pages and have
  project-scoped IDs/revisions. HTML opens in an isolated, ephemeral WebKit view
  with inline interaction, no network access, no shared cookies and no native
  command bridge. Markdown uses native formatted text. Mermaid source can be
  saved; interactive diagrams can be supplied as self-contained HTML.

## Antigravity approvals

Antigravity's headless CLI cannot prompt for command permissions. Kai registers
one named `PreToolUse` hook in `~/.gemini/config/hooks.json` on its first
Antigravity launch, merging it with existing hooks. The hook delegates to normal
provider policy outside a Kai-owned run. During a Kai run, a private, authenticated
Unix socket carries the exact tool call to the same approval UI used by Code and
the notch. Only a one-call approval produces `auto_approve`; a closed host,
malformed/oversized request, expired request or cancellation denies the call.
User-configured provider rules retain their precedence. No blanket allow rule or
permission-skip flag is installed. The hook is inactive after Kai's turn ends.

The live verification used a hook only inside each disposable test project;
existing global hooks, user conversations and permission lists were untouched.


## Closing tabs

Command-W and Control-W close the current chat tab. Closing the last tab leaves
an empty Code workspace; Command-W on an empty workspace preserves its draft.
Saved chats and drafts remain available. Mini Code and the extended notch route
Command-W to that same tab command; repeated chords cannot become a bare double-
Command dismissal. This mapping is local to Kai. Command-Q still quits and bare
double-Command still dismisses the unified surface.
