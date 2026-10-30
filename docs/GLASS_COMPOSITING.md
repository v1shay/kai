# Native glass and Metal compositing

Kai uses an AppKit backdrop behind GPUI, with independent tint. Its Metal
framebuffer must preserve fractional color and premultiplied source-over
coverage until Core Animation composites that backdrop. This applies to classic
NSVisualEffectView and the macOS 26 NSGlassEffectView path.

## Corrections in 0.12.6

GPUI 0.2.2 used `One` as the destination alpha blend factor in the normal
primitive and premultiplied path-sprite pipelines. RGB used source-over, but
alpha accumulated by addition. Two layers with alpha 0.25 and 0.40 stored 0.65
instead of `0.40 + 0.25 * (1 - 0.40) = 0.55`. Core Animation consequently hid
more of the native backdrop as translucent UI layers accumulated.

Both destination alpha factors now use `OneMinusSourceAlpha`. Path
rasterization already used that factor. Transparent windows also use
RGBA16Float drawables so dark gradients retain fractional coverage/color through
the blend operations. Opaque windows keep BGRA8Unorm. Switching glass/opaque
rebuilds the matching compositing pipelines only when the format changes.
Source colors remain sRGB; the Metal layer explicitly declares that color space.
Image/glyph atlases and path intermediates retain their existing formats.

Shader colors, buffer counts, draw scheduling, user blur/opacity and morph
animations are unchanged. This adds no per-frame passes or application threads,
but the high-precision transparent drawables use twice the bytes per pixel.
At the captured 3420x1946 window size, three such drawables can require about
76 MiB more GPU/shared storage than the 8-bit equivalents. Opaque mode avoids
that cost. This is not a claim that total GPU allocation or process RSS is
unchanged.

The correction is in `integrations/Code/vendor/gpui`, selected by Cargo's local
patch mechanism. The published 0.2.2 framework is compiled once. Its Apache
license and modification notice ship with the app. See the vendor's
KAI-MODIFICATIONS.md when upgrading GPUI.

## Verification

- A one-pixel Metal target is drawn using production pipeline builders and read
  back from the GPU. The old code reproduced alpha 166/255 instead of 140/255.
  The correction passes overlapping, opaque and ten-layer cases for straight
  and premultiplied RGB pipelines in both framebuffer formats.
- A 256-pixel narrow dark-gradient GPU test retains over 240 distinct coverage
  values in the floating-point target; the 8-bit target has fewer than 20.
  Actual renderer construction and glass/opaque/glass format round trips pass.
- The signed native probe passes backdrop/tint/material/radius checks, GPU
  precision, workspace/notch/Quick Chat transitions, and repeated/reversed opens.
- Full Rust/UI run before the format upgrade: 870 passed, 19 ignored, two
  animation timing failures under compiler load. Both pass isolated reruns
  without changed assertions. The two native backdrop/GPU checks pass after the
  final format upgrade; the unchanged UI suite was not repeated in full.
- Live old/new captures use copied selected wallpaper, blur 64, opacity 0,
  Glacier/dark/classic, and the same dimmed test backdrop. Captures:
  `.build/validation/glass-visual-before.png` and `glass-visual-after.png`.
  The temporary apps were stopped after capture and prior app focus restored.
  Early obstructed/default-fixture captures were discarded. This is a comparison
  of Kai builds, not a side-by-side MonoCode quality certification.
- One resident-memory sample per disposable app after capture: 209.19 MiB old,
  216.47 MiB new; 15/16 threads at sampling. Startup/driver variability and GPU
  allocations mean these are not steady-state resource or release-gate results.
  `.build/validation/glass-visual-memory.json` records them.

Earlier 0.12.5 wallpaper precision fixes remain. They did not correct the Metal
blend equation or the final drawable's precision.

Apple API references:
- https://developer.apple.com/documentation/quartzcore/cametallayer/pixelformat
- https://developer.apple.com/documentation/quartzcore/cametallayer/colorspace
