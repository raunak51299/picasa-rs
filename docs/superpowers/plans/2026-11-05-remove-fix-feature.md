# Remove the Fix Feature Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove picasa-rs's color-fix feature completely — button, panel, shortcut, save flow, adjustment pipeline, and help/docs — while keeping the static-width control bar and filename tooltip.

**Architecture:** Delete the user-facing Fix UI and its `F` shortcut, then remove the state, actions, and save job that supported it. Finally delete the adjustment/save modules, simplify share and rendering to neutral pixels, remove the `rayon` dependency, and update help/docs.

**Tech Stack:** Rust 2024, egui, wgpu/WGSL, Cargo.

**Spec:** User request: "remove the entire Fix feature" after clarifying they meant the Fix button/panel in the picasa viewer.

## Global Constraints

- Keep the uncommitted static-width control bar and filename hover tooltip in `src/app/ui.rs`.
- Do not commit or push unless requested.
- Keep the existing share, delete, slideshow, filmstrip, and rotation behavior unchanged.
- The project gate is `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo build --release --locked`.

---

### Task 1: Remove the Fix UI and shortcut

**Files:**
- Modify: `src/app/ui.rs`
- Modify: `src/app/input.rs`

**Interfaces:**
- Consumes: nothing.
- Produces: no `Fix` button, no fixes panel, no save-confirmation dialog, no `F` shortcut, no `Edited` readout.

- [ ] Remove the `Fix` button, the `show_fixes` panel, the save-confirmation dialog, and the `EDITED_READOUT_WIDTH` readout from `src/app/ui.rs`.
- [ ] Remove the `pending_save` key handling, `Escape` fixes branch, and `F` key handler from `src/app/input.rs`.
- [ ] Update the in-app help line to drop `F: fixes` and change the share copy text to refer only to EXIF orientation.
- [ ] Remove `edited` from the static-width regression test row.
- [ ] Run `cargo test --locked app::ui::tests` and confirm the static-width test passes.

### Task 2: Remove Fix state, actions, and save flow

**Files:**
- Modify: `src/app/mod.rs`
- Modify: `src/app/input.rs`
- Modify: `src/app/events.rs`

**Interfaces:**
- Consumes: Task 1's removal of all callers of the Fix state/actions.
- Produces: no `show_fixes`, `adjustments`, `pending_save`, `SaveRequest`, `ToggleFixes`, `SetAdjustments`, `Save`, `ConfirmSave`, `CancelSave`, or `JobResult::Saved`.

- [ ] Delete `SaveRequest`, the four Fix-related `UiAction` variants, the `show_fixes`/`adjustments`/`pending_save` fields, and their initializers/resets.
- [ ] Delete `request_save`, `confirm_save`, `save_now`, and `finish_save` from `src/app/input.rs`.
- [ ] Remove `SaveMode`/`save` imports and `SaveRequest` from the input imports.
- [ ] Remove the save job variant and its `drain_jobs` arm; keep the copy job.
- [ ] Remove the save-related `reload_full`/`reload_thumb` state and all branches in `src/app/events.rs` and `src/app/input.rs`.
- [ ] Remove the `pending_save` condition from the slideshow advance gate.
- [ ] Run `cargo check` and fix any remaining references.

### Task 3: Remove the adjustment and save backends

**Files:**
- Delete: `src/adjust.rs`
- Delete: `src/save.rs`
- Modify: `src/main.rs`
- Modify: `src/share.rs`
- Modify: `src/rotate.rs`
- Modify: `src/renderer/mod.rs`
- Modify: `src/renderer/quad_pipeline.rs`
- Modify: `src/renderer/shaders/photo.wgsl`
- Modify: `Cargo.toml`

**Interfaces:**
- Consumes: Task 2's removal of all app-layer callers.
- Produces: `share::display_rgba(path)` and `share::copy_image(path)` with no adjustments argument; a photo shader with no brightness/contrast/gamma/gain uniforms.

- [ ] Delete the two module files and their `mod` declarations.
- [ ] Simplify `share.rs` to decode with EXIF orientation only; update its tests.
- [ ] Remove the now-unused EXIF/ICC save helpers from `rotate.rs` and their test.
- [ ] Remove `adjust`/`gains` from `PhotoDraw`, `PhotoUniforms`, the renderer uniform construction, and `photo.wgsl`.
- [ ] Remove `rayon` from `Cargo.toml` and refresh `Cargo.lock`.

### Task 4: Update docs and verify

**Files:**
- Modify: `README.md`
- Modify: `docs/picasa-parity.md`
- Modify: `src/main.rs`

- [ ] Remove the color-fix feature bullet, the `F` controls row, the "Saving color fixes" section, and the remaining "including fixes" wording from `README.md`.
- [ ] Remove the one-click fixes row from `docs/picasa-parity.md`.
- [ ] Remove the `F` line from the CLI help.
- [ ] Run the full gate: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`, `cargo build --release --locked`.
- [ ] Launch the release build and verify the bottom bar has no Fix button and the width/tooltip behavior is unchanged.
