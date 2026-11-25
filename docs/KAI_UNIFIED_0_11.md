# Kai Unified 0.11

This experimental build stays on `v1shay/kai-code-unified`.

## Appearance and startup

Settings → Glass material selects Automatic, Classic blur, or Liquid Glass.
Automatic uses the public macOS 26 material when the OS provides it; older Macs
use the independent native Gaussian backdrop and tint. Liquid Glass is selectable
on supported systems. Opacity controls the native tint separately from reading
surfaces; text itself remains opaque. Opaque and high-contrast settings remain
available. No additional browser or duplicate app renderer is introduced.

Settings → Startup surface selects Code workspace (the new default), Extended
notch, or Compact notch for the next launch. Presentation starts as soon as the
shared native owner is ready, while discovery/catalogs continue independently.
Existing installations retain their chats, drafts, permissions and workspace.

Automatic chat titles can be disabled in the same section. After the first reply,
a bounded separate read-only ephemeral Codex request generates a semantic title;
it is not inserted into the conversation. This requires an available Codex CLI.
Manual renames and deleted chats always win over a late generated name.

## Pane sizing and control toggles (0.11.3)

Build, model/thinking and top-row popovers open on a click, close when clicked
again, and dismiss on outside clicks. Selecting another top-row popover replaces
the previous one. Outside dismissal preserves the originating press so a closing
menu cannot reopen itself on mouse-up.

Files, Subagents and Changes have draggable resize handles on their inner edges;
widths persist between launches. The terminal remains dockable on all four edges.
Drag a divider down to its minimum, then about 40 pixels further to collapse the
pane. Collapse clips its full-size contents instead of shrinking the UI itself,
and can reverse from its current pose. Minimizing a terminal retains its shell.

Pane dragging is separate from window dragging. Empty tab-strip/title chrome
moves the window; terminal headers and resize handles operate on their pane.
Native background window dragging is disabled and pane gestures temporarily lock
window movement, restoring it on release or explicit chrome dragging.

## Adaptive composer toolbar (0.11.2)

Code and Quick Chat keep their composer controls on one row. As the actual
conversation area narrows, permission and voice labels fold into icons, then
orchestration/thinking labels compact. At smaller widths, secondary controls
move into the click-only “…” menu. The model remains reachable, and microphone
and send/stop stay together at the right edge. Growing the conversation area
restores full labels. Width, icon and label changes use a finite 240 ms transition
that reverses from its current pose; Reduced Motion snaps to the fitting layout.
An open overflow menu keeps its trigger visible until dismissed.

This responds to the space left by sidebars, file inspectors and terminal docks,
not just the outer window width. Model identity glyphs and chevrons fade as the
model chip narrows; the exact name remains available in its picker and menu.

## Terminal and navigation

Drag the terminal header toward any of the four highlighted workspace edges.
The live terminal moves without restarting its shell. Drag its inner divider to
resize it; edge and dimensions persist. The panel keeps at least 35% of the
available conversation area free. Opening a terminal starts it on demand.

Project branches now fold with real clipped height animations. The separate
conversation pane slides without repeatedly reflowing its rows. Reversing either
animation continues from its current position. Hidden reveals no longer keep the
welcome provider logo spinning or redrawing an idle workspace.

The context ring uses actual provider-reported used/limit telemetry. It is
independent from subscription quotas, and stays absent when that telemetry is
unavailable.

## Child agents and orchestration

Current native Codex `subAgentActivity` events register real children even when no
spawn-tool event is emitted. The child viewer has per-child pet tabs, model,
thinking effort, delegated scope/task, workspace, progress and transcript. It
keeps exact parent ownership for approvals, steering and stopping. When the
provider omits plaintext assignment text, the actual scope is shown instead.

Active children participate in compact-notch pet cycling. Floating families use
small animated children inside the parent's existing pet window, rather than a
new window for each child. Large families page on scroll; every child remains
available in the viewer. Finished ancestors remain registered while descendants
still depend on them.

Orchestrator `code_workspace.list_models` reads the actual selected provider's
catalog for the target project. `create_chat` and `prompt_chat` accept exact
`model` and `effort` values and capture them in the durable task. Invalid IDs or
unsupported levels are rejected before starting work. Existing provider adapters
execute each worker; no model-name substitution is used.

## Keyboard commands

The same catalog drives Code, the extended notch, Quick Chat, and Settings help.
Terminal Ctrl input and marked-text editing remain local. Native notch commands
that need the workspace reveal the existing window. Draft and captured-context
ownership checks apply to keyboard actions too. Existing global Kai gestures,
including Control–Option and double-Command, remain available.

| Shortcut | Action |
| --- | --- |
| ⌃ T | New chat |
| ⌃ W | Close tab |
| ⌃ L | Focus message |
| ⌃ ⇧ L | Focus conversation |
| ⌃ M | Choose agent and model |
| ⌃ ⇧ M | Thinking level |
| ⌃ P | Switch project |
| ⌃ ⇧ P | Fold project |
| ⌃ K | Search chats |
| ⌃ B | Project sidebar |
| ⌃ ⇧ B | Conversation sidebar |
| ⌃ E | Files |
| ⌃ G | Source control |
| ⌃ J | Terminal |
| ⌃ ⇧ J | Move terminal to next edge |
| ⌃ U | Context and usage |
| ⌃ O | New orchestrator |
| ⌃ R | Dictate |
| ⌃ ⇧ R | Real-time voice |
| ⌃ ⇧ A | Subagents |
| ⌃ F | Fold latest work |
| ⌃ N | Next tab |
| ⌃ ⇧ N | Previous tab |
| ⌃ ⇧ T | Next project |
| ⌃ ⇧ Y | Previous project |
| ⌃ ⇧ X | Stop agent |
| ⌃ ⇧ H | Keyboard shortcuts |
| ⌘ ⇧ O | Open folder |
| ⌘ ⇧ N | New chat |
| ⌘ ⇧ K | Return to notch |

## Build iteration and verification

`scripts/build-unified-app.sh` builds an optimized signed bundle. Native library
compiles are cached by compiler/SDK, flags and source contents; a failed compile
never replaces the last usable library. Set `KAI_UNIFIED_CONFIGURATION=debug` for
incremental development; the shipped build remains optimized.

`scripts/test-unified-packaged.py` validates isolated signed copies using `glass`,
`startup-workspace`, `startup-extended`, `startup-compact`, or `performance`.
Reports go to `.build/validation`. Probes use PID-scoped data, never the installed
workspace. Performance reports distinguish startup presentation from discovery,
and process CPU time from cumulative `ps` percentages.

See [validation and reference evidence](NATIVE_EXPERIENCE_PASS.md). Actual Liquid
Glass appearance requires a macOS 26 test host. Native desktop automation was
unavailable on this host; native integration probes verify behavior, geometry,
materials and controls, but do not certify pixel-identical screenshot matching.
