# Kai

Kai is a macOS menu bar companion for local Codex projects and chats, with an
animated pet, a notch interface, file previews, and voice dictation. It also
includes a terminal client and a scriptable agent CLI.

## Onboarding: from clone to your first chat

### 1. Prepare your Mac

You need macOS 13 or later, Git, Swift 5.9 or later, Python 3.11 or later, and
a signed-in Codex CLI. Internet access is needed to install dependencies,
sign in, and use Codex. Kai's Python code uses only the standard library;
there is no `pip install` step or separate Kai API key.

Open Terminal and install Apple's Command Line Tools if you do not already
have them:

```sh
xcode-select --install
```

Finish the installer before continuing. If the tools are already installed,
skip this step. Check the available tools:

```sh
git --version
swift --version
python3 --version
```

If Swift is older than 5.9, update your developer tools (or install a compatible
Xcode version). If Python is missing or older than 3.11, install a current
Python. A convenient route is [Homebrew](https://brew.sh): follow its installation
and shell setup instructions first, then run:

```sh
brew install python
brew install --cask codex
```

Skip packages you already have. Alternatively, if Node.js and npm are already
installed, install Codex with `npm install -g @openai/codex`. See the
[official Codex CLI setup guide](https://developers.openai.com/codex/cli)
for installation options.

### 2. Clone Kai

From the folder where you keep your projects:

```sh
git clone https://github.com/v1shay/kai.git
cd kai
```

Run the remaining repository commands from this `kai` folder.

### 3. Sign in to Codex

```sh
codex --version
codex login
codex login status
```

Complete the browser sign-in with an account that has Codex access. If you are
already signed in, just check the status. Kai uses the local Codex credentials
and configuration; it has no separate sign-in screen. The
[Codex login reference](https://developers.openai.com/codex/cli/reference#codex-login)
also documents device authentication and API key login.

Confirm that your CLI includes the App Server used by Kai:

```sh
codex app-server --help
```

### 4. Build and launch the notch app

```sh
./scripts/build-app.sh
open .build/debug/Kai.app
```

The script builds the Swift app, bundles the Python bridge, pets, and equation
renderer, then signs the bundle locally with an ad hoc signature. You do not
need an Apple Developer account. This is a local source build, not a notarized
download. The result is `.build/debug/Kai.app`.

Look for the **海** menu in the macOS menu bar. Kai is a menu bar app and does
not show a Dock icon. Choose **show notch**, then hold **Control + Option** for
one second to open the chat panel. Holding **Command** for one second opens
the notch and starts dictation; use the menu and Control + Option gesture for
the initial typed-chat walkthrough.

You can keep launching the bundle from this folder. Optionally, copy
`.build/debug/Kai.app` into your Applications folder using Finder and launch
that copy instead. Keep the clone for rebuilding and updates. Kai does not
automatically configure launch at login.

### 5. Allow permissions for voice and global shortcuts

For dictation, hold Command for one second and allow **Speech Recognition**
and **Microphone** access when macOS asks. Release Command to finish dictation.
After accepting the first permission prompts, try the gesture again.

If the global keyboard gestures do not work, check **System Settings → Privacy
& Security → Accessibility** and **Input Monitoring** for Kai (or the launching
app macOS identifies), and enable the requested access. Microphone and Speech
Recognition access can be changed in the same Privacy & Security area. Quit
and reopen Kai after changing permissions. You can type prompts without
microphone or speech access.

### 6. Select a project and send your first prompt

On first launch, Kai starts with this repository; later it remembers the last
selected project. The project list also includes folders associated with your
local Codex chats.

1. Open the chat panel and click a project in the left sidebar.
2. Click an existing chat to view its history, or **+ in project** to create a
   chat in the selected folder. **+ new chat** and the action bar's **+ New**
   create a standalone chat instead, under
   `~/Library/Application Support/Kai/Chats`.
3. Type a first prompt such as “Summarize this project without changing files.”
   Press Return or click Send.
4. Read the response and activity indicator. Use **Approve** or **Reject**
   when a task requests permission, or **Stop** to interrupt a running task.
5. Click the file icon to browse the selected folder. Drop images onto the
   interface to attach them to your next prompt.

To add another existing folder that has no Codex chats yet, create a chat there
with Kai's terminal client:

```sh
python3 kai.py --cwd "/absolute/path/to/your/project"
```

Select that project, choose **New chat** (or enter `/new`), then quit the
terminal client. Reopen the notch app if the folder has not appeared yet.
Replace the example path with a real folder. Chats currently owned by another
Codex process may be readable but read-only; release that chat in the other
client or create a new chat in Kai to send a prompt.

### 7. Customize Kai

Use the **海** menu to choose a **pet**, adjust **pet size**, change the
**corner gradient**, toggle **sounds**, or toggle **render equations**.
Choose **quit kai pet** to exit. Double-tap Command or Control while the notch
is open to close it.

## Optional speech, automatic compaction, and media

The **海** menu includes these settings, all off by default:

- **speak Codex responses** reads newly streamed assistant text in the selected
  chat using a local Piper ONNX voice. **local ONNX voice** selects a discovered
  model; **rescan local voices** refreshes the list; **stop speaking** clears
  queued audio. Sending another prompt or starting dictation also stops speech.
- **compact immediately on send** shrinks the chat panel as soon as you send a
  prompt. Approvals, errors, and task completion can reopen the panel.
- **YouTube when idle (Webby, Dia, Chrome)** detects playing videos and Shorts
  through browser accessibility controls. The page must expose its video URL
  and a Pause control; background tabs and localized controls may not expose
  these. **Spotify when idle** reads Spotify's playback state and artwork.
- **media thumbnail size** adjusts the cover independently of **pet size** and
  remembers your choice. Artwork crossfades and gradient sweeps blend status
  changes; rapid switches cancel older transitions, and Reduce Motion uses
  shorter fades.
- When media is playing and Kai is idle, the compact notch shows the artwork,
  artwork-derived gradients, and the listening waveform. The waveform measures
  the speaker output mix, excluding Kai's own audio. It does not use the
  microphone. Other apps' audio can therefore also affect the bars. Codex work,
  approvals, dictation, and the expanded chat panel take priority over media.

Use **allow media permissions…** to request Accessibility and Screen Recording
access. Spotify also requests macOS Automation access when first queried.
Enable the requested permissions in System Settings and restart Kai if macOS
requires it. Audio/video sample buffers are discarded after measuring the
levels; no recording is saved. Kai fetches artwork from the media provider.
The menu status reports speaker connection or permission failures.

Speech needs an installed `piper-tts` Python runtime and a complete Piper model
(`voice.onnx` plus `voice.onnx.json`). Discovery uses Spotlight and also scans
`~/.local/share/piper` and `~/.cache/piper`; unrelated ONNX models are excluded.
Kai does not download models. If you need a separate runtime, from the clone:

```sh
python3 -m venv .speech-venv
.speech-venv/bin/pip install piper-tts
defaults write com.kai.notchprototype speechPython -string "$PWD/.speech-venv/bin/python"
```

Restart Kai after configuring the runtime. Without that setting, Kai checks
Anaconda, Homebrew, and `/usr/local` Python locations. The selected model stays
loaded, short phrases are synthesized as text arrives, and only a small amount
of audio is queued ahead. Cold model loading and phrase length affect latency;
there is no fixed latency guarantee. The status shows the first-audio timing.

To check speaker capture directly without opening the notch:

```sh
.build/debug/Kai.app/Contents/MacOS/NotchPrototype --diagnose-speaker-audio
```

This runs a four-second capture check and prints packet count and peak level.
Play audio during the check; silence should produce a zero peak. Run `swift test` and `python3 -m unittest -q
test_kai.py` for regression checks. For the optional live voice playback test,
run `KAI_TEST_VOICE="/absolute/path/to/voice.onnx" swift test`.

## Updating an existing installation

Quit Kai from its menu, then run these commands inside your clone:

```sh
git pull --ff-only origin main
./scripts/build-app.sh
open .build/debug/Kai.app
```

If you copied Kai to Applications, replace that copy with the newly built
bundle and open it instead. Check macOS permissions again if a rebuilt app's
shortcuts or dictation stop working. If you installed Codex with Homebrew,
update it with `brew upgrade --cask codex`; for npm, rerun
`npm install -g @openai/codex`.

## Setup troubleshooting

| Symptom | What to check |
| --- | --- |
| `git`, `swift`, or `xcrun` is missing; build reports missing developer tools | Finish installing Apple's Command Line Tools and check `xcode-select -p`. If using full Xcode, open it once to finish its setup. |
| Python syntax/import errors or the bridge exits at launch | Check `python3 --version`; Kai needs 3.11+. The GUI searches `/opt/homebrew/bin`, `/usr/local/bin`, and `~/.local/bin` before its inherited PATH. A Python installed only in a shell-specific environment may not be found. |
| `codex: command not found`, or App Server fails to start | Install the CLI, check `codex --version` and `codex app-server --help`, and check `codex login status`. Restart Kai after fixing installation or login. |
| CLI works in Terminal but Kai cannot connect | Install Python and Codex in a standard location listed above. GUI apps may not inherit shell configuration such as an npm version manager's PATH. Run `python3 kai_cli.py --cwd . -c '/status' --format json` from the clone to diagnose the CLI connection. |
| Kai seems to launch without a window | Look for the **海** menu; choose **show notch** and hold Control + Option for one second to open the chat panel. |
| Dictation or global shortcuts do not work | Review the permissions in step 5, then quit and reopen Kai. |
| Your project is missing | Create a local Codex chat in that folder using the terminal flow in step 6, then reopen Kai. |
| A chat is read-only | Another Codex client owns it. Release it there or create a new chat. |

For a direct bridge diagnostic, run `python3 kai_bridge.py --cwd .` in Terminal
and inspect its startup output/errors; press Control-C to stop. Include the
error and your macOS, Python, Swift, and Codex versions when filing an
[issue](https://github.com/v1shay/kai/issues).

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
standalone chat; **+ in project** starts one in the selected project. Type and press Return or click Send; dropped
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

## OpenAI Dot activity

Dots do not appear in Codex App Server's thread list. Kai instead includes a
small MCP bridge with two explicit tools: `kai_activity` and `kai_complete`.
They carry only a task id, short title, indicator state, and short status. The
bridge never receives a Dot transcript, prompt, or private tool arguments.

For a local MCP client, configure this command:

```sh
python3 /absolute/path/to/kai/kai_dot_mcp.py --stdio
```

For a Dot connection in ChatGPT developer mode, generate a temporary URL token
and run the Streamable HTTP transport:

```sh
KAI_DOT_TOKEN=$(python3 -c 'import secrets; print(secrets.token_urlsafe(32))')
python3 kai_dot_mcp.py --host 127.0.0.1 --port 8766 --token "$KAI_DOT_TOKEN"
```

Expose the printed `http://127.0.0.1:8766/<token>/mcp` address through a
temporary HTTPS tunnel, then register the resulting URL under **ChatGPT
Settings → Security and login → Developer mode → Plugins**. Keep the token and
URL private and stop the endpoint after testing. A stable deployment should
replace the development URL token with MCP OAuth 2.1 or OpenAI-managed mTLS.
For example, with ngrok installed, run `ngrok http 8766` and replace the local
origin in the printed MCP address with the HTTPS origin ngrok gives you.

Keep Kai running on this Mac. Both processes use
`~/Library/Application Support/Kai/dot-events.jsonl`; set `KAI_DOT_EVENTS` in
both environments to choose another shared path. The tool descriptions tell
the Dot to report meaningful state changes and always finish the task, so the
notch uses the existing animation, pet assignment, completion, and failure
behavior without reading the conversation.

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
