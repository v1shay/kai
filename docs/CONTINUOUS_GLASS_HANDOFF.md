# Continuous material during surface morphs

The workspace already renders at its final size and animates one native contour.
0.12.7 preserves that arrangement and its timing. It changes material ownership:

- The real workspace/mini Code material and tint remain at their existing opacity
  while the native contour moves. Renderer content and native foreground views
  retire independently at the existing content fade deadlines.
- App-to-app handoffs still retire the outgoing source as before; the incoming
  surface owns glass. Reversal translates the current content opacity between
  these ownership modes.
- A cached bridge fills only the portion outside the actual window (normally the
  menu-bar gap). A fixed exclusion mask prevents drawing beneath the workspace
  even for L-shaped/windowed external regions. No bridge is shown when no space
  outside the real window is needed.
- Classic bridges use a copy of Rust's actual native backdrop implementation,
  including radius/material/state/filter policy. Tint and appearance are copied.
  Liquid Glass uses the matching public AppKit view and tint. Opaque mode retains
  its existing solid color rather than introducing a glass material.
- Unchanged profiles reuse their native view/window. Profile or bridge geometry
  changes are processed once at the transition boundary. Native Core Animation
  drives the existing path/opacity animations; there is no new polling loop,
  per-frame window resizing or repeated blur allocation.

The native expansion/collapse totals (0.84/0.60 seconds), path animation timing,
component entrance order, all existing contours and keyboard behavior remain.
The 0.12.6 GPU compositing formats/precision are unchanged. This is a structural
performance check, not a new frame-presentation/idle-CPU qualification.

Verification:

- `scripts/test-unified-glass-continuity.sh`: real AppKit layer/geometry audit
  verifies live material, foreground/pet retirement, gap-only bounds, no warm
  workspace resize, same cached bridge, profile invalidation, windowed exclusion,
  interrupted reveals, opaque restoration and reduced motion.
- Native state harness: owner continuity, approvals, child pets, shortcuts,
  direct extended-notch return and retained background workspace pass.
- Final native matrix: compact/extended/workspace/mini Code, Control-Option,
  double-Command, reversals, reduced motion and input/window-level cleanup pass.
- Rust native backdrop/GPU tests (2) pass. The signed packaged glass probe also
  passes the real Rust-to-Swift backdrop-copy contract and the existing native
  material/surface-transition sequence.

The user requested to perform the visual review. No screen-capture comparison was
performed for this handoff change. User settings and the running copy are retained.
