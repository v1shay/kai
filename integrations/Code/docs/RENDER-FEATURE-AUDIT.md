# Native rendering feature audit

Audited 2026-10-15 against the checked-out Rust source and primary product/source references. This is a feature inventory with specific repairs, not a promise of full parity or zero defects. Rendering provider data does not grant tools permission or expose capabilities the provider never offered.

## Sources and contracts

- [Codex app features](https://learn.chatgpt.com/docs/features) and the [Codex App Server](https://learn.chatgpt.com/docs/app-server) separate local app interactions from account-dependent cloud services. Code's existing native protocols and the installed generated schema remain the authority for actual RPC fields.
- [Codex plan-mode source](https://github.com/openai/codex/blob/main/codex-rs/collaboration-mode-templates/templates/plan.md) distinguishes a proposed-plan response from the progress checklist tool. A rendered plan must not silently switch permission mode or start implementation.
- [ChatGPT’s feature index](https://learn.chatgpt.com/docs/features) includes document artifacts and visualizations; the host desktop contract specifies writing-block syntax. Displaying a draft in Code is a narrower capability than a complete editable artifact workspace.
- [T3 Code's Markdown renderer](https://github.com/pingdotgg/t3code/blob/main/apps/web/src/components/ChatMarkdown.tsx), [file-link integration](https://github.com/pingdotgg/t3code/blob/main/apps/web/src/markdown-links.ts), and [inline-path classification](https://github.com/pingdotgg/t3code/blob/main/packages/shared/src/markdownLinks.ts) provide source evidence of directive handling, media/artifact views, explicit file locations, conservative inline-code file links, and preview-specific controls. Code's implementation is independently written Rust and keeps its own native view system.
- [OpenCode tools](https://opencode.ai/docs/tools/) describes tasks, task lists, questions, and tool permissions. These are distinct UI concepts rather than undifferentiated tool JSON.
- [Claude Agent SDK user input](https://platform.claude.com/docs/en/agent-sdk/user-input) and [Gemini ACP](https://geminicli.com/docs/cli/acp-mode/) distinguish protocol requests and progress updates from assistant text. Existing request-response ownership and negotiated capabilities remain in the transport modules.
- The exact desktop `::code-comment{...}` and `:::writing{...}` attribute syntax is additionally specified by this task's host desktop rendering contract. It is not claimed to be a universal portable feature of all Codex/Claude/Gemini versions.

## Concrete gaps repaired in this pass

| Gap found in source | Native implementation | Limits and evidence |
|---|---|---|
| Desktop review findings appeared as directive source | Complete `::code-comment` directives become glass review cards plus a clickable file/line-range reference | Actual file opening remains through the existing validated citation handler. Unknown/malformed/oversized directives and code examples remain literal |
| Proposed plans were unclassified XML-like text | Complete standalone `<proposed_plan>` envelopes become plan cards | Presentation only; no automatic tool execution or approval bypass |
| Writing-block delimiters appeared in chat | Complete `:::writing` envelopes become named draft/document cards; email subjects are retained | Read/copy/expand is implemented; a full editable writing workspace, revision history and email sending are separate, unimplemented features |
| Expanded native status details displayed raw Markdown and inert file links | Bounded visible prose details render bold/inline code/lists/links; explicit user clicks route through the normal file/default-browser handler | Terminal stdout/stderr and failure diagnostics remain literal. Text over 16 KiB uses the full existing literal scroller, avoiding unbounded markup parsing in a visible card |
| Glass status bodies lacked a copy action | An expanded card has a small native Copy action that copies the original body | Constructed only for expanded cards; copying does not create a prompt or execute anything |
| Inline-code file references were inert | Conservative path-shaped code spans retain code styling and gain a file click target, including line/column or `#L` locations | No filesystem reads during parsing. Commands, git refs, slash commands, model versions, URLs, hostnames, and code identifiers remain literal. Whitespace-containing paths require an explicit Markdown link |
| Historical provider task lists fell back to generic tool content | Codex `update_plan`/`plan_update`, Claude `TodoWrite`, and OpenCode `todowrite`/`todo_write` become status-backed checklists | Complete only when every authoritative task state says completed. Working, pending, cancelled and unknown states are distinct. Summaries have explicit remainder counts and an 8 KiB budget |
| Dense formatted output repeatedly rescanned every inline span | An ordered sweep replaces quadratic per-segment highlight scans | Maintains source precedence for overlapping styles. Test includes 10,000 independent links and overlapping/zero-width/clamped spans |

No browser, DOM, JavaScript rendering runtime, HTML execution, network fetch, or new dependency was added. Directive parsing runs in the existing transcript parsing pipeline; expanded status formatting is restricted to visible small cards. Complete envelopes are recognized only as standalone source lines outside fenced/inline examples. Attribute parsing is capped at 16 KiB and 16 fields; draft/plan envelopes at 64 KiB. Unknown extensions are preserved instead of guessed.

## Existing coverage confirmed in source

- Native request cards and a pinned pending-request tray preserve the owning run/request ID for actual approval, decline, questions and forms. Imported requests do not gain live responder authority.
- Voice speaker cards, realtime delegation wrappers, historical question replies, native tool statuses, completion/failure/cancellation and child-agent progress have typed display models.
- Markdown already supports Unicode-safe spans, emphasis, strike-through, bounded syntax-highlighted code, ordered/unordered/task lists, headings, quotes, tables and image attachment references.
- Standard explicit file links support absolute/relative paths, percent escapes, `file://` local paths, line ranges and compiler-style line/column suffixes. Unsafe schemes and malformed locations are rejected at the central citation boundary.
- Native file-tree previews already distinguish source/text, images, PDF, audio/video and archives; actual transport input capability remains provider-specific. File preview coverage is not the same thing as arbitrary media understanding by every model.
- Imported history and live tails use bounded rows, worker parsing, context-envelope hiding and literal preservation for source examples; private persisted interaction envelopes remain versioned.

## Remaining rendering and product scope

These require explicit implementation and separate verification; the audit does not silently label them supported.

| Feature family | Current boundary / work still needed |
|---|---|
| Editable draft/artifact workspace | Draft cards display/copy; direct rich-text editing, selection-based revisions, durable undo, exported document creation and synced artifact versions are not implemented by this renderer |
| Interactive diagrams and code previews | Mermaid, math, Vega, HTML and React fences remain readable source. Safe execution/preview needs its own isolation, resource policy, cancellation and lifecycle implementation |
| Rich table/quote internals | Current table cells and quote blocks flatten some inline style/link metadata. Column sizing, per-cell links, horizontal scrolling and CSV/Markdown export need dedicated tests/UI |
| Footnotes and math | Full footnote backlink navigation, math typesetting and citation annotation galleries are not implemented |
| Provider-only source IDs | A search/file citation token without a supplied source mapping cannot be resolved safely from the token alone. The renderer must not invent a URL or read arbitrary files to guess one |
| Artifact/thread/follow-up directives | Review/writing/plan directives above are implemented. Thread-management directives, artifact-template launchers and follow-up buttons require real application routing and permission-scoped handlers; unknown directives currently remain literal |
| Live draft/plan previews | Incomplete envelopes stay readable source until complete; streamed editable artifact state and cross-page directive continuation indexing remain separate work |
| Review workflow | Findings render and open file ranges; accept/reject/apply-fix state, editor diagnostics, review navigation and persistent finding resolution are distinct features |
| Accessibility and media parity | Full VoiceOver semantics, keyboard focus/accessibility audits, all animated media playback and remote/private credentialed asset handling are not implied by SVG icons or file type detection |
| Native provider lifecycle | Persistent background-agent progress, terminal/PTy sessions, remote workers, provider settings/model budget controls and feature entitlement belong to the transport/workspace audit, not a Markdown presentation claim |

## Verification

Added focused tests cover directive escaping and complete-envelope recognition, code/quote/JSON literal preservation, malformed/duplicate attributes, Unicode file ranges, writing and plan boundaries, expanded detail styling, inline-code file classification, authoritative multi-provider task states, bounded large task previews, and the 10,000-link sweep. The first parser/benchmark run passed 87 tests (including seven new tests); the final integrated suite also includes the later inline-file regression and native GPUI review-card expand/copy/no-agent-launch test. Check the final root test report for the combined result.
