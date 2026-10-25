# Continuous chat scrubber

A short, dense tick bar sits at the vertical center of the chat panel's left edge. Hover creates a spring-driven magnifying lens. Click or drag to a position; release to open that part of the conversation. The first and last ticks reach the actual beginning and latest end. Navigation does not stop at individual prompts.

The rail owns its own GPUI entity and one canvas with 49 strokes. Three damped springs animate the selected position, hover lens, and emphasis. Velocity carries through direction changes. Display-linked redraws stop when the springs settle; reduced motion snaps immediately. Pointer movement schedules no history work. Release makes one cancellable request, and later requests supersede earlier ones.

## Lazy source navigation

History seeks run on the existing Rust state worker. They replace only a bounded decoded history window (200 messages and the existing 256 KiB page budget). Native JSONL uses source byte positions and record boundaries; SQLite and OpenCode use indexed keys. Interior positions are approximate source locations, not measured percentages of every rendered message's height. The endpoints use real head/tail readers. No upfront transcript-wide message or prompt index is built. Giant selected records use the existing on-disk chunk indexes and bounded decoding; locating/indexing such a record can still require reading it. Compressed archives reuse their prepared seekable source rather than decompressing again for each jump.

The previous transcript remains visible while the destination loads. Draft text is preserved. Older-history paging and the existing eight-resident-page cache remain available. Applying a seek disables transcript entrance animation so old text does not fade or bounce into place again.

## Running agents

Reading older history leaves the agent transport running. Live text continues in the existing bounded response buffer without overwriting the historical viewport. Seeking to the bottom during a run reconstructs a bounded current-turn view from the running prompt, attachments, checkpoint metadata, and live response preview. It does not reread and duplicate the unfinished native response. Completion remains saved normally, and later seeks use persisted history.

Tests cover compact left/center geometry, continuous pointer mapping, one request per release, draft preservation, stream isolation and return without duplicate output, reduced motion, finite spring settling, native/local/OpenCode head/tail/interior seeks, giant Unicode bodies, read bounds, cancellation, and superseded generations.
