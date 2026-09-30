# Kai notch indicator handoff

This folder is a copy of the notch indicator artwork and animation logic, ready
to hand to another agent. The source project remains in its original locations.

## What is here

- `svg/`: 24 scene icons plus the open and closed file glyphs. These are real,
  standalone SVG files at a 28 × 18 viewBox. They are static first-frame art.
- `scene_catalog.json`: all 24 scenes with their eight primitive tracks, poses,
  key times, phase offsets, transition times, and loop times. Use this to port
  the animations to a web, mobile, or other renderer.
- `swift/IndicatorScenes.swift`: the editable source of all scene geometry and
  timing. It is the source of truth for the SVG and JSON exports.
- `swift/NotchIndicatorEngine.swift`: the complete macOS Core Animation player,
  including live microphone bars, scene morphing, pet gradient waves, accent
  glows, and the file glyph. It uses only these two Swift files plus system
  AppKit and AVFoundation.
- `gradient_profiles.json`: pet palettes and six gradient recipes per pet.
- `preview/index.html`: an independent listening-to-thinking SVG motion study
  with a microphone option. Open it directly in a browser.
- `tools/main.swift`: exporter for the scene JSON and SVG snapshots.

## Integration contract

The scene canvas is **28 × 18** in Core Animation coordinates, with the origin
at the bottom left. SVG exports flip the Y axis for normal SVG coordinates.
Each scene has at most eight stable primitives, identified by track index.
A pose defines center `x` and `y`, `width`, `height`, `radius`, `rotation` in
radians, `opacity`, `shape`, and `lineWidth`. Invisible slots have opacity zero.
The shapes are `capsule`, `ellipse`, `question`, `hourglassLeft`, and
`hourglassRight`. The last three paths are implemented in the Swift engine and
mirrored in `tools/main.swift`.

On a scene change, capture the current on-screen poses, interpolate each track
to the new scene's first pose over `transitionSeconds`, then run its pose
keyframes repeatedly over `loopSeconds`. Start each track's loop at
`phaseSeconds`; `keyTimes` are normalized positions between 0 and 1. This
lets any scene interrupt and morph into any other scene. `listening` is special:
its four bar heights respond to audio at 30 fps, so its loop duration is zero.
The engine also exposes `setExternalLevels(_:)` for externally measured audio.

For macOS, add both copied Swift files to the same target. Create
`NotchIndicatorEngine()` on the main actor, add `engine.layer` to your host
layer, and call `engine.layout()` after changing its bounds. Use
`engine.showScene("code")`, `engine.showThinking()`,
`engine.startListening(useMicrophone: true)`, or `engine.hide()`.
`engine.fileLayer`, `engine.accentLayer`, and `engine.leftAccentLayer` are
optional companion layers. Decode `gradient_profiles.json` as
`GradientCatalog`, select `profileByPetID[id]`, and pass it to
`engine.setProfile(profile, wave: true)` for the designed color treatment.
Microphone use needs a macOS usage description and permission.

The SVGs use a neutral silver gradient. To match a pet, use that pet's
`gradients` stops from `gradient_profiles.json` in the SVG paint, or render the
scene geometry as a mask over the gradient as the native engine does.

## Regenerate the exports after editing scene geometry

From this folder, on macOS:

```sh
CLANG_MODULE_CACHE_PATH=/private/tmp/notch-clang-cache \
SWIFT_MODULECACHE_PATH=/private/tmp/notch-swift-cache \
swiftc swift/IndicatorScenes.swift tools/main.swift -o /private/tmp/notch-indicator-export
/private/tmp/notch-indicator-export .
```

`scene_catalog.json` and `svg/` are generated copies. Edit the Swift scene
definitions, then run the exporter.
