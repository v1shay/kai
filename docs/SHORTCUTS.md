# Configurable input shortcuts

Open **海 → Shortcuts & Input → Shortcut actions**, choose **Hold Command**, **Hold Fn**, or **Hold
Option**, then select an action. Every key offers all seven choices independently.
The checkmark shows its saved assignment.

| Action | Destination | Context |
| --- | --- | --- |
| Send in new chat | Fresh standalone Codex chat | Dictated text |
| Send in active / last opened chat | Current selected chat; otherwise a running chat or the last opened chat | Dictated text, continuing the existing draft |
| Send in new chat with appshot context | Fresh standalone Codex chat | Dictated text plus captured application text and screenshot |
| Realtime voice in selected chat | Current chat, with the same running/last-opened fallback | Native Codex voice |
| Realtime voice in selected chat with appshot context | Same destination as realtime voice in selected chat | Native Codex voice plus an appshot image turn in that chat |
| Realtime voice in new chat | Fresh standalone Codex chat | Native Codex voice |
| Realtime voice in new chat with appshot context | Fresh standalone Codex chat | Native Codex voice plus captured application text and screenshot |

Defaults preserve the original shortcuts: **Command → active / last opened**,
**Fn → new chat**, **Option → new chat with appshot**. Older saved “dictation”
assignments migrate to the corresponding key's original action. Existing voice
assignments remain voice.

By default, hold Command or Fn alone for one second to start dictation. Option
dictation uses 0.2 seconds. **Shortcuts & Input → Hotkey hold timing** lets you
adjust each key from 0.10 to 5 seconds and restore the original timings. Voice
defaults to a 0.25-second activation hold. Recording and the listening indicator
begin only after that threshold; speak once the indicator appears. Release to finish recognition. **Dictation
auto-send** still controls sending separately for each key; it defaults on for
Option and off for Command/Fn. A new chat or context attachment must be
acknowledged before recognized text can be sent. A failed request or changed
conversation cancels automatic sending and preserves the available draft.
Send or clear an existing draft before choosing an action that creates a chat.

All realtime actions **record microphone audio after the activation hold**. Wait
for the listening indicator, speak the first prompt while holding, then release. After release Kai prepares the
chosen chat, connects native Codex voice, sends the held recording, and switches
to the live microphone. No Apple speech recognition or client transcription is
used. The first recording is memory-only, bounded to 60 seconds, and discarded
when cancelled. Taps shorter than the configured minimum (0.25 seconds by default)
and keyboard chords cancel without
starting a call. Press the shortcut again to end or cancel startup.

Voice starts in the **compact notch**: pet plus listening, thinking, speaking or
attention indicator. Click the compact band, choose **海 → expand notch**, or hold
**Control–Option** to open the full transcript and Approve/Reject controls while
voice continues. **Escape**, **Close**, or **海 → compact notch** returns to compact
view without ending the call. Agent work, completion, approvals, and state
refreshes preserve your choice of view. **海 → review voice approval** also opens
a small review/allow/reject menu bound to the reviewed request. The composer voice button and menu
start/end control remain available; a typed draft is sent as the first prompt
only after the voice connection is ready, and is cleared only on acknowledgement.
Use **■**, **end voice conversation**, or the assigned voice shortcut again to end
the call.
Voice requires a writable Codex chat and the existing ChatGPT login; the separate
Code integration supports dictation. [Native voice and appshot details](VOICE.md).

**Chats & Models → Default model** sets the model for new chats and automatic
sends, independently of the in-chat model controls. New voice sessions use it
for the backing Codex agent. Manual sends keep the in-chat selection, and a
running turn keeps its current model. The native realtime voice model and
Code's own model controls remain separate.

New-chat voice waits for confirmation that the fresh chat was created before
connecting voice and forwarding the held prompt. Local prompt recording happens
before chat creation. It works with no chat selected or while leaving a Code
chat, and can create a fresh chat while the previous chat is working. If creation
fails or startup is cancelled, voice never starts in the old chat.

Appshot capture happens during the hold, before preparing the destination chat. **Prefer lightweight appshot context**
applies to every appshot assignment: it prefers selected text and captures a
screenshot when selected text is unavailable. Screen mode captures the screenshot
as well. Existing macOS screen-recording and accessibility permissions determine
what context is available. Releasing a dictation key during capture cancels it;
voice startup waits for both the held audio and appshot after release. Cancelled captures
discard late screenshots.

Control-Option retains its running-chat expansion shortcut. Modifier chords do
not start input shortcuts, and releasing one modifier from a chord does not turn
the remaining key into an appshot or voice shortcut. Release and press that key
again to start a new hold. Closing the notch or navigating to another chat
cancels pending preparation.


## Expanded notch input

Command, Fn, and Option use the same configured actions and hold timings in the
expanded notch. Voice shortcuts keep the expanded chat open while recording,
creating/selecting the destination, attaching any appshot, and connecting native
Codex voice. The selected-chat action uses that chat's existing context and tools.

**Shortcuts & Input → Expanded notch: Command dictation only** is off by default.
Enable it to use Command for text dictation into the current draft, without
automatic sending; Fn and Option input shortcuts are disabled in the expanded
notch. Compact-notch shortcut assignments are unchanged. The dedicated voice
button still controls native Codex voice.

The expanded editor supports Command-C/V/X/A and Command-Z/Shift-Command-Z.
Typing or pressing a keyboard shortcut cancels a pending modifier hold, so a
copy/paste chord cannot later start listening while Command remains held.
