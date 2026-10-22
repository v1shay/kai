# Native Codex voice in Kai

Reopen the rebuilt Kai app, select a writable **Codex chat**, and click **◉**
beside Send, or choose **海 → start voice in selected chat**. The call starts in
the compact notch with a pet and audio/thinking indicator. Click the compact band,
choose **海 → expand notch**, or hold **Control–Option** to open the transcript and
approval controls at any time. Escape or Close compacts the view while voice
continues; automatic updates preserve your chosen view. Press **■**, the assigned
voice shortcut again, or the menu's end control to end the call. Voice uses
Codex's speech model, the selected thread, its agents, tools, and approvals.
There is no automatic Apple speech fallback.

**海 → Voice & Audio → Realtime voice** selects the native voice for the next conversation:
Arbor, Breeze, Cove, Ember, Juniper, Maple, Sol, Spruce, or Vale.
The checked choice is saved across launches and applies to the voice button,
menu, and every realtime shortcut, including appshot and new-chat variants.
**Codex default** leaves the voice choice to Codex. Changes never interrupt an
existing call; end it and start another to hear the new selection. This setting
is separate from **local ONNX voice**, which controls optional response playback.
The list was checked against the installed App Server's `thread/realtime/listVoices`
and a real V3 start. V3 accepts the V1 voice family; the separately advertised V2
voices are rejected on this connection. Kai supplies the choice through
`thread/realtime/start.voice`.

Choose **海 → Shortcuts & Input → Shortcut actions → Hold Command**, **Hold Fn**, or **Hold Option**,
then choose voice in the **selected chat** or a **new chat**, each available
with or without **appshot context**. New-chat voice creates a standalone Codex
chat and waits for confirmation before connecting voice. **Hold the key until the listening indicator appears,
speak your first prompt, then release.** Kai records the held microphone audio
locally, waits for any appshot, prepares the destination, then sends the recording
through native WebRTC voice and switches to the live mic. No Speech Recognition
API is involved. Short taps under the configured hold minimum (0.25 seconds by
default) and keyboard chords cancel without
starting voice. Press again to end; Control–Option opens the current voice chat
during a call and the running chat otherwise. Assignments
persist across launches. The other choices are **Send in new chat**, **Send in
active / last opened chat**, and **Send in new chat with appshot context**.
Defaults remain Command → active/last chat, Fn → new chat, and Option → new chat
with appshot. Dictation auto-send settings apply only to the three Send actions.
See [shortcut behavior](SHORTCUTS.md).

Voice with appshot captures application text and, when available, a screenshot
during the hold. After release, microphone permission and the WebRTC offer, Kai submits
the image as a normal `localImage` input to a Codex turn in the same thread. That
turn describes the captured state and its response is handed into native voice.
The image remains in the ordinary Codex transcript. Text metadata also goes into
V3 `initialItems`; those items support text only, so they do not carry the image.
The visual description can arrive shortly after the call connects. Starting
with appshot in an existing chat requires its turn to finish; the new-chat option
creates a fresh chat while the old one keeps working. It never steers an image
into an already running turn. Captured content is supplied as untrusted context.

The public [App Server documentation](https://learn.chatgpt.com/docs/app-server)
describes normal image inputs and turn streaming. The voice startup fields are
experimental and were checked against the installed CLI's generated protocol.

## Authentication: the earlier diagnosis was wrong

The existing ChatGPT login works for native voice. The earlier
`realtime conversation requires API key auth` result came from testing
`transport: {type: "websocket"}`. It did not establish that voice requires a
separate API key.

The installed ChatGPT/Codex app's voice code uses WebRTC. Its newer path creates
a call through `/wham/realtime/calls` and attaches App Server with an
`existingCall`; its other path sends a WebRTC offer directly to
`thread/realtime/start`. Kai uses the latter path, verified against the installed
`codex-cli 0.162.1` with `account/read` reporting `chatgpt` authentication.

Kai now:

1. Creates a WebRTC peer in a private, nonpersistent WebKit document embedded in
   Kai. It offers an audio transceiver and the `oai-events` data channel.
2. Sends the offer to the existing headless App Server connection:
   `thread/realtime/start`, transport `webrtc`, version `v3`, and the same
   `gpt-live-1-codex` voice model used by the installed app.
3. Applies the SDP answer received through `thread/realtime/sdp`.
4. Enables input and output only after both App Server and the peer/data channel
   are ready. WebRTC handles audio transport, echo cancellation, jitter, and
   speaker playback. Spoken interruptions are handled by the realtime service.
5. Receives transcript deltas, tool events, delegated agent work, and approvals
   through the original App Server connection and selected thread.

**Authentication and token refresh stay in App Server.** Kai does not extract,
copy, log, or inject tokens from the Codex app. The WebKit document never receives
a bearer token, browser session cookie, or API key. It loads only Kai's bundled
media script, blocks navigation, and uses a main-frame microphone permission
delegate. macOS Microphone permission is still required. Speech Recognition
permission is used only by the separate hold-to-dictate feature.

## Chat and transcript behavior

Voice is bound to the chat selected at startup. Changing chats cancels startup
or ends the call; stale SDP answers and callbacks cannot activate another chat.
Typing during a call creates or steers an ordinary persisted turn in the same
Codex thread; App Server hands its response to the live voice session. V3
`appendText` is quiet context injection, so Kai does not use it as a prompt trigger. Stopping
voice does not interrupt agent work already delegated to that chat; the existing
Stop control interrupts that task. The menu's stop-speaking control ends an
active voice call.

Code chats belong to a separate agent integration and cannot attach to Codex's
native voice sideband. Kai explicitly asks you to select a Codex chat instead of
silently starting a different voice engine or creating another chat.

The installed CLI advertises `thread/timeline/list` but still returns
`not supported yet`. When that endpoint is implemented, Kai reads the canonical
mixed timeline in rollout order. On current builds, native transcript deltas
stream into Kai's conversation, and completed segments are restored from an
owner-only journal under `~/Library/Application Support/Kai/Voice/`, keyed by a
hash of the thread ID. Delegated Codex turns retain their existing server
history. Full spoken dialogue is restored in Kai; visibility of every spoken
segment in other clients depends on their server timeline support. Audio and
SDP credentials are never written to those journals. An orderly stop saves
partial text; force-quitting can lose an unfinished segment.

## Verification

```sh
python3 -m unittest -q test_kai_voice.py test_kai.py test_kai_activity.py \
  test_kai_cache.py test_kai_code.py test_kai_reliability.py test_kai_experimental.py
node --test test_voice_media.mjs
swift test --disable-sandbox
./scripts/build-app.sh
python3 scripts/diagnose-codex-voice.py
python3 scripts/diagnose-codex-voice.py --held-prompt
python3 scripts/diagnose-codex-voice.py --held-prompt --voice ember
```

Verified on October 16, 2026: the existing ChatGPT login started native voice,
returned an SDP answer, connected the WebRTC peer and data channel, delivered
inbound audio packets, and streamed an assistant transcript. The held-prompt
diagnostic also passed: the recorded fixture was sent through the initial-prompt
path, its user transcript appeared, and assistant audio/transcript returned.
The live mic track and speakers stayed muted throughout that fixture test.

The opt-in live diagnostic creates an ephemeral test chat using the existing
ChatGPT login. The default diagnostic never opens a microphone and keeps output
muted, feeds a generated test-audio fixture into the voice model, and checks native startup,
SDP negotiation, peer/data-channel connection, inbound audio bytes, and an
assistant transcript. The macOS `say` command creates the diagnostic fixture only; it is not part of
the voice conversation implementation. In the diagnostic window, click **Start
test audio** when enabled to satisfy WebKit's trusted-gesture rule for synthetic
Web Audio. Normal microphone transport does not depend on that synthetic source. The `--held-prompt` variant exercises the same initial audio replay as normal
voice. It obtains a microphone stream so WebKit permits audio startup, but never
enables that real microphone track, and keeps output muted. It checks both user
and assistant transcripts. The diagnostic closes the peer and App Server afterward and never
prints account identity, tokens, SDP credentials, or transcript content.

Automated tests cover media activation ordering, late permission results,
cancellation, stale SDP, service errors, private message serialization,
backpressure, transcript recovery, mixed history, and the shared approval/tool
flow. Physical microphone capture, acoustic echo cancellation, and audible
speaker behavior require a real-device conversation.

Protocol opt-in and managed authentication follow the
[official App Server documentation](https://learn.chatgpt.com/docs/app-server).
WebKit media capture uses Apple's public
[media permission delegate](https://developer.apple.com/documentation/webkit/wkuidelegate/webview(_:requestmediacapturepermissionfor:initiatedbyframe:type:decisionhandler:)).
Experimental wire details were verified from the installed app and generated
protocol bindings; these may change with future Codex releases.


A held first prompt must contain sustained microphone activity. Empty recordings,
silence, quiet background noise, and isolated clicks are discarded on release
without creating a chat or starting a realtime connection. A valid recording
keeps its quiet lead-in and tail. Voice startup failures release the indicator
and leave a text status; changing chats clears voice input and error state.
