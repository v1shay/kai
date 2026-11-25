# Kai workspace motion

Use `src/workspace_motion.rs` for GPUI timing and
`Sources/NotchPrototype/WorkspaceMorph.swift` (`KaiMotion`) for AppKit timing.
Pane changes use a 220 ms cubic ease-out. When a new gesture interrupts one,
start from the pane's current rectangle. Honor Reduce Motion by applying the
final layout immediately.

Request another frame only while the transition is active. Keep stable pane and
terminal identities through the transition so focus, drafts, and PTYs survive.
