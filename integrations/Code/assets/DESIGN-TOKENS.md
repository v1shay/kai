# MonoCode design reference for Orbit

Source: MonoCode commit `889579b61c2342cc27751092ff0effbe9ec8e162`, inspected locally in `.build/monocode-reference`. This note distinguishes source values, native glass surfaces, and flattened fallback colors.

## Structure and geometry

The full workspace places a project rail at the left, a session/inspector sidebar beside it, and a flexible chat pane filling the remaining width. Project rail defaults to 200 px (min 180, max 360), session sidebar defaults to 260 px (min 260, max 560). These are independently collapsible. Headers are 40 px high. macOS project header leaves 78 px for traffic controls. Title tab pills are 30 px high with 8 px horizontal padding, 6 px gaps and 6 px corner radius.

Project rail order: header/window controls; Search, Inbox and optional Notes/Automations; project/mono sections; project cards; footer/settings/usage. Navigation rows are 32 px high, 8 px horizontal padding, 8 px icon-label gap, 6 px corner radius. Section labels are 12 px with subdued ink, not oversized headings.

Session list: project/worktree header; sessions/source-control/files/skills tab strip; new conversation action; lightweight session cards. Cards use 10 px horizontal padding and 6 or 8 px vertical padding. Titles are 13 px semibold, metadata 11 px, with 6 px icon gaps. Status symbols are 12 px; provider symbols usually 14 px. Supporting count/shortcut labels are 10 or 11 px. Use one-line truncation in chrome.

Chat: 40 px title bar, flexible scroll region, compact composer dock. Agent body uses 14 px text, 24 px line height; tool summaries use 12.5–14 px text and 20 px line height, code previews 12 px mono/20 px line height. Composer uses 14 px text/22 px line height, 12 px internal text padding, 160 px maximum text-field height, 8 px radius, 1 px border. Toolbar uses 8 px side/bottom padding, 4 px gaps, 26 px buttons, 14 px icons. Input focus strengthens the border; primary submit uses a square rounded button rather than a large decorative pill.

## Color tokens

| Token | Dark source | Light source | Opaque dark / light approximation |
|---|---|---|---|
| Base | HSL(240 0% 9%) | HSL(240 0% 97%) | `#171717` / `#f7f7f7` |
| Ink | HSL(240 0% 92%) | HSL(240 0% 18%) | `#ebebeb` / `#2e2e2e` |
| Opaque rail | 90% base + 10% black | base | `#151515` / `#f7f7f7` |
| Separator | ink at 7% | ink at 7% | `#262626` / `#e9e9e9` |
| Subtle selection | ink at 8% | ink at 5% | `#282828` / `#ededed` |
| Selection | ink at 10% | ink at 6% | `#2c2c2c` / `#ebebeb` |
| Strong selection | ink at 12% | ink at 7% | `#303030` / `#e9e9e9` |
| Hover | ink at 15% | ink at 10% | `#373737` / `#e3e3e3` |
| Emphasis | ink at 20% | ink at 14% | `#414141` / `#dbdbdb` |
| Composer | ink at 3% | ink at 3% | `#1d1d1d` / `#f1f1f1` |
| Accent | HSL(211 92% 62%) | same | `#459bf7` |
| Skill | `#e8c547` | `#a07c10` | source values |
| Mention | `#38bdf8` | `#0284c7` | source values |
| Link | `#7dd3fc` | HSL(211 92% 40%) | `#7dd3fc` / `#0863c4` |
| Markdown heading | `#f9a8c9` | `#be185d` | source values |

Flattened values assume the base backdrop and rounded 8-bit sRGB channels. Alpha text levels are 80% for secondary title text, 45–55% for metadata, 40% for shortcuts. These solid values describe the opaque fallback. The requested native glass treatment is documented below; retain 1 px structural separators in either mode.

Primary submit: dark white surface/black arrow; light ink surface/base arrow. Running status: accent blue spinner, amber alert, emerald check. Keep semantic colors sparse. Status also uses shape/text, not color alone.

## Native glass surfaces (requested treatment)

The source's `--sidebar-opacity` is 0.85, `--window-background-opacity` is 0.40, and sidebar/native background blur defaults to 24 px. `--chat-background-empty-opacity` and `--chat-background-session-opacity` are both 0.24, but those values control optional **artwork image** opacity, not an additional dark-tint layer.

When native glass is enabled in dark mode, document/body/root backgrounds become transparent. The sidebar base tint is HSL(240 0% 9%) at 85% alpha and the central workspace is the same base tint at 40% alpha. The Mac host can paint the shared native tint; once that host acknowledges it, the source's CSS sidebar/workspace paints become transparent to prevent opacity from compounding.

For GPUI, use the blurred native window/material as backing and exactly one visible tint per pane: `#171717` at 85% for the project/session rails, `#171717` at 40% for the workspace. Do not paint a whole-window 40% tint and then another 85% tint over the same rail pixels. Ordinary borders and selection are overlays of ink at 7% and 10% alpha. The composer fill is ink at 3% with a 10% ink border; its source applies a small blur. Native material can supply the backdrop softness without per-message blur. Light mode uses opaque surfaces in the source's defaults.

The upstream macOS main-window implementation uses transparent NSWindow plus WindowServer blur and a shared NSView tint. Its borderless/floating chats instead use NSVisualEffectView with UnderWindowBackground material, BehindWindow blending and Active state. Orbit can use GPUI's native blurred-window backing while matching the same pane tint hierarchy.

## Orbit glass refinement

Following review of the native app, Orbit uses a lighter variation of these glass tokens: project rail tint `#171717` at 60%, sessions pane at 70%, main chat pane at 40%. This exposes the shared AppKit blur more clearly. Code, user-message and tool cards use ink at 4%; hover states use ink at 8%. No chat card paints an opaque black fill or runs a separate blur pass. Modal surfaces use a 78% dark tint for contrast above the workspace.

Orbit adapts the upstream 24 px WindowServer blur when available, retaining GPUI's semantic AppKit material as its fallback. Native traffic lights are hidden; the 40 px headers retain draggable regions and an explicit 28 px sidebar reopen control. Command–W/Command–M provide close/minimize actions.

Project artwork uses the upstream six-effect chooser: None, Dither, ASCII, Halftone, Scanlines, Haze. Scope is empty chats only or all sessions, with independent 5–65% visibility controls (24% default). Artwork sits behind the chat content and remains visible when motion is off; GIF frame notifications stay within the background entity.

Virtual-list insets belong to the list itself, because GPUI lays out its rows as independent roots. Both lists have 8 px horizontal padding, each row fills the remaining width, and project labels have `min-width: 0` with ellipsis. Session cards keep a 68 px height with 6 px vertical padding, metadata/title line heights of 16/20/16 px, and two 2 px gaps.

The optional file inspector is 360 px wide with a 40 px header. Its virtual tree uses 30 px rows, 8 px list insets, restrained indentation, and 14 px file-type SVGs. Preview cards share the translucent ink treatment of chat code cards. Closing the inspector releases its entity and preview resources. Below 1180 px window width, the sessions column yields space while Files is open.

The history control occupies a constant 28 px strip. Loading uses a short opacity pulse; completion flashes a small check for 400 ms. Motion-off renders static status. Page insertion restores the existing logical row and intra-row offset, so the animation never moves the reader's viewport.

## Typography and motion

Source sans stack begins `system-ui, -apple-system, BlinkMacSystemFont`; native equivalent `.AppleSystemUIFont`. Code begins `ui-monospace, SFMono-Regular, Menlo`. Most chrome uses 11–14 px sizes and medium/semibold weights, not large marketing text. Label leading is 1.4 to preserve descenders.

Source feedback duration 120 ms, reorder duration 160 ms, tab-close duration 200 ms. Ease-out is cubic-bezier(0.22, 1, 0.36, 1); tab motion cubic-bezier(0.3333, 0.6667, 0.6667, 1). Under reduced motion, these transitions are disabled. Orbit can retain its bounded spring for direct manipulation while matching the shorter source timing for ordinary hover/focus feedback.

## Embedded asset API

`assets/generated.rs` exports `pub fn asset(path: &str) -> Option<&'static [u8]>`. Paths accept `name`, `name.svg`, or `assets/name.svg`. Standard chrome names: `inbox`, `settings`, `chevron-down`, `chevron-right`, `check`, `close`, `more`, `branch`, `shield`, `sparkles`, `terminal`, `refresh`, `copy`, `stop`, `panel`, `plus`, `search`, `sidebar`, `arrow`, `folder`. Additional supplied names: `file`, `archive`, `clock`, `alert`, `chat`, `pin`, `play`, `pause`, `external`.

Providers: `codex`, `claude`, `gemini`, `opencode`, `antigravity`, `cursor`, `grok` return monochrome masks. `providers/codex` etc. return untouched originals. GPUI `svg()` paints alpha masks using its text color, so gradients do not render in that element; use the top-level minimal masks. Suggested identification colors: Claude `#d97757`, Codex violet, Gemini blue, OpenCode standard ink.

File and project actions also use downloaded Hugeicons assets: `trash`, `folder-open`, `file-code`, `file-document`, `file-image`, `file-audio`, `file-video`, `file-pdf`, `file-data`, and `file-archive`.

Source mapping: `sidebar`=LayoutAlignRightIcon; `panel`=SidebarRight01Icon; `branch`=GitBranchIcon; `shield`=ShieldAlertIcon; `arrow`=ArrowUp01Icon. See per-file comments for every original Hugeicons export.
