# Kai pet assets

`animation_catalog.json` is the playback entry point. It lists the 30 distinct
pets, their sprite sheets, hashes, frame grids, state animations, and optional
look directions. `catalog.json` retains all 32 discovered copies and records
the two duplicate sheets. `validation_report.json` records structural checks.
`ANIMATION_CATEGORIES.md` categorizes the shared state contract and describes
the actual motion used by every pet in every state. `gradient_profiles.json`
provides pet-matched gradient tokens for ambient, thinking, working, success,
warning, and error UI; see `GRADIENT_PROFILES.md` and the generated visual
preview in `gradient_profiles_preview.png`.

The 17 original downloads are preserved in `original_zips/`. Their extracted
files are in `zip_pets/`. The 6 locally installed custom pets are copied into
`installed_custom/`. The 9 built-in sheets extracted from the installed
ChatGPT app are in `codex_built_in/`. These bundled copies are tied to the app
version from which they were extracted.

## Playback contract

- Decode the WebP sheet with alpha. Its grid is 8 columns, with 192 × 208 pixel
  cells and a top-left origin.
- Use `stateAnimations[name].row` and `columns` for frame selection. Advance
  after each corresponding `durationsMs` value; honor `loop`.
- The 8 × 9 sheets have the nine standard state rows. The 8 × 11 sheets add
  sixteen look-direction poses in rows 9 and 10.
- The 8 × 11 sheets also have a separate still/neutral frame at row 0,
  column 6. It is recorded as `neutralFrame`; it is not part of the idle loop.
  The 8 × 9 sheets use the first idle frame as their still frame.
- Use the neutral frame when macOS Reduce Motion is enabled. Keep the sprite's
  aspect ratio, and render with alpha over the panel background.
- Resolve paths relative to this folder. Validate the sheet hash before
  caching frames so a changed source cannot silently mismatch the catalog.

The original 22 sheets passed dimensions, frame presence, unused-cell
transparency, frame-edge clearance, frame variation, and hash checks. The eight
new downloads passed dimensions, alpha, required-frame presence, unused-cell
transparency, and hash checks. Lawliet's six-frame wave and timing are recorded
as a per-pet animation override. The notch app reads this catalog for pet and
animation selection and resolves the matching gradient by pet ID.
