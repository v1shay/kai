# Experimental adaptive panel contrast

Kai Unified 0.12.22 (59). Disabled by default. Enable **Settings → Appearance → Adaptive contrast · Experimental**. The Appearance search accepts “adaptive”. Balanced and Stronger contrast select 4.5:1 and 7:1 semantic-ink targets; Responsive, Balanced settling and Steadier adjust the delay before a panel changes text polarity. The built-in Dark, Bright, Split and Color playground works without screen access and responds to the glass controls.

The chosen behavior is **local panels, consistent text blocks**. A bright conversation pane can use dark ink while a dark sidebar uses light ink. A paragraph is never divided into differently colored characters to follow the wallpaper. Links, syntax and status colors retain semantic distinctions while shifting toward the selected ink. Pet sprites, provider artwork, text contents and layouts are preserved.

## What the opacity slider means

The existing 60% slider is a design input, not the final visible opacity. In the default native composition, shared backdrop tint contributes about 34%; the main surface and sidebar bring the combined tint to about 40% and 49%, respectively. The renderer avoids stacking two full-strength 60% layers. Own wallpaper is composed after those fills with its actual crop, fade and alpha. This experiment models those layers before choosing ink.

Each panel receives a small RGB grid of the external background, excluding this Kai process. For each candidate ink, the solver finds the least protective local tint that meets its internal target on every sample. It chooses the candidate needing less interference with the background, retaining the previous ink when the candidates are close. This preserves the glass where it is already legible. A half-white/half-black panel needs extra tint because one uniform ink cannot otherwise contrast with both extremes. Merely averaging that background would miss the problem.

Persisted settings are delivered after native attachment, including compact/extended-only startup; an earlier GPUI frame cannot consume an undelivered configuration.

The sampling bridge uses a presentation-only path: it neither consumes pending approval/voice acknowledgements nor republishes chat history. Color changes invalidate painting directly.

Semantic colors are checked over the protected luminance range. Extra internal margin accounts for small quantization/material differences. This is a sampled experimental model, **not a claim that every rendered pixel or decorative element meets WCAG**. The [W3C contrast guidance](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html) explains the underlying relative-luminance ratios and why antialiased thin text can benefit from extra margin.

## Surfaces and stability

- Code: sidebar, project/chat list, conversation, file inspector, sub-agent inspector and changes inspector receive separate scopes. Scopes cover eager construction and GPUI's lazy layout, prepaint and paint phases, including inherited text styles. They unwind before another panel/window draws.
- Mini Code: independent bounds, same color rules and shared prepared wallpaper. It does not trigger another decode or transcript load.
- Extended notch: actual glass tint/haze/ink gradients and prepared wallpaper are modeled separately. Native text, control strokes, neutral template glyphs, caret and local AppKit appearance are reversible. Masks and sprite atlases are untouched.
- Compact notch: retains its physical black background and normal ink.
- Terminal: retains its existing ANSI palette on an opaque base surface. Arbitrary application-selected terminal colors are not remapped.
- Window movement and morphing: conservative protection while geometry changes, then finite settling updates. Existing pet positions, window animation curves, layout and transition timelines are unchanged. Ink changes discretely after settling rather than fading black text through low-contrast gray.
- Moving external imagery: abrupt brightness changes temporarily include both black and white in the sample range. Active animated own wallpaper uses conservative bounds. Protection can increase immediately; reducing it and reversing ink wait for stability.
- Settings/theme/wallpaper changes: cached numeric samples are recomposed; regions use monotonically increasing revisions. Disabling stops the sampler, removes guards and restores original native ink. Existing preferences remain valid and keep the experiment off.

## Capture, privacy and budget

Live sampling uses [ScreenCaptureKit](https://developer.apple.com/documentation/screencapturekit/capturing-screen-content-in-macos). Enabling never prompts automatically. If macOS Screen Recording access is unavailable, the settings show an explicit **Allow local backdrop sampling…** action. Granting/retrying is controlled by the user and macOS. The playground remains usable without permission; live adaptation requires it.

Only currently visible panels request streams. Capture excludes the current Kai process to prevent a feedback loop. There is no audio or cursor capture. Each needed display is reduced to about 96 pixels across (bounded for portrait displays), at most two frames per second. Only tiny numeric panel RGB samples cross the existing native bridge. Frames are held transiently in memory, never written to disk, sent over the network, logged, or inserted into agent context. Streams stop when the relevant windows are hidden, minimized, off their active Space, or the display/session sleeps or locks. They resume when needed. Screen coordinates are normalized per display, including negative display origins and Retina scaling.

Changed sample hashes suppress repeated solver work. Geometry events coalesce, updates are throttled independently of the animation frame rate, and settling uses a finite set of cancellable callbacks. Registries and color caches are bounded. The disabled render path returns the existing child directly. No polling process, provider call, screen-sized texture or additional web renderer is introduced.

## Limits and visual trial

The grid is deliberately coarse; thin bright features between samples can be missed. Native material blur, Liquid Glass refraction, HDR/EDR, color management, overlapping windows and rapidly changing video are not an exact reconstruction of WindowServer's final pixels. There can be capture latency before a background change is seen. Extra tint/margin and temporal guards reduce these risks; they do not eliminate them. This is why the feature is opt-in and experimental.

Try a bright window, dark window, split background and colorful wallpaper behind Kai. Move between Code, Mini Code, extended and compact notch, resize sidebars, change glass opacity, switch wallpaper, and toggle the experiment off. Confirm that each panel changes as a unit, text stays readable, and the original appearance returns when off. Also try multiple displays/Spaces and sleep/wake. Manual visual review is intentionally separate from the automated checks; no claim of pixel-perfect material parity is made.

## Verification for this release

Run `scripts/test-adaptive-contrast.sh` for the isolated native and GPUI checks. The native capture harness uses existing permission only, opens a temporary validation window, and accepts an explicit permission-required state when access is absent. It never requests access itself.

Automated checks cover independent bright/dark/split/saturated panels, six opacity levels, tiny safety increases, bounded region eviction, delayed polarity changes, settings persistence, global bridge routing without a chat, lazy GPUI render/prepaint/paint palette isolation, inherited text colors, unchanged layout/draft, and exact restoration when disabled. Native checks cover real ScreenCaptureKit samples reaching the bridge, morph freeze/unfreeze, coalesced geometry updates, strength changes, stopping streams and clearing frames when hidden/disabled, plus reversible text/selection/fonts/caret/template icons and unchanged masks/sprites. Existing theme, wallpaper and workspace-motion tests also run. The signed app is separately smoke-tested.

Development-build measurements are budget checks, not a release benchmark: the 32-panel × 64-sample solver batch took about 21 ms in one unoptimized run; 100 native ink updates on the small fixture took about 7 ms. Real material/visual quality, battery use, HDR and multi-display layouts still require visual trials on the target Mac.
