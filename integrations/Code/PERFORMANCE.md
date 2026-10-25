# Performance verification

Run the reproducible state workloads with `cargo run --release --bin bench`. They use a temporary database and leave actual agent sessions untouched. Capture the JSON output, hardware model, release commit, and background workload together. These measurements do not demonstrate frame rate or interactive latency.

For the native app, build with `./scripts/bundle.sh`, open `dist/Code.app`, and use Apple's Instruments:

1. Time Profiler: type, switch between 100+ sessions, open a large conversation, scroll while an agent streams, then return to idle. Look for main-thread filesystem access, database work, Markdown parsing, and repeated layout of offscreen rows.
2. Allocations and VM Tracker: compare a fresh window, one paged transcript, scrolling through a transcript over 20 MiB, four active agents, and ten minutes of idle use. Record the UI process separately from agent processes. Confirm distant decoded pages and closed file previews release their content.
3. Animation Hitches / Core Animation: measure frame pacing on the actual display. Check sidebar and picker springs, and scrolling with large code output. A display's refresh rate sets its achievable FPS.
4. App Launch: record the first presented frame across cold and warm starts. Separately record when discovery finishes; the window intentionally opens first.
5. Energy Log: confirm CPU/GPU activity settles after transitions and resumes only for input or streaming. Repeat with Motion off.

Use identical conversation data and background workloads for any comparison. The app's responsiveness architecture and a microbenchmark are not evidence of superiority over another client.

## Original baseline before the Monocode redesign

Measured October 11, 2026 on macOS/aarch64, hardware model `Mac15,12`, while development tools were open. Single runs, not percentile distributions:

| Workload | Time |
| --- | ---: |
| Open SQLite + schema | 4.76 ms |
| Insert 10,000 summaries, implicit WAL transactions | 642.90 ms |
| Upsert 10,000 summaries, implicit WAL transactions | 494.65 ms |
| Load 1,000 sidebar summaries through Store | 1.41 ms |
| Indexed SQL page of 300 summaries | 0.32 ms |
| Title search through Store | 4.92 ms |
| FTS message search through Store | 3.74 ms |
| Parse 4 MiB Markdown into 98,690 rows | 49.81 ms |

The running release app's process RSS was 56,352 KiB (55.0 MiB) after roughly one minute with discovery complete; CPU was 0.4% in that sample. RSS excludes some shared/GPU allocations and is not an Instruments memory-footprint result. The packaged executable was 6.8 MB. A second sample at four minutes remained at 56,352 KiB RSS with 0.2% CPU. Discovery indexed 300 session summaries from 23 workspaces. Native visual inspection was blocked because the computer-use service failed to start; startup, typing latency, scroll FPS, and visual smoothness remain unmeasured.


## October 12 Rust and resource review

Reviewed all 17 production Rust modules, including the embedded asset catalog, plus the benchmark, native UI tests, dependency configuration, and packaging script. The runtime remains Rust/GPUI with Metal and direct macOS APIs. There is no JavaScript runtime or web view. macOS audio and Quick Look subprocesses remain for platform media support; the Swift icon generator is build-time only. Thin LTO, a single release codegen unit, and stripped binaries remain enabled.

The review removed recurring UI polling for quiet agents and replaced pipe retry sleeps with readiness checks. Actual stream events are coalesced into frames. Production backpressure waits on a condition variable, with limits of 128 events and 2 MiB of payload; one larger replacement is allowed when empty within the 16 MiB live-response limit. Runtime pipe waits retain a 100 ms lifecycle check for child exit and cancellation. These are bounded resource controls, not a claim that an active provider uses no CPU.

Discovery now uses atomic batches, cached statements, and unchanged-row suppression. Session ownership changes no longer re-tokenize unchanged message bodies in FTS. Superseded queued selection/search requests are skipped before expensive reads, while all writes remain ordered. OpenCode metadata is fetched in batches of at most 128 rows and small parts decode without temporary spools. History rehydration inspects visible rows rather than scanning all deferred rows on each scroll. Long inline-styled Markdown uses an ordered span pass, and URLs share allocations across rows. Composer replacements allocate their final size once. Background imports reuse decoded rasters, unchanged settings skip writes, and abandoned file/video previews cancel and reap native helpers.

Correctness checks added during optimization include atomic batch rollback, zero SQLite changes for unchanged upserts, exact streamed comparison of giant messages with identical lengths/bookends but different middles, stale queued read cancellation with write preservation, byte/item backpressure and cancellation, cross-row Unicode formatting, retained raster ownership, and media-helper cancellation. A 1,000-part OpenCode fixture uses at most nine metadata queries, keeps at most 128 metadata rows queued, and creates no small-part spools.

### Matched release measurements

Measured on October 12, 2026, `Mac15,12`, macOS 14.2/aarch64. Each value is the median of five runs per version, alternating version order. The original source was preserved before review; both versions use the same dense-Markdown workload. Development tools were open, but no build or test suite ran concurrently and the development app was stopped for these measurements. This checkout has no commit yet, so the baseline is a local source snapshot rather than a release commit.

| Workload | Before review | After review |
| --- | ---: | ---: |
| Open SQLite + schema | 1.176 ms | 1.200 ms |
| Insert 10,000 summaries, implicit WAL transactions | 433.622 ms | 360.057 ms |
| Unchanged upsert of 10,000 summaries, implicit transactions | 353.446 ms | 53.245 ms |
| Load 6,000 sidebar summaries | 6.243 ms | 6.387 ms |
| Indexed SQL page of 300 summaries | 0.178 ms | 0.184 ms |
| Title search | 2.094 ms | 2.107 ms |
| FTS message search | 2.075 ms | 2.092 ms |
| Parse 4 MiB Markdown with code | 29.858 ms | 30.369 ms |
| Parse one long paragraph with 10,000 repeated inline-style groups | 10.918 ms | 7.164 ms |

Ordinary reads and the general Markdown workload remain similar; no improvement is claimed for those small differences. The new discovery batch API inserts 10,000 summaries in 28.548 ms and repeats an unchanged batch in 24.829 ms. Those supplemental timings measure the new batch path; they are distinct from the individual-call measurements above.

The separately run **debug** warmed image-import kernel processes twenty 1280 × 720 PNGs: 2.911 seconds with the former double-preparation path versus 1.423 seconds with raster reuse. This isolates decoding/effect work; it is not release-build end-to-end import latency.

Raw matched outputs and executable SHA-256 hashes are saved locally under `.build/performance-review/` in `baseline-matched.json`, `optimized-matched.json`, and `context.json`. The saved benchmark executables are separate to avoid Cargo's shared output-path caching substituting one version for the other. An earlier compiler-contended run is excluded from the reported table.

### Release verification and idle behavior

At completion of the optimization review, `cargo test --all-targets` passed 141 application tests and 39 benchmark-target tests; the one ignored image-import workload also passes when invoked explicitly. Some shared module tests run in both targets. `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` pass. `./scripts/bundle.sh` builds the release app and `codesign --verify --deep --strict dist/Orbit.app` passes.

The rebuilt release app was launched with the existing isolated development history. At 25 seconds it reported 61,184 KiB RSS (59.8 MiB) and 0.0% sampled CPU; at 98 seconds it remained at 61,104 KiB RSS (59.7 MiB), 0.0% sampled CPU, and 0.92 seconds cumulative CPU time. A two-second native `sample` captured every main-thread sample waiting in the AppKit event loop and every state/preferences/background worker sample waiting on a semaphore. The report recorded a physical footprint of 91.3M and a peak of 195.3M. This exceeds the proposed 80 MB physical-footprint target; process RSS alone must not be used to claim that target is met.

The pre-review app also parked correctly at idle. Its sample came from a much older process, so those memory values are not a controlled before/after comparison. Samples are saved in `idle-before.sample.txt` and `idle-after.sample.txt`. No live provider/model requests were sent. Native UI automation was unavailable in this session; startup-to-first-frame, interactive latency, scroll FPS, GPU energy, and visual smoothness remain unmeasured and require release Instruments profiling.

## Rendering regression checks

The current October 12 build passes 145 application tests and 39 benchmark-target checks. One additional focused image-import workload is intentionally ignored in the ordinary suite and passes when run explicitly. Twenty-four native GPUI layout/paint and input integration tests check that the 10,000-row transcript renders fewer than 100 rows at each of four viewport positions. Column bounds are tested for 200 px/260 px widths, equal heights, and adjacent edges. Long Unicode project/session names are checked for equal 8 px outer insets, constrained labels, and 10 px session text insets without vertical overflow. The 360 px file inspector preserves at least 360 px for chat at the tested window width. The collapsed sidebar exposes a tested 28 px reopen button. These checks validate native layout and virtualization in GPUI’s test renderer; they do not measure Metal FPS or prove visual smoothness.

Compact project/chat menu checks verify 160 px anchored popovers, unchanged workspace bounds, window-edge clamping, outside-click passthrough, Escape dismissal, and release after finite exit animation. All seven provider SVGs fit the bottom rail with equal spacing. Chat removal/Undo remains filtered across discovery and search, and preference-worker restart tests preserve both removal and restoration without touching native sources.

The native tests also cover grapheme editing, multiline input, clipboard operations, UTF-16 IME composition, bounded paste behavior, search submission, modal focus restoration, project-chat navigation, nested project grouping, draft retention, rich transcript/tool rendering, project removal/Undo, and stale-load errors. Prepending variable-height messages preserves both the logical scroll anchor and the existing message's pixel position. Actual scroll events trigger one older-page request at a time. Twelve-page fixtures exercise eviction and rehydration while preserving the viewport.

State regressions traverse histories beyond 10,000 turns. Native fixtures cover transcripts and individual JSON records over 20 MiB, lossless Unicode/escape handling, skipped oversized control/media payloads, fenced-code continuations, local/native seam deduplication, fixed snapshots, and generation-scoped requests. These checks establish bounded reads and correct paging, not a latency guarantee for an initial scan of one giant record. File tests cover lazy directory pagination, Unicode-safe text pages, confinement, preview race cancellation, and released assets. The benchmark reads 6,000 summaries instead of the original 1,000.

Background fixtures check Monocode's dark-theme pixel values, Haze blur/gradient treatment, real GIF frame order/delays/repeat counts, inactive-window artwork, failed-import recovery, managed source preservation, native ImageIO thumbnails, SVG text/geometry, and bounded embedded rasters. GIF work pauses when hidden, inactive, or motion is off; only its entity requests frame updates. Twelve regular background tests include native GPUI artwork visibility, retained pixel ownership, and coalesced settings writes; the separate image-import workload is run explicitly.

Provider fixtures cover all seven streaming adapters without sending live model prompts, plus bounded native Gemini JSONL and OpenCode SQLite history. The OpenCode fixture keeps a cursor open while a second writer successfully checkpoints WAL, proving paging does not retain a database read transaction. Giant injected-plugin fixtures skip hidden middle bodies without leaking them as chat or requiring hundreds of empty pages.

## Native composer and streaming follow-up

The wallpaper now fades after cover-cropping into the chat pane's actual aspect ratio. All six effects and GIF frames reach true transparency; bounded artwork stops below the chat area. Static decoding is reused across resize updates, resize work is debounced, and GIF frame order/playheads survive layout changes. Compact agent/model controls use finite transitions and cancel queued probes when closed. Native account/model inspection is bounded and cached, with unknown authentication kept distinct from verified login.

Apple Speech dictation runs on a dedicated worker only after an explicit microphone action. Audio buffers feed Apple's request directly. Recognition callbacks coalesce into a four-event queue; stale partials cannot replace terminal results. Draft revisions collapse the caret, clear marked text, and scroll to the newly measured last line. Editing, sending/stopping, chat changes, Escape, and teardown cancel capture. macOS privacy keys are checked before requesting access; unbundled development launches report a clear error instead of triggering Apple's fatal missing-key behavior.

Image imports decode serially off the UI thread. Four images are allowed per prompt, with bounded encoded sizes, 2048px managed send rasters, 160px retained thumbnails, and a single 768px on-demand preview. At most 32 distinct unsent draft images are retained across chats. Managed storage is capped at 512 MiB/4096 entries. Last-reference cleanup reserves a queue slot during import, so UI removal only enqueues work to one sleeping worker. Submitted files remain for saved transcript references. Removed images are excluded immediately when sending or switching providers, even while their exit animation is visible.

Live Markdown and typed Codex speaker/delegation/status/question cards are parsed from a bounded tail on a worker. Each run has one pending parse, newer deltas coalesce, and both revision and invocation identity reject stale results. Private card markers survive corrected text snapshots. Giant voice/code continuations carry optional private fence metadata without changing original speaker text; cross-page code styling and subsequent citations remain intact. Question options populate the draft; the app does not claim to answer historical tool calls or implement Codex's live audio transport.

Native test-renderer checks cover image preview/hover removal, draft retention while switching providers, unsupported-provider rejection, removal during an exit animation, question choices, disclosure settling, non-selected dictation, last-line scrolling, and stale live parsing. Actual microphone capture/permission dialogs and visual release-window inspection were not exercised: the desktop computer-use service failed to start. Startup-to-first-frame, scrolling FPS, typing latency, and GPU/energy targets still require Instruments on the target display.

Final source verification on October 12: **212 application tests and 54 benchmark-target tests pass**, including 33 native Orbit UI integration checks. The separately invoked ignored image-import workload also passes. Strict Clippy and formatting checks pass. The warmed image workload remains a debug-only kernel check, not a UI latency or release performance measurement. Direct Foundation bindings enable only std/NSString rather than the framework crate's entire default feature set.

The final signed release was reopened with the user's existing history and static dithered JPG wallpaper. This is an uncontrolled real-use observation, not a matched benchmark: an early sample included UI redraws and 17.5% sampled CPU, while a later three-second trace had 2,386 of 2,409 main-thread samples waiting in the AppKit event loop, with the state/preferences/background workers asleep. Subsequent process readings at 8:52 and 9:12 uptime reported 0.6% and 0.5% CPU; cumulative CPU increased only 0.13 seconds between them. No persistent redraw loop was established, so no speculative rendering change was applied.

The later sample reported a physical footprint of 175.2M (297.8M peak); the settled process reading was 201,312 KiB RSS. These measure different memory accounting categories at different instants. The 80 MB target remains unmet with this workload. Saved traces are `.build/latest-idle.sample.txt` and `.build/latest-idle-followup.sample.txt`. Current native-window interaction and wallpaper allocation behavior need a controlled Instruments session before claiming a memory or energy improvement.

## Code rename and bundled wallpaper

The app now displays as Code and bundles the user-selected Bonsai PNG. One managed source is shared across projects; existing background-worker decoding, viewport caching, cover-cropping, and fade rendering remain in use. Settings can change/remove artwork or restore the bundled default. Removing one project's wallpaper retains the shared source, and the removal survives reload. PNG signature detection precedes SVG sniffing, preventing an embedded SVG metadata thumbnail from misclassifying the artwork.

Verification passes **213 application tests and 54 benchmark-target tests**, including native settings-entry/draft-preservation checks and shared-wallpaper/remove/restore fixtures. The bundle identity and existing data directory remain stable across the display-name change. This pass adds no new frame-rate, memory, or energy claim.

## Anchored glass controls

Search (360 px), workspace selection (320 px), and wallpaper controls (340 px) now use anchored translucent surfaces without a full-window backdrop or shadow. Image previews also anchor above the composer. Dropdown entrance and exit transitions request frames only for 320 ms and 120 ms respectively. Animation notifications target the owning GPUI view explicitly, including transitions initiated during root layout. Search and workspace lists virtualize 10,000-row fixtures; controls leave all three workspace columns unchanged and clamp to an 8 px window margin. Wallpaper settings retain their effects, scope, opacity, import, restore, and removal controls in a compact layout.

Verification passes **217 application tests and 54 benchmark-target tests**, including 37 native UI checks, plus strict Clippy and formatting checks. Checks also cover toggling a dropdown closed from its own button with motion both enabled and disabled. GPUI's debug selector map retains previously painted selectors; exit cleanup is therefore verified against the live surface state rather than stale debug geometry. These are functional/layout checks, not a new FPS or energy measurement.

## Gradual wallpaper fade

All artwork effects now compose with progressive haze and a continuous transparency curve instead of the shorter piecewise-linear mask. The artwork extent scales with the chat pane and remains capped at 780 px for empty chats and 700 px for occupied chats. Detail stays crisp at the top and softens toward the tail. Blur runs on a premultiplied float raster capped at 256 px per side; transparent source pixels cannot introduce dark halos. Haze blending, alpha fading, and BGRA conversion share one in-place output pass, retaining the native raster allocation. Static processing remains on the background worker and reused while scrolling; GIF processing retains its 512 px frame limit and focus/visibility pauses. No idle animation or per-scroll blur is added.

New checks cover the continuous fade, actual detail softening across all six effects, crisp top pixels, bounded haze allocation, transparent edges, and cached decode/GIF behavior. All **220 application tests and 54 benchmark-target tests** pass, along with strict Clippy and formatting checks. Animation tests use native frame scheduling rather than manually repainting with an outdated frame timestamp. No new hardware-performance measurement is claimed.


## Agent execution and process-lifecycle repair

The source was first committed and pushed as `7fe618b` before these repairs. The native launcher now supplies a child-only PATH for CLI interpreters, authentication scripts, and status/model probes. Directory discovery is cached for 60 seconds on demand, bounded across eight homes and bounded runtime-manager entries; it launches no shell profiles and adds no recurring UI work. The app remains Rust/GPUI. External CLIs retain their own implementation and runtime dependencies, including Node where applicable.

Real minimal-PATH prompts (`PATH=/usr/bin:/bin`) completed through the Rust adapter for Codex, including resume, OpenCode, AGY/Antigravity, and Cursor after explicit workspace trust. Claude reported insufficient credit; Grok reported missing sign-in; Gemini reported unsupported client/account eligibility after its trust requirement was satisfied. These are observed provider responses, not simulated successes. No billing, credentials, or provider eligibility was changed. Tiny prompt checks do not establish every coding workflow, model, tool, or approval path.

Further regressions now cover blocked-stdin cancellation, full event/byte queues, empty/unknown successful streams, CLI-wrapper descendants, and nonreaping exit detection. Cancellation terminates input writes with a non-retryable error; group ownership remains reserved until owned descendants are terminated and the direct child reaped. Cancelled terminal status survives queue saturation without blocking the producer. Native UI tests exercise composer submission through a disposable CLI process, saved response/completion rendering, failed-draft restoration without clobbering new typing, thread-local failure notices, and explicit Trust & retry/Cancel behavior. Workspace confirmation retains native tool-permission mode.

Verification: **239 application tests + 57 benchmark-target tests pass** (296 active checks, some shared modules run in both targets); image-import and authenticated live-prompt workloads are intentionally ignored in the ordinary suite. Strict Clippy, formatting, and whitespace checks pass. No new FPS, startup, memory, or energy measurement is claimed; the previously observed memory target remains unmet.


## Imported Codex chat ownership

An imported chat can be owned by the Codex desktop app while still appearing idle. Resuming its native ID directly fails with `thread-store conflict: ... already has an active writer`. New-chat and ordinary resume smoke tests did not cover this ownership case.

The first imported prompt now uses native `codex exec fork`, preserving sandbox mode, model, images, stdin, and stored context while keeping the original writer open. A continuation card appears only after Codex returns a different native ID. The UI chat stays stable, and the state worker explicitly binds that ID; subsequent prompts resume the child. Indexed, read-only native metadata lookup switches paging to the child's rollout, retrying when its index row arrives at completion. Rediscovery, stale UI snapshots, and restart cannot replace the child's binding with its parent or duplicate the sidebar entry. No transcript bodies are copied or decoded by the frontend to create the fork, and no writer lock is stolen or deleted.

Reproduced the exact active-writer failure using a disposable native app-server holding a previously created tiny test chat. The repaired Rust fork path completed a real read-only prompt while that holder remained open; a subsequent real resume of the child also completed. New native UI tests exercise imported fork then owned resume, original-file preservation, and explicit recovery. State tests cover ID rebinding, late native indexing, stale metadata, restart, saved messages/model options, and child deduplication. Error presentation summarizes stderr rather than repeating a raw stack trace in the transcript and composer.

Verification: **250 application tests + 60 benchmark-target checks pass** (310 active checks with shared module duplication), strict Clippy, formatting, and whitespace checks. Image and authenticated live-prompt tests remain explicit opt-ins; the live ownership scenario was verified separately. This pass adds no FPS, memory, or startup claim.


## Kai native indicators

Kai’s artwork and animation catalog were imported from the local `notchdex` source: 26 SVGs (24 status scenes plus open/closed file glyphs), 20 integration SVGs, and the complete pose/timing data. Production playback is Rust/GPUI; the Swift engine is reference material, not a new runtime or subprocess. Original SVGs and attribution remain embedded. Existing CLI provider marks are retained.

The live player parses and validates the catalog once, retains eight tracks with at most four keys each, and uses fixed pose/shape-weight arrays. Status changes interpolate position, size, rotation, radius, opacity, and shape mixes from their current presentation. Shape blending has five bounded slots per primitive; typically one or two are painted. Brand changes retain at most two SVG marks with finite crossfades; a live tool mark may gently breathe while work is active. Silver shading samples the original three-stop palette across shared coordinates; it approximates the original global Core Animation gradient mask. The compatible nine-vertex file glyph has its own finite 0.42-second transition.

Historical activity stays static and virtualized. Only the visible live status loops; the file button animates only while changing. Idle/terminal/hidden states stop frame requests. Existing Apple Speech capture supplies four atomic meter values without allocating or adding queue events; processing is limited to 256 first-channel samples per callback. The indicator uses the native gain/attack/release treatment and a cancellable 30 Hz task only during ready, visible listening. No extra microphone session or permission request is made for the visual meter.

The full native suite exposed a latent GPUI assertion when a spring was supplied as animation progress: overshoot exceeds the framework’s permitted 0–1 range. Spring shaping now occurs inside visual transforms while the framework’s time progress stays bounded, preserving bounce without the assertion.

Verification: **274 application tests + 60 benchmark-target checks pass** (334 active checks, shared module duplication); strict Clippy and formatting pass. Coverage includes all 24 scene geometry/keys, interrupted numeric and mixed-shape morphs, finite/paused state, bounded brand transitions, real-level meter math, file-glyph interruption, MCP integration identity, and native header layout/paint for every scene. The verified release now updates the existing `dist/Code.app` bundle, replacing its executable by rename so the running instance retains its mapped executable. The updated bundle passes strict code-signature verification. No app is launched, closed, or restarted during packaging; the user can quit and reopen Code to load the changes. No new FPS, memory, startup, or energy claim is made.

### Live status placement correction

The live player now mounts directly beside the activity caption above the composer during agent startup, runtime work, and dictation. The prior placement in the titlebar left the activity caption without its replacement for the old indicator. Only one player is mounted; idle/terminal presentation returns to the titlebar. Dictation and launch glyphs take precedence over tool brand marks so those marks cannot cover the waveform or launch feedback.

Native paint regressions now check actual nonzero canvas bounds and visible vector paths for all 24 scenes beside the caption, not just the header wrapper. A motion-enabled test advances the clock and verifies changing geometry, then completion and stopped frame requests; another verifies startup before the runtime exists. Paint counters and clock controls compile only in tests. **276 application tests + 60 benchmark-target checks pass** (336 active checks); strict Clippy, formatting, and whitespace checks pass. No new performance measurement is claimed.

## Continuous dictation, syntax previews, and native changes

Apple's final recognition result commits a phrase instead of stopping dictation. The worker replaces just the recognition request on the existing microphone engine/tap. Earlier revisions updated the current partial phrase; the append-only pass below supersedes that behavior. Empty partial/final results retain the most recent phrase, generations reject callbacks from finished requests, and explicit Stop flushes the latest queued snapshot before releasing the composer state. The four-event queue coalesces cumulative snapshots. Capture does not copy or queue PCM; a nonblocking request swap can skip a buffer during restart. Genuine provider/permission errors and the existing 64 KiB draft limit still stop recognition explicitly. Composer selection remains collapsed, and fresh wrapping/layout follows the latest caret through pauses and width changes.

Source previews use a dependency-free, extension-aware lexer on the worker instead of a synthetic Markdown-fence roundtrip. Twenty language families have token colors; ordinary text stays plain. Rows remain virtualized and source reads remain 64 KiB. Up to 32 small, versioned lexer checkpoints preserve multiline state between pages without scanning earlier bytes; size/mtime changes invalidate stale states. A direct citation into an uncached middle page starts locally, so a multiline token beginning before that page may have approximate coloring until the page boundary is reached through normal paging.

Clicking a Completed card opens a finite, animated glass Changes inspector. It shows current working-tree changes against HEAD, including staged edits, rather than claiming a historical per-turn snapshot. File summaries and the selected patch are read on demand; there is no Git watcher or idle refresh. Both lists are virtualized. Captures are limited to 64 KiB/256 file paths and 256 KiB per patch, with visible limits; rows have UTF-8-safe 2 KiB chunks and cached horizontal extent. Add/remove backgrounds, old/new gutters, counts, and syntax spans are native GPUI. Renames retain both scoped paths; binary, symlink, and mode-only changes get explicit notes. Cancelled/stale reads cannot replace the current selection. Git runs read-only without pagers, external diffs, textconv, or configured clean/process filters; owned process groups are terminated before reaping, with an eight-second read deadline. No global Git configuration, index, or source file is changed by review.

Verification: **301 application tests + 60 benchmark-target checks pass** (361 active checks, two opt-ins ignored), strict Clippy, formatting, and whitespace checks. Regressions include finalized/empty/stale dictation results and stop flushes, real native audio buffer formats without microphone capture, long Unicode wrapped-caret layout, page-state invalidation, staged/unstaged/renamed/deleted/untracked Git fixtures, bounded large patches, configured filter suppression, cancellation, and a real completion-click → Changes pane → close UI flow. These changes add bounded work while their features are used; zero hardware overhead, FPS, memory, and energy improvements have not been measured or claimed.


## Append-only dictation and saved native workspace

Dictation now publishes only accepted, monotonic spoken material. It buffers unstable trailing words rather than rewriting the composer when Apple revises a partial. The input tracks consumed speech independently of editable text: automatic updates append only new words to the current draft, so manual deletions remain deleted. Phrase finalization replaces only the recognition request on the existing microphone engine/tap; stale callbacks, empty results, and pause boundaries cannot retract accepted text. Explicit Stop flushes the remaining word and queued snapshot. Real speech-service/privacy errors and the existing 64 KiB composer limit still stop capture with a notice. A live microphone pause test remains a manual check; automated checks use recorded callback sequences and native buffer fixtures.

One saved workspace contains bounded window geometry/mode, project, sidebar, up to eight tab IDs, active agent/model/edit mode, drafts, file preview page/scroll, and selected Changes file. Inactive transcript bodies stay cold. The active transcript restores one selected raw page and cursor boundaries, rather than replaying every page between the newest turn and the saved position. Immutable generated indices/envelopes survive through private links; source transcripts are neither modified nor copied. Cursor metadata is capped at 2 MiB. Changed sources, over-budget checkpoints, and unsupported large cross-volume artifact links fall back to recent messages with a notice. OpenCode database/WAL fingerprints conservatively invalidate on other database writes too. Monitor changes clamp the restored window onto an available display.

UI changes replace a single 250 ms debounce task. The snapshot worker has one replaceable pending slot and a sleeping wake channel; JSON serialization, normalization, atomic 0600 writes, and synchronization occur off the UI thread. No idle save timer or conversation scan is added. The 4 MiB workspace-file cap budgets the worst JSON escaping for nine full 64 KiB drafts without truncating valid editor contents. Normal Cmd-Q awaits save barriers before platform shutdown because GPUI's platform shutdown hook otherwise gives only 100 ms; forced termination cannot guarantee the final unsaved keystroke. File previews reopen directly at the selected byte page with a 64 KiB read.

Settings now expose independent native glass blur (0–64 px) and panel opacity. Unchanged blur values skip native calls; optional CGS radius functions resolve once, and AppKit material remains the fallback. Panel tint changes do not fade text, controls, or the whole window. These are on-demand changes, not a new rendering loop.

Verification: **321 application tests + 60 benchmark-target checks pass** (381 active checks, two opt-ins ignored); strict Clippy, formatting, and whitespace checks pass. Regressions cover append-only corrections, deleted text staying deleted, pause/restart/Stop boundaries, nine worst-case full drafts across restart, native slider interactions, inactive tabs staying cold, exact page/row/pixel restoration, final-draft flush on Quit, a source preview beyond 16 MiB, and deep native/local/OpenCode 20 MiB conversation restoration without constructor body replay. No new startup, FPS, memory, or energy measurement is claimed.


## Native commands, usage, and explicit handoff

The slash picker uses one lazily initialized static catalog per CLI (27–29 controls), plus at most 128 validated Claude headless names for the active checkout. Native catalogs retain at most 16 provider/checkout scopes. Filtering runs only when the command query/provider/catalog revision changes; descriptors share cached names through Arc. A uniform virtual list paints at most seven visible rows. Its 320 ms entrance and 120 ms dismissal stop requesting frames when settled. Paths, selections, IME input, and ordinary prose keep normal editor navigation. Unsupported commands remain drafts instead of silently becoming ordinary paid prompts.

Usage is extracted from already decoded protocol events, bounded, and saved only on terminal reports. The frontend retains at most 128 snapshots, temporarily allowing the four active runs. Unknown provider fields remain unknown. There is no tokenizer, account polling, or estimated context budget. Existing native session IDs are reused when changing models for the next turn.

Cross-provider handoff is explicit: cancel only Code-owned work, await process-group cleanup and durable parsed output, then prepare context on the state worker. Reuse a valid recent history page when possible; otherwise decode one bounded 256 KiB/200-chunk page. A 48 KiB serialized packet carries an objective up to 4 KiB, recent readable text up to 25 KiB, prior notes up to 8 KiB, and up to 16 source references. The first saved objective uses incremental 4 KiB SQLite BLOB reads rather than decoding a giant message. The UI retains at most eight Arc-shared packets; payload construction/escaping occurs on the launch worker. Latest draft edits and completed image imports are collected at transfer time, avoiding stale snapshots. Source native sessions remain intact; the destination receives a new native ID. Hidden state and live tools are not transferred, and automatic quota fallback is not implemented.

Verification: **344 application tests + 66 benchmark-target checks pass** (410 active checks, two opt-ins ignored), including native keyboard shortcuts, virtualized 128-command catalogs, provider-scoped commands/usage, native command capability confirmation, same-session model changes, process/finalization barriers, real disposable destination-child execution, durable context/restart seams, latest-draft preservation, and preventing internal handoff envelopes from leaking into drafts. Strict Clippy, formatting, and whitespace checks pass. Ordinary tests do not invoke authenticated models; installed CLI help and official headless protocol documentation were checked separately. No new dependencies are added. No claim of zero overhead or measured startup/FPS/energy improvement is made.

Release state-engine spot check (October 13, 2026, arm64 macOS; one run, disposable fixtures): a 6,000-summary sidebar read took **5.19 ms**, an indexed 300-summary page **0.19 ms**, title/FTS searches **2.15/2.01 ms**, and 4 MiB Markdown parsing **27.51 ms** on the worker. Raw output is `.build/commands-benchmark.json`. This exercises existing storage/parsing paths, not command animation latency, source-to-destination inference, startup, memory, or energy. There is no matched pre-change baseline for this pass, so these numbers do not establish an improvement or zero regression.

The optimized release updates the existing `dist/Code.app`; its executable UUID matches `target/release/orbit` and strict code-signature verification passes. Packaging did not stop or launch the running app.


## Project-list entrance

The first populated project list uses one shared, finite entrance timeline. Visible rows fade in and move 8 px horizontally/5 px vertically with the existing spring curve, staggered by 28 ms from top to bottom. A fixed 32 px outer row preserves list measurement, scroll geometry, and neighboring columns. Only the virtual list's mounted rows build animated contents; there are no per-project tasks, source scans, image effects, or dependencies. Visible-row staggering is capped at 31 steps, so the entrance settles within 1.288 seconds. Completed timelines are released; refresh and later scrolling do not start new entrances. Motion-off renders immediately, and inactive/hidden sidebars request no entrance frames.

All **65 native UI checks pass**, including asynchronous project arrival, rendered positional staggering, fixed row/column geometry, offscreen virtualization, finite cleanup, no refresh replay, and motion-off behavior. The timing fixture uses a test-only clock override to avoid dependence on host scheduling; it is absent from production builds. Strict Clippy, formatting, and whitespace checks pass. No new animation-FPS or energy measurement is claimed.


## Launch window activation

Launch now explicitly activates the application and orders its actual window forward on the next native frame, after initial window creation/restoration. The existing initial app activation is retained. Dock reopen requests also activate the existing window through GPUI. These are one-shot lifecycle callbacks; agent events, discovery, and ordinary rendering never bring the app forward. Window levels remain normal. No polling, retry timer, dependency, or repeated foreground activation is added.

All 65 native UI regression checks and strict Clippy pass, along with formatting/whitespace checks. These tests do not reproduce cross-application WindowServer ordering. The running Code instance is intentionally left untouched; actual foreground placement on the updated app's next launch remains a manual check.


## Shared list and message entrance

Projects, conversation listings, opened transcripts, submitted prompts, and first incoming response content now share the same finite 420 ms fade/spring with 28 ms staggering. Each visible region owns at most one timeline; all three request one shared next-frame wake from the parent. Conversation listings restart for a different project/all-projects scope or reopening the column, rather than for ordinary metadata or active-chat updates. The old whole-transcript fade was removed.

Transcript staggering starts at the actual painted viewport, including the tail of a large history. A first-layout probe records only visible row bounds into one small shared cell; it emits no paint primitives and is removed after that first pass. Measurement overdraw does not add phantom delays. No transcript scan, message cloning pass, per-row task, or growing animation map is introduced. Relative child offsets preserve measured outer-row geometry, wrapping widths, and scroll anchors. New-message entrances affect only appended rows. Subsequent streaming deltas/final parsing keep the same timeline, and real scrolling settles it immediately. Prepending older history also clears the entrance while preserving the viewport. Motion-off skips it; inactive/hidden regions request no entrance frames; completed timelines release their shared state. No new dependency or worker is added.

All **68 native UI checks pass**, including project/conversation staggering, scope changes, same-scope updates, a 10,000-row chat at its visible tail, fixed geometry/scroll anchors, no replay on scrolling, first-response-only streaming animation, and motion-off rendering. Strict Clippy, formatting, and whitespace checks pass. Test clocks and selector instrumentation do not ship. This pass adds bounded transient animation work; no claim of zero hardware overhead or measured FPS/energy improvement is made.


## Contextual Select All and physical provider rotation

Command–A now uses the explicit `Orbit && !TextInput` predicate for Settings. The existing TextInput binding selects all in composer, search, and other text fields; Command–Shift–A remains an alias. Native keyboard checks exercise composer/search clipboard selection, Settings with workspace focus, and preservation of the draft.

The actual provider SVG rotates around its own center during discovery, conversation loading, and the corresponding entrance transition. Acceleration uses an analytic first-order velocity response, and completion uses an analytic underdamped angular spring, retaining angle and angular velocity through interruptions. Fixed 34 px home/18 px header bounds keep layout stable. The renderer applies an affine transform to the existing cached SVG; no ring, new artwork, external physics library, raster decoding loop, loading deadline, or minimum display time is added. Readiness and input are independent of the visual settling.

A small GPUI entity owns the rotor. It requests native frames only during visible, active rotation or settling, pauses on inactivity, and stops at an upright identity transform. Motion-off and system Reduce Motion render a static mark. A settled event updates the parent once; there is no extra worker or recurring timer. GPUI still performs its normal window-frame rendering while animation is active; this is not a claim that the entire chat subtree is cached or that animation has zero hardware cost. Markdown parsing, discovery, and history reads remain on their existing workers.

Verification: **351 application tests + 66 benchmark-target checks pass** (417 active checks, two opt-ins ignored), including all 69 native UI checks. The full run used four test threads to limit simultaneous heavy history fixtures. Two earlier entrance tests now freeze their test clocks before the first paint, removing dependence on host scheduling. Coverage includes actual SVG bounds/rotation/settling, frame-rate-independent acceleration, interrupted momentum, upright completion, motion-off, and contextual keyboard selection. Strict Clippy, formatting, and whitespace checks pass. No new startup, FPS, memory, or energy measurement is claimed.

## Large live protocol events

The 1 MiB per-event abort has been removed. It was a raw JSONL framing cap, independent of saved-history paging and the separate 16 MiB live-response budget. Ordinary events still use the in-memory serde path, now framed in buffered slices instead of per-byte pushes. There is no new idle task, worker thread, dependency, or normal-event disk I/O.

Above 1 MiB, the existing runtime worker spills that one event to a mode-0600 temporary file, unlinks its pathname immediately, and frees the large framing allocation. The handle is closed after dispatch or cancellation. A 64 KiB buffered scanner builds a bounded protocol projection rather than decoding arbitrary tool output, base64, or unknown metadata into a giant serde Value. Event byte length alone no longer fails a run. Disk usage and work for this exceptional path scale with the incoming event; this is not zero-cost processing.

The projection retains response text, native identities, status/error state, interactive cards, command capabilities, and reported usage for all seven adapters. It budgets 16 MiB of decoded text, 256 KiB of metadata strings, 8,192 nodes plus 256 reserved shallow nodes, at most 256 selected object fields/128 array entries, 256-byte encoded keys, and 128 nesting levels. Strings are consumed in buffered spans, with bounded encoded prefixes decoded safely across UTF-8 and escaped surrogate boundaries. Required shallow metadata can still arrive after a large collection exhausts the main node budget. Preview shortening emits a coalesced status notice; unused bulk fields are omitted from the display projection. Genuine JSON/protocol, disk, process, and provider errors remain errors. The existing total live-response limit and lazy native-history readers are unchanged.

Regressions exercise an actual disposable child emitting a 20 MiB command payload, an intact 1.8 MB Unicode reply, and a separate oversized final record without a newline; tool state, completion, and usage must still arrive. They also cover normal/spill threshold boundaries, unlinked 0600 buffer ownership and release, cancellation before an oversized event finishes, normal-vs-large dispatch equivalence across all seven adapters, dynamic Cursor tool/model usage keys, UTF-8/escape boundaries, bounded structures, malformed JSON, and cancellation while decoding. These tests do not invoke authenticated models. No new FPS, idle-memory, or energy measurement is claimed.

Verification: **359 application tests + 66 benchmark-target checks pass** (425 active checks; two opt-ins ignored), including all 69 native UI checks. Strict all-target Clippy, formatting, and whitespace checks pass. The fixture proves bounded framing/projection and protocol continuity; it is not an Instruments memory/energy benchmark.

The optimized build replaces the existing Code.app executable without stopping or launching the running instance. Strict signature verification passes, and the bundled executable UUID matches `target/release/orbit` (`E813E188-B9C6-3CAF-87E0-E71B10C7BF0C`). Compared with the previously packaged executable, on-disk size increased by **16,576 bytes (0.109%)** to 15,173,360 bytes. This is binary size, not a runtime memory or speed measurement.


## October 14 compacted history and reconnect repair

The installed Codex 0.162.1 emits nonterminal reconnect diagnostics as exec JSONL `type:error`; treating every such record as final failure caused Code to terminate the agent before its own retries completed. These records now remain bounded statuses. A final turn failure or premature process exit remains a failure with useful diagnostics; successful completion clears the pending transient diagnostic. Tests exercise reconnect → successful response, preserved partial output → final failure, and interrupted exit with both success and failure exit codes. No new network retry loop or automatic prompt resubmission was added.

Read-only inspection found **279 stale `.jsonl` locations** in this installation's Code metadata. Their compressed siblings existed. Native indexed lookup and bounded exact-ID archive recovery now run before loading or restoring history; runtime SessionId/Finish lookups retain the fast indexed path. Imported fork ownership remains durable. Archived chats decode sequentially to private disk on their first selection, using 64 KiB buffers and a maximum 128 MiB decoder window (actual window depends on the compressed source). The complete history is never collected in a Rust string/vector. The archive source and any large-message indexes participate in existing reference-counted paging artifacts and viewport hard links; restoring a compressed viewport reuses the decoded artifact without replay. Cancellation checks run during preparation so a superseded selection releases its partial artifact promptly. No work runs per frame or during idle use.

Verified all 279 stale paths through Rust recovery, loaded three real archived chats, and opened an additional real LBNL project archive read-only. Source stamps remained unchanged and no message bodies were printed. Fixtures also cover compressed history over 20 MiB, bounded decoded pages, checkpoint/viewport reuse, file cleanup, cancellation, corrupted input, stale parent metadata, and archived relocations. The full suite passes **369 application tests + 68 benchmark-target checks** (437 active checks, including shared module duplication); four explicit workload/live/installed-data tests are excluded from ordinary runs. Strict Clippy, formatting, and whitespace checks pass. No FPS, idle memory, or startup improvement is claimed by these functional repairs.

## Continuous chats and native delegation

Cross-provider handoffs now retain one visible chat and tab. A SQLite transaction records the original immutable native baseline, destination binding/model, bounded context packet, and native identity aliases before the destination process launches. Subsequent turns remain in Code's local lazy history. Rediscovery cannot split the conversation into duplicate sidebar entries. Provider metadata preserves the logo/name on earlier assistant turns. Frozen page upper bounds prevent new turns from leaking into previously cached older-page checkpoints. No transcript is copied into memory or duplicated merely to switch providers.

`/delegate <task>` and the small composer button request a child through the selected Codex, Claude, or OpenCode parent harness. The existing process and event channel own its lifecycle; Code does not start an unrelated replacement CLI. Child launch/tool completion is distinct from child task completion. Native child IDs, results, failures, interruptions, and background work become bounded expandable glass cards. Expanded reports scroll instead of clipping. Question selections are scoped to the individual card, and historical speaker labels retain provider identity. The native protocols and installed sources were checked against [Codex's integration documentation](https://learn.chatgpt.com/docs/app-server), [Claude's headless interface](https://code.claude.com/docs/en/headless), and [OpenCode's agent model](https://opencode.ai/docs/agents/).

Codex exec omits some native child activity items. A read-only incremental observer supplements the active parent's stream from its own indexed native rollout; it owns no writer and stops with the runtime. OpenCode's installed exec command omits running child updates, so its emitted task results are supported while full live child inspection remains a transport limitation. Native interactive approvals and direct child steering are not implemented by these exec adapters. Account, network, policy, and provider failures remain visible rather than being relabeled as success.

## Disk-backed live responses

The former 16 MiB accumulated live-response failure is removed. A Rust response buffer retains at most 256 KiB of text before spilling into a mode-0600 owned temporary file. Appends, range comparisons, Unicode-safe corrections, and interleaved final rendering use bounded chunks on the runtime worker. Published source prefixes share ownership; a correction that would modify an already-published prefix copies before truncating. Disk failures poison the buffer and propagate as real failures. The GUI keeps at most a 64 KiB live tail and parses only its small visible end. Quiet runs retain the existing event-driven wakeups.

The canonical source is published before terminal delivery, including cancellation with a full event queue. Large completed responses stream into a transactional SQLite BLOB in 64 KiB chunks and then open through existing 256 KiB lazy history pages, avoiding a full final Markdown parse or Rust string. These BLOB bodies skip FTS tokenization to prevent SQLite from materializing enormous responses; ordinary text messages keep their existing full-text indexing, and chat titles remain searchable. Individual oversized protocol fields still use the bounded projection described above. Database, disk, and provider capacities continue to apply.

Fixtures cover Unicode responses over 20 MiB, bounded previews, correction/snapshot immutability, private-file cleanup, poisoned I/O failures, atomic streamed storage, full lazy round trips, ordinary FTS integrity, same-chat handoff/restart across three providers and 883 rows, frozen-page rehydration, native child lifecycle parsing, and actual process-backed prompt flows. These are functional/resource-bound checks; they do not establish a startup, FPS, idle-memory, or energy improvement.

Verification: **407 application checks + 80 benchmark-target checks pass** (487 active checks, including shared-module duplication; five opt-in workloads/live/installed-data tests excluded). Strict all-target Clippy, formatting, and whitespace checks pass. A real authenticated Codex parent/child test observed a native child ID, working and completed states, the child result, and a completed parent response. A separate real OpenCode prompt completed through the Rust runtime. The final native observer also supports an explicit `CODEX_HOME` and filters inherited child activity from earlier turns. Text and card metadata both spill privately; recent deduplication keys are bounded. Full-frame animation latency and energy remain unmeasured.

## October 15 feature audit verification

The integrated Rust build passes **584 checks** (493 application checks and 91 parser/benchmark-target checks; 10 explicit opt-ins excluded), strict all-target Clippy, formatting, and whitespace checks. The rebuilt `dist/Code.app` passes strict signature verification. New rich-output parsing uses an ordered span sweep rather than repeated whole-span scans, with a 10,000-link regression. Project/archive lookup pages at most 128 explicit summary IDs and does not load transcripts. Provider metadata remains demand-driven on existing workers; no dependency or idle polling loop was added.

One release run on this macOS arm64 host, using the existing 10,000-session / 100-project / 300-message fixture, measured:

| Workload | Milliseconds |
|---|---:|
| Open database and initialize schema | 6.47 |
| Insert 10,000 summaries, one transaction per call | 1,153.64 |
| Upsert 10,000 summaries, one transaction per call | 323.72 |
| Discovery batch insert, 10,000 summaries | 125.98 |
| Unchanged discovery batch, 10,000 summaries | 130.91 |
| Actual sidebar API, 6,000 summaries | 28.31 |
| Indexed SQL page, 300 summaries | 0.94 |
| Title search | 6.54 |
| Transcript full-text search | 7.19 |
| Parse 4 MiB Markdown | 111.24 |
| Parse dense styled/link paragraph, 580,000 bytes | 22.86 |

Command: `cargo run --offline --release -j 2 --bin bench`. Raw local output: `.build/feature-audit-benchmark.json`. These are single-run state/parser timings under the host's current workload, not controlled before/after comparisons, startup, input latency, memory, scrolling, FPS, energy, or agent inference measurements. Expensive parsing and database work run outside the UI thread. Live visual inspection was unavailable because the desktop control service failed to start; native GPUI layout/paint/input tests passed.

## Shared dark dropdown surface

All dropdown shells now use the agent picker's existing `0x242424bb` translucent backing: agent/model selection, project/chat actions, slash commands, create/rename, search, workspace, wallpaper, appearance, status, permissions, handoff and trust/conflict controls. The shared color is independent of workspace opacity so low-opacity panels cannot erase menu contrast. Existing finite entrance/exit animations, borders and layout remain unchanged. This replaces color values only; it adds no blur pass, worker, allocation or recurring frame request.

## Recoverable native workflow pass

New queue, context, team, checkpoint and terminal controls remain Rust/GPUI. The terminal engine is the pinned Alacritty Rust crate with default features disabled; six lockfile packages were added, with most dependencies already shared through GPUI. No browser renderer or JavaScript service was added. One shell is created only when opened, with bounded scrollback and no hidden viewport projection. Stopped/closed capture, terminals and animations do not retain their active refresh loops.

Context excerpts share `Arc<str>` storage across UI snapshots, teams and queues. Up to four 24 KiB excerpts are selected explicitly; full source histories remain paged. New workflow metadata has a shared 2 MiB encoded JSON budget checked prospectively on explicit actions, without allocating an encoded JSON document or counting every frame. State serialization and writes remain on the worker. Failed additions retain the original draft and durable work. The saved workspace remains capped at 8 MiB, with a regression covering worst-case escaped text plus nine full composer drafts.

Checkpoints stream 64 KiB chunks to deduplicated immutable content with SQLite manifests, cached validated file signatures, paged native lists, and bounded disposable metadata. Read-only prompts make cheap records without scanning files. First capture of an uncached large checkout still performs real disk IO. Up to 512 records and 4 GiB of retained backup/recovery/metadata are supported; limits stop a new capture rather than silently omit files. New agent/terminal launches wait during restoration and finalization. See [checkpoint bounds](docs/CHECKPOINTS.md) and [terminal bounds](docs/NATIVE-TERMINAL.md).

An additional release run on the same host with the existing 10,000-session / 100-project / 300-message fixture measured:

| Workload | Milliseconds |
|---|---:|
| Open/schema | 5.99 |
| Insert 10,000 summaries, implicit transactions | 491.86 |
| Upsert 10,000 summaries, implicit transactions | 100.42 |
| Discovery batch insert / unchanged | 41.22 / 49.82 |
| Actual sidebar API, 6,000 summaries | 9.18 |
| Indexed SQL page, 300 summaries | 0.40 |
| Title / transcript full-text search | 2.87 / 3.97 |
| Parse 4 MiB Markdown | 52.00 |
| Parse dense 580,000-byte styled/link paragraph | 11.08 |

Raw output: `.build/workflows-benchmark.json`; command: `cargo run --offline --release -j2 --bin bench`. These single-run state/parser measurements are not controlled before/after results and do not measure startup, scrolling, input latency, footprint, energy, inference or checkpoint throughput. They do not establish that the app meets its original 80 MB or 120 FPS targets. The desktop control service could not start for live-window inspection; actual microphone acoustic/device QA remains manual.

Final verification: **650 ordinary checks pass** (559 app + 91 shared parser/state target checks; 10 opt-ins excluded), plus three separately invoked installed Codex sandbox checks without model inference. Strict all-target Clippy, formatting and whitespace checks pass. Native fixtures include real PTYs, concurrent different-provider task routing, pre-canceled shell startup, prompt snapshot recovery/conflicts/GC, transformed native-input hash verification and queue draft/settings/context preservation. These establish specific functional/bounded-work properties, not perfect parity or a measured interactive-performance guarantee.
