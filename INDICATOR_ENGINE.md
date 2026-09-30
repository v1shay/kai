# Kai notch indicator engine

The right-side notch indicator is a small scene engine built around eight
persistent Core Animation primitives. Listening, thinking, pencil, terminal,
Git, image, document, code, search, read, file browsing, planning, review,
permission, user questions, success, failure, declined, waiting, external
tools, multiple agents, context compression, and warnings are scenes rendered
by that same pool.

The menu's `Random Morph Cycle` shuffles every non-listening scene and visits
each exactly once. Each run uses a new order and the same interruptible
transition path as ordinary product state changes.

## Why primitives

Core Animation cannot safely morph arbitrary unrelated SVG paths unless their
control-point topology matches. Kai instead composes its tiny symbols from
rounded vector strokes. Every primitive has the same animatable properties:

- center position
- width and height
- corner radius
- rotation
- opacity

This vocabulary produces bars, dots, borders, cursors, chevrons, pencils,
connected nodes, pages, and other notch-scale glyphs. It also guarantees that every scene can
transition into every other scene.

## Adding a scene

Add one `IndicatorScene` to `IndicatorScenes.swift`. A scene contains up to
eight `IndicatorTrack` values. Each track is only a list of poses, key times,
and an optional phase offset. Pad unused tracks with `.hidden` through the
existing helper, add the scene to `IndicatorScenes.all`, and expose its ID from
the menu or product state mapper.

No source or destination transition pair is created. `NotchIndicatorEngine`
always reads the current presentation state of all eight primitives and uses
one generic transition to the new scene's first pose. The target scene's loop
is installed after that transition finishes. Interrupting a transition starts
the next one from what is actually visible on screen.

## Dynamic scenes

Listening is the only scene that updates from Swift at 30 fps because its four
bar heights come from live audio. It uses the same primitive pool and enters or
leaves through the universal scene transition. All deterministic scene loops
run as Core Animation keyframes on the compositor.

## Theme independence

Motion layers are masks. Pet gradients and their moving highlight are separate
fill layers behind the mask. Pet changes therefore do not reset geometry,
audio response, loop phase, or an in-progress scene transition. Theme intent
is tracked independently as ambient, thinking, or working, so a pet switch in
the middle of a morph still selects the correct recipe.

## Runtime cost

- Eight mask layers are allocated once.
- Two persistent gradient layers are allocated once.
- A temporary incoming gradient and edge highlight exist only for 520–900 ms
  during a theme wave.
- Only listening uses a 30 fps main-run-loop timer.
- Thinking and tool scenes run on Core Animation without per-frame Swift work.
- Scene changes allocate animation objects, then release them after completion.
- The audio engine runs only in microphone listening mode.

This keeps runtime work constant as the scene catalog grows. New scene data
increases source size but does not add layers, timers, or transition code.
