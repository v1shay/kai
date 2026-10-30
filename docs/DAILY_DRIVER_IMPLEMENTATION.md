# Daily driver implementation — October 29, 2026

Requested scope: continuous provider conversations; attention-driven notch;
external command/MCP and remote backend; inspectable activity; review/restore;
persistent artifacts; Finder, Quick Look, command palette and named workflows;
repair installed Antigravity/Cursor/Grok; enforce measurable performance gates.

Implementation and verification are tracked here as work proceeds. Existing
sessions, provider credentials and user drafts must survive. Remote clients use
the host's session/model/permission authority; they never execute a second agent
or infer completion from a command receipt. No mobile frontend is requested.

Baseline: `1153d5c`, installed 0.11.3 (36). Latest isolated first frame 262 ms,
223 MiB process RSS, 1.60% of one core idle. These are measurements, not targets
already achieved. Release gates must fail on missing evidence or exceeded budgets.

## Work

- [x] Reproduce and repair installed provider startup, model and approval paths.
- [x] Improve context handoffs and per-turn model/provenance across providers.
- [x] Attention priority and exact parent/child navigation across surfaces.
- [x] Shared versioned commands with durable deduplication and scoped clients.
- [x] Remote session/history/status/model/control/approval/file API and reconnect.
- [x] Searchable command palette, projects, chats, recent results and workflows.
- [x] Finder folder/file routing, native previews and App Intents/Shortcuts.
- [x] Inspectable activity, review scope and coordinated restore checks.
- [x] Persistent document/HTML artifacts and lazy isolated rendering.
- [x] Performance instrumentation, strict budgets and packaged verification.
- [ ] All strict performance budgets achieved on the delivery candidate.
- [x] Signed experimental build, backup/install and report measured limitations accurately.

## Verification and delivery

The implementation checklist is not a claim of unrestricted provider parity.
Providers expose their own capabilities: steering currently requires Codex;
Antigravity command approvals use its documented PreToolUse hook; exact model
selection requires a provider-advertised catalog entry. Cursor's configured
model streamed and resumed with retained context. Explicit Composer 2.5 selection
returned the provider's plan-upgrade requirement; Kai did not substitute a model.
Cursor, Grok and Antigravity each passed an isolated real command approval with
the file verified absent before consent and correct after one-time approval.

Native Finder routing, Quick Look, formatted Markdown and an ephemeral interactive
HTML preview passed an AppKit/WebKit harness. Shortcuts metadata was extracted by
the real App Intents compiler processor and is included in the signed bundle.
Remote authentication, scope, revocation, restart/reconnect, durable receipts,
OAuth/PKCE, bounded replay, MCP discovery and MCP Apps resources passed 19 Python
checks. The MCP Apps JavaScript harness also covers uncertain approval decisions.

The backend supports session/history/model/task/approval/control, scoped file
reads, checkpoint review and persistent artifacts. A remote client still needs
network routing (an existing SSH tunnel or configured HTTPS endpoint), and the
Mac must remain awake with Kai running. This pass does not provision a cloud
relay, remote audio/terminal streaming, or an attachment-upload protocol.

Strict performance evidence is written to `.build/validation/` by
`scripts/collect-release-performance.py`. Budgets never auto-adjust and missing
measurements fail. First-frame timing is the first GPUI workspace frame, not the
end of the deliberately longer entrance choreography or a GPU presentation
measurement. CPU/RSS/thread measurements cover the app process, with CLI agents
and dev servers excluded. Startup timing includes the launch-to-main interval when invoked by the packaged
probe. Background idle uses the compact native surface with Code hidden; the
visible workspace has its own idle budget. Input-interfered samples and
mixed-binary cohorts cannot certify a release. The delivery gate is **failed**;
passing functional tests alone does not meet the performance release requirement.

Native computer-use inspection is unavailable on this machine (Sky Computer Use
service startup fails). Native harnesses exercise real windows; they do not
replace a human visual pass or a macOS 26 Liquid Glass device test.

The final full Rust suite completed 862 tests with zero failures (19 opt-in tests
ignored). Later focused regressions cover ACP thought completion, remote scope
spoofing, workflow-name isolation, reserved callback fields, and the actual
Python HTTP → Unix IPC → native session-owner control round trip. That end-to-end
test found and now covers macOS socket-path length and accepted-socket blocking
mode. The native suite completed 208 tests with zero failures and one skip;
the real native morph matrix passed all surface transitions and reversals.

Final quiet-interval measurements completed all probes without native control
input. Candidate hashes matched throughout and match the installed build:

| Measurement | Result | Budget | Gate |
| --- | ---: | ---: | --- |
| Startup first workspace frame, isolated signed copies, p95 | 2,102.41 ms | 200 ms | Fail |
| Warm chat switch, p95 (24 samples) | 27.76 ms | 50 ms | Pass |
| Warm reveal, p95 (20 samples) | 179.63 ms | 50 ms | Fail |
| Visible idle CPU, one-core percentage | 0.535% | 0.5% | Fail |
| Compact native surface, Code hidden, idle CPU | 2.195% | 0.1% | Fail |
| Root render CPU, p95 (2,075 samples) | 0.467 ms | 8.3 ms | Pass |

Average app-process RSS was 245.27 MiB visible and 250.90 MiB compact, with about
12 and 11 threads respectively. Startup includes pre-main launch/signature work
and uses relocated, re-signed isolated copies. Root render CPU is not GPU or
presentation latency. No idle result was substituted or budget loosened.
The performance gate **fails**; 0.12.0 (37) remains an experimental test build.
The installer preserved 0.11.3 and did not restart the user's running copy.
Use Command-Q and reopen to load the new executable.

Installed at `/Users/vishayagarwal/Documents/ChatGPT/notchdex/.build/Kai Unified.app`.
Previous bundle: `Kai Unified-0.11.3-36-backup-20261029-175245.app` in the same directory.
Finder/Shortcuts Launch Services registration completed; the running app was not restarted.


0.12.1 (38) fixes opening-time wallpaper grain changes: unmeasured/stale effect
crops cannot replace the current frame, and the first measured canvas prepares
without the interactive-resize debounce. No morph timing or geometry changed.
All 25 focused wallpaper regressions pass (one opt-in test ignored), including
GIF playback, resize, settings and shutdown; the signed packaged dither probe
also passed startup, native-notch wallpaper and surface transitions. The earlier
performance measurements above apply to 0.12.0, not this unqualified hotfix.


0.12.2 (39) maps Command-W to Close tab throughout Code, mini Code and the native
extended notch. Three focused dispatch regressions pass, including the last tab,
empty draft retention and mini-window visibility. The native composer harness
also verifies repeated Command-W cannot trigger double-Command dismissal.
