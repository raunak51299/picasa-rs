# Filmstrip: full-folder browsing with lazy thumbnails

## Intent

The filmstrip currently builds only the 16 photos before and after the current
image (`THUMB_RADIUS`). The scroll area therefore has no content beyond that
small window, so dragging stops even when the folder contains many more
photos. The viewer should be able to browse the entire folder through the
filmstrip while loading thumbnail textures only for the tiles that are needed.

## Goals

- Give the filmstrip a scroll extent covering every photo in the folder.
- Render only the visible tiles plus a small prefetch buffer.
- Request thumbnails for the visible range as the viewport moves, rather than
  loading the whole folder at startup.
- Keep the current image and the visible filmstrip range in the thumbnail
  cache; discard thumbnails far from both.
- Preserve click-to-navigate, hover names, selection styling, and automatic
  scrolling to the current photo after navigation.
- Keep rapid dragging responsive by dropping queued thumbnail work that has
  fallen outside the current window.

## Non-goals

- Changing thumbnail resolution or decoding.
- Changing main-photo prefetching or cache policy.
- Scanning or indexing the folder in the background.
- Adding a spinner or changing the visual design of individual tiles.

## Constraints

- egui 0.37 immediate mode; there is no built-in horizontal virtual list.
- The existing loader is a priority queue with an `in_flight` set and a
  current-index staleness check.
- Thumbnail cache capacity is 320 textures.
- No new dependencies.

## Approaches Considered

1. **Render every tile, lazy-load only visible textures.** Simplest, but the
   per-frame widget count grows with the folder and becomes sluggish on large
   folders.
2. **Virtualize the filmstrip.** Size the scroll content for the full folder,
   paint only visible tiles, and request thumbnails for that viewport. Requires
   viewport-aware loading and cache eviction, but scales to large folders.
   Chosen.
3. **Load in pages when the strip reaches the end.** Cheap, but interrupting a
   drag with page boundaries does not provide continuous browsing.

## Design

### Virtualized filmstrip

Replace the `start..end` loop based on `THUMB_RADIUS` with an egui
`ScrollArea::show_viewport` implementation:

- Content width is `photo_count * tile_stride - spacing`, so the scroll area
  represents the whole folder.
- Tiles remain 52 x 52 with 6 px spacing.
- The viewport rectangle determines the visible index range; only those tiles
  are painted and given click responses.
- A pure helper computes the visible range from the viewport and clamps it to
  the folder size. Unit tests cover normal, partial, and end-of-list ranges.
- The current photo is still scrolled into view when `scroll_filmstrip` is set,
  using `Ui::scroll_to_rect` so it also works when the current tile is outside
  the visible range.

### Visible-range thumbnail loading

- The UI expands the visible range by a small buffer (8 photos on each side)
  and records it in `App`.
- When the expanded range changes, the app requests missing thumbnails for that
  range. Tiles inside the viewport use a higher priority than the buffer.
- The existing current-image thumbnail prefetch remains, so the loading overlay
  and current photo still have a thumbnail when the strip is hidden or the user
  has scrolled away.
- Thumbnails still show the extension placeholder until their texture arrives.

### Loader staleness

- Add a thumbnail viewport range to the loader shared state, updated by the UI.
- A thumbnail job is stale only when it is outside both:
  - the current thumbnail prefetch radius, and
  - the current filmstrip viewport (including its buffer).
- This lets jobs far from the current photo but visible in the strip run, while
  dropping queued jobs left behind by a fast drag.
- Clear the viewport when the folder changes.

### Thumbnail cache eviction

- Thumbnail eviction ranks entries by their distance to the nearer of:
  - the current photo, and
  - the visible/prefetch filmstrip range.
- The current photo is always retained.
- This prevents thumbnails at the user's scroll position from being evicted
  immediately when the current photo is elsewhere in the folder.

### Events and errors

- Accept a completed thumbnail when it is near the current photo or inside the
  recorded filmstrip range.
- Record a thumbnail failure only when it is relevant by the same rule.
- If a stale job is still relevant, re-request that thumbnail.

### Folder changes

When the listing is replaced, clear the recorded filmstrip range and the
loader viewport, then let the next UI frame request thumbnails for the new
folder around the current photo.

## Edge Cases

- Empty folder: no filmstrip and no viewport requests.
- Current photo scrolled far away: the strip still loads what is visible.
- Rapid drag: queued work outside the new viewport becomes stale.
- Thumbnail decode failure: placeholder remains and the file is not retried
  until the folder is refreshed.
- End of folder: range helpers clamp instead of overflowing.

## Verification

- Unit tests for visible-range math, range expansion, cache eviction, and
  loader staleness.
- `cargo fmt --check`.
- `cargo test`.
- Release-build manual test with a folder of several hundred photos:
  - startup does not load every thumbnail;
  - dragging to the middle and end reveals every photo;
  - thumbnails appear as tiles enter view;
  - clicking a far thumbnail navigates to it;
  - navigating away scrolls the strip back to the current photo.
