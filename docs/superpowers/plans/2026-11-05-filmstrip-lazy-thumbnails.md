# Filmstrip Full-Folder Browsing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the filmstrip scroll across every photo in the folder while loading and keeping thumbnails only for the current photo, the visible strip, and a small buffer.

**Architecture:** Replace the fixed `current ± THUMB_RADIUS` tile loop with an egui viewport-virtualized horizontal scroll area sized to the whole folder. A pure range helper decides which indices to paint and prefetch. The loader gains a visible-range field so queued thumbnails for that range are not treated as stale; thumbnail cache eviction ranks by distance to the nearer of the current photo and that range.

**Tech Stack:** Rust, egui 0.37 (`ScrollArea::show_viewport`), winit background loader threads.

**Spec:** `docs/superpowers/specs/2026-11-05-filmstrip-lazy-thumbnails-design.md`

## Global Constraints

- No new dependencies.
- Tiles stay 52 x 52 with 6 px spacing; filmstrip buffer is 8 photos on each side.
- Thumbnail resolution, decoding, and the main-photo prefetch are unchanged.
- The current photo is always retained by thumbnail cache eviction.
- Do not commit or push until the user explicitly asks.

## Review Focus

- Scrolling to the last photo in a very large folder: the last tile must be reachable and the range must clamp instead of overflowing (`visible_range_clamps_at_the_end_of_the_folder`, `filmstrip_width_accounts_for_spacing`).
- A fast drag that leaves queued thumbnails behind: jobs outside both the current prefetch radius and the visible range must become stale (`thumb_staleness_accepts_a_visible_filmstrip_range`).
- The current photo far from the user's scroll position: thumbnails at the strip position must still be accepted and kept (`thumb_relevance_accepts_current_or_visible`, `eviction_keeps_the_visible_strip_near_its_scroll_position`).
- Current photo and a visible tile tie at eviction distance zero: the current photo must never be dropped (`eviction_never_drops_the_current_photo_even_if_the_viewport_scores_it_equally`).
- Empty folder or folder replacement: no viewport requests and no stale indices from the previous folder (`visible_range_is_empty_without_photos`, `expand_range_adds_a_buffer_and_clamps`, `reset_thumb_viewport` on every listing replacement).

---

### Task 1: Filmstrip geometry and range helpers

**Files:**
- Create: `src/app/filmstrip.rs`
- Modify: `src/app/mod.rs:1-8` (module declarations)

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `pub(super) const THUMB_TILE: f32 = 52.0;`
  - `pub(super) const THUMB_SPACING: f32 = 6.0;`
  - `pub(super) const THUMB_STRIDE: f32 = THUMB_TILE + THUMB_SPACING;`
  - `pub(super) const THUMB_BUFFER: usize = 8;`
  - `pub(super) const THUMB_RELEVANCE_RADIUS: usize = THUMB_RADIUS + 8;`
  - `pub(super) fn filmstrip_width(count: usize) -> f32`
  - `pub(super) fn visible_range(viewport: egui::Rect, count: usize) -> std::ops::Range<usize>`
  - `pub(super) fn expand_range(range: &Range<usize>, count: usize, buffer: usize) -> Range<usize>`
  - `pub(super) fn range_distance(index: usize, current: usize, viewport: Option<&Range<usize>>) -> usize`
  - `pub(super) fn thumb_is_relevant(index: usize, current: usize, viewport: Option<&Range<usize>>, radius: usize) -> bool`

- [ ] **Step 1: Write the failing tests in `src/app/filmstrip.rs`**

```rust
use std::ops::Range;

use super::THUMB_RADIUS;

pub(super) const THUMB_TILE: f32 = 52.0;
pub(super) const THUMB_SPACING: f32 = 6.0;
pub(super) const THUMB_STRIDE: f32 = THUMB_TILE + THUMB_SPACING;
pub(super) const THUMB_BUFFER: usize = 8;
pub(super) const THUMB_RELEVANCE_RADIUS: usize = THUMB_RADIUS + 8;

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Rect};

    fn viewport(min_x: f32, max_x: f32) -> Rect {
        Rect::from_min_max(pos2(min_x, 0.0), pos2(max_x, THUMB_TILE))
    }

    #[test]
    fn filmstrip_width_accounts_for_spacing() {
        assert_eq!(filmstrip_width(0), 0.0);
        assert_eq!(filmstrip_width(1), THUMB_TILE);
        assert_eq!(
            filmstrip_width(3),
            THUMB_TILE * 3.0 + THUMB_SPACING * 2.0
        );
    }

    #[test]
    fn visible_range_covers_every_partly_visible_tile() {
        assert_eq!(visible_range(viewport(0.0, 100.0), 10), 0..2);
        assert_eq!(visible_range(viewport(110.0, 300.0), 10), 1..6);
    }

    #[test]
    fn visible_range_clamps_at_the_end_of_the_folder() {
        assert_eq!(visible_range(viewport(100.0, 300.0), 3), 1..3);
        assert_eq!(visible_range(viewport(0.0, 10_000.0), 4), 0..4);
    }

    #[test]
    fn visible_range_is_empty_without_photos() {
        assert_eq!(visible_range(viewport(0.0, 100.0), 0), 0..0);
    }

    #[test]
    fn expand_range_adds_a_buffer_and_clamps() {
        assert_eq!(expand_range(&(10..20), 100, 8), 2..28);
        assert_eq!(expand_range(&(0..4), 100, 8), 0..12);
        assert_eq!(expand_range(&(96..100), 100, 8), 88..100);
        assert_eq!(expand_range(&(0..0), 0, 8), 0..0);
    }

    #[test]
    fn range_distance_uses_the_nearer_of_current_and_viewport() {
        assert_eq!(range_distance(60, 0, Some(&(55..65))), 0);
        assert_eq!(range_distance(50, 0, Some(&(55..65))), 5);
        assert_eq!(range_distance(90, 0, Some(&(55..65))), 26);
        assert_eq!(range_distance(2, 0, None), 2);
        assert_eq!(range_distance(2, 0, Some(&(0..0))), 2);
    }

    #[test]
    fn thumb_relevance_accepts_current_or_visible() {
        assert!(thumb_is_relevant(5, 0, None, 21));
        assert!(!thumb_is_relevant(22, 0, None, 21));
        assert!(thumb_is_relevant(500, 0, Some(&(490..510)), 21));
        assert!(!thumb_is_relevant(510, 0, Some(&(490..510)), 21));
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test filmstrip::tests`
Expected: FAIL to compile with `cannot find function ...`.

- [ ] **Step 3: Implement the helpers**

```rust
pub(super) fn filmstrip_width(count: usize) -> f32 {
    if count == 0 {
        0.0
    } else {
        count as f32 * THUMB_TILE + (count - 1) as f32 * THUMB_SPACING
    }
}

pub(super) fn visible_range(viewport: egui::Rect, count: usize) -> Range<usize> {
    if count == 0 {
        return 0..0;
    }
    let start = ((viewport.min.x.max(0.0) / THUMB_STRIDE).floor() as usize).min(count);
    let end = ((viewport.max.x.max(0.0) / THUMB_STRIDE).ceil() as usize).min(count);
    start..end.max(start)
}

pub(super) fn expand_range(range: &Range<usize>, count: usize, buffer: usize) -> Range<usize> {
    let start = range.start.saturating_sub(buffer).min(count);
    let end = range.end.saturating_add(buffer).min(count);
    start..end.max(start)
}

pub(super) fn range_distance(
    index: usize,
    current: usize,
    viewport: Option<&Range<usize>>,
) -> usize {
    let current_distance = index.abs_diff(current);
    let Some(viewport) = viewport.filter(|range| !range.is_empty()) else {
        return current_distance;
    };
    let viewport_distance = if index < viewport.start {
        viewport.start - index
    } else if index >= viewport.end {
        index - viewport.end + 1
    } else {
        0
    };
    current_distance.min(viewport_distance)
}

pub(super) fn thumb_is_relevant(
    index: usize,
    current: usize,
    viewport: Option<&Range<usize>>,
    radius: usize,
) -> bool {
    index.abs_diff(current) <= radius || viewport.is_some_and(|range| range.contains(&index))
}
```

- [ ] **Step 4: Register the module**

Add `mod filmstrip;` to the module list in `src/app/mod.rs`, keeping the existing alphabetical order:

```rust
mod animation;
mod cache;
mod events;
mod filmstrip;
mod input;
```

- [ ] **Step 5: Run the tests and format check**

Run: `cargo test filmstrip::tests`
Expected: PASS for all seven tests.

Run: `cargo fmt --check`
Expected: exit 0.

---

### Task 2: Loader viewport-aware thumbnail staleness

**Files:**
- Modify: `src/loader/mod.rs:97-106` (`is_stale`)
- Modify: `src/loader/mod.rs:168-176` (`Shared` fields)
- Modify: `src/loader/mod.rs:188-195` (`Shared` construction)
- Modify: `src/loader/mod.rs:233-244` (`Loader` methods)
- Modify: `src/loader/mod.rs:288-299` (`worker_loop`)
- Modify: `src/loader/mod.rs:411-427` (tests)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces:
  - `pub fn Loader::set_thumb_viewport(&self, viewport: Option<Range<usize>>)`
  - `fn is_stale(index: usize, current: usize, kind: JobKind, thumb_viewport: Option<Range<usize>>) -> bool`
  - `fn option_viewport(start: usize, end: usize) -> Option<Range<usize>>`

- [ ] **Step 1: Update the failing tests**

Replace the existing `stale_radius_depends_on_job_kind` body so every call passes `None` as the fourth argument, and add:

```rust
#[test]
fn thumb_staleness_accepts_a_visible_filmstrip_range() {
    assert!(is_stale(100, 0, JobKind::Thumb, None));
    assert!(!is_stale(100, 0, JobKind::Thumb, Some(80..120)));
    assert!(is_stale(120, 0, JobKind::Thumb, Some(80..120)));
    assert!(!is_stale(48, 0, JobKind::Thumb, None));
    assert!(is_stale(49, 0, JobKind::Thumb, None));
    assert!(!is_stale(0, 0, JobKind::Animation, None));
    assert!(is_stale(1, 0, JobKind::Animation, None));
    // The viewport only rescues thumbnail jobs.
    assert!(is_stale(100, 0, JobKind::Full, Some(80..120)));
}

#[test]
fn empty_viewport_bounds_mean_no_viewport() {
    assert_eq!(option_viewport(0, 0), None);
    assert_eq!(option_viewport(4, 4), None);
    assert_eq!(option_viewport(4, 9), Some(4..9));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test loader::tests`
Expected: FAIL to compile because `is_stale` takes three arguments and `option_viewport` does not exist.

- [ ] **Step 3: Add the viewport state and staleness rule**

At the top of `src/loader/mod.rs`, add:

```rust
use std::ops::Range;
```

Add to `struct Shared`:

```rust
thumb_viewport_start: AtomicUsize,
thumb_viewport_end: AtomicUsize,
```

Initialize both to `AtomicUsize::new(0)` in `Loader::new`.

Replace `is_stale` with:

```rust
fn is_stale(
    index: usize,
    current: usize,
    kind: JobKind,
    thumb_viewport: Option<Range<usize>>,
) -> bool {
    match kind {
        JobKind::Full => index.abs_diff(current) > STALE_FULL_RADIUS,
        JobKind::Thumb => {
            index.abs_diff(current) > STALE_THUMB_RADIUS
                && !thumb_viewport.is_some_and(|range| range.contains(&index))
        }
        JobKind::Animation => index != current,
    }
}

fn option_viewport(start: usize, end: usize) -> Option<Range<usize>> {
    (start < end).then_some(start..end)
}
```

Add a new `impl Shared` block:

```rust
impl Shared {
    fn thumb_viewport(&self) -> Option<Range<usize>> {
        option_viewport(
            self.thumb_viewport_start.load(AtomicOrdering::Relaxed),
            self.thumb_viewport_end.load(AtomicOrdering::Relaxed),
        )
    }
}
```

Add to `impl Loader` next to `set_current`:

```rust
pub fn set_thumb_viewport(&self, viewport: Option<Range<usize>>) {
    let (start, end) = viewport.map_or((0, 0), |range| (range.start, range.end));
    self.shared
        .thumb_viewport_start
        .store(start, AtomicOrdering::Relaxed);
    self.shared
        .thumb_viewport_end
        .store(end, AtomicOrdering::Relaxed);
}
```

In `worker_loop`, read the viewport before the stale check:

```rust
let current = shared.current_index.load(AtomicOrdering::Relaxed);
let thumb_viewport = shared.thumb_viewport();
let event = if is_stale(job.index, current, job.kind, thumb_viewport) {
```

- [ ] **Step 4: Run the tests and format check**

Run: `cargo test loader::tests`
Expected: PASS, including both new tests.

Run: `cargo fmt --check`
Expected: exit 0.

---

### Task 3: Virtualized filmstrip and visible-range requests

**Files:**
- Modify: `src/app/mod.rs:41` (keep `THUMB_RADIUS`), `src/app/mod.rs:169` (App field), `src/app/mod.rs:371` (initialization), `src/app/mod.rs:649-650` and `src/app/mod.rs:721-723` (listing replacement)
- Modify: `src/app/filmstrip.rs` (App methods)
- Modify: `src/app/ui.rs:5-7` (imports), `src/app/ui.rs:272-397` (filmstrip block)

**Interfaces:**
- Consumes: Task 1 helpers; Task 2 `Loader::set_thumb_viewport`.
- Produces:
  - `App.thumb_viewport: Option<Range<usize>>`
  - `pub(super) fn App::reset_thumb_viewport(&mut self)`
  - `pub(super) fn App::request_thumb_range(&mut self, range: Range<usize>, priority: u8)`
  - `pub(super) fn App::update_thumb_viewport(&mut self, visible: Range<usize>)`

- [ ] **Step 1: Add the App state**

In `src/app/mod.rs`, add `use std::ops::Range;` with the other `std` imports, then add the field after `scroll_filmstrip`:

```rust
thumb_viewport: Option<Range<usize>>,
```

Initialize it in `App::new`:

```rust
thumb_viewport: None,
```

- [ ] **Step 2: Add the viewport methods to `src/app/filmstrip.rs`**

Add `use super::App;` at the top, then:

```rust
impl App {
    pub(super) fn reset_thumb_viewport(&mut self) {
        self.thumb_viewport = None;
        if let Some(loader) = &self.loader {
            loader.set_thumb_viewport(None);
        }
    }

    pub(super) fn request_thumb_range(&mut self, range: Range<usize>, priority: u8) {
        let Some(loader) = &self.loader else {
            return;
        };
        for index in range {
            let Some(path) = self.nav.path(index) else {
                continue;
            };
            if self.thumbs.contains_key(path) || self.failed_thumb.contains(path) {
                continue;
            }
            loader.request_thumb(path, index, priority);
        }
    }

    pub(super) fn update_thumb_viewport(&mut self, visible: Range<usize>) {
        let expanded = expand_range(&visible, self.nav.len(), THUMB_BUFFER);
        if self.thumb_viewport.as_ref() == Some(&expanded) {
            return;
        }
        self.thumb_viewport = Some(expanded.clone());
        if let Some(loader) = &self.loader {
            loader.set_thumb_viewport(Some(expanded.clone()));
        }
        self.request_thumb_range(visible, 2);
        self.request_thumb_range(expanded, 0);
    }
}
```

- [ ] **Step 3: Reset the viewport when the listing is replaced**

In `open_path`, immediately after `self.nav = nav;`, add:

```rust
self.reset_thumb_viewport();
```

In `rescan_folder`, immediately after `self.nav = nav;`, add:

```rust
self.reset_thumb_viewport();
```

- [ ] **Step 4: Replace the fixed tile window in `src/app/ui.rs`**

Update the imports:

```rust
use super::filmstrip::{
    THUMB_SPACING, THUMB_STRIDE, THUMB_TILE, filmstrip_width, visible_range,
};
use super::{App, UiAction, truncate};
```

Inside the `if !self.nav.is_empty() && filmstrip_alpha > 0.01` block, replace the `start`/`end` computation and the `ScrollArea` content with a viewport-driven version. Keep the `Area`, `Frame`, builder options, and tile painting exactly as they are, except:

```rust
let mut scroll_to_current = self.scroll_filmstrip;
let count = self.nav.len();
let content_width = filmstrip_width(count);
let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));
let mut visible = None;

egui::Area::new(egui::Id::unique("filmstrip"))
    .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -12.0))
    .order(egui::Order::Foreground)
    .show(ctx, |ui| {
        egui::Frame::NONE
            // ... existing fill, stroke, corner radius, margin ...
            .show(ui, |ui| {
                ui.set_opacity(alpha);
                ui.spacing_mut().item_spacing = egui::vec2(THUMB_SPACING, 0.0);
                let output = egui::ScrollArea::horizontal()
                    .id_salt("filmstrip_scroll")
                    .scroll_bar_visibility(
                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                    )
                    .scroll_source(egui::scroll_area::ScrollSource::ALL)
                    .on_hover_cursor(egui::CursorIcon::Grab)
                    .on_drag_cursor(egui::CursorIcon::Grabbing)
                    .max_height(62.0)
                    .show_viewport(ui, |ui, viewport| {
                        ui.set_min_size(egui::vec2(content_width, THUMB_TILE));
                        let origin = ui.max_rect().min;
                        let range = visible_range(viewport, count);

                        for index in range.clone() {
                            let Some(path) = self.nav.path(index) else {
                                continue;
                            };
                            let selected = index == current_index;
                            let rect = egui::Rect::from_min_size(
                                origin + egui::vec2(index as f32 * THUMB_STRIDE, 0.0),
                                egui::vec2(THUMB_TILE, THUMB_TILE),
                            );
                            let response = ui.interact(
                                rect,
                                ui.make_persistent_id(("filmstrip_thumb", index)),
                                egui::Sense::click(),
                            );

                            // Keep the existing texture drawing, extension
                            // placeholder, selection stroke, hover stroke,
                            // click handling, and hover text here, using
                            // `rect` and `response` instead of allocating.
                        }

                        if scroll_to_current {
                            let current_rect = egui::Rect::from_min_size(
                                origin
                                    + egui::vec2(current_index as f32 * THUMB_STRIDE, 0.0),
                                egui::vec2(THUMB_TILE, THUMB_TILE),
                            );
                            ui.scroll_to_rect(current_rect, Some(egui::Align::Center));
                            scroll_to_current = false;
                        }

                        range
                    });
                visible = Some(output.inner);
            });
    });

self.scroll_filmstrip = scroll_to_current;
if let Some(range) = visible {
    self.update_thumb_viewport(range);
}
```

Use `ui.spacing_mut().item_spacing = egui::vec2(THUMB_SPACING, 0.0)` and import `THUMB_SPACING` as well; the item spacing is no longer needed for layout but keeps the style consistent if other widgets are added.

Delete the now-unused `let start = ...`, `let end = ...`, and `ui.horizontal(...)` wrapper. Do not render tiles outside `range`.

- [ ] **Step 5: Build and run the existing tests**

Run: `cargo test`
Expected: PASS. The range tests from Task 1 and loader tests from Task 2 still pass.

Run: `cargo fmt --check`
Expected: exit 0.

- [ ] **Step 6: Quick manual smoke check**

Run: `cargo build --release`
Expected: exit 0.

Launch `target\release\picasa-rs.exe` with a folder of at least 100 photos, drag the strip to the right, and confirm the scroll thumb travels the full folder width and tiles appear as they enter view.

---

### Task 4: Range-aware thumbnail acceptance, errors, and stale retries

**Files:**
- Modify: `src/app/filmstrip.rs` (relevance method)
- Modify: `src/app/events.rs:8` (imports), `src/app/events.rs:112-136` (Thumb), `src/app/events.rs:179-181` (thumb error), `src/app/events.rs:197-218` (Stale)

**Interfaces:**
- Consumes: `App.thumb_viewport`, `thumb_is_relevant` from Task 1, `App::request_thumb_range` from Task 3.
- Produces: `pub(super) fn App::is_thumb_relevant(&self, index: usize) -> bool`.

- [ ] **Step 1: Add the App relevance method**

In `src/app/filmstrip.rs`, inside the existing `impl App`:

```rust
pub(super) fn is_thumb_relevant(&self, index: usize) -> bool {
    thumb_is_relevant(
        index,
        self.nav.current(),
        self.thumb_viewport.as_ref(),
        THUMB_RELEVANCE_RADIUS,
    )
}
```

- [ ] **Step 2: Use it for completed thumbnails**

In `src/app/events.rs`, replace:

```rust
if !self.nav.is_near(thumb.index, THUMB_RADIUS + 8) {
    return;
}
```

with:

```rust
if !self.is_thumb_relevant(thumb.index) {
    return;
}
```

- [ ] **Step 3: Record only relevant thumbnail errors**

Replace the `JobKind::Thumb` arm in the `LoaderEvent::Error` match with:

```rust
crate::loader::JobKind::Thumb => {
    if self.is_thumb_relevant(index) {
        self.failed_thumb.insert(path);
    }
}
```

- [ ] **Step 4: Retry only relevant stale thumbnails**

Replace the `needed` calculation and the retry block with:

```rust
let needed = match kind {
    crate::loader::JobKind::Full => index == self.nav.current(),
    crate::loader::JobKind::Thumb => self.is_thumb_relevant(index),
    crate::loader::JobKind::Animation => index == self.nav.current(),
};
if needed {
    match kind {
        crate::loader::JobKind::Animation => self.request_animation_for_current(),
        crate::loader::JobKind::Thumb => {
            self.request_thumb_range(index..index + 1, 1);
        }
        crate::loader::JobKind::Full => self.request_window(),
    }
}
```

Remove the now-unused `THUMB_RADIUS` import from `src/app/events.rs`.

- [ ] **Step 5: Run the tests and format check**

Run: `cargo test`
Expected: PASS.

Run: `cargo fmt --check`
Expected: exit 0.

---

### Task 5: Viewport-aware thumbnail cache eviction

**Files:**
- Modify: `src/app/cache.rs:38-49` (`evict_thumb_cache`)
- Modify: `src/app/cache.rs:85-98` (`eviction_targets`)
- Modify: `src/app/cache.rs:100-141` (tests)

**Interfaces:**
- Consumes: `range_distance` and `App.thumb_viewport` from Task 1 / Task 3.
- Produces: `fn eviction_targets(entries: &[(PathBuf, usize)], current: usize, viewport: Option<&Range<usize>>, limit: usize) -> Vec<PathBuf>`.

- [ ] **Step 1: Write the failing eviction tests**

Update every existing `eviction_targets` call to pass `None` as the third argument, then add:

```rust
#[test]
fn eviction_keeps_the_visible_strip_near_its_scroll_position() {
    let cached = entries(&[0, 50, 60, 90]);
    assert_eq!(
        eviction_targets(&cached, 0, Some(&(55..65)), 2),
        [PathBuf::from("photo90.jpg"), PathBuf::from("photo50.jpg")]
    );
}

#[test]
fn eviction_never_drops_the_current_photo_even_if_the_viewport_scores_it_equally() {
    let cached = entries(&[0, 205]);
    assert_eq!(
        eviction_targets(&cached, 0, Some(&(200..210)), 1),
        [PathBuf::from("photo205.jpg")]
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test app::cache::tests`
Expected: FAIL to compile because `eviction_targets` takes three arguments.

- [ ] **Step 3: Implement viewport-aware ranking**

Add imports to `src/app/cache.rs`:

```rust
use std::ops::Range;

use super::filmstrip::range_distance;
```

Change `evict_thumb_cache` to pass the recorded range:

```rust
for path in eviction_targets(
    &entries,
    current,
    self.thumb_viewport.as_ref(),
    THUMB_CACHE_LIMIT,
) {
    // Dropping the handle frees the egui texture.
    self.thumbs.remove(&path);
}
```

Change photo eviction to pass `None`:

```rust
for path in eviction_targets(&entries, current, None, limit) {
    self.photos.remove(&path);
}
```

Replace `eviction_targets` with:

```rust
/// Return the cached paths to drop so at most `limit` entries remain, furthest
/// from `current` and the visible viewport first. The current photo is always
/// dropped last, even when a visible tile ties it at distance zero. Paths that
/// are no longer in the folder sort last.
fn eviction_targets(
    entries: &[(PathBuf, usize)],
    current: usize,
    viewport: Option<&Range<usize>>,
    limit: usize,
) -> Vec<PathBuf> {
    let limit = limit.max(1);
    if entries.len() <= limit {
        return Vec::new();
    }

    let mut ranked: Vec<&(PathBuf, usize)> = entries.iter().collect();
    ranked.sort_by_key(|(path, index)| {
        (
            std::cmp::Reverse(range_distance(*index, current, viewport)),
            *index != current,
            path.clone(),
        )
    });

    ranked
        .into_iter()
        .take(entries.len() - limit)
        .map(|(path, _)| path.clone())
        .collect()
}
```

- [ ] **Step 4: Run the tests and format check**

Run: `cargo test app::cache::tests`
Expected: PASS, including both new tests.

Run: `cargo fmt --check`
Expected: exit 0.

---

### Task 6: Full verification

**Files:** none unless verification finds a defect.

- [ ] **Step 1: Run the full automated suite**

Run: `cargo fmt --check`
Expected: exit 0.

Run: `cargo test`
Expected: PASS with zero failures.

Run: `cargo build --release`
Expected: exit 0.

- [ ] **Step 2: Create a several-hundred-photo folder for manual testing**

Run in PowerShell:

```powershell
Add-Type -AssemblyName System.Drawing
$dir = Join-Path $env:TEMP 'picasa-filmstrip-test'
New-Item -ItemType Directory -Force -Path $dir | Out-Null
for ($i = 1; $i -le 400; $i++) {
    $bmp = New-Object System.Drawing.Bitmap 160, 120
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.Clear([System.Drawing.Color]::FromArgb(255, ($i * 7) % 256, ($i * 13) % 256, ($i * 29) % 256))
    $g.Dispose()
    $bmp.Save((Join-Path $dir ('photo-{0:D4}.jpg' -f $i)), [System.Drawing.Imaging.ImageFormat]::Jpeg)
    $bmp.Dispose()
}
```

- [ ] **Step 3: Exercise the full-folder behavior**

Launch: `target\release\picasa-rs.exe "$env:TEMP\picasa-filmstrip-test\photo-0001.jpg"`

Verify:

- the filmstrip starts near the first photo and does not eagerly decode all 400 thumbnails;
- dragging to the middle and the end reaches every photo with no hard stop;
- thumbnails appear progressively as tiles enter view;
- clicking a far thumbnail navigates to that photo;
- pressing the arrow keys navigates away and the strip scrolls back to the current photo;
- opening a second folder clears the old viewport and starts around the new current photo.

- [ ] **Step 4: Report results**

Report the exact commands run and their outcomes. Do not claim success without fresh output. If any check fails, stop and fix it before reporting.
