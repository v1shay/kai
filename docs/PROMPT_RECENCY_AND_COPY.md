# Prompt recency and clipboard controls

Kai Unified 0.12.8 (45) orders project and chat lists by user submission time. Opening a chat, receiving an assistant response, completing a task, and acknowledging activity do not promote a chat. Priority uses that same ordering across projects and toggles back to Projects on a second click, including the single-sidebar layout.

Submission timestamps persist in the existing workspace activity metadata. The initial migration uses retained turn submission timings where present, otherwise the existing imported session summary time; older source histories do not all provide a user-prompt timestamp in their summary. Subsequent submissions record their own millisecond timestamp. Project ordering uses the closest registered owner, so a nested project does not also promote its parent. Pins break equal-time ties rather than overriding recency.

Message controls sit at the end of an authored message, rather than after every Markdown block. Clipboard serialization includes its paragraphs, fenced code, headings, lists, tables, and attachment links, excluding tool/status payloads. Message ranges are indexed alongside the existing transcript layout; text is assembled on click. Existing code-block, review-card, file-context, file-draft, and saved-prompt copy buttons share the same feedback control.

Copy feedback switches to a green check with a short fade/slide and resets after 1.5 seconds. Repeated clicks extend confirmation; an older reset cannot cancel a newer click. State is scoped to a visible control and chat, timer tasks retain only weak references, and reduced-motion preferences disable the transition.

Validation: 34 focused native and unit tests passed, covering project/chat ordering, navigation without recency changes, persisted prompt timestamps, Priority toggling, multi-block message grouping, actual clipboard content and check feedback, and existing project-tree/file-editor/review-card behavior. No live provider prompts were sent and no existing chats were modified. Release binary and signed app built successfully; visual review is left to the user.

0.12.9 (46): message copy footers follow the sender edge: right for user prompts, left for assistant responses. The previous footer unconditionally justified both to the right. Clipboard contents and checkmark feedback are unchanged.
