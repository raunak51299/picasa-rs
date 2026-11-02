//! HUD, filmstrip, help, and error overlays.

use std::time::Instant;

use crate::save::SaveMode;

use super::{App, THUMB_RADIUS, UiAction, truncate};

impl App {
    pub(super) fn build_ui(&mut self, ctx: &egui::Context) -> Vec<UiAction> {
        let mut actions = Vec::new();
        let now = Instant::now();
        let screen = ctx.viewport_rect();
        let screen_size = screen.size();
        let Some(current_path) = self.nav.current_path().map(|path| path.to_path_buf()) else {
            return actions;
        };
        let current_index = self.nav.current();
        let windowed = self
            .window
            .as_ref()
            .is_some_and(|window| window.fullscreen().is_none());
        let loading =
            self.current_photo().is_none() && !self.failed_full.contains_key(&current_path);
        let error = self.failed_full.get(&current_path).cloned();
        let show_help = now < self.show_help_until;
        let toast = self
            .toast
            .as_ref()
            .filter(|toast| now < toast.until)
            .map(|toast| toast.text.clone());
        let rotation = self.pending_rotation.as_ref().map(|request| {
            (
                request
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string(),
                request.clockwise,
            )
        });
        let deletion = self.pending_delete.as_ref().map(|path| {
            path.file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });
        let saving = self.pending_save.as_ref().map(|request| {
            request
                .path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string()
        });

        let show_controls = self.hud_alpha > 0.01;
        let pointer_near_bottom = self
            .cursor
            .map(|position| position.y as f32 > self.camera.viewport.y - 230.0)
            .unwrap_or(false);
        let show_filmstrip = show_controls && (pointer_near_bottom || self.filmstrip_hovered);

        if !show_controls
            && !loading
            && error.is_none()
            && !show_help
            && toast.is_none()
            && rotation.is_none()
            && deletion.is_none()
            && saving.is_none()
            && !self.show_info
            && !self.show_fixes
            && !self.show_share
            && !self.drag_hover
        {
            return actions;
        }

        if show_controls {
            let alpha = self.hud_alpha;
            let bottom_offset = if show_filmstrip { 108.0 } else { 18.0 };
            let photo = self.current_photo();
            let file_name = current_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let dimensions = photo
                .map(|photo| format!("{} × {}", photo.display_width, photo.display_height))
                .unwrap_or_else(|| "—".to_string());
            let zoom = if self.camera.has_image() {
                if self.camera.fit_mode {
                    format!("Fit {:.0}%", self.camera.zoom_percent())
                } else {
                    format!("{:.0}%", self.camera.zoom_percent())
                }
            } else {
                "—".to_string()
            };
            let rotation = self.camera.rotation_degrees();
            let index_label = format!("{} / {}", current_index + 1, self.nav.len());

            egui::Area::new(egui::Id::unique("controls"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -bottom_offset))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha((190.0 * alpha) as u8))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_white_alpha((22.0 * alpha) as u8),
                        ))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::symmetric(12, 7))
                        .show(ui, |ui| {
                            ui.set_opacity(alpha);
                            // In a photo-sized window the single HUD row can be
                            // wider than the window. Wrap it instead of letting
                            // the edges clip.
                            ui.set_max_width((ctx.content_rect().width() - 32.0).max(200.0));
                            ui.spacing_mut().item_spacing.x = 7.0;
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    egui::RichText::new(truncate(&file_name, 32))
                                        .size(13.0)
                                        .strong(),
                                );
                                ui.label(
                                    egui::RichText::new(dimensions)
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(160)),
                                );
                                ui.separator();
                                ui.label(
                                    egui::RichText::new(index_label)
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(185)),
                                );
                                ui.label(
                                    egui::RichText::new(zoom)
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(185)),
                                );
                                if rotation != 0 {
                                    ui.label(
                                        egui::RichText::new(format!("{rotation}°"))
                                            .size(12.0)
                                            .color(egui::Color32::from_gray(185)),
                                    );
                                }
                                if !self.adjustments.is_neutral() {
                                    ui.label(
                                        egui::RichText::new("Edited")
                                            .size(11.0)
                                            .color(egui::Color32::from_rgb(118, 186, 255)),
                                    );
                                }
                                ui.separator();

                                let playing = self.slideshow.is_some();
                                if small_button(ui, if playing { "Pause" } else { "Play" })
                                    .clicked()
                                {
                                    actions.push(UiAction::ToggleSlideshow);
                                }
                                if small_button(ui, "−s").clicked() {
                                    actions.push(UiAction::SlideshowSlower);
                                }
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{}s",
                                        self.slideshow_interval.as_secs()
                                    ))
                                    .size(12.0)
                                    .color(egui::Color32::from_gray(185)),
                                );
                                if small_button(ui, "+s").clicked() {
                                    actions.push(UiAction::SlideshowFaster);
                                }
                                ui.separator();

                                if small_button(ui, "−").clicked() {
                                    actions.push(UiAction::ZoomOut);
                                }

                                let (min_zoom, max_zoom) = if self.camera.has_image() {
                                    (
                                        (self.camera.min_scale * 100.0).max(1.0),
                                        (self.camera.max_scale * 100.0).max(2.0),
                                    )
                                } else {
                                    (10.0, 400.0)
                                };
                                let mut zoom_value = if self.camera.has_image() {
                                    self.camera.zoom_percent().clamp(min_zoom, max_zoom)
                                } else {
                                    100.0
                                };
                                ui.spacing_mut().slider_width = 90.0;
                                let slider =
                                    egui::Slider::new(&mut zoom_value, min_zoom..=max_zoom)
                                        .logarithmic(true)
                                        .show_value(false);
                                if ui.add(slider).changed() {
                                    actions.push(UiAction::SetZoom(zoom_value / 100.0));
                                }

                                if small_button(ui, "+").clicked() {
                                    actions.push(UiAction::ZoomIn);
                                }

                                if small_button(ui, "Fit").clicked() {
                                    actions.push(UiAction::Fit);
                                }
                                if small_button(ui, "1:1").clicked() {
                                    actions.push(UiAction::ActualSize);
                                }
                                if small_button(ui, "Rot -").clicked() {
                                    actions.push(UiAction::RotatePersist(false));
                                }
                                if small_button(ui, "Rot +").clicked() {
                                    actions.push(UiAction::RotatePersist(true));
                                }
                                if small_button(ui, "Fix").clicked() {
                                    actions.push(UiAction::ToggleFixes);
                                }
                                if small_button(ui, "Share").clicked() {
                                    actions.push(UiAction::ToggleShare);
                                }
                                if small_button(ui, "Info").clicked() {
                                    actions.push(UiAction::ToggleInfo);
                                }
                                if small_button(ui, "Delete").clicked() {
                                    actions.push(UiAction::Delete);
                                }
                                // Windowed mode uses the native title-bar
                                // close button instead.
                                if !windowed && small_button(ui, "X").clicked() {
                                    actions.push(UiAction::Close);
                                }
                            });
                        });
                });
        }

        let close_alpha = self.hud_alpha;
        if !windowed && close_alpha > 0.01 {
            egui::Area::new(egui::Id::unique("viewer-close"))
                .anchor(egui::Align2::RIGHT_TOP, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    // A quarter disc tucked into the top-right corner, with a
                    // cross centred in it.
                    const SIZE: f32 = 58.0;
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(SIZE, SIZE), egui::Sense::click());
                    let pointer_inside = response
                        .hover_pos()
                        .is_some_and(|pos| (pos - rect.right_top()).length() <= SIZE);
                    let response = response.on_hover_text("Close viewer");
                    let fill_alpha = if pointer_inside { 205.0 } else { 160.0 };
                    let fill = egui::Color32::from_black_alpha((fill_alpha * close_alpha) as u8);
                    let painter = ui.painter().with_clip_rect(rect);
                    painter.circle_filled(rect.right_top(), SIZE, fill);

                    let center = rect.right_top() + egui::vec2(-SIZE * 0.42, SIZE * 0.42);
                    let arm = 10.0;
                    let stroke = egui::Stroke::new(
                        2.0,
                        egui::Color32::from_white_alpha((235.0 * close_alpha) as u8),
                    );
                    painter.line_segment(
                        [
                            center + egui::vec2(-arm, -arm),
                            center + egui::vec2(arm, arm),
                        ],
                        stroke,
                    );
                    painter.line_segment(
                        [
                            center + egui::vec2(-arm, arm),
                            center + egui::vec2(arm, -arm),
                        ],
                        stroke,
                    );

                    let clicked_inside = response.clicked()
                        && response
                            .interact_pointer_pos()
                            .is_some_and(|pos| (pos - rect.right_top()).length() <= SIZE);
                    if clicked_inside {
                        actions.push(UiAction::Close);
                    }
                });
        }

        if show_filmstrip {
            let alpha = self.hud_alpha;
            let mut scroll_to_current = self.scroll_filmstrip;
            let mut hovered = false;
            let start = current_index.saturating_sub(THUMB_RADIUS);
            let end = (current_index + THUMB_RADIUS + 1).min(self.nav.len());
            let uv = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0));

            egui::Area::new(egui::Id::unique("filmstrip"))
                .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0.0, -12.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha((180.0 * alpha) as u8))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_white_alpha((18.0 * alpha) as u8),
                        ))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::symmetric(9, 7))
                        .show(ui, |ui| {
                            ui.set_opacity(alpha);
                            ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
                            egui::ScrollArea::horizontal()
                                .id_salt("filmstrip_scroll")
                                .scroll_bar_visibility(
                                    egui::scroll_area::ScrollBarVisibility::AlwaysHidden,
                                )
                                .scroll_source(egui::scroll_area::ScrollSource::ALL)
                                .on_hover_cursor(egui::CursorIcon::Grab)
                                .on_drag_cursor(egui::CursorIcon::Grabbing)
                                .max_height(76.0)
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        for index in start..end {
                                            let Some(path) = self.nav.path(index) else {
                                                continue;
                                            };
                                            let selected = index == current_index;
                                            let (rect, response) = ui.allocate_exact_size(
                                                egui::vec2(64.0, 64.0),
                                                egui::Sense::click(),
                                            );

                                            if let Some(texture) = self.thumbs.get(path) {
                                                let texture_size = texture.size_vec2();
                                                let scale = (64.0 / texture_size.x.max(1.0))
                                                    .min(64.0 / texture_size.y.max(1.0))
                                                    .min(1.0);
                                                let draw_size = texture_size * scale;
                                                let draw_rect = egui::Rect::from_center_size(
                                                    rect.center(),
                                                    draw_size,
                                                );
                                                ui.painter().image(
                                                    texture.id(),
                                                    draw_rect,
                                                    uv,
                                                    egui::Color32::WHITE,
                                                );
                                            } else {
                                                ui.painter().rect_filled(
                                                    rect,
                                                    6.0,
                                                    egui::Color32::from_gray(28),
                                                );
                                                let extension = path
                                                    .extension()
                                                    .and_then(|extension| extension.to_str())
                                                    .unwrap_or("?")
                                                    .to_uppercase();
                                                ui.painter().text(
                                                    rect.center(),
                                                    egui::Align2::CENTER_CENTER,
                                                    extension,
                                                    egui::FontId::proportional(9.0),
                                                    egui::Color32::from_gray(105),
                                                );
                                            }

                                            if selected {
                                                ui.painter().rect_stroke(
                                                    rect.expand(1.0),
                                                    7.0,
                                                    egui::Stroke::new(
                                                        2.0,
                                                        egui::Color32::from_rgb(118, 186, 255),
                                                    ),
                                                    egui::StrokeKind::Outside,
                                                );
                                            } else if response.hovered() {
                                                ui.painter().rect_stroke(
                                                    rect,
                                                    6.0,
                                                    egui::Stroke::new(
                                                        1.0,
                                                        egui::Color32::from_white_alpha(55),
                                                    ),
                                                    egui::StrokeKind::Inside,
                                                );
                                            }

                                            let clicked = response.clicked();
                                            let item_hovered = response.hovered();
                                            hovered |= item_hovered;

                                            if selected && scroll_to_current {
                                                response.scroll_to_me(Some(egui::Align::Center));
                                                scroll_to_current = false;
                                            }

                                            if clicked {
                                                actions.push(UiAction::Navigate(index));
                                            }

                                            let name = path
                                                .file_name()
                                                .unwrap_or_default()
                                                .to_string_lossy()
                                                .to_string();
                                            response.on_hover_text(name);
                                        }
                                    });
                                });
                        });
                });

            self.scroll_filmstrip = scroll_to_current;
            self.filmstrip_hovered = hovered;
        } else {
            self.filmstrip_hovered = false;
        }

        if loading {
            let file_name = current_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            let thumb = self.thumbs.get(&current_path);
            egui::Area::new(egui::Id::unique("loading"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(150))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(18))
                        .show(ui, |ui| {
                            ui.vertical_centered(|ui| {
                                if let Some(texture) = thumb {
                                    let texture_size = texture.size_vec2();
                                    let max_width = screen_size.x * 0.36;
                                    let max_height = screen_size.y * 0.36;
                                    let scale = (max_width / texture_size.x.max(1.0))
                                        .min(max_height / texture_size.y.max(1.0))
                                        .min(1.0);
                                    let draw_size = texture_size * scale;
                                    let (rect, _) =
                                        ui.allocate_exact_size(draw_size, egui::Sense::hover());
                                    ui.painter().image(
                                        texture.id(),
                                        rect,
                                        egui::Rect::from_min_max(
                                            egui::pos2(0.0, 0.0),
                                            egui::pos2(1.0, 1.0),
                                        ),
                                        egui::Color32::from_white_alpha(190),
                                    );
                                    ui.add_space(10.0);
                                }
                                ui.add(
                                    egui::Spinner::new()
                                        .size(22.0)
                                        .color(egui::Color32::from_rgb(118, 186, 255)),
                                );
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(format!("Loading {file_name}…"))
                                        .size(12.0)
                                        .color(egui::Color32::from_gray(200)),
                                );
                            });
                        });
                });
        }

        if let Some(message) = error {
            let file_name = current_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            egui::Area::new(egui::Id::unique("error"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(190))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(180, 80, 80)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(18))
                        .show(ui, |ui| {
                            ui.set_max_width(460.0);
                            ui.label(
                                egui::RichText::new(format!("Could not open {file_name}"))
                                    .size(14.0)
                                    .strong(),
                            );
                            ui.add_space(6.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(message)
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(170)),
                                )
                                .wrap(),
                            );
                            ui.add_space(8.0);
                            ui.label(
                                egui::RichText::new("Press Esc to close, or F5 to retry.")
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(130)),
                            );
                        });
                });
        }

        if let Some((file_name, clockwise)) = rotation {
            let direction = if clockwise { "right" } else { "left" };
            egui::Area::new(egui::Id::unique("rotate-confirm"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(210))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(30)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(18))
                        .show(ui, |ui| {
                            ui.set_max_width(430.0);
                            ui.label(
                                egui::RichText::new(format!("Rotate {file_name} {direction}?"))
                                    .size(14.0)
                                    .strong(),
                            );
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new(
                                    "The EXIF orientation tag will be rewritten in the file. \
                                     The compressed pixels are not re-encoded.",
                                )
                                .size(11.0)
                                .color(egui::Color32::from_gray(170)),
                            );
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if small_button(ui, "Rotate and save").clicked() {
                                    actions.push(UiAction::ConfirmRotation);
                                }
                                if small_button(ui, "Cancel").clicked() {
                                    actions.push(UiAction::CancelRotation);
                                }
                            });
                        });
                });
        }

        if self.show_fixes {
            let mut adjustments = self.adjustments;
            let mut changed = false;
            egui::Area::new(egui::Id::unique("fix-panel"))
                .anchor(egui::Align2::LEFT_TOP, egui::vec2(18.0, 18.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(205))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(24)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(16))
                        .show(ui, |ui| {
                            ui.set_max_width(320.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Fixes").size(14.0).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if small_button(ui, "Close").clicked() {
                                            actions.push(UiAction::ToggleFixes);
                                        }
                                    },
                                );
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new("One-click looks")
                                    .size(10.0)
                                    .color(egui::Color32::from_gray(135)),
                            );
                            ui.horizontal_wrapped(|ui| {
                                for (label, preset) in [
                                    ("Auto", crate::adjust::Adjustments::auto()),
                                    ("Lighten", crate::adjust::Adjustments::lighten()),
                                    ("Darken", crate::adjust::Adjustments::darken()),
                                    ("Warm", crate::adjust::Adjustments::warm()),
                                    ("Cool", crate::adjust::Adjustments::cool()),
                                ] {
                                    if small_button(ui, label).clicked() {
                                        adjustments = preset;
                                        changed = true;
                                    }
                                }
                            });
                            ui.add_space(8.0);
                            ui.separator();
                            ui.add_space(4.0);
                            ui.spacing_mut().slider_width = 190.0;

                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut adjustments.brightness, -0.3..=0.3)
                                        .text("Brightness"),
                                )
                                .changed();
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut adjustments.contrast, 0.25..=2.5)
                                        .text("Contrast"),
                                )
                                .changed();
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut adjustments.gamma, 0.2..=3.0)
                                        .text("Gamma"),
                                )
                                .changed();
                            changed |= ui
                                .add(egui::Slider::new(&mut adjustments.red, 0.5..=1.5).text("Red"))
                                .changed();
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut adjustments.green, 0.5..=1.5)
                                        .text("Green"),
                                )
                                .changed();
                            changed |= ui
                                .add(
                                    egui::Slider::new(&mut adjustments.blue, 0.5..=1.5)
                                        .text("Blue"),
                                )
                                .changed();

                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(!adjustments.is_neutral(), |ui| {
                                    if small_button(ui, "Save").clicked() {
                                        actions.push(UiAction::Save {
                                            mode: SaveMode::Overwrite,
                                            adjustments,
                                        });
                                    }
                                    if small_button(ui, "Save a copy").clicked() {
                                        actions.push(UiAction::Save {
                                            mode: SaveMode::Copy,
                                            adjustments,
                                        });
                                    }
                                });
                                if small_button(ui, "Revert").clicked() {
                                    adjustments = crate::adjust::Adjustments::default();
                                    changed = true;
                                }
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(if adjustments.is_neutral() {
                                    "No adjustments"
                                } else {
                                    "Preview only — Save applies it to the file"
                                })
                                .size(10.0)
                                .color(egui::Color32::from_gray(150)),
                            );
                        });
                });
            if changed {
                actions.push(UiAction::SetAdjustments(adjustments));
            }
        }

        if self.show_share {
            let file_name = current_path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            egui::Area::new(egui::Id::unique("share-panel"))
                .anchor(egui::Align2::LEFT_BOTTOM, egui::vec2(18.0, -18.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(205))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(24)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(16))
                        .show(ui, |ui| {
                            ui.set_max_width(300.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Share").size(14.0).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if small_button(ui, "Close").clicked() {
                                            actions.push(UiAction::ToggleShare);
                                        }
                                    },
                                );
                            });
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(truncate(&file_name, 42))
                                    .size(11.0)
                                    .color(egui::Color32::from_gray(170)),
                            );
                            ui.add_space(8.0);
                            ui.horizontal_wrapped(|ui| {
                                if small_button(ui, "Copy image").clicked() {
                                    actions.push(UiAction::CopyImage);
                                }
                                if small_button(ui, "Copy path").clicked() {
                                    actions.push(UiAction::CopyPath);
                                }
                            });
                            ui.add_space(4.0);
                            ui.horizontal_wrapped(|ui| {
                                if small_button(ui, "Open").clicked() {
                                    actions.push(UiAction::OpenDefault);
                                }
                                if small_button(ui, "Show in folder").clicked() {
                                    actions.push(UiAction::Reveal);
                                }
                                if small_button(ui, "Print").clicked() {
                                    actions.push(UiAction::Print);
                                }
                            });
                            ui.add_space(6.0);
                            ui.label(
                                egui::RichText::new(
                                    "Copy image copies the current view, including fixes. \
                                     Ctrl+C / Ctrl+Shift+C / Ctrl+E / Ctrl+Enter / Ctrl+P.",
                                )
                                .size(10.0)
                                .color(egui::Color32::from_gray(145)),
                            );
                        });
                });
        }

        if self.show_info {
            egui::Area::new(egui::Id::unique("info-panel"))
                .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-18.0, 64.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(205))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(24)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(16))
                        .show(ui, |ui| {
                            ui.set_max_width(340.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new("Photo info").size(14.0).strong());
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if small_button(ui, "Close").clicked() {
                                            actions.push(UiAction::ToggleInfo);
                                        }
                                    },
                                );
                            });
                            ui.add_space(6.0);

                            if let Some(info) = &self.photo_info {
                                egui::Grid::new("photo-info-grid")
                                    .num_columns(2)
                                    .spacing([12.0, 5.0])
                                    .show(ui, |ui| {
                                        let mut row = |label: &str, value: &str| {
                                            ui.label(
                                                egui::RichText::new(label)
                                                    .size(10.0)
                                                    .color(egui::Color32::from_gray(135)),
                                            );
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(value)
                                                        .size(12.0)
                                                        .color(egui::Color32::from_gray(225)),
                                                )
                                                .wrap(),
                                            );
                                            ui.end_row();
                                        };

                                        row("File", &info.file_name);
                                        if let Some((width, height)) = info.dimensions {
                                            row("Dimensions", &format!("{width} × {height}"));
                                        }
                                        if let Some(size) = info.file_size {
                                            row("Size", &crate::info::format_file_size(size));
                                        }
                                        if let Some(date) = &info.date {
                                            row("Date", date);
                                        }
                                        if let Some(camera) = &info.camera {
                                            row("Camera", camera);
                                        }
                                        if let Some(lens) = &info.lens {
                                            row("Lens", lens);
                                        }
                                        if let Some(exposure) = &info.exposure {
                                            row("Exposure", exposure);
                                        }
                                        if let Some(aperture) = &info.aperture {
                                            row("Aperture", aperture);
                                        }
                                        if let Some(iso) = info.iso {
                                            row("ISO", &iso.to_string());
                                        }
                                        if let Some(focal) = &info.focal_length {
                                            row("Focal length", focal);
                                        }
                                        if let Some(gps) = &info.gps {
                                            row("GPS", gps);
                                        }
                                    });
                            } else {
                                ui.label(
                                    egui::RichText::new("Reading metadata…")
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(160)),
                                );
                            }
                        });
                });
        }

        if let Some(file_name) = deletion {
            egui::Area::new(egui::Id::unique("delete-confirm"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(210))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(30)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(18))
                        .show(ui, |ui| {
                            ui.set_max_width(430.0);
                            ui.label(
                                egui::RichText::new(format!(
                                    "Move {file_name} to the Recycle Bin?"
                                ))
                                .size(14.0)
                                .strong(),
                            );
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new(
                                    "You can restore the file from the Recycle Bin. \
                                     The viewer advances to the next photo.",
                                )
                                .size(11.0)
                                .color(egui::Color32::from_gray(170)),
                            );
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if small_button(ui, "Delete").clicked() {
                                    actions.push(UiAction::ConfirmDelete);
                                }
                                if small_button(ui, "Cancel").clicked() {
                                    actions.push(UiAction::CancelDelete);
                                }
                            });
                        });
                });
        }

        if let Some(file_name) = saving {
            egui::Area::new(egui::Id::unique("save-confirm"))
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(210))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(30)))
                        .corner_radius(egui::CornerRadius::same(16))
                        .inner_margin(egui::Margin::same(18))
                        .show(ui, |ui| {
                            ui.set_max_width(440.0);
                            ui.label(
                                egui::RichText::new(format!("Save color fixes to {file_name}?"))
                                    .size(14.0)
                                    .strong(),
                            );
                            ui.add_space(5.0);
                            ui.label(
                                egui::RichText::new(
                                    "The adjusted pixels replace the original file. \
                                     JPEG metadata and orientation are preserved. \
                                     Use Save a copy to keep the original unchanged.",
                                )
                                .size(11.0)
                                .color(egui::Color32::from_gray(170)),
                            );
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if small_button(ui, "Save").clicked() {
                                    actions.push(UiAction::ConfirmSave);
                                }
                                if small_button(ui, "Cancel").clicked() {
                                    actions.push(UiAction::CancelSave);
                                }
                            });
                        });
                });
        }

        if let Some(text) = toast {
            egui::Area::new(egui::Id::unique("toast"))
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 64.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha(200))
                        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(190, 92, 92)))
                        .corner_radius(egui::CornerRadius::same(12))
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.set_max_width(520.0);
                            ui.label(
                                egui::RichText::new(text)
                                    .size(12.0)
                                    .color(egui::Color32::from_gray(225)),
                            );
                        });
                });
        }

        if show_help {
            let remaining = self.show_help_until.saturating_duration_since(now);
            let fade = (remaining.as_secs_f32() / 0.6).clamp(0.0, 1.0);
            egui::Area::new(egui::Id::unique("help"))
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 22.0))
                .order(egui::Order::Foreground)
                .show(ctx, |ui| {
                    egui::Frame::NONE
                        .fill(egui::Color32::from_black_alpha((155.0 * fade) as u8))
                        .stroke(egui::Stroke::new(
                            1.0,
                            egui::Color32::from_white_alpha((18.0 * fade) as u8),
                        ))
                        .corner_radius(egui::CornerRadius::same(14))
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.set_opacity(fade);
                            ui.label(
                                egui::RichText::new(
                                    "Scroll: zoom   |   Left/Right: browse   |   Space: slideshow   |   F: fixes   |   I: info   |   C: share   |   Del: recycle   |   Esc: close panel / stop / exit   |   Drop a file or folder to open",
                                )
                                .size(12.0)
                                .color(egui::Color32::from_gray(215)),
                            );
                        });
                });
        }

        if self.drag_hover {
            let rect = ctx.viewport_rect();
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::unique("drop-hint"),
            ));
            painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(120));

            let card = egui::Rect::from_center_size(rect.center(), egui::vec2(380.0, 82.0));
            painter.rect_filled(card, 16.0, egui::Color32::from_black_alpha(225));
            painter.rect_stroke(
                card,
                egui::CornerRadius::same(16),
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(45)),
                egui::StrokeKind::Inside,
            );
            painter.text(
                card.center() + egui::vec2(0.0, -10.0),
                egui::Align2::CENTER_CENTER,
                "Drop to open",
                egui::FontId::proportional(18.0),
                egui::Color32::from_gray(240),
            );
            painter.text(
                card.center() + egui::vec2(0.0, 15.0),
                egui::Align2::CENTER_CENTER,
                "A photo, or the first image in a folder",
                egui::FontId::proportional(12.0),
                egui::Color32::from_gray(175),
            );
        }

        actions
    }
}

fn small_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(12.0))
            .fill(egui::Color32::from_white_alpha(14))
            .corner_radius(egui::CornerRadius::same(9))
            .min_size(egui::vec2(0.0, 24.0)),
    )
}
