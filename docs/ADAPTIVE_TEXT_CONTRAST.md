# Adaptive text colors · Experimental

Kai Unified 0.12.22 (59). Enable **Settings → Appearance → Adaptive text colors**. Disabled by default.

This feature changes foreground text colors only. Glass tint, background color, opacity, blur, materials, wallpaper, borders, SVG icons, images, pets, font families, font sizes and layout retain their existing values. The previous adaptive-contrast pass was reverted before this implementation.

Each visible panel chooses light or dark ink from its sampled background. A switch waits for a stable preference, then follows a 240 ms smoothstep color transition. All text in a block shares the same direction; syntax and link colors preserve their hue while adapting brightness. Native text uses a finite 60 Hz tween; GPUI redraws cached text during its finite animation. Reduced Motion uses an immediate color change. Moving surfaces retain their current direction until they settle. The black compact notch retains its original text.

GPUI applies an optional callback exclusively at glyph painting. It does not rewrite the global theme or render a protective overlay. Deferred draws keep the callback belonging to their own panel. Terminal text keeps its explicit ANSI colors. Native AppKit adaptation writes only text foreground/caret colors and temporary restoration metadata. It preserves selections, text strings, fonts, masks, images and existing reveal animations.

ScreenCaptureKit supplies tiny background samples, excluding this Kai process. Existing pane tint and prepared wallpaper are read to estimate what sits behind the text; these layers are never modified. Live sampling uses existing Screen Recording permission or an explicit user action in settings to request it. Samples remain in transient local memory and never enter provider context, network requests, screenshots or logs. Capture is bounded to about two frames per second per required display, with unchanged samples skipped, and stops for hidden surfaces or a sleeping/locked session. Native attachment explicitly delivers persisted preferences, including notch-only startup.

A single ink color cannot guarantee high contrast against both pure black and pure white within the same text block. This experiment chooses the more readable direction with hysteresis and leaves the background exactly as configured. Coarse sampling, rapid video, HDR and native material effects can affect the estimate. Use the setting for visual trials rather than treating it as a per-pixel accessibility certification.

Validation covers text-only paint filtering, identical resolved themes at multiple opacity levels in light/dark schemes, independent panel state, preserved drafts/layout, bounded eased transitions, native text/selection/font restoration, and unchanged native background/stroke/image/opacity/appearance properties. Signed-app probes exercise existing notch/workspace transitions with sampling enabled.
