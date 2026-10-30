# Kai GPUI modifications

Vendored from the published `gpui` crate version 0.2.2 (Zed Industries), under
its included Apache-2.0 license. Cargo uses this source through `[patch.crates-io]`.
The framework is compiled once. Buffer counts and draw scheduling are unchanged;
transparent drawables use more bytes per pixel, as documented below.

Modified files:

- `src/platform/mac/metal_renderer.rs`: use source-over destination alpha for
  normal primitive and premultiplied path-sprite pipelines. The original `One`
  factor added coverage, suppressing native glass behind overlapping layers.
  Includes a one-pixel Metal readback diagnostic using these same pipeline
  builders, validating overlapping/opaque/ten-layer draws in both RGB modes and
  framebuffer formats. Transparent windows use RGBA16Float with explicit sRGB;
  opaque windows use BGRA8Unorm. Format changes rebuild matching pipelines.
  The GPU diagnostic verifies gradient coverage precision and actual renderer
  format round trips. The shader library is retained for rare format switches.
- `src/platform.rs`: expose that bounded diagnostic to Kai's native glass probe.

The path rasterization pipeline already used source-over and is unchanged.
Other platforms, shader colors, image/glyph atlas formats, buffering counts and
render scheduling are unchanged. Transparent drawable storage doubles per pixel;
opaque storage is unchanged. Preserve these two source files when updating the vendored crate.
