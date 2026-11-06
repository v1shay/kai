# Kai Unified 0.12.69 (106)

This supersedes the presentation design in
`SINGLE_CONVERSATION_SURFACES_2026_11_03.md`. The original native notch,
expanded-notch layout, Mini Code, menu-bar view, and full workspace keep their
own presentation. They share the same Orbit conversation owner and running
provider tasks. Mini Code and the menu-bar view use the original composer entity;
the native notch sends editor changes through that owner. Switching presentation
does not restart inference, create a replacement chat, or transfer task ownership.
Editor hit-testing and IME geometry are tracked per window.

## Changes included

- Restore the native notch presentations and their morph transitions while
  keeping conversation state, approvals, attachments, drafts, and provider work
  under the same owner.
- Default the full-workspace opening animation to once per application launch.
  Settings → Appearance → Open at startup includes **Opening animation only on
  first open**. Disabling it restores repeated entrances. This preference does
  not disable other interface animations.
- Aggregate available provider quotas in the workspace footer, share quota data
  with composer controls, and omit unavailable quotas. Use full-color provider
  artwork outside the notch's color-synchronized indicators.
- Resolve project labels from the frontend project catalog and explicit renames.
- Discover Antigravity's model catalog and route `/models` to the native picker.
- Own multiple terminal sessions independently, with draggable tabs, split groups,
  resize handles, live shell titles, and automatic collapse of empty groups.
  Dock movement and tab movement retain the same PTY processes.
- Measure terminal cells from the selected font, report those dimensions to the
  PTY, and support taller/larger viewports. Create an owned standalone-chat
  workspace when its first terminal opens. Bind macOS terminal shortcuts within
  terminal focus instead of invoking chat actions.
- Correct GPUI text measurement so a narrow truncation pass cannot shorten the
  formatting runs reused by a later, wider pass.

## Verification scope

The preceding incremental releases have their own focused verification results.
For this publication, compilation and signed application packaging are checked;
no tests are run, as requested. This does not claim a fresh end-to-end check of
all providers, remote hosts, visual transitions, or shortcuts. Existing terminal
work is left running when installing the update, so relaunch is required to
activate the new build.
