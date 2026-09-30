# Right-wing listening and thinking indicator plan

## Goal

Place the four-shape listening/thinking indicator in the right wing of the
expanded notch. It must mirror the pet's left-wing placement, remain centered
between the physical notch and Kai's outer edge, use the selected pet's exact
gradient profile, and keep its motion uninterrupted while a pet theme changes.

## Exact geometry

The current notch uses an expanded width of `notchWidth + 160`, which creates
an 80-point wing on each side. Geometry must always come from measured screen
values rather than a model-specific notch constant.

```swift
let expandedWidth = notchWidth + 160
let physicalRight = (canvasWidth + notchWidth) / 2
let kaiRight = (canvasWidth + expandedWidth) / 2
let rightWing = CGRect(
    x: physicalRight,
    y: 0,
    width: kaiRight - physicalRight,
    height: notchHeight
)
let indicatorCenter = CGPoint(x: rightWing.midX, y: rightWing.midY)
```

This is the exact horizontal mirror of the pet calculation. The renderer's
anchor point is `(0.5, 0.5)`, so its center is assigned to `indicatorCenter`.
Every resulting origin and size is rounded to `screen.backingScaleFactor`, as
the pet frame already is. The root notch mask remains the final clip, allowing
the right indicator to be naturally revealed by the notch expansion.

Reference-scale dimensions inside the physical notch:

- Full component bounds: 28 × 18 points.
- Listening bar width: 3 points.
- Bar centers: −10.5, −3.5, 3.5, and 10.5 points from component center.
- Listening height range: 3–16 points, always centered vertically.
- Thinking dot diameters: 3.5, 4.0, 4.5, and 5.0 points from left to right.
- Thinking travel: at most 1.25 points vertically; the component itself never
  moves. These values preserve a visibly larger right side without shifting
  the group away from the wing center.

## Component structure

`VoiceThinkingIndicatorLayer` is one reusable layer with three independent
parts:

1. `MotionController` owns four normalized shapes and the state machine:
   `listening → morphing → thinking`.
2. `ShapeMaskLayer` owns four rounded sublayers. It receives only geometry and
   alpha values from `MotionController`.
3. `PetGradientLayer` draws color and highlight, then uses `ShapeMaskLayer` as
   its mask. Theme changes therefore cannot reset or disturb motion.

The notch controller creates one instance, adds it beside `sprite`, and updates
only its frame when screen geometry changes.

## Listening

Use one `AVAudioEngine` input tap. For every audio buffer:

- Calculate energy for four logarithmic speech bands.
- Apply noise-floor subtraction and adaptive gain.
- Use a fast attack and slower release to match the reference.
- Apply gains `[0.91, 0.94, 0.97, 1.00]` for a subtle rightward bias.
- Publish four normalized heights through a small lock-free snapshot.

The layer samples the latest snapshot at 30 fps. Thirty visual updates per
second are enough for the tiny component; audio analysis remains independent
of display timing. Layer mutations use disabled implicit transactions so Core
Animation never slides between geometry states.

## Listening-to-thinking morph

The same four mask layers remain alive throughout the handoff. At the moment
listening ends:

1. Read each layer's presentation geometry and opacity.
2. Capture those values as the transition origin.
3. Over 900 ms, interpolate height, width, corner radius, vertical offset, and
   opacity into the first frame of the thinking-dot wave.
4. Continue the thinking phase from the same phase clock.

This avoids replacement, crossfade, and snapping. The bars visibly compress
into four dots while the moving highlight continues across both states.

## Thinking

Thinking uses four dots with fixed increasing base diameters from left to
right. A 1.4-second phase wave moves across them by changing brightness, a
small scale amount, and at most 1.25 points of vertical position. The diameter
offsets are larger than the animated scale delta, so the rightmost dots remain
larger at every frame. Core Animation keyframes can run this phase without a
continuous CPU display loop after the listening morph completes.

## Exact pet-gradient synchronization

Decode `animation_catalog.json` and `gradient_profiles.json` at launch, then
join them by pet ID into one runtime value:

```swift
struct PetStyle {
    let pet: Pet
    let gradients: GradientProfile
}
```

Pet selection publishes one `PetStyle` on the main actor. In the same disabled
Core Animation transaction, the controller changes the sprite sheet and asks
the indicator to transition to `style.gradients`. The gradient is never loaded
asynchronously after the sprite, so mismatched frames cannot appear. Startup
validation requires exactly one profile for every pet ID.

- Listening uses the pet's `ambient` stops, with the profile highlight mixed
  into the leading edge.
- Morphing interpolates from the currently visible gradient without restarting
  the motion clock.
- Thinking uses the profile's `thinking` stops and `cycleDurationMs`.

## Pet-switch wave

Do not directly animate one `CAGradientLayer.colors` array. Intermediate color
interpolation can become gray or muddy. Instead:

1. Keep the outgoing gradient visible.
2. Place the incoming pet gradient directly above it.
3. Reveal the incoming layer with a feathered luminance mask traveling from
   the physical-notch edge toward Kai's outer edge over 520 ms.
4. Put a narrow white-to-transparent highlight at the moving boundary, tinted
   with the incoming profile's `highlight` color.
5. Remove the outgoing layer after the reveal completes.

The wave affects fill only. Bar heights, thinking-dot phase, audio response,
and the listening-to-thinking transition continue without interruption.

## Gradient rendering

A single gradient spans the entire four-shape group so the colors flow across
the indicator rather than restarting inside every bar. A second masked shine
layer provides the polished highlight:

- Base: the pet profile's ordered gradient stops.
- Shine: transparent → `highlight` at 55% alpha → transparent.
- Shine motion: slow translation during listening, faster left-to-right travel
  during thinking.
- No blur larger than 1 point at notch scale; larger glows make the shapes look
  soft and reduce separation from the black notch.

## Efficiency and lifecycle

- Four shape layers, two normal gradient layers, and one temporary gradient
  during a pet switch.
- Audio analysis only while listening.
- A 30 fps geometry timer only while listening or morphing.
- Thinking runs as Core Animation keyframes and consumes no per-frame Swift
  work.
- All sprite sheets and profiles are decoded once and cached.
- Reduce Motion uses static bars for listening level and static right-weighted
  dots for thinking; pet theme transitions become a 150 ms dissolve.
- Screen changes recalculate only the pet and indicator frames. Their animation
  state and theme remain intact.

## Verification gates

1. Overlay debug guides for `physicalRight`, `kaiRight`, `rightWing.midX`, and
   `notchHeight / 2`; verify on every attached notched display.
2. Capture at 2× scale and confirm all indicator edges land on half-point pixel
   boundaries.
3. Switch through all 22 pets during listening, morphing, and thinking; the
   sprite and gradient ID must match on every captured frame.
4. Switch pets rapidly to confirm an in-progress wave begins from the currently
   presented colors rather than an old model value.
5. Measure idle CPU after entering thinking and confirm the display timer and
   audio engine have stopped.
6. Confirm the right-wing center is the exact mirror of the pet center around
   the physical notch midpoint.
