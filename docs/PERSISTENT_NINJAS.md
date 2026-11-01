# Personal Ninjas

Personal Ninjas are durable identities in the sidebar, separate from the Ninja inference provider. Each identity has a name, persona, pixel character style, execution host, provider/model/thinking selection, native permissions, and a revisioned memory document. Its tools run on its execution host. Ordinary Ninja inference continues to keep coding tools on the current Mac while sending inference to the selected engine.

## Use

1. Click **+** beside **Ninjas**. Choose This Mac or an online Tailscale machine with the worker installed.
2. Name it, choose its character and workspace on that host, select an installed provider and its model, then save it.
3. Assign a task now, tonight at 22:00, or daily. Submitting a task captures the current settings. Later changes cannot change permissions or providers underneath an already queued/running task.
4. Continue a chat or start another. Memory follows the identity. Compatible chats resume their native session; a provider/harness change starts a new native session with memory and bounded recent-work context.

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

## Workspace handoff

For a saved remote Ninja, **Handoff current workspace via Taildrop** prepares a Git repository bundle, an exact tracked/untracked working tree honoring Git ignores, and bounded chat context. The receiving worker verifies the complete archive and every file before selecting the imported workspace. The UI waits for a receipt. Paths, links, duplicate members, expanded size, and checksums are checked; a changing source file fails the snapshot rather than silently sending inconsistent contents.

Both devices must remain online until receipt is confirmed. A transfer cannot begin after the laptop has powered off. macOS may require the worker's Downloads permission to read a Taildrop receipt; protected project folders also retain their normal system permissions. The managed imported workspace lives in the worker's private application-support directory. Global provider credentials are never copied.

## Verification for 0.12.28

- Native Codex: actual shell command, editing an existing file, persistent memory write, and a new-chat memory read after worker restart.
- GPT-OSS20B via Codex and Claude: actual shell commands and existing-file edits.
- Studio native Codex: file edit, memory, client-disconnected completion, scheduled task, and an outside-workspace command requiring an exact native approval.
- Studio Ninja/Claude/Ollama/GPT-OSS20B: file edit, scoped MCP memory, and scheduled memory retrieval.
- Eight focused worker/UI checks; four GitHub UI checks and a live account bootstrap with the hub closed; eighteen inference protocol/auth checks; five archive/liveness checks.
- The broader run passed 918 checks, with seven failures reproduced using the preceding build. Those concern older project navigation/pinning, removed composer controls, provider-picker test selectors, and completion-diff UI behavior; this pass does not claim they were resolved.
- Actual Taildrop delivery to the Studio succeeded. Receipt/import verification was blocked by the Studio's Downloads privacy prompt during validation; do not treat delivery alone as a completed handoff.

Full live microphone interaction and manual visual review are outside this pass. Existing notebook, project and conversation files are not migrated or rewritten by worker installation.
