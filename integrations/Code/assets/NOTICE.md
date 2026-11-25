# Source and asset provenance

The application uses provider marks solely to identify installed agent products. Code is not affiliated with or endorsed by the owners of those marks. Their trademark rights remain with the respective owners; the software licenses below do not grant trademark rights.

## MonoCode provider assets and design references

`providers/claude.svg`, `providers/opencode.svg`, `providers/cursor.svg`, `providers/grok.svg`, and `providers/antigravity.svg` were originally copied from [MonoCode](https://github.com/hardbeat920/monocode), commit `889579b61c2342cc27751092ff0effbe9ec8e162`, `src/assets/providers/`. Copyright (c) 2026 Nick. Copies of the original MIT license and trademark notice are in `LICENSES/Monocode-MIT.txt` and `LICENSES/Monocode-NOTICE.txt`.

The top-level provider SVGs, except Codex, retain the original path geometry with white fills for GPUI's monochrome alpha-mask renderer. Codex uses the full-color upstream SVG in both asset locations and preserves its gradient and white backing circle. `codex-mask.svg` retains the same outline and negative-space terminal details for the rotating loading icon and gradient status masks, whose renderers accept alpha masks only. The Claude and OpenCode path geometry and original view boxes are retained. Original colored SVG files remain in `providers/`. Cursor and Grok retain the exact upstream paths with white fills. Antigravity retains its original outline path while omitting decorative gradient layers and filter definitions for the monochrome renderer.

### Native background and window behavior

`src/background.rs` ports the dark-theme pixel algorithms from that same MonoCode revision's `src/features/settings/model/newThreadBackgroundEffects.worker.ts`, with controls and scope/opacity defaults from `src/features/projects/model/chatBackground.ts`, `src/features/projects/ui/ProjectBackgroundDialog.tsx`, and `src/features/projects/ui/useProjectBackgroundEffect.ts`. Haze follows its gradient-blur treatment. Rust decoding, bounded frame streaming, persistence, and GPUI rendering are Code implementations.

`src/native.rs` adapts MonoCode's `src-tauri/src/macos.rs`: hidden native traffic lights, optional 24 px WindowServer blur, and UnderWindowBackground/BehindWindow/Active AppKit material. Code retains GPUI's window and renderer rather than using the upstream Tauri host. These adaptations retain the MonoCode MIT attribution and license linked above.

## Google Gemini

`providers/gemini.svg` is Google's original SVG, identified by the `og:logo` metadata on [Gemini's official website](https://gemini.google.com/): [original Gemini sparkle SVG](https://www.gstatic.com/lamda/images/gemini_sparkle_aurora_33f86dc0c0257da337c63.svg). The original mark belongs to Google. Its use here identifies Google's Gemini product; it is not covered by the MonoCode or Hugeicons MIT licenses.

`gemini.svg` uses the exact outline path from that SVG's first clip path. Its gradient, embedded JPEG, and redundant clipping definitions are omitted for GPUI's monochrome renderer. The path and view box are unchanged.

## Hugeicons chrome icons

Outline interface icons come from [`@hugeicons/core-free-icons` 4.3.0](https://www.npmjs.com/package/@hugeicons/core-free-icons/v/4.3.0), the same library used by MonoCode. Copyright (c) 2025 Hugeicons. The original MIT license is in `LICENSES/Hugeicons-MIT.txt`, downloaded from [Hugeicons' canonical repository](https://github.com/hugeicons/hugeicons/blob/main/LICENSE.md).

Original icon node geometry is retained. React attribute names are converted to SVG attributes, `currentColor` becomes white, and outline widths are set to 1.75 to match MonoCode's chrome wrapper (the submit arrow uses the source composer's 2.25 width). `stop.svg` uses the original SquareIcon path with a solid fill. Files contain their original exported icon names in attribution comments.


### File tree and media icons

The file tree's media and document glyphs are downloaded from the canonical npm package above. The original Hugeicons geometry is embedded locally, so browsing files performs no icon network requests. They follow the same 24 × 24 view box and 1.75 stroke convention as the application chrome and are scaled to native interface size by GPUI.

| Code SVG | Hugeicons export |
| --- | --- |
| `trash.svg` | `Delete02Icon` |
| `file-code.svg` | `FileCodeIcon` |
| `file-image.svg` | `FileImageIcon` |
| `file-audio.svg` | `FileAudioIcon` |
| `file-video.svg` | `FileVideoIcon` |
| `file-pdf.svg` | `Pdf02Icon` |
| `file-document.svg` | `FileTextIcon` |
| `file-data.svg` | `FileDatabaseIcon` |
| `file-archive.svg` | `FileArchiveIcon` |
| `folder-open.svg` | `FolderOpenIcon` |

## Code

`orbit.svg` is the pre-existing Code application mark. `generated.rs` only embeds the asset files; it does not implement UI behavior.

### Account lock indicators

`lock-keyhole.svg`, `lock-keyhole-open.svg`, `mic.svg`, and `paperclip.svg` are unmodified SVG files downloaded from [Lucide's official icon repository](https://github.com/lucide-icons/lucide/tree/main/icons). Copyright Lucide Contributors; ISC license retained in `LICENSES/Lucide-ISC.txt`.

## User-selected default wallpaper

`wallpapers/bonsai-sakura.png` is the user-provided “ASCII Bonsai at Sakura Sunset.png” from Downloads, bundled at the user's request. `Code.icns` uses the same artwork, resized with a rounded mask by the build-time icon generator. No third-party license or ownership claim is inferred.

The `model-moon`, `model-earth`, `model-sun`, and `model-star` vector glyphs are
original Kai Unified artwork. They identify Luna, Terra, Sol, and Astra model
families and are shared by the native notch and Code composer.

Codex color mark: MonoCode, `crates/monocode-gpui/assets/codex.svg`, Git blob `4333ab231ac7ad38953c90398a13f4b5b91aab58`, https://github.com/hardbeat920/monocode/blob/main/crates/monocode-gpui/assets/codex.svg (MIT; see bundled MonoCode/Code attribution). Codex is an OpenAI mark.

### Account usage protocols

Cursor protobuf quota fields, the Cursor checksum, Grok Build billing protocol, and Antigravity's structured `/usage` output were independently adapted from [AgentUsage](https://github.com/vlondon/AgentUsage), MIT license retained in `LICENSES/AgentUsage-MIT.txt`. No missing quota is treated as zero. Cursor, Grok, and OpenCode image fills are fixed to the light neutral brand mark for dark surfaces; native notch alpha masks remain separate.

Terminal font metrics and focused macOS action bindings were compared with MonoCode Native (`crates/monocode-gpui/src/terminal.rs`, main, 2026-11-06; https://github.com/hardbeat920/monocode/blob/main/crates/monocode-gpui/src/terminal.rs). MonoCode attribution and MIT license are included above.
