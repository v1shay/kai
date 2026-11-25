# Kai Server on macOS

Kai Unified build 190 or newer runs a persistent per-user remote service.
Build 195 adds background native command dispatch, live chat and workspace
subscriptions, scoped files and PTYs, pairing QR codes, and signed ZIP imports.

## Connect two Macs

1. Install a compatible Kai build on both Macs and open each app once.
2. Find the other Mac by name above **Projects** and **Chats**. The device row
   shows the hardware icon and connection status.
3. Select **Enable Remote Access** and sign each Kai node in to the same
   tailnet. Kai uses a separate embedded node with a name ending in `-kai`.
   Tailnet policy must permit TCP port 47832 between the Kai nodes.
4. Select the other Mac and click **Connect**. On the host, check the
   authenticated tailnet account and choose **Read Only** or **Allow Agent
   Control**.
5. Expand the Mac to see its projects. Expand a project to see its chats, then
   click a chat to open it in the normal workspace tab strip. Use the Mac's
   row menu to give it a display name. Closing the remote tab or clicking a
   local chat returns to the local workspace. Repeat the pairing in the other
   direction to grant reciprocal access.

For a read-only pairing, open a conversation and choose **Request Agent
Control**. The host upgrades the same pairing when it approves. The host
must be logged in, awake, and connected. The server protocol also supports a scoped file
grant, interactive terminals, remote updates, approved screenshots, and
single-use pairing links. These controls are not exposed in the build 205
workspace.

## Connection recovery

The Kai server keeps running when its embedded connection is unavailable.
Tailscale handles normal route changes and relay failover. Kai checks the local
backend every five seconds. After 45 seconds of sustained failure, Kai closes
the existing listener and embedded backend, then reopens them with the same
saved profile. Retries use exponential backoff up to five minutes plus jitter. An
unexpected listener exit starts recovery immediately. A stale control session
with recent peer traffic can keep serving that traffic.

| Failure | Recovery |
| --- | --- |
| Short Wi-Fi outage, sleep and wake, interface or IP change, NAT rebinding, blocked direct UDP, or relay change | Tailscale reconnects or switches paths. The saved pairing carries over. |
| DNS failure, captive portal, network filter, ISP outage, or Tailscale coordination or relay outage | Kai retries while the connection is unavailable. Sign in to a captive portal or have the network administrator resolve an enforced restriction. |
| Incorrect clock, certificate validation failure, incompatible control server, or conflicting VPN | Restore the system or network configuration. Kai keeps its existing profile and retries using the configured trust and network policy. |
| Embedded backend reports a stale control connection, fails to start, stops answering, or loses its listener | Kai recycles the embedded backend using the persisted node. The workspace owner and supported agent processes continue independently. |
| Kai process exits, hangs, or is restarted by an update | Launchd restarts supervision after an exit. The updated supervisor checks for three missed local heartbeats before restarting. After an app update, the installed supervisor binary is refreshed on bootstrap and takes effect on its next start. |
| The other Mac sleeps, shuts down, loses power, logs out, or has Kai turned off | Its peer stays visible offline if paired. On its next macOS login and network connection, the peer can use the original enrollment. |
| One Kai peer is offline, a tailnet rule denies traffic, or a Kai grant was revoked | Kai keeps the local node running. Restore authorized access at the relevant policy or pairing layer. |
| First enrollment or account approval is pending, or an enrolled node needs reauthentication after key expiry | Kai preserves the pending flow and reports the required action in Remote Devices. |
| Node removed, credentials lost, corrupt state, disk write failure, or the stored profile belongs to another node | Restore the original Kai identity and Tailscale state together. An explicit enrollment is needed if the original credentials cannot be recovered. |
| A system image is cloned, an account is moved to a different tailnet, or the previous node is replaced in the admin console | Keep each installation’s credentials with its original device. A replacement node needs explicit enrollment and a new pairing. |

Clients rediscover their paired hosts and resume supported streams from the
last cursor. Kai gives interrupted mutations back to their caller for review
instead of restarting a task or repeating a write as part of network recovery.

### Use the Mac’s Tailscale app during an embedded outage

In the **Remote Devices** sidebar section, choose **Use Mac’s Tailscale as
Backup** from the section menu. This uses the already connected, signed
standalone Tailscale app in Applications while the embedded connection
recovers. The backup is for outgoing connections. The other Mac continues to
host its normal embedded Kai server.

The host must approve the Mac’s standalone Tailscale identity once. The
original embedded enrollment and pairing remain valid. Kai routes backup
connections from the Mac’s Tailscale address to discovered Kai nodes and
continues enforcing Tailscale rules and Kai grants. When embedded connectivity
returns, Kai uses it again for new requests. Revoke each grant separately if
the Mac should lose access over both connections.

The sidebar queries device summaries and expanded branches on dedicated
workers. Project metadata and history load independently. Remote and local
rows use the same virtualized tree, including empty projects. Compatible
servers read history from Kai's database and original agent transcripts
through the same history reader used locally. Journal events stream into the
workspace with native Markdown and text effects. Reopening Kai reconnects
to the selected remote conversation.

## Ownership and synchronization

`~/Library/Application Support/Kai Unified/server` holds the canonical
installation path, publisher requirement, private Kai identity, trust records,
and embedded Tailscale state. Back up `identity.json` and `tsnet/` together.
Preserve the adjacent Kai workspace and session data for restoration.

`com.kai.unified.server` starts one supervisor and one server at login, each
protected by its own process-lifetime lock. The service authenticates remote
requests against the Tailscale stable node ID and the paired Kai ID. The app
uses a private local Unix socket. App builds and protocol versions are separate.

The remote service can start the existing native workspace owner in the
background on demand. A workspace lock gives the desktop and the background
owner the same write authority. Reopening the app can reveal that existing
workspace. The workspace owner uses Kai's existing native provider, approval
and receipt machinery. Durable provider owners and remote-created PTY workers
live in separate processes. A server replacement can leave those owners running.
A host reboot ends running processes. The login agent restarts the server with
the same identity and restores access to saved history after login. Following a
server crash, launchd restarts supervision and clients can resume supported
streams from their last cursor. An unclean native provider exit retains saved
output and holds uncertain commands for review.

The workspace stream pushes a metadata snapshot, changes, activity, and the
active conversation's ordered journal. Short-lived disconnections can resume
from the active owner's byte cursor. Saved history and recent cached
conversation pages remain separate from active output. The server keeps
explicitly-created remote shells, Codex CLI, and Claude CLI terminals alive
when their viewers close. The client renders their bytes with Kai's existing
Ghostty surface when Metal is available.

Scoped file access supports lazy folders, file search, text previews,
revision-checked text edits, uploads, Git diffs, and native previews using Quick
Look. Large downloads verify a content hash and resume to the same destination
after an interrupted transfer. The Artifacts panel reads saved text and
document artifacts for the authorized project. The existing remote access and
MCP integrations are documented in [Remote access](REMOTE_ACCESS.md).

## Install and update

The **Remote Devices** section menu can check the GitHub release feed or
prepare a downloaded signed ZIP. After preparation, **Restart to Update** uses
the existing workspace save and relaunch path. The remote update endpoint
requires a separate update grant. The supervisor validates the replacement,
stops the old server, exchanges the bundles atomically, and checks the new
server. A journal supports restoring the old app when activation is
interrupted or fails. The old signed app is retained beside the active
installation.

Use a certificate-backed publisher consistently across installs and updates.
To generate an updater-compatible asset after building and signing:

```sh
scripts/package-kai-release.sh ".build/unified/Kai Unified.app" .build/remote-release
```

Publish the resulting ZIP with a normal GitHub release on `v1shay/kai`. An
automatic check runs shortly after the server starts and every six hours.
The downloaded asset must match GitHub's reported size and SHA-256 digest.
A local ZIP import independently verifies the app signature, publisher, build
number, and bundled server before preparing it. A certificate-backed local
development install needs an archive from its own signer or the signed source
channel.

To use that source channel, install Go, Rust, and Xcode. Add trusted public
SSH keys to `server/developer-allowed-signers` in OpenSSH allowed-signers
format, then set `server/updates.json`:

```json
{"channel":"developer","branch":"release-branch","signing_identity":"<certificate SHA-1>"}
```

Kai verifies the exact commit before checkout or build. The build runs in an
isolated checkout, is signed with the configured local identity, and stores
its source commit in the bundle. The deployed app changes only after
activation. Use `{"channel":"off"}` to disable periodic update checks.

## Verification and limits

From `integrations/Remote`, run `go test -tags libsqlite3 ./...`. The embedded
network suite uses an isolated Tailscale control plane and relay. From the
source root, run `python3 -m unittest tests_remote_backend.py` for the
native workspace bridge and legacy API tests.

To verify 100 relaunches on an isolated test Mac or VM, run:

```sh
xcrun swiftc -parse-as-library scripts/test-kai-ui-reopen.swift -o /tmp/test-kai-ui-reopen
/tmp/test-kai-ui-reopen "/path/to/Kai Unified.app"
```

The acceptance checklist records the distinction between source or VM proof
and testing on the user's actual two Macs. The iOS client is future work.
Existing local and external PTYs need an explicit attach integration. Provider-specific behavior and actual user-tailnet
policies need their own end-to-end checks.
