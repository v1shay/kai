# Kai remote host and external tools

Kai has one session owner. The optional HTTP/MCP transport sends commands to
that owner; it never starts a second provider runtime, copies credentials to a
client, or treats an acknowledgement as a finished agent task. The desktop host
must be running and the Mac must be awake.

## Start and pair

Open Settings → Remote access & ChatGPT → Start remote host. It listens on
`127.0.0.1:47831`. Generate a read-only or control pairing code in that panel.
Codes expire after five minutes and grant only the selected project. A control
client can submit work using the host's current edit permission, with workspace
containment and native approval review; a remote prompt cannot elevate access.

The packaged helper is in `Kai Unified.app/Contents/Resources/kai_remote.py`.
Its `--data` argument is the host's private `remote` directory, beside its saved
attachments/workspace data. Do not use another installation's data directory.

For a native API client, generate a bearer credential locally:

```sh
python3 /path/to/kai_remote.py --data /path/to/remote pair \
  --name 'My phone' --scope control --project /absolute/project
```

Keep the returned token on the client. The host stores its hash. Credentials
expire after 30 days. `clients` lists registrations; `revoke CLIENT_ID` revokes
one. Revocation also ends an existing event subscription.

## Connect from another device

An SSH tunnel carries the loopback listener without opening a public port:

```sh
ssh -N -L 47831:127.0.0.1:47831 user@your-mac
```

The client then uses `http://127.0.0.1:47831`. SSH Remote Login/network routing
must already be available. Kai does not change the Mac's VPN or SSH settings.

Alternatively, configure `remote/listener.json`, then restart the remote host:

```json
{
  "bind": "0.0.0.0",
  "port": 47831,
  "public_url": "https://your-host.example:47831",
  "tls_cert": "/absolute/server-cert.pem",
  "tls_key": "/absolute/server-key.pem"
}
```

Non-loopback listeners require TLS. A TLS reverse proxy may forward to the
loopback listener instead; set `public_url` to that proxy's exact origin.
The listener is a backend, not a cloud relay or mobile UI. Tailscale can supply
routing when configured; Kai does not start or reconfigure it automatically.

## API, version 1

All `/v1` requests require `Authorization: Bearer TOKEN`.

- `GET /v1/snapshot`: authoritative scoped host, projects, chats, provider
  installation states, checked model catalogs, parent/child tasks and approvals.
- `GET /v1/events`: SSE snapshots with `epoch:sequence` IDs. Reconnect using
  `Last-Event-ID`; a restart or evicted cursor produces a fresh snapshot.
- `POST /v1/command`: shared native commands. Discover exact IDs before acting.
- `GET /v1/receipts/REQUEST_ID`: recover a mutation's acknowledgement.

Read commands: `list_projects`, `list_chats`, `read_chat`, `list_models`,
`task_status`, `wait_tasks`, `list_workflows`, `list_artifacts`, `read_artifact`,
`read_file`, `review`. For `read_file`, an optional `chat_id` selects that chat’s recorded execution
workspace, including its worktree; reads remain confined to that workspace.
Discovery/history/artifacts/diffs are paged. Use returned
`next_offset`, `next_cursor`, or `next_position` without constructing cursors.

Control commands: `create_project`, `create_chat`, `prompt_chat`, `open_project`,
`open_chat`, `set_model`, `stop`, `steer`, `answer`, `restore`, `save_workflow`,
`run_workflow`, `write_artifact`. Model/effort changes require an idle chat and an
exact current catalog selection. Live steering is supported by Codex; other
providers accept another prompt when the current turn finishes.

```json
{
  "action": "create_chat",
  "request_id": "a-new-unique-id",
  "project": "/absolute/project",
  "agent": "codex",
  "name": "Investigate build",
  "prompt": "Find the failing build step and report the cause."
}
```

Mutations require a stable `request_id`. Retry using that ID and identical
arguments. A completed receipt is returned without dispatching twice. An
`uncertain` result is retained after a lost acknowledgement/restart; inspect
actual chat/task state before issuing a new command. Receipts are client-scoped.

`stop`/`steer` also require the exact current `run_id`. `answer` requires the
opaque `approvalRequestId` and provider choice/form answers from the snapshot's
`approval` object; stale answers and answers for another chat are rejected.
`restore` uses an exact chat/checkpoint ID and `direction: "undo" | "redo"`.
It restores checkpointed workspace files, preserving chat history and external
side effects, and rejects conflicting/new file edits or active work.

## ChatGPT / MCP

The Streamable HTTP MCP endpoint is `/mcp`. It exposes `kai_workspace` with the
same scopes, IDs, receipts and native approval routing as the API. Tools can read
results and issue work to installed providers. Tool output remains conversation
data, not trusted system instructions.

An OAuth-capable MCP client can discover authorization metadata at
`/.well-known/oauth-protected-resource` and
`/.well-known/oauth-authorization-server`. Pair through its browser flow using
the one-time code from Kai. PKCE S256, registered redirect URIs and one-time
codes are required. A hosted ChatGPT MCP client needs a reachable HTTPS origin;
a loopback-only listener cannot be reached from ChatGPT's servers.

The transport accepts at most 16 simultaneous connections and 32 pending host
commands, bounds replay history to 4 MiB/256 snapshots, and limits request bodies
to 2 MiB. Disable the remote host to release its helper process and IPC threads.
No remote audio, terminal-stream UI, or file-upload frontend is included in this
backend version; file reads, generated artifacts and agent controls are exposed.


For clients advertising MCP Apps support, tool metadata links the self-contained
`ui://kai/workspace` resource. It renders scoped parent/child status, model/effort
and exact-run controls, with native approval choices. The resource uses the MCP
Apps host bridge, no direct network requests or external scripts. Read-only
clients receive status without mutation controls. The desktop's separate saved
HTML artifact preview intentionally has no workspace command bridge.
