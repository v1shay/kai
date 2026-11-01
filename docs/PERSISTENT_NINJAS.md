# Personal Ninjas

Personal Ninjas are durable identities in the sidebar, separate from the Ninja inference provider. Each identity has a name, persona, pixel character style, execution host, provider/model/thinking selection, native permissions, and a revisioned memory document. Its tools run on its execution host. Ordinary Ninja inference continues to keep coding tools on the current Mac while sending inference to the selected engine.

## Use

1. Click the Ninja **+** beside **Ninjas**. An already configured remote host is preferred; This Mac remains an explicit choice. The workspace path always belongs to the chosen execution host.
2. Name it, choose its character and workspace on that host, select an installed provider and its model, then save it.
3. Assign a task now, tonight at 22:00, or daily. Submitting a task captures the current settings. Later changes cannot change permissions or providers underneath an already queued/running task.
4. Saved Ninjas open as message conversations. The gear toggles their separate settings card; character choices are icon-only, and intensity choices use the shared finite tile shimmer. Recent conversation chips select chats; New chat starts empty. Compatible chats resume their native session; a provider/harness change starts a new native session with memory and bounded recent-work context.

The worker reuses the normal native providers, including their shell, editing, MCP and delegation mechanisms. It is not a second chatbot implementation. Native requests retain their task, request ID, and delegated child owner; they can be answered through Kai's existing notch approval/question controls.

**Full access** and **Never ask** are separate controls. Read-only remains read-only. For Codex, Full + Never sends danger-full-access + never. For Claude, explicit editable Full + Never selects bypassPermissions; read-only still removes its mutating built-ins. Other providers keep their native permission protocol.

## Remote operation

The Studio installation uses `~/.kai-ninja/Worker.app`, a user LaunchAgent, and the existing owner-authenticated Tailscale inference host. No public listener or Funnel is added. The worker has no windows, microphone, pet renderer, or GUI event loop. It sleeps on events or the next scheduled deadline. Models are not polled while idle; owned Ollama models and llama.cpp processes retire after approximately two minutes. Native provider brokers retain their existing finite idle lifetime.

Remote tasks already run on the Studio, so closing Kai or disconnecting the laptop does not own or terminate them. After a worker restart, ambiguous in-progress work is marked interrupted, never automatically replayed. Queued work and memory remain durable. After reboot, a user login is still required for a user LaunchAgent and FileVault; this installation does not disable either protection.

`--keep-reachable` holds an AC-only system-sleep assertion. It does not hold a display-sleep assertion. The Studio display is configured to sleep after five minutes. No power assertion is added to This Mac by the local UI.

To install a later packaged build on a destination Mac with an existing Ninja host:

```sh
python3 scripts/install-ninja-worker.py '/path/to/Kai Unified.app' --keep-reachable
```

The installer refuses an update while the worker has pending tasks. Providers use that host's installed CLIs and logins. Installing a provider is distinct from signing into it.

## Calls and the remote computer

**Call** is a personal-Ninja control, using Codex's native realtime transport on the selected execution host. Only the media peer and microphone live on the laptop. The host uses its own Codex login, Ninja persona/memory, host workspace, and captured permissions. Calling does not replace the identity's saved agent or model. Other providers' recent conversation context is carried into the host Codex call. Recorded shortcut openings use that same call path.

The bounded startup snapshot is supplied through realtime V3 `initialItems`, with `includeStartupContext: false` (verified against the Studio's installed Codex 0.164.0 schema). This avoids a second automatic machine-tree scan: under a user LaunchAgent that scan was observed blocked in `codex_core::realtime_context::render_tree` at a macOS privacy-protected folder. This setting does not change the native workspace, sandbox, approvals, or macOS folder permissions. Persona, memory, recent work, and the last four conversation turns share a startup budget below 30 KiB; their full stored versions remain intact.

SDP offers/answers live only in the worker's bounded in-memory signaling buffer, never the persisted UI snapshot or task record. Calls use a cursor-based revision long-poll. A lost client expires its signaling lease after 90 seconds; disconnected calls end without canceling independently assigned background work. Closing the panel does not hang up. End call flushes the native transcript; spoken corrections replace partial segments, with a bounded durable transcript for later context.

**Computer** opens Apple's native Screen Sharing app at the selected host's Tailscale address. Its native video stream and Observe/Control modes are used; video is not embedded in Kai's message card. macOS Screen Sharing must be enabled on the host, with the account authorized there. Tailscale network membership does not replace macOS's screen-sharing authentication.

**Take over** first persists a hold for this Ninja, stops its active tasks, and waits for native startup/tools to settle, then opens Screen Sharing. Queued work stays queued. **Resume Ninja** releases the hold. Holds survive worker restarts. Other Ninjas are unaffected.

## Reference files

**Send reference** uses Taildrop for explicitly selected files. It appends their host Downloads names to the message draft. It does not transfer a local project, change the Ninja's workspace, or make laptop files its execution root. The old workspace-import RPC and handoff button are retired. Protected host folders keep their normal macOS permission requirements.

## Verification for 0.12.31

- Sixteen focused worker/UI tests: single-line readable model labels, conversation/settings layout, selected-chat boundaries, animated intensity selection, exact status glyphs, persistent takeover holds, bounded spoken corrections, call-provider routing without mutating the saved profile, startup-context bounds, and private cursor-based signaling.
- Fourteen existing media lifecycle checks cover capture/start/answer/stop, mute, recorded prompts, and stale media callbacks.
- Existing native Codex WebRTC/approval tests are rerun alongside sidebar and voice routing checks.
- Studio Screen Sharing responds with its native RFB protocol after being enabled by the user. The installed LaunchAgent passed a real WebRTC media connection and clean call end, starting from a Claude Ninja while preserving its saved provider/model. A separate live test stopped an active task for takeover, held the queue, resumed it, and verified the resulting file on the Studio. Results are recorded in `.build/validation/ninja-conversations/`.
- Manual visual review and a live spoken microphone conversation are not claimed by these automated checks.

## Verification for 0.12.28

- Native Codex: actual shell command, editing an existing file, persistent memory write, and a new-chat memory read after worker restart.
- GPT-OSS20B via Codex and Claude: actual shell commands and existing-file edits.
- Studio native Codex: file edit, memory, client-disconnected completion, scheduled task, and an outside-workspace command requiring an exact native approval.
- Studio Ninja/Claude/Ollama/GPT-OSS20B: file edit, scoped MCP memory, and scheduled memory retrieval.
- Eight focused worker/UI checks; four GitHub UI checks and a live account bootstrap with the hub closed; eighteen inference protocol/auth checks; five archive/liveness checks.
- The broader run passed 918 checks, with seven failures reproduced using the preceding build. Those concern older project navigation/pinning, removed composer controls, provider-picker test selectors, and completion-diff UI behavior; this pass does not claim they were resolved.
- Actual Taildrop delivery to the Studio succeeded. Receipt/import verification was blocked by the Studio's Downloads privacy prompt during validation; do not treat delivery alone as a completed handoff.

Full live microphone interaction and manual visual review are outside this pass. Existing notebook, project and conversation files are not migrated or rewritten by worker installation.
