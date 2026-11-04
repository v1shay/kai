# Ninja state and host repair — 0.12.58 (95)

## Causes addressed

- Native navigation previously exposed only Ninja task history mixed into ordinary chats. Ninjas without history could not be selected. The bridge now publishes a Ninja hierarchy, and the native sidebar routes profiles and their chats to the existing shared owner.
- The worker snapshot includes all pending tasks but only a bounded recent terminal history. A cached conversation could retain a running task after it left that window. One reconciler now governs task state for chat history, sidebar rows, native active tasks and approvals. Missing/disconnected status is unknown, not running and not fabricated completion. Full completed transcript bodies remain borrowed and unchanged.
- Host observation was started by window rendering. It now starts with the application entity, including when the native notch is the visible surface.
- The native compact working flag depended on a separate success notification. A native regression reproduced it remaining true after both task lists cleared. Working state now retires from the authoritative owner state, including failure, stop and unknown status, without playing a false completion.
- On the Studio, the saved Screen Recording code requirement did not match the current ad-hoc worker signature. Updating that bundle invalidated the old grant. A small permission-bearing Worker.app now remains unchanged while Runtime.app is replaced separately.
- The inference gateway owned the old worker while launchd repeatedly tried to start another one (18,417 launches observed). Gateway and desktop wake the installed service instead of spawning a competing worker.
- First-time installation no longer requires an existing host.json. It discovers the destination user's Tailscale login or accepts an explicit owner. Bundle validation occurs before installed services are stopped.

## Verification

- Focused Rust Ninja tests: 36 passed, 1 ignored.
- Persistent worker tests: 18 passed, 1 ignored.
- Gateway and managed-launch tests: 19 passed.
- Installer tests: 5 passed, including a different home/account and preserving the host bundle across updates.
- Native AppKit continuity harness: passed. Covers selecting an empty Ninja, repeated surface transitions, drafts, send acknowledgements, attachment ownership and clearing compact work without a success event.
- Actual Studio service after migration: one launch, no exits, runtime parent is the stable host. Zero active tasks before replacing it. Updated again to 0.12.58 and verified every stable-host file remained byte-identical.
- Signed package: Ninja chat, extended-notch startup, compact-notch startup and menu-bar probes passed. Eight inline image previews loaded. 120 Ninja frame samples: median 16.63 ms, p95 17.63 ms (fixture workload, not provider latency).
- Installed and relaunched 0.12.58 (95) in the existing app location. Executable/native-library/host hashes match the tested package; strict signature verification passed. The local persistent worker was left running.
- Command logs, package reports and installation hashes are in `.build/validation/ninja-state-host-20261103/`.

## Limits

Screen capture is NOT runtime-verified after migration. The user deferred re-adding Worker.app in macOS Screen & System Audio Recording. No TCC records were modified. The stable-host permission inheritance still needs a real worker screenshot after that grant.

Native checks use actual AppKit controls with controlled transport; Rust tests exercise the owner separately. This is not a claim that every real provider, computer, or operating-system configuration has been validated. The computer-use service was unavailable during this pass.
