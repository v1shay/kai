# Kai pet animation categories

The table below describes the original 22 sprite sheets. The catalog now has
30 distinct pets and 32 discovered copies; the extra Kabi and Tiko sheets are
duplicates. Blau, Haaland, Invincible, Killua, L Lawliet, Noir Webling,
Patrick Star, and Peter were added from downloaded 8×9 sheets.

## Canonical state categories

| Product category | Atlas state | Frames | Playback | Intended meaning |
| --- | --- | ---: | --- | --- |
| Ambient | `idle` | 6 | Loop | Quiet breathing, blinking, flame/sprout movement, or another low-distraction resting motion. |
| Directional travel | `running-right` | 8 | Loop | Locomotion toward screen-right. |
| Directional travel | `running-left` | 8 | Loop | Locomotion toward screen-left. |
| Social gesture | `waving` | 4 | One-shot | Greeting or attention gesture, then return. |
| Physical reaction | `jumping` | 5 | One-shot | Anticipation, lift, peak, descent, and settle. |
| Error reaction | `failed` | 8 | One-shot | Sad, broken, dizzy, deflated, or error response. |
| User-attention state | `waiting` | 6 | Loop | Waiting for approval, help, or input. |
| Active-task state | `running` | 6 | Loop | Active work or processing. The state name means “task running,” although several older custom pets interpret it as literal running. |
| Evaluation state | `review` | 6 | Loop | Inspection or review of completed work. Some older pets interpret it as celebration or another personality loop. |
| Directional attention | look rows 9–10 | 16 poses | Pointer-driven | Clockwise attention directions from 000° up through 337.5°. Present only in v2 sheets. |

All sheets use 192 × 208 pixel cells and eight columns. Standard rows 0–8
have fixed timing shared by every pet; see `animation_catalog.json` for the exact
per-frame durations. The v2 neutral still is row 0, column 6 and does not belong
to the six-frame idle loop.

## Motion vocabulary

The actual movements fall into a smaller set of reusable mechanics:

- **Micro-expression:** blinking, smiling, eye movement, screen glyph changes.
- **Body bob:** breathing, bouncing, squash/stretch, tilting, or swaying in place.
- **Biped/quadruped travel:** walking, running, galloping, or waddling.
- **Hover/flight travel:** floating, fin/wing cycling, flame motion, or ghost drift.
- **Mechanical travel:** tread roll, rigid-body shuffle, segmented-stack lean.
- **Limb gesture:** paw, hand, wing, clamp, fin, or appendage wave.
- **Prop interaction:** laptop, tablet, book, notepad, apple, plan/blueprint, or mug.
- **Display-state animation:** monitor face, terminal glyph, NULL/ERROR/question mark, or emoticon changes.
- **Failure deformation:** slump, collapse, melt, kneel, cry, sparks, or error-screen change.
- **Directional look:** eye/head turn, face-surface turn, whole-body yaw/pitch, or flexible-body bend.

## Per-pet inventory

“Travel” covers both left and right rows using opposing versions of the same
basic motion. Descriptions below are visual observations of the complete ordered
frame rows, not meanings inferred only from the row names.

| Pet | Idle | Travel | Wave / jump | Failed | Waiting | Active task (`running`) | Review | Look directions |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 77 | Lying cat blinks and briefly sleeps. | Low quadruped gallop. | Paw wave; crouch-to-leap cycle. | Ears droop, cries, curls down, then sits sadly. | Seated blink and head tilt. | Alert pounce/run-in-place. | Paw-to-chin thinking and head tilts. | None. |
| Agumon | Standing blink, grin, and small body shift. | Alternating dinosaur sprint. | Claw wave; crouch and airborne leap. | Facepalm, dizzy stars, slump, and collapse. | Expectant standing blink/bob. | Literal run-in-place. | Pointing/claw gestures, head turns, and wink. | None. |
| Aqua Wisp | Hover bob, blink, and brief body turn. | Fin/leg cycling hover in each direction. | Fin wave; subtle vertical hover-hop. | Face droops and cube body collapses toward the floor. | Quiet hovering blink. | Faster hovering/appendage processing loop. | Face/appendage inspection with a small orb-like gesture. | None. |
| Kabi | Sleeps on the ground with breathing and sleep bubbles. | Upright waddling step cycle. | Arm wave while holding an apple; seated leg-lift hop. | Drops the apple, slumps, falls, and returns to sleep. | Repeating apple-eating loop. | Repeating apple-eating loop. | Repeating apple-eating loop rather than inspection. | None. |
| Ultronite | Rigid standing micro-bob and eye/head changes. | Humanoid mechanical jog. | Raised-hand wave; squat and armored jump. | Head drop, kneel, sit, and recovery. | Small calculating hand/head gestures. | Hands and forearms move in a processing gesture. | Chin/hand thinking poses and head tilts. | Head and upper-body turn with red-eye tracking across 16 angles. |
| Winter Curry Dog | Standing blink with ear and expression changes. | Side-on puppy run. | Paw wave; squat with both paws raised. | Cries, bows, sinks down, and recovers. | Blink, droop, and questioning head tilt. | Small standing bounce/gesture loop. | Paw-to-mouth thinking and body lean. | None. |
| Tiko | Camera-eye/display micro-animation and chassis bob. | Tread-driven travel with clamp-arm motion. | Clamp wave; suspension/tread hop. | Sparks, tilts, and falls onto its side. | Eye-screen scan and small clamp gestures. | Low tool/ground-work loop around the tread base. | Holds and studies a green plan/panel. | None. |
| NezukoCoder | Seated laptop work with blinking. | Runs while carrying an open laptop. | Free-hand wave; airborne laptop-carrying leap. | Tears, slumped head, and laptop-hugging sadness. | Seated laptop blink/typing loop. | Seated typing/work loop. | More focused laptop inspection with eye and head changes. | None. |
| Savage Codex Hacker | Laptop typing, face glyph, and sunglasses changes. | Sideways scoot while holding the laptop. | Free-hand wave; subtle laptop/body hop. | Frustrated slump and face-down-on-laptop reaction. | Laptop typing loop. | Laptop typing loop. | Sunglasses/head tilt while inspecting the laptop. | None. |
| Ghost | Floating blink and smile changes. | Directional ghost drift with tail deformation. | Side-lobe wave; squash/stretch hover-hop. | Sad face followed by a melt into the floor and re-form. | Gentle floating smile/blink. | Hands/appendages gather toward the center in a processing gesture. | Body/face turns and contemplative tilts. | None. |
| Macintosh | Standing screen-face blink. | Small-legged computer walk. | Side-arm wave; feet-up hop. | Sad screen, wobble, sit/slump, and recover. | Screen blink with small hand movement. | Walking/bobbing-in-place loop. | Screen squint, body tilt, and thinking gesture. | None. |
| Lil Finder | Blink with a small arm/body gesture. | Alternating biped walk. | Arm wave; arm-and-leg jump. | Crying, eyes squeezed shut, and body droop. | Blink, head/body tilt, and small hand gesture. | Literal run/walk-in-place. | Chin-scratch/thinking poses. | None. |
| CRT Pal | CRT face changes and subtle body bob. | Alternating biped walk. | Arm wave; knee-lift hop. | Screen droops while the monitor tilts, squashes, and sits low. | CRT expression changes with a pronounced monitor tilt. | Walking/dancing-in-place loop. | Repeated monitor bend and inspection tilt. | None. |
| Bsod | Robot blink, smile, antenna, and body bob. | Side-on robot walk. | Arm wave; compact vertical bob/hop. | Screen turns red with error/X eyes while the body panics and slumps. | Head/screen tilt and expectant body bob. | Works at an open laptop. | Eyes scan and change expression while standing. | Head and body pitch/yaw together through 16 angles. |
| Codex | Terminal-face blink and calm body bob. | Small biped walk. | Small hand wave; buoyant body hop. | Screen-face error symbols, grimaces, and worried poses. | Screen gaze/expression loop. | Works on a handheld tablet. | Face changes with hand and body gestures. | Large head/body turn around the full direction loop. |
| Dewey | Blink, cheek/face changes, and soft droplet squash. | Waddling side walk. | Arm wave; squash/stretch hop. | Alarmed shake, cry, and partial melt/slump. | Expressive mouth and body bob. | Works at a laptop. | Happy bounce/celebratory body movement. | Face and droplet body turn/squash across 16 angles. |
| Fireball | Blink plus continuous flame flicker. | Fast biped run with trailing flame shape. | Hand wave; squat and flame-led leap. | Panic, flame burst, X eyes, crying, and slump. | Pondering/shrugging hand and face changes. | Works at a laptop on a desk. | Cheerful dance/celebration rather than close inspection. | Head, face, and flame direction change across 16 angles. |
| Hoots | Blink, glasses, and small head/body tilt. | Wing-flapping flight. | Wing wave; wing-lift hop. | Eyes close and the owl slumps sadly. | Wing fidget/shrug and expectant pose. | Writes on a notepad and alternates with a mug. | Reads and studies an open book. | Eyes, head, and body turn as an owl-like gaze loop. |
| Null Signal | NULL display alternates with a red emoticon and cable/body bob. | Small mechanical walk with screen held forward. | Arm/display gesture; whole-unit bounce. | Display changes to ERROR while arms and body shake. | Screen shows a question mark with questioning tilts. | Works at a laptop. | NULL display flicker with small inspection turns. | Screen housing and body rotate through 16 viewing angles. |
| Rocky | Blink and quiet resting bob. | Small grounded shuffle/walk. | Side gesture; squash-and-rise jump. | Sweat/alarm expressions, shake, and worried recovery. | Yawn/thoughtful hand-to-mouth loop. | Works at a laptop. | Happy bounce/celebration. | Whole body turns around a stable base through 16 angles. |
| Seedy | Blink with sprout sway. | Small biped walk with leaf follow-through. | Subtle arm/sprout greeting; springy sprout-led jump. | Crying, dizzy eyes, open-mouth alarm, and recovery. | Head and sprout tilt with expectant expressions. | Works at a laptop. | Happy bounce/celebration with sprout movement. | Body, face, and sprout rotate/tilt through 16 angles. |
| Stacky | Screen blinks, top lights flicker, and stacked body bobs. | Segmented leaning shuffle. | Top-unit/arm greeting; stack stretches and hops vertically. | Distressed screen faces, shaking arms, and X eyes. | Top-unit arm gestures and pondering screen changes. | Works at a laptop. | Top unit lifts/celebrates while the stack bounces. | Segmented stack bends and tilts through 16 angles. |

## Useful implementation groupings for Kai

For a notch renderer, the state rows can be exposed through five higher-level
groups while retaining the original state id:

1. **Persistent calm:** `idle`.
2. **Persistent status:** `waiting`, `running`, `review`.
3. **One-shot feedback:** `waving`, `jumping`, `failed`.
4. **Spatial movement:** `running-left`, `running-right`.
5. **Pointer attention:** the 16 look directions when present.

This grouping separates animation lifetime from visual content. Persistent
states may loop indefinitely; feedback states play once and transition to a
status state; travel states can accompany movement between notch positions;
look poses respond continuously to a target point.

## Coverage

- Standard nine states: all 30 distinct pets. L Lawliet uses its declared
  six-frame wave and per-state timing while keeping waving a one-shot gesture.
- Sixteen look directions: Ultronite plus all nine built-in pets (`bsod`,
  `codex`, `dewey`, `fireball`, `hoots`, `null-signal`, `rocky`, `seedy`, and
  `stacky`).
- No look directions: the remaining 20 v1 custom/downloaded pets.
