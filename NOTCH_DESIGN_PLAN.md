# Notch shape and motion plan

## Reference measurements

- The 3420 × 2224 recording is from a 1710 × 1112 point, 2× display. The hardware notch occupies approximately pixels x=1500–1920 and y=0–76, or 210 × 38 points.
- The expanded black silhouette occupies approximately x=1340–2080 and y=0–78: about 370 × 39 points. Its center is x=1710 pixels. Thus each side grows about 80 points while the bottom stays at the menu bar edge.
- At full expansion, the upper shoulders turn inward about 7 points horizontally over 9 points vertically; the lower corners have an approximately 14 point radius. There is no drop shadow or border.
- The recording starts at hardware width. It does not show a usable continuous opening transition: by frame 14 (about 0.23 seconds) it is already fully expanded. The closing transition begins around 11.08 seconds and reaches hardware width around 11.63 seconds, about 0.55 seconds later. Exact opening easing cannot be recovered from these frames.
- The HEIC is a camera photo, so perspective and optical blur make it unsuitable for pixel-perfect screen measurements. It confirms that the real cutout is centered and the menu bar height aligns with its bottom. The screenshot provides the same hardware baseline but shows two nested displays, so the recording is the source for the animated outline.

## Implementation

1. Query `NSScreen.auxiliaryTopLeftArea`, `auxiliaryTopRightArea`, and `safeAreaInsets.top` at runtime. Their gap is the hardware cutout. On this machine AppKit reports a 209 point gap with a half-point center offset from the screen midpoint; the screen pixels show a 210 point centered opening. When those centers differ by at most one point, use the screen midpoint and symmetrize the gap to avoid the rounding error. The physical cutout's antialiased last row lies immediately below AppKit's 38-point safe area, so extend the overlay one point downward to cover it. Remeasure when the screen configuration changes.
2. Put a transparent, mouse-transparent, borderless panel over the top center of the notched screen. Draw only the black cutout as one `CAShapeLayer` path. The panel has enough side room for the 160 point total expansion.
3. Define the outline from width and the measured height. Keep the two top shoulder curves and two lower cubic corners in the same path at both widths. That makes Core Animation interpolate corresponding Bézier segments without changing topology.
4. Listen for Command modifier transitions. Start a 0.3 second timer on key-down; cancel it on key-up. If Command is still down when the timer fires, animate to the expanded width. On release, animate back and hide the panel after the path reaches hardware width. Take the presentation path as the next animation's start so a quick release or re-press reverses smoothly.
5. Use approximately 0.42 seconds for opening and the measured 0.55 seconds for closing. The recorded opening is too abrupt to infer its curve, so the prototype uses a fast ease-out. Tune that only against additional continuous opening footage.
6. Later modes can add a separate content layer above this silhouette. Their text, glow, and indicator animation should be independent of the outline path; the geometry remains one reusable width-driven component.

## Planned content-mode transitions

The recording shows three contents at the same expanded size: “Listening” with a cyan four-bar indicator and right-edge glow, “Thinking” with three magenta dots and glow, and “Speaking” with orange waveform bars and glow. Transitions occur while the black outline stays fixed. At roughly 2.5 seconds, Listening clears and Thinking appears; at roughly 5.8 seconds, Thinking clears and Speaking appears. The capture does not resolve enough frames to claim exact fade curves.

Keep a state enum (`closed`, `opening`, `listening`, `thinking`, `speaking`, `closing`) separate from the width target. Put the title, indicator, and glow in independent layers clipped to the black outline. On a mode change, fade the outgoing group to zero, switch the title and indicator asset, and fade the incoming group up, with a brief empty interval if matching the observed transition. Animate dot opacity and waveform bar heights within their own layers. This lets later mode work reuse the measured outline and Command-hold behavior without changing their timing or Bézier path.

## Verification

- Built successfully with `swift build`. A live 2× capture during a synthetic three-second Command hold shows the side boundaries at x≈1340/2080 pixels and the bottom at y≈78, matching the recording. After release, the panel returned to the physical notch. A 0.15-second hold showed no expansion.
- Inspect both corners at 100% when visually tuning further; the black outline is deliberately the only rendered content in this stage.
- Test on the built-in display and after reconnecting or changing displays. A Mac without `auxiliaryTopLeftArea`/`auxiliaryTopRightArea` intentionally shows no overlay.
