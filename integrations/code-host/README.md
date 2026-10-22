# Kai ↔ Code connection

The prepared apps are:

- `../../.build/debug/Kai.app`
- `../../.build/code-connection/Code.app` (optimized build, signed locally)

Launch the connected Code build when your existing Code runs have finished, then relaunch Kai. Enable **Connect to Code** in Kai's menu. The menu shows “connected” once the handshake succeeds. Code chats appear under **CODE**; running chats also appear under **ACTIVE**. Control + Option selects the compact indicator's running chat.

Code owns each agent process, its session, drafts, permissions, and persistence. Kai never launches a duplicate CLI for a Code session. Existing direct Codex functionality remains available, and Code can connect even if Codex's app server is unavailable.

## Available controls

| Control | Behavior |
| --- | --- |
| Discover sessions | Running/current sessions plus up to 40 recent chats; no eager history import |
| Open chat | Uses Code's normal session switch and history loading |
| Transcript | Bounded read-only window; larger history stays in Code |
| Send text / dictation | Uses Code's normal send path, only for the loaded idle session without a local draft or attachments |
| Stop | Targets the session and exact run generation |
| Models, approvals, images, slash commands | Use Code's native controls; unsupported actions are rejected without falling through to Codex |
| Disconnect / restart | Preserves displayed text, disables commands, backs off polling, and reconnects |

Kai’s **+ Codex chat** opens a new direct Codex chat; new chats for other providers are created in Code.

Active steering is not advertised: Code's current send button cancels an active run rather than accepting a live prompt. Kai explicitly blocks that path. Native workspace/fork confirmations still happen in Code; a staged prompt stays visible there until confirmed.

## Install the source change into the original Code project

The session could read the Code source but could not write outside the Kai workspace. `code-connection.patch` contains the exact Code host changes and five UI regression tests. Its apply check passed against the original project. From Terminal:

```sh
/Users/vishayagarwal/Documents/ChatGPT/notchdex/scripts/apply-code-connection.sh "/Users/vishayagarwal/Documents/ChatGPT/rust-agents"
cd "/Users/vishayagarwal/Documents/ChatGPT/rust-agents"
./scripts/bundle.sh
```

The script checks the entire patch before applying it and refuses conflicting or already-applied changes. No existing app is quit or relaunched by the script.

## Local protocol

Default endpoint: `~/Library/Application Support/Orbit/kai/control.sock`.
For a custom `ORBIT_DATA_DIR`, set `KAI_CODE_SOCKET` when launching Kai to the corresponding `kai/control.sock` path. No port, network server, provider tokens, or copied authentication are involved.

Protocol 1 uses one request and one response per Unix connection. Requests carry a unique ID and, for mutations, Code's current instance epoch. Methods are `snapshot`, `open`, `send`, and `cancel`. Mutations are acknowledged once; a lost acknowledgement is never automatically resent. Stop requests also carry the exact `runId`; sends carry a session version token so an older prompt cannot start against newer session state.

The host checks peer identity on macOS, a private socket/directory, size limits, bounded queue capacity, and UI dispatch deadlines. Code's main thread revalidates the target, run generation, current loading state, local draft, and existing consent gates. A bounded cache deduplicates recent request IDs. Queue expiry prevents late dispatch after an acknowledgement timeout. Requests are processed serially on the Code UI; bounded snapshot assembly happens on the UI, while byte serialization and socket I/O stay off the UI.

## Glass controls

Kai's **Glass appearance** menu edits corner radius, blur amount, opacity, fade height, vertical offset, haze, pet tint, rim shimmer, native material style, and four smooth fade curves. Settings persist and can be reset. Blur amount mixes the native blur with the clear backdrop; AppKit does not expose a public arbitrary pixel radius for `NSVisualEffectView`. Liquid Glass is used for light glass on systems that provide it, with native frost on older macOS versions. Text stays outside the backdrop hierarchy.

## Verification

Kai's Swift and Python tests, Code's build checks, and five native Code UI regression tests cover the new behavior. Live socket binding is blocked by this tool session's sandbox, so the two transport tests remain available to run from Terminal:

```sh
cargo test --manifest-path integrations/code-host/Cargo.toml
```

Visual appearance, real provider runs, permission sheets, and app-to-app socket traffic require a normal desktop launch. These were not claimed as verified in the sandbox.
