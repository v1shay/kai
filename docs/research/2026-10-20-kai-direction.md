# Kai: from a beautiful notch to an indispensable assistant

Research and product proposal · October 20, 2026

> Scope correction · October 22, 2026: Kai is a better interface for the existing Codex engine, chats, voice, tools, and approvals. The user wants aesthetic and interaction improvements, with Codex voice as the model for useful progress. The broader assistant/platform strategy below is historical exploration, not the current implementation mandate. Prioritize voice/text continuity, clear chat selection, readable conversations, compact/expanded presentation, attractive state transitions, accessible controls, and review of Codex's real output. Do not introduce a separate agent runtime, general-purpose intent router, memory system, connector platform, or automation product as part of this interface work.

## The recommendation

**Build the shortest path from seeing a problem to finishing it.**

Kai should own a complete interaction: point at something → say or type what you want → continue your work → inspect a useful result. The product promise is: **“Point. Ask. Done.”** A second line can explain the substance: “Kai turns what's on your Mac into finished work.”

Start with builders who move between a repository, browser, issue tracker, and communication apps. This fits Kai's actual foundation. Expand to other people after the core loop is dependable and onboarding no longer requires developer knowledge. “The most capable AI for everyone” is a destination; it does not specify a launch product.

Kai currently uses Codex as its execution engine. It can compete with the Codex interface for daily use while benefiting from that engine. A stronger model or independent agent runtime is a different, substantially larger undertaking. Interface and workflow improvements should be measured as interface and workflow improvements, rather than claims of superior model intelligence.

This document contains recommendations and hypotheses, not evidence that Kai already outperforms the products below. Research used public first-party product pages and documentation; competitors were not installed or benchmarked. The accompanying concept is a local simulation, without live AI, recording, or app control.

## What the competitive research changes

| Product | Current first-party evidence | Implication for Kai |
| --- | --- | --- |
| Codex / ChatGPT Work | OpenAI documents an embeddable harness, threads, events, approvals, and steering. The broader desktop product has tools for computer/browser work; Remote supports substantial engineering workflows. | Chat, voice, tools, and task execution are foundations. Compare the exact product surface tested; Codex and ChatGPT Work are not interchangeable names. |
| VoiceOS | Its site emphasizes cursor context, voice-to-action, finding information across apps, and task follow-through. | “Screen-aware voice assistant” is already a competitive category. Beat the whole workflow, including target selection and reviewing the result. |
| HeyClicky | Its October 2 changelog describes multiple accounts, voice follow-ups, and computer-use recovery. Earlier entries describe routines, teaching overlays, and companions appearing from the notch. | Neither pets, a notch, agents, nor routines are unique advantages. Reliability and task continuity require real depth. |
| Wispr Flow | Its fetched context-awareness documentation describes app-aware formatting, visible names, cursor text, dictionaries, and context exclusions. It currently says context awareness uses text without screenshots. | High-quality dictation needs precise, small context. A screenshot on every utterance is unnecessary. |
| Superwhisper | Its S1 announcement describes local cleanup with S1-mini and dedicated speech/language models. | Local, responsive speech and cleanup are serious existing competition. Evaluate local speech rather than assuming cloud inference is always required. |
| Raycast | Its AI page advertises projects with memory, screen awareness, automations, custom agents, tools, and bringing subscriptions/keys. | A launcher with models and integrations is also crowded. Kai needs a distinctive, excellent interaction people repeat daily. |

Sources: [Codex harness](https://developers.openai.com/blog/codex-as-a-platform), [App Server](https://learn.chatgpt.com/docs/app-server), [computer and browser workflows](https://developers.openai.com/training/walkthroughs/using-your-computer-and-browser), [Remote](https://developers.openai.com/blog/mastering-codex-remote-for-engineering), [VoiceOS](https://www.voiceos.com/), [VoiceOS agent](https://www.voiceos.com/features/agent), [HeyClicky changelog](https://www.heyclicky.com/changelog), [Flow context awareness](https://docs.wisprflow.ai/articles/4678293671-feature-context-awareness), [Superwhisper S1](https://superwhisper.com/blog/s1), [Raycast AI](https://www.raycast.com/core-features/ai).

These are vendor descriptions, not independent performance evidence. Search excerpts for Flow differed from the fetched page; the description above follows the fetched page. Avoid repeating unverified “10×” productivity or millisecond comparisons from marketing.

## Kai's starting position, grounded in this repository

Existing strengths:

- Native compact notch, pet animation, task scenes, and compact voice presentation.
- Codex App Server integration, streaming history, approvals, and active-turn steering in `kai.py`.
- Native realtime voice over WebRTC, hold-to-speak, editable dictation, and optional local Piper output.
- Appshot/context capture, projects/chats, file previews, and activity integrations.
- Recent targeted performance work: retained transcript rendering, sidebar virtualization, background attachment encoding, and release packaging.

Concrete gaps worth investigating first:

- `README.md` requires Git, Swift, Python, CLI login, and a build script. That is suitable for early technical adopters, but a major obstacle to broad accessibility.
- `ApplicationContext.swift` reads focused-element values and can capture all on-screen windows through `CGWindowListCreateImage(.null, .optionOnScreenOnly, ...)`. The inspected function has no explicit secure-field filtering. Build narrower capture and a visible scope model; this observation is not a claim that all secure controls expose their contents.
- `MiniAppInterface.swift` contains nominal 5.7–9 point text and many custom layer controls; `ConversationViewport.swift` has a 9-point history control. Final apparent size depends on scaling. Review at normal and enlarged sizes on real displays.
- A source search found an explicit accessibility label on timing sliders, but did not establish a semantic accessibility tree for the main custom layer interface. This is an audit lead, not a completed VoiceOver assessment.
- The October 17 performance audit explicitly does not establish end-to-end latency or energy guarantees. Its microbenchmark must not be presented as whole-app responsiveness.
- Text steering already exists. The improvement is making “change this task” versus “start another task” easy to understand, including image and active-turn restrictions, rather than adding steering from scratch.

Reviewed: `README.md`, `kai.py`, `kai_voice.py`, `Sources/NotchPrototype/{ApplicationContext,MiniAppInterface,NotchSizing,NotchInputRegion,CompactVoicePresentation,ConversationViewport,ShortcutPreparation,App}.swift`, `docs/performance-audit.txt`, and `docs/reliability-audit.txt`. Older feasibility notes are historical context; current source and freshly fetched documentation take precedence.

## The ten best product bets, in build order

### 1. A precise “this” that users can see and correct

Point at a failing test, selected paragraph, design element, or email. Invoke Kai. Highlight the intended region and show a removable chip: **“This issue · Browser”**, **“Selected text”**, or **“Window screenshot”**. Users can switch to a region or attach another source.

Capture order: selected text → semantic element under the pointer/focus → chosen window/region image when necessary. Read the smallest adequate source. Freeze the reference at invocation so “this” does not drift as the person changes apps. Show capture time and refresh when the source is stale.

Example: point at a bug report and say “Fix this in Kai and give me a patch.” Kai binds the report and repository, produces a patch and verification summary, and preserves the issue as a source.

Why it can win: fewer explanatory prompts and fewer wrong-target actions. Cursor context itself is already offered elsewhere; the hypothesis is that visible scope plus continuity produces better outcomes.

Acceptance: correct target on at least 95% of a curated supported-app set; all included sources inspectable/removable before sending; unsupported targets fall back to explicit region selection. The 95% figure is a proposed gate, not a result.

### 2. Fast dictation and serious delegation, without a mode puzzle

Offer three explicit intents: **Write**, **Ask**, **Do**. Remember the person's preference; allow a shortcut to select each. Write inserts text at the original cursor; Ask explains; Do creates a durable task. Show the inferred intent before a consequential action and make it changeable.

Use direct, bounded operations for writing and known simple actions. Use the agent for work needing search, tools, or multiple steps. Avoid paying the latency and context cost of a full agent turn for every grammar fix. If routing is uncertain, preserve the transcript and ask one useful question. Never infer permission to send an email from ordinary dictation.

Write should preserve identifiers, literal strings, paths, negations, names, and spoken corrections. Let people approve additions to an editable personal dictionary. Corrections in another app should only be learned within a visible, opt-in correction flow.

Prerequisite: Kai currently dictates into its composer/chat paths. Insertion into arbitrary apps is new work: focus restoration, supported Accessibility writing APIs, paste fallback with clipboard preservation, and honest unsupported-field handling.

### 3. Tasks that keep their identity while you keep working

A small task shelf has **Working**, **Needs you**, and **Ready**. Each entry has one stable identity, sources, workspace/account, and last meaningful event. “Use the other screenshot” steers the selected task. “Also research pricing” starts another task only when that intent is clear.

Offer explicit **Update this task** and **New task** controls. Restore state after reconnect, avoid duplicate submissions, and distinguish uncertain state from completion. Keep typed/voice follow-ups attached to the selected task even when the foreground app changes.

The product should not open a large panel for every token or steal keyboard focus when a task finishes. A restrained status update invites inspection. Existing active-task views and steering are useful foundations; durable task storage, reconciliation, and recovery still need design.

### 4. Finished work with a receipt

The first screen after execution should answer: **What changed? Where is it? What was checked? What needs me?**

Examples: a patch with changed-file and test links; an editable email draft showing its account and recipients; a researched shortlist with sources; a spreadsheet with the output file and checks. Show “not checked” when verification did not happen. Never fabricate test counts or source coverage.

Support compare/preview and a contextual next action. Offer undo only when there is a real reversible operation or tested compensating action. A Git diff is not a universal rollback; an external email cannot reliably be unsent. Bind approvals to the exact action/account/recipient and invalidate them if the action changes.

Why it can win: reduced inspection time and better justified trust. A receipt is a product contract, not just another generated summary.

### 5. Accessible through every input and display

Every core journey works by keyboard and typed text. Voice is an accelerator. Add toggle-to-talk alongside hold-to-talk, remappable shortcuts, clear stop/cancel controls, and captions. Do not require press-and-hold, pointer precision, hearing, or speech.

Represent custom controls with real AppKit controls or `NSAccessibilityElement` objects carrying roles, labels, values, actions, focus, and screen frames. Announce milestone changes rather than every streamed token. Restore focus after dismissal; keep Escape behavior predictable.

Use a readable detached workspace for long results, diff review, file browsing, and larger text. Provide menu-bar, external-display, and non-notched-display entry points. Support Reduce Motion, Reduce Transparency, higher contrast, and at least 200% text/layout enlargement without clipped actions. Test with VoiceOver and people with relevant access needs.

Treat the notch as a quick entry and status surface, with a full workspace available when the task needs it. This improves both accessibility and ordinary daily usability.

### 6. A first useful result before any tutorial

Ship a signed and notarized application with a supported authentication path and compatible runtime packaging. Review redistribution/update requirements before bundling any runtime. Do not assume a subscription can fund arbitrary providers or unsupported APIs.

Ask for microphone permission when speech starts, screen access when a visual source is chosen, and app-control permissions when an action requires them. Explain the value in one sentence and offer a working alternative after denial.

First-run journey: open Kai → choose voice or type → “summarize this selected text” → see the useful result. A sample task can demonstrate the experience before login, but must be labeled as a demo. Target a real first success within two minutes after download on a supported machine, excluding unavoidable third-party login time; measure login friction separately.

### 7. Privacy controls that are specific enough to trust

Show separate controls for **capture**, **send**, **retain**, and **remember**. A local app is not automatically an offline app. A local memory store does not imply inference stays on-device. Explain the actual provider destination for each request.

Exclude secure fields and user-blocked apps before capture. Treat custom fields as uncertain when their sensitivity cannot be established. Crop before sending, use short retention for temporary captures, and record source IDs rather than raw screenshots in diagnostics. Retention must account for backend conversation copies, not merely delete the local PNG.

Offer local dictation and local small transformations as an evaluated option. If unavailable or too slow on the device, say so and let users choose cloud processing. Do not silently fall back from private local mode to a remote service.

### 8. Teach me → do it → make it repeatable

Guide a person through the next step with a labeled, dismissible highlight. “Do this step” hands over the same task context; “I'll do it” lets Kai observe the result after explicit capture. Keep the sequence synchronized as the app changes.

After a successful repeated task, offer **Save as a routine**. Store an inspectable recipe: inputs, account, required tools, steps, verification, scope, and stop conditions. Validate it against the current UI/tool version. An observed click sequence alone is too fragile to become a dependable automation.

Teaching overlays and routines already exist in competitors. Kai's opportunity is a smoother transition between guidance, execution, and a verified reusable workflow.

### 9. Memory with receipts and boundaries

Remember useful, approved preferences such as “Use this work repository” or “Keep replies concise.” Separate personal, project, task, and account memory. Each memory has an origin, edit/delete control, and optional expiry. Show when a memory influences a decision.

Prefer a compact, retrieved task context over replaying every earlier conversation. Resolve conflicting memories explicitly. Avoid background desktop recording as the initial memory strategy: it adds sensitive data, cost, and complexity before proving value.

### 10. Quiet assistance with a completion standard

An opt-in follow-up can notice a failed check, a pending draft, or a stalled task and offer the next useful action. Every suggestion should be dismissible, explain its source, and respect Focus, meetings, screen sharing, and notification preferences.

Start with one routine: **“Prepare my next work session.”** Summarize unfinished selected-project work, identify the next action, and link back to files and tasks. Handle sleep/offline catch-up once and avoid duplicate external effects. Expand after people keep it enabled and use its outputs.

## Make it affordable to use all day

Accessibility also includes cost. Preserve the convenience of Kai's existing supported Codex login path; show when an operation needs a different paid service rather than implying one subscription covers everything. Keep local dictation and basic transformations usable without agent credits if device performance, model licensing, and distribution economics permit.

Use a small/local model only after it meets quality gates for that particular task. Put a configurable time and spend ceiling on delegated tasks; expose metered usage and ask before expanding an exhausted budget. Cache reusable context safely, fetch sources once, and avoid launching several agents for a task a single direct action can finish. Evaluate cost per verified completed task, including retries, rather than cost per token alone.

For a hosted offering, validate unit economics with actual speech minutes, task lengths, tool charges, and user cohorts before promising unlimited access. Offer one understandable default experience; make bring-your-own-provider options available to advanced users without making setup harder for everyone.

## Three ambitious bets after the core works

**“This, and this.”** Pin an issue, a screenshot, and a file into a small task capsule as you move through apps. Kai keeps their identities and provenance, so one natural request can act across them without copying everything into a chat. Prototype the source-binding interaction before building a broad desktop index.

**“Show me the change.”** For coding and design work, return an interactive before/after preview tied to an isolated patch and real checks. Let the person point to the preview and steer the same task. This requires a trustworthy preview environment; a generated screenshot must never masquerade as the implemented app.

**“Make this my way.”** Turn a successfully reviewed workflow into an editable recipe that remembers the person's chosen accounts, output style, and verification rules. Run it in preview mode before scheduling it. The advantage would come from repeatable completion quality, rather than an ever-larger gallery of agents.

## Make Kai more attractive by giving it more restraint

Recommended direction: **Obsidian + Sea Glass**. Keep the black hardware-connected silhouette, use clear neutral typography, and reserve a pale sea-glass accent for the main action or active voice state. Let the pet carry personality.

Three surfaces:

1. **Presence**: hardware notch plus minimal wings. Small pet, literal state label, and task count. Respect the camera cutout: do not place content in the central unavailable region.
2. **Action**: a roughly 480–560 point wide panel below the notch, sized to content. Intent, source chips, input, one primary action. No persistent project/file sidebars here.
3. **Workspace**: detachable, resizable 900–1200 point starting width for review, history, files, and multiple tasks; constrained to available screen size. These dimensions are initial design proposals, not rigid minimums.

Suggested tokens:

| Token | Proposal |
| --- | --- |
| Main surface | `#101416` |
| Raised surface | `#1B2225` |
| Main text | `#F2F6F5` |
| Supporting text | `#A9B9B6` |
| Accent | `#B7F5DA` |
| Body / secondary text | 15 / 13 points at default size |
| Comfortable controls | 32–40 point targets where space allows; enlarge mode available |
| Corner radii | 12 point controls, 22–26 point panels; preserve hardware notch geometry |
| Motion | Keep recent 220–280 ms shell transitions; immediate state feedback; no idle shimmer |

Check actual contrast in every state and both appearances; token choices alone do not establish accessibility. White/neutral response text improves scanning; multi-color gradients can remain in the optional pet theme or activity ornament. Avoid gradient body copy and multiple glowing borders.

Use concise action language: “Review patch,” “Open draft,” “Continue task,” “Choose source.” Show provider/model details when requested, while the default surface emphasizes the work. Replace mystery symbols with labeled, discoverable controls.

Two optional visual directions can be explored in the concept: **Paper** for a lighter studio feel and **Ink** for a quieter monochrome tool. The recommended default is Obsidian. None requires bitmap generation; this is an interface and motion design problem.

## Speed: set a budget, then instrument the actual loop

All numbers below are proposed release targets, not measurements or promises. Establish a hardware/network baseline before accepting them.

| Journey | Initial target | Measurement boundary |
| --- | --- | --- |
| Activation acknowledgement | p95 ≤100 ms | After the configured shortcut threshold is crossed → visible state feedback |
| Speech readiness | p95 ≤250 ms, warm and permission granted | Activation → audio input actually accepting samples; show permission/cold-start states separately |
| Local target/context preview | p95 ≤200 ms | Invocation → minimal source chip/highlight, excluding user selection time |
| Short dictation insertion | p50 ≤700 ms, p95 ≤1.5 s | Speech ends → final text inserted; eight-second utterance on declared hardware/network |
| Simple spoken answer | p50 ≤1.2 s, p95 ≤2.5 s | Speech ends → first intelligible answer audio, excluding tool-dependent answers |
| Long agent task acknowledgement | p95 ≤300 ms | Submit → durable local task visible; separate backend acceptance from local acknowledgement |
| Main-thread frames | 60 Hz target on supported displays | Instrument actual interactive workloads; ≤16.7 ms is a frame budget, not proof of smoothness |
| Idle behavior | No active microphone; no animation timer for hidden UI | Trace idle capture, wakeups, CPU and energy; account for explicitly enabled integrations |

Also record cold launch, first permission grant, Bluetooth route change, interruption-to-silence, p95/p99 tails, peak memory, and a 60-minute mixed-work session. Preserve metrics by provider, device, utterance length, locale, network, and warm/cold state. Collect timings and error categories without raw conversation contents.

Keep the bridge warm if measurements justify its idle cost. Fetch minimal context while recording; do not block activation on a screenshot or history read. Cancel stale capture and synthesis. Cache versioned capability metadata and static assets. Bound context and result rendering. Evaluate smaller/local models on specific fast tasks; choose from live provider capabilities, not a hard-coded claim about a “best” model.

## Implementation sequence and release gates

Effort labels are rough implementation size, not calendar estimates: S = bounded change; M = multiple components; L = new subsystem with integration and live-device work. Sequence by exit criteria. Scope dates after a short technical spike.

| Phase | Work | Size | Exit gate |
| --- | --- | --- | --- |
| Foundation | End-to-end timing events; semantic accessibility audit; secure-field/source capture spike; supported app/backend capability inventory | M | Baseline and reproducible failures exist; no unlabelled capture or completion states |
| Daily loop | Visible source chips and bounded capture; readable action panel; Write/Ask/Do; clear task steering; first result receipt | L | Core supported-app scenarios work by voice and keyboard, including denied permissions |
| Dependability | Persistent task shelf, reconnection, idempotent action handling, failure recovery, meaningful verification | L | Sleep/network/agent failures produce recoverable states; no duplicate external effects in fault tests |
| Accessibility and distribution | Detached workspace; full semantic controls; large text; toggle speech; external displays; signed installer and supported auth/runtime | L | VoiceOver/keyboard users complete core journeys; nontechnical users install without Terminal |
| Expansion | Two or three excellent app workflows, guide/do handoff, editable memory, inspectable routines | L | Repeat usage and measured workflow success justify adding more integrations |

Accessibility review begins in Foundation and applies to every phase. The distribution milestone does not postpone basic readable text, focus, or keyboard operation.

Technical ownership:

- `ApplicationContext.swift` → a task-scoped capture service with semantic/region sources, secure-field filtering, deadlines, and explicit lifecycle.
- `ShortcutPreparation.swift` + hold/voice controllers → instrumentation, readiness signals, cancellation, and toggle alternatives. Reuse existing generation checks.
- `MiniAppInterface.swift` + `ConversationViewport.swift` → readable action/review layouts and semantic elements. Keep decorative animation on layers; do not make visual layers the only representation of an action.
- `kai.py` / bridge → maintain existing App Server runtime; add a normalized capability registry, stable task identifiers, state reconciliation, and truthful receipts from authoritative events.
- New local task store → durable task envelope containing ID, workspace/account, intent, source references, backend IDs, status, pending decision, outputs, and verification evidence. Use migrations and explicit retention policy.
- Actions/connectors → a few typed API/MCP adapters first, semantic UI control second, visual control when necessary. Validate postconditions. Never equate a successful click with a successful task.

OpenAI documents `turn/steer` and `expectedTurnId`, event-driven execution, and experimental capability gating. Use the shipping installed protocol/schema, feature-detect methods, and test compatibility. Broad desktop feature availability is not evidence that every feature is available through App Server. Native voice and other experimental integration paths need versioned acceptance checks. [App Server protocol](https://learn.chatgpt.com/docs/app-server)

## Prove that Kai wins

Create a 30-task benchmark before expanding the feature count. Use the same accounts, data, permissions, network, and comparable model settings where possible. Version the tasks and products. Separate product setup costs from repeated daily use. Randomize product order to reduce learning effects.

Task groups: ten everyday writing/retrieval tasks; ten builder tasks crossing browser/repository/files; ten recovery/accessibility scenarios. Examples: rewrite selected text without changing identifiers; fix a bug from a visible issue; return a sourced shortlist; make a calendar draft in the intended account; recover a running task after sleep; finish through keyboard only; reject a permission and use a fallback.

Run with 12–20 participants spanning builders, nontechnical users, and people with relevant access needs; use a balanced subset per participant to avoid fatigue. This is formative research, not a universal statistical proof. Test only comparable supported tasks in head-to-head speed comparisons; report unsupported tasks separately as coverage, rather than silently dropping them or calling them ordinary failures.

Primary metrics: verified task completion; active human time; wall time to acceptable output; corrections; wrong targets/accounts; duplicated effects; abandonment; p50/p95 latency. Dictation needs both word/identifier error rates and meaning preservation. An attractive screenshot is not a completion metric.

Initial ship gates: ≥90% verified success on the declared supported core set; no wrong-recipient or duplicate-effect incidents in the release fault suite; ≥25% lower median active human time versus the strongest comparable baseline without lower completion quality. These are proposed targets, not established competitor gaps. Report uncertainty and per-task results, including regressions.

Pilot with 10–20 frequent users. Track repeat completed tasks, voluntary preference, retention, and how often people return to another tool to finish. The most useful signal is users choosing Kai for their next real task.

## The first demo worth building

Point at a bug in a browser and say: “Fix this in Kai. Keep the existing shortcut behavior.” Kai shows the issue and repository as sources, starts a task, and lets the person return to their work. A small indicator says “Checking the fix.” The result is a patch preview with actual verification evidence and an easy typed or spoken follow-up. An explicit review action shows what would be applied; it does not silently merge or publish.

Follow it with a noncoding task: point at an email and say “Draft a reply using the latest project update.” Kai shows both sources, the intended account and recipient, and an editable draft. Sending remains a separate intentional action unless a clearly scoped prior authorization covers it.

If those two journeys are fast, readable, recoverable, and pleasant, Kai has a credible reason to exist beyond aesthetics. Build those end to end before adding a marketplace, dozens of weak integrations, continuous screen memory, or another model picker.
