<div align = "center">

<p align="center">
  <video src="https://github.com/user-attachments/assets/edd3a4d6-ef99-4bbe-8780-9a0e0c8b51ae" width="850" controls></video>
</p>

<p align="center">
  <video src="https://github.com/user-attachments/assets/2dee9871-5334-4355-ae9b-2ce6bab71b39" width="700" controls></video>
</p>

<p align="center">
  <video src="https://github.com/user-attachments/assets/ee597c1f-4ab5-4488-82f9-0f3c60db89e7" width="700" controls></video>
</p>

<img width="680" height="558" alt="Screen Recording 2026-10-01 at 5 58 28 PM — under 1 MB" src="https://github.com/user-attachments/assets/29891865-d6cd-4de0-b4b1-04d43d48d97b" />

</div>

### setup

requires macOS 13+, Git, Swift 5.9+, Python 3.11+, and a signed-in Codex CLI

```sh
xcode-select --install
```
```sh
git --version
```
```sh
swift --version
```
```sh
python3 --version
```

### install kai

```sh
git clone https://github.com/v1shay/kai.git
cd kai

codex login
codex login status
codex app-server --help

./scripts/build-app.sh
open .build/debug/Kai.app
```

look for the 海 icon in the menu bar
- ```Command``` to dictate into your last chat
- ```Fn``` to dictate into a new chat
- ```Control``` + ```Option``` to open projects / chats 

kai uses your existing local Codex login and requires no separate account or API key; the optional local speech feature needs Piper

`build-app.sh` packages an optimized release executable at the existing
`.build/debug/Kai.app` location. For an unoptimized debugger build, use
`KAI_BUILD_CONFIGURATION=debug ./scripts/build-app.sh`.

[Performance changes and verification](docs/performance-audit.txt)

### permissions

for dictation, allow **Microphone** and **Speech Recognition**

if global shortcuts dont work: enable Kai under **System Settings → Privacy & Security → Accessibility** and **Input Monitoring**, then reopen

### agent CLI

`kai_cli.py` provides a scriptable non-curses interface for automated agent workflows

```sh
python3 kai_cli.py --cwd /path/to/project \
  -c '/projects /path/to/project' \
  -c '/chats' \
  --format json
```

### troubleshooting

run tests with

```sh
python3 -m unittest -v test_kai.py
```


[Detailed onboarding, updates, and troubleshooting](docs/ONBOARDING.md)

## Voice conversations

Click **◉** beside Send in the notch, or use **海 → start voice in selected chat**.
Kai connects to native Codex voice over WebRTC using your existing ChatGPT login.
It stays with the selected Codex chat, agents, tools, and approvals. Click **■**
to end voice. Voice starts in the compact notch with the pet and listening,
thinking, or speaking indicator. Click the compact band, choose **海 → expand notch**,
or hold **Control–Option** to open the transcript and approval controls during the
call. **Escape** or **Close** returns to compact view without ending voice.
There is no Apple speech fallback. Native voice requires a Codex
chat; Code chats retain their separate dictation integration.

Choose **海 → Voice & Audio → Realtime voice** to select the native voice for your next conversation.
Your choice persists across launches. **Codex default** uses Codex's configured
voice. Changing the selection leaves the current call running with its original voice.

Choose **海 → Shortcuts & Input → Shortcut actions → Hold Command / Fn / Option** to assign any of:

- Send in new chat
- Send in active / last opened chat
- Send in new chat with appshot context
- Realtime voice in selected chat
- Realtime voice in selected chat with appshot context
- Realtime voice in new chat
- Realtime voice in new chat with appshot context

Assignments persist. Defaults remain Command → active/last chat, Fn → new chat,
and Option → new chat with appshot. For voice, **hold the chosen key and speak
your first prompt**, then **release to connect and send that recorded audio**.
The call continues hands-free; press the shortcut again or choose **end voice
conversation** to end it. Short taps and keyboard chords cancel recording.
The voice button also accepts a typed draft as the first prompt. Dictation's release-to-send
behavior remains in **Dictation auto-send**. Voice with appshot attaches the
captured image to the same Codex chat and brings its description into the call.

[Shortcut behavior and defaults](docs/SHORTCUTS.md)

[Authentication, transcript compatibility, and verification](docs/VOICE.md)

## Defaults and menu organization

**海 → Chats & Models → Default model** saves a model from Codex's available
catalogue when creating a new chat, including chats created by voice or appshot
shortcuts. Existing chats keep their chosen model for manual sends, automatic
dictation sends, and voice. **Follow chat model (default)** restores the previous
new-chat behavior. An unavailable saved model stops new-chat creation with a
clear message instead of silently using another model.

**海 → Shortcuts & Input → Hotkey hold timing** has separate sliders for
Command, Fn, and Option, from 0.10 to 5 seconds. Original timings remain the
defaults: 1 second for Command/Fn dictation, 0.2 seconds for Option dictation,
and 0.25 seconds for voice. Voice starts recording and shows its listening
indicator only after the configured hold time. Speak after that indicator appears;
release to connect the call. Quick taps never start recording or animate the notch. Timings
persist across launches and can be restored with **Restore original hold timings**.

All existing controls remain under **Chats & Models**, **Shortcuts & Input**,
**Voice & Audio**, **Appearance**, **Media**, and **Advanced & Connections**.
Opening the notch, starting/ending voice, and reviewing voice approvals remain
at the top of the menu.

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

Local builds use the existing “system local code signing” certificate when available. You can select a certificate with `KAI_SIGNING_IDENTITY`. Without a certificate, builds use ad-hoc signing; rebuilding may require granting Screen Recording again. After changing signing identity, remove the old Kai entry in System Settings → Privacy & Security → Screen Recording, add the rebuilt Kai app, enable it, and restart Kai.


Kai's file viewer scrolls text previews and supports selecting/copying their text.
Double-click a file in the file list, the preview header, or the preview contents
to choose **Copy file** or **Show in Finder**. Copy file places a file reference
on the clipboard for pasting into Finder or other applications. Text previews
show up to 2 MB, with an explicit notice for larger files.

Local file citation cards, file-list rows, and preview headers also show Finder
and clipboard buttons. Click Finder to reveal the file or clipboard to copy the
file reference; click the filename to open its Kai preview.
