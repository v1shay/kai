# Personal Ninjas

Personal Ninjas are durable identities in the sidebar, separate from the Ninja inference provider. Each identity has a name, persona, pixel character style, execution host, provider/model/thinking selection, native permissions, and a revisioned memory document. Its tools run on its execution host. Ordinary Ninja inference continues to keep coding tools on the current Mac while sending inference to the selected engine.

## Use

1. Click the Ninja **+** beside **Ninjas**. An already configured remote host is preferred; This Mac remains an explicit choice. The workspace path always belongs to the chosen execution host.
2. Name it, choose its character and workspace on that host, select an installed provider and its model, then save it.
3. Assign a task now, tonight at 22:00, or daily. Submitting a task captures the current settings. Later changes cannot change permissions or providers underneath an already queued/running task.
4. Saved Ninjas open as inline message conversations in the normal chat column, alongside Code tabs. The gear toggles their separate settings card; character choices are icon-only, and intensity choices use the shared finite tile shimmer. Recent conversation chips select chats; New chat starts empty. Compatible chats resume their native session; a provider/harness change starts a new native session with memory and bounded recent-work context.

The worker reuses the normal native providers, including their shell, editing, MCP and delegation mechanisms. It is not a second chatbot implementation. Native requests retain their task, request ID, and delegated child owner; they can be answered through Kai's existing notch approval/question controls.

**Full access** and **Never ask** are separate controls. Read-only remains read-only. For Codex, Full + Never sends danger-full-access with shell escalation disabled. MCP elicitations remain deliverable: the native Computer Use per-app gate is covered by the editable/Never choice; user questions are not auto-answered. For Claude, explicit editable Full + Never selects bypassPermissions; read-only still removes its mutating built-ins. Other providers keep their native permission protocol.

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

**Computer** opens an embedded native RFB/Apple Remote Desktop viewer inside Kai. The client uses the pinned MIT RV session engine and the host's existing Screen Sharing service over Tailscale. There is no browser canvas, WebSocket proxy, separate Screen Sharing app, or additional host streaming daemon. Enter the host macOS account in the in-app login form; passwords are ephemeral secure fields, never saved with a Ninja or its drafts. Tailscale membership does not replace host authentication.

The viewer starts in **Watch** mode. It sends no keyboard, mouse, clipboard, or release events in that mode and does not pause the Ninja. **Take over** persists a hold for the selected Ninja, stops its active tasks, waits for native tools to settle, and only then enables input in the existing viewer connection. **Watch** releases held keys/buttons and disables input; it does not silently resume work. Close the viewer and use **Resume Ninja** to release the queue hold. Other Ninjas are unaffected. Closing/changing the owning surface drops the viewer session. Frame events are coalesced, and framebuffer copies happen off the UI thread; no desktop pixels are persisted.

## Shared native surfaces

Saved personal Ninjas supply their own owner token, conversation, model, permission state, draft, and submit/stop path to the extended notch. Their history is distinct from the underlying Code chat. Native menu-bar clicks open the selected identity in the existing mini Code surface, anchored to the menu-bar item; click again or click away to collapse it. Right-click or Option-click retains the settings menu. The selected provider opens its current/recent chat; a pinned personal Ninja opens its own conversation.

Activity uses the Ninja's character rather than the user's selected pet, including the floating task view. Six generated transparent pixel-art running loops are reused in Code, mini Code and the extended notch, with a low-opacity runner crossing the conversation background. The shared conversation pet is hidden on personal-Ninja surfaces; clicking their character does not cycle ordinary pets or recolor the UI. The native thinking shimmer uses each Ninja's color, and message text uses the existing bounded glyph decode animation. GIF assets total under 353 KiB, are cached, and are replaced with a static character under Reduced Motion. Personal drafts are kept separately from ordinary Code drafts.

## Workspace and app permissions

Editable Workspace mode allows shell tools, network requests, and normal system temporary-directory use, while retaining its selected-directory write root. Read only and Full access remain explicit separate choices. Never ask changes approval policy, not the filesystem scope. Editable native Codex sessions enable app tools; Never ask authorizes ordinary app tools without artificial prompt rejection. The developer instructions still require confirmation for destructive operations, credential/security changes, and external publishing/sending unless the exact action was already authorized. Native questions are never auto-answered by the worker. Literal Codex `never` also declines MCP app approvals internally, so the native adapter uses granular policy with shell escalation off and MCP elicitations on. Only the exact native node_repl/cua_repl Computer Use per-app approval is automatically accepted for editable/Never sessions; audio capture, other tools, and forms with user fields are excluded. This grant is not persisted globally.

macOS privacy grants (Accessibility, Screen Recording, protected folders) and provider authentication remain native OS/account boundaries. These are not overridden by a prompt or sandbox setting.

## Reference files

**Attachments** accepts file drops and Finder/image pastes in the Ninja composer, using Taildrop for explicitly selected remote reference files. It appends their host Downloads names to the message draft. It does not transfer a local project, change the Ninja's workspace, or make laptop files its execution root. The old workspace-import RPC and handoff button are retired. Protected host folders keep their normal macOS permission requirements.

## Changes in 0.12.34

Saved Ninjas can be the only open tab. Closing an underlying Code tab no longer starts a new Code conversation or dismisses the selected Ninja. Their corner toolbar uses unboxed action icons; the viewer uses the existing glass surface, centered login fields, and corner Watch/control/close icons. Menu-bar opening retires the visible workspace through the existing morph.

The composer uses workspace, access, model, thinking, orchestration, and icon send controls. These edit the selected host profile; they never submit through the underlying Code chat. The old Continue chat, Send reference, and preset schedule buttons are removed. Drop files or paste Finder files/images into the composer; remote references use the existing Taildrop route, and send waits for file delivery. Orchestration is an explicit profile policy passed to the host harness.

A visible vector companion replaces the faded background runner. Four embedded SVG poses walk the conversation perimeter above the content during activity and perch while idle. The extended notch uses the same vector assets and identity tint, with cached frames and Core Animation motion. Reduced Motion and inactive windows remain static.

## Verification for 0.12.33

- The original menu-bar handler reproduces a GPUI entity-lease panic. The regression test returns that lease before opening mini Code; the signed native status-item probe opens and closes mini Code three times.
- The viewer password-field test renders the real chat column, enters text, and verifies that the GitHub panel behind it remains closed. Canvas input reaches the session; Watch cancels stale takeover callbacks and releases held inputs. Reconnection starts in Watch.
- The final transport received the Studio's actual desktop and delivered a click to the disposable native field plus the exact text `kai-control-ok`. The Studio wrote `REMOTE_KEYBOARD_AND_MOUSE_OK`. No desktop pixels were saved, and the fixture was removed.
- ARD now retains its zeroized authentication key for encrypted native keystrokes. The RFB version and VNC decoder remain unchanged; no proxy, native-protocol fallback, new host service, or additional dependency is used. The vendored MIT patch and its provenance are documented in `integrations/Code/vendor/rv-session/KAI-PATCH.md`.
- Saved Ninja chats fill their normal chat column. All six running GIFs decode to twenty-four transparent frames and remain below 80 KiB each; cached backgrounds stop animating when hidden or under Reduced Motion.
- Manual animation review is not claimed by automated checks.

## Verification for 0.12.32

- Native RFB/ARD login received an actual 2560x1440 Studio framebuffer, view-only, with no input sent and no screen pixels saved.
- Viewer tests verify letterbox coordinates and that Observe/Watch cannot emit input, including blur/close release paths.
- Real installed Codex command/exec checks verify Read only, Workspace directory writes plus GitHub HTTPS, and Full access, all with Never ask and no model inference.
- The updated Studio worker passed a real Codex Workspace/Never task using shell, GitHub HTTPS, file write/read, and native Finder Computer Use inspection with no ordinary approval prompts. A separate native question remained pending for an explicit answer; no deletion was requested or performed.
- The final signed startup and idle probes passed. Isolated compact idle measured approximately 1.7% CPU by process-time delta, 250 MiB RSS and 13 threads; the desktop viewer allocates its connection runtime only when opened.
- Ninja bridge tests verify its exact history/draft owner and stale-surface rejection without changing the underlying Code draft.
- All six generated GIFs decode to twelve transparent frames and remain below 40 KiB each.
- Native Swift menu identity tests and packaged startup checks are rerun. Manual motion review is not claimed.

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
