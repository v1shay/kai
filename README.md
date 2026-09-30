# Kai

## Notch app

Build the signed local bundle with `scripts/build-app.sh`, then open
`.build/debug/Kai.app`. Kai needs a signed-in `codex` CLI and Python 3.11+ on
the Mac. The bundle contains the Swift frontend, Python App Server bridge,
animation catalog, and referenced spritesheets. The first launch opens the
last selected project or this repository.

Use the 海 menu's **show notch** item, or hold Command for one second, to open
the notch. Click a project to reveal its Codex chats; click a chat to read its
history. The conversation scrolls independently. The file icon opens the
selected project's real file tree. Click a folder to fold or unfold it, or a
file to preview text, photos, and other Quick Look content. **+ New** starts a
chat in the selected project. Type and press Return or click Send; dropped
images are attached to that prompt. Approve, Reject, and Stop appear when the
selected task needs them. Dictation remains available through the existing
Command hold gesture.

The App Server provides project and chat state, messages, structured activity,
turn events, and approvals. Kai reads files directly from the selected local
project. A chat owned by another Codex process remains readable and is marked
read only; sending to it becomes available when that writer releases it. The
existing animations and appearance controls are documented in
`INDICATOR_ENGINE.md` and `NOTCH_DESIGN_PLAN.md`. macOS may request microphone
and keyboard monitoring permission from the launching app.

The menu bar's **render equations** setting toggles bundled KaTeX rendering in
the native conversation view, including existing chat history. It handles
`$…$`, `$$…$$`, `\(…\)`, and `\[…\]`; invalid equations stay readable as
source. The **pet** menu stays open while choosing or randomizing pets; choose
**done**, press Escape, or click outside to close it.

## Terminal client

A minimal terminal client for local Codex tasks. Requires Python 3.11+, Git, and a signed-in `codex` CLI on macOS. No Python packages are required.

```sh
python3 kai.py --cwd /path/to/project
```

Kai opens on Projects. Click a project, then click a chat and type a message. Arrow keys and Enter do the same thing. Use the on-screen Back and Quit buttons, or press Esc to go back. In Chats, New chat creates a task in that project. Mouse clicks require a terminal with mouse reporting enabled.

`/help` lists the slash commands. `/projects <number|path>` opens a project, `/chat <number|id>` opens a task, `/history` shows the conversation, and `/prompts` lists every prior user prompt in that task. Use Up or Page Up to scroll. `/attach <image>` queues a PNG, JPEG, GIF, WebP, or TIFF for the next prompt.

`/files` opens the full project tree, initially expanded. Use Up/Down or the mouse wheel to select a row, Enter or click to open a file or toggle a folder, Right to fold a folder, and Left to unfold it. Back from a file preview returns to the tree; Back from the tree returns to the active chat.

The App Server connection belongs to Kai. A task that the desktop app currently owns can still be read and is refreshed every two seconds, but Kai shows it as read-only until the other writer releases it. `/status` shows the current access state. Git and file views read the actual local folder. ChatGPT Chat and Work tasks are outside this Codex protocol.

Run the local checks with `python3 -m unittest -v test_kai.py`.

## Agent CLI

`kai_cli.py` is a separate, non-curses frontend for fast scripted checks. It
starts its own App Server connection and never launches, reads, or controls the
TUI. Both frontends use the same Kai command/state layer, so the slash commands,
views, task behavior, approvals, attachments, file previews, and Git output stay
in sync.

Run several commands in one isolated session and return the final view:

```sh
python3 kai_cli.py --cwd /path/to/project \
  -c '/projects /path/to/project' -c '/chats' --format json
```

Send a prompt to the first task and wait up to two minutes for a completed turn
or an approval request:

```sh
python3 kai_cli.py --cwd /path/to/project \
  -c '/projects /path/to/project' -c '/chat 1' \
  -c 'Inspect the current changes' --wait 120 --format json
```

Use `--emit each` for one snapshot after every command. Commands may instead be
piped one per line, or omit `-c` in a terminal for a small interactive shell.
Text output is the same content returned by the TUI's active view; JSON adds
stable state fields such as `threadId`, `activeTurnId`, `readOnly`, and
`pendingApproval` for agents.
