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
