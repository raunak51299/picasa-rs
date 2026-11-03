//! Cursor-anchored zoom, pan, and rotation with frame-rate-independent smoothing.

use std::f32::consts::{PI, TAU};

const FIT_WIDTH_FRACTION: f32 = 0.88;
const FIT_HEIGHT_FRACTION: f32 = 0.80;
const ZOOM_SMOOTHING: f32 = 14.0;
const ROTATION_SMOOTHING: f32 = 11.0;
const MAX_SCALE: f32 = 16.0;

#[derive(Clone, Copy, Debug, Default)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn length(self) -> f32 {
        self.x.hypot(self.y)
    }

    pub fn distance(self, other: Self) -> f32 {
        (self.x - other.x).hypot(self.y - other.y)
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    fn add(self, rhs: Self) -> Self::Output {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::AddAssign for Vec2 {
    fn add_assign(&mut self, rhs: Self) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, rhs: Self) -> Self::Output {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, rhs: f32) -> Self::Output {
        Vec2::new(self.x * rhs, self.y * rhs)
    }
}

/// 2D camera used by the photo renderer.
///
/// Image-space coordinates are measured in source pixels with the origin at
/// the top-left of the *oriented* image. Screen-space coordinates are physical
/// window pixels with the origin at the top-left of the window.
pub struct Camera {
    pub viewport: Vec2,
    pub image_size: Vec2,
    pub scale: f32,
    pub target_scale: f32,
    pub offset: Vec2,
    pub target_offset: Vec2,
    pub angle: f32,
    pub target_angle: f32,
    pub fit_mode: bool,
    pub min_scale: f32,
    pub max_scale: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            viewport: Vec2::new(1.0, 1.0),
            image_size: Vec2::default(),
            scale: 1.0,
            target_scale: 1.0,
            offset: Vec2::default(),
            target_offset: Vec2::default(),
            angle: 0.0,
            target_angle: 0.0,
            fit_mode: true,
            min_scale: 0.02,
            max_scale: MAX_SCALE,
        }
    }
}

impl Camera {
    pub fn set_viewport(&mut self, width: f32, height: f32) {
        self.viewport = Vec2::new(width.max(1.0), height.max(1.0));
        if self.fit_mode {
            self.target_scale = self.fit_scale(self.target_angle);
            self.target_offset = Vec2::default();
        }
        self.clamp_offsets();
    }

    pub fn set_image(&mut self, width: f32, height: f32) {
        self.image_size = Vec2::new(width.max(1.0), height.max(1.0));
        self.angle = 0.0;
        self.target_angle = 0.0;
        self.fit_mode = true;
        self.scale = self.fit_scale(0.0);
        self.target_scale = self.scale;
        self.offset = Vec2::default();
        self.target_offset = Vec2::default();
        self.min_scale = (self.scale * 0.25).max(0.01);
        self.clamp_offsets();
    }

    pub fn clear_image(&mut self) {
        self.image_size = Vec2::default();
        self.scale = 1.0;
        self.target_scale = 1.0;
        self.offset = Vec2::default();
        self.target_offset = Vec2::default();
        self.angle = 0.0;
        self.target_angle = 0.0;
        self.fit_mode = true;
    }

    pub fn has_image(&self) -> bool {
        self.image_size.x > 1.0 && self.image_size.y > 1.0
    }

    /// Scale at which the rotated image fits comfortably inside the viewport.
    pub fn fit_scale(&self, angle: f32) -> f32 {
        if !self.has_image() {
            return 1.0;
        }
        let (sin, cos) = angle.sin_cos();
        let rotated_w = self.image_size.x * cos.abs() + self.image_size.y * sin.abs();
        let rotated_h = self.image_size.x * sin.abs() + self.image_size.y * cos.abs();

        let available_w = self.viewport.x * FIT_WIDTH_FRACTION;
        let available_h = self.viewport.y * FIT_HEIGHT_FRACTION;
        let scale = (available_w / rotated_w.max(0.001)).min(available_h / rotated_h.max(0.001));

        // Do not enlarge small images beyond their natural size.
        scale.clamp(0.001, 1.0)
    }

    fn rotated_offset(&self, offset: Vec2, angle: f32) -> Vec2 {
        let (sin, cos) = angle.sin_cos();
        Vec2::new(
            offset.x * cos - offset.y * sin,
            offset.x * sin + offset.y * cos,
        )
    }

    /// Apply an exponential zoom step anchored at a screen-space cursor point.
    pub fn zoom_at(&mut self, cursor: Vec2, factor: f32) {
        if !self.has_image() {
            return;
        }
        let new_scale = (self.target_scale * factor).clamp(self.min_scale, self.max_scale);
        self.set_target_scale_at(new_scale, cursor);
    }

    /// Set the target scale, keeping the image point under `cursor` fixed.
    pub fn set_target_scale_at(&mut self, new_scale: f32, cursor: Vec2) {
        if !self.has_image() {
            return;
        }
        let new_scale = new_scale.clamp(self.min_scale, self.max_scale);
        let center = self.viewport * 0.5 + self.target_offset;
        let from_center = cursor - center;
        let local =
            self.rotated_offset(from_center, -self.target_angle) * (1.0 / self.target_scale);
        let new_center = cursor - self.rotated_offset(local * new_scale, self.target_angle);

        self.target_scale = new_scale;
        self.target_offset = new_center - self.viewport * 0.5;
        self.fit_mode = false;
        self.clamp_offsets();
    }

    pub fn toggle_actual_size(&mut self, cursor: Vec2) {
        if !self.has_image() {
            return;
        }
        if !self.fit_mode && (self.target_scale - 1.0).abs() < 0.01 {
            self.reset_fit();
        } else {
            self.set_target_scale_at(1.0, cursor);
        }
    }

    pub fn reset_fit(&mut self) {
        self.fit_mode = true;
        self.target_scale = self.fit_scale(self.target_angle);
        self.target_offset = Vec2::default();
    }

    pub fn rotate(&mut self, delta: f32) {
        self.target_angle += delta;
        // Keep the angle bounded without changing its visual orientation.
        if self.target_angle.abs() > TAU * 64.0 {
            self.target_angle %= TAU;
            self.angle %= TAU;
        }
        if self.fit_mode {
            self.target_scale = self.fit_scale(self.target_angle);
            self.target_offset = Vec2::default();
        }
        self.clamp_offsets();
    }

    /// Set the absolute view rotation without animating. Used when a photo is
    /// selected or finishes loading so its session rotation appears at once.
    pub fn set_rotation(&mut self, angle: f32) {
        let angle = angle.rem_euclid(TAU);
        self.angle = angle;
        self.target_angle = angle;
        if self.fit_mode {
            self.scale = self.fit_scale(angle);
            self.target_scale = self.scale;
            self.offset = Vec2::default();
            self.target_offset = Vec2::default();
        }
        self.clamp_offsets();
    }

    pub fn pan_by(&mut self, delta: Vec2) {
        if !self.has_image() {
            return;
        }
        self.target_offset += delta;
        // Panning should feel direct rather than lagging behind the pointer.
        self.offset = self.target_offset;
        self.clamp_offsets();
    }

    pub fn update(&mut self, dt: f32) -> bool {
        if !self.has_image() {
            return false;
        }

        let zoom_t = 1.0 - (-ZOOM_SMOOTHING * dt).exp();
        let rotate_t = 1.0 - (-ROTATION_SMOOTHING * dt).exp();

        self.scale += (self.target_scale - self.scale) * zoom_t;
        self.offset.x += (self.target_offset.x - self.offset.x) * zoom_t;
        self.offset.y += (self.target_offset.y - self.offset.y) * zoom_t;
        self.angle += (self.target_angle - self.angle) * rotate_t;

        if (self.target_scale - self.scale).abs() < 0.0005 {
            self.scale = self.target_scale;
        }
        if self.offset.distance(self.target_offset) < 0.08 {
            self.offset = self.target_offset;
        }
        if (self.target_angle - self.angle).abs() < 0.0005 {
            self.angle = self.target_angle;
        }

        self.clamp_offsets();
        self.is_animating()
    }

    pub fn is_animating(&self) -> bool {
        (self.target_scale - self.scale).abs() > 0.0004
            || self.offset.distance(self.target_offset) > 0.05
            || (self.target_angle - self.angle).abs() > 0.0004
    }

    /// Image-space dimensions of the rotated image.
    pub fn rotated_extents(&self, angle: f32, scale: f32) -> Vec2 {
        let (sin, cos) = angle.sin_cos();
        Vec2::new(
            (self.image_size.x * cos.abs() + self.image_size.y * sin.abs()) * scale,
            (self.image_size.x * sin.abs() + self.image_size.y * cos.abs()) * scale,
        )
    }

    fn clamp_offset_value(&self, offset: Vec2, angle: f32, scale: f32) -> Vec2 {
        let extents = self.rotated_extents(angle, scale);
        let max_x = ((extents.x - self.viewport.x) * 0.5).max(0.0);
        let max_y = ((extents.y - self.viewport.y) * 0.5).max(0.0);
        Vec2::new(offset.x.clamp(-max_x, max_x), offset.y.clamp(-max_y, max_y))
    }

    pub fn clamp_offsets(&mut self) {
        self.offset = self.clamp_offset_value(self.offset, self.angle, self.scale);
        self.target_offset =
            self.clamp_offset_value(self.target_offset, self.target_angle, self.target_scale);
    }

    /// Screen-space center of the image.
    pub fn screen_center(&self) -> Vec2 {
        self.viewport * 0.5 + self.offset
    }

    /// Screen-space half extents of the rotated, scaled image.
    pub fn screen_half_size(&self) -> Vec2 {
        self.rotated_extents(self.angle, self.scale) * 0.5
    }

    /// Convert a screen point into oriented image-space pixels.
    pub fn screen_to_image(&self, screen: Vec2) -> Vec2 {
        let from_center = screen - self.screen_center();
        let unrotated = self.rotated_offset(from_center, -self.angle);
        unrotated * (1.0 / self.scale) + self.image_size * 0.5
    }

    pub fn contains_screen_point(&self, screen: Vec2) -> bool {
        if !self.has_image() {
            return false;
        }
        let point = self.screen_to_image(screen);
        point.x >= 0.0
            && point.y >= 0.0
            && point.x <= self.image_size.x
            && point.y <= self.image_size.y
    }

    pub fn zoom_percent(&self) -> f32 {
        self.scale * 100.0
    }

    /// Nearest multiple of 90 degrees, used for the HUD readout.
    pub fn rotation_degrees(&self) -> i32 {
        ((self.target_angle / PI * 180.0).round() as i32).rem_euclid(360)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    fn camera() -> Camera {
        let mut camera = Camera::default();
        camera.set_viewport(1200.0, 900.0);
        camera.set_image(1000.0, 800.0);
        camera
    }

    fn settle(camera: &mut Camera) {
        for _ in 0..2000 {
            if !camera.update(1.0 / 60.0) {
                return;
            }
        }
        panic!("camera did not settle");
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor_fixed() {
        let mut camera = camera();
        let cursor = Vec2::new(700.0, 500.0);
        let before = camera.screen_to_image(cursor);

        camera.set_target_scale_at(camera.target_scale * 2.0, cursor);
        settle(&mut camera);

        let after = camera.screen_to_image(cursor);
        assert!((after.x - before.x).abs() < 0.01, "{after:?} != {before:?}");
        assert!((after.y - before.y).abs() < 0.01, "{after:?} != {before:?}");
    }

    #[test]
    fn damping_converges_without_overshoot() {
        let mut camera = camera();
        camera.set_target_scale_at(camera.target_scale * 2.0, Vec2::new(600.0, 450.0));

        let mut previous = camera.scale;
        for _ in 0..30 {
            camera.update(1.0 / 60.0);
            assert!(camera.scale >= previous - 1e-6);
            assert!(camera.scale <= camera.target_scale + 1e-6);
            previous = camera.scale;
        }

        settle(&mut camera);
        assert!((camera.scale - camera.target_scale).abs() < 1e-5);
        assert!(!camera.is_animating());
    }

    #[test]
    fn smoothing_is_frame_rate_independent() {
        let mut slow = camera();
        let mut fast = camera();
        for camera in [&mut slow, &mut fast] {
            camera.set_target_scale_at(camera.target_scale * 2.0, Vec2::new(400.0, 300.0));
            camera.rotate(FRAC_PI_2);
        }

        for _ in 0..60 {
            slow.update(1.0 / 60.0);
        }
        for _ in 0..144 {
            fast.update(1.0 / 144.0);
        }

        assert!((slow.scale - fast.scale).abs() < 1e-3);
        assert!((slow.offset.x - fast.offset.x).abs() < 0.01);
        assert!((slow.offset.y - fast.offset.y).abs() < 0.01);
        assert!((slow.angle - fast.angle).abs() < 1e-3);
    }

    #[test]
    fn rotation_refits_and_tracks_the_angle() {
        let mut camera = camera();
        let fit_scale = camera.target_scale;

        camera.rotate(FRAC_PI_2);
        assert!((camera.target_angle - FRAC_PI_2).abs() < 1e-6);
        assert!(camera.target_scale < fit_scale);

        settle(&mut camera);
        assert_eq!(camera.rotation_degrees(), 90);
        let extents = camera.rotated_extents(camera.angle, camera.scale);
        assert!((extents.x - camera.image_size.y * camera.scale).abs() < 0.01);
        assert!((extents.y - camera.image_size.x * camera.scale).abs() < 0.01);

        camera.rotate(-FRAC_PI_2);
        settle(&mut camera);
        assert_eq!(camera.rotation_degrees(), 0);
        assert!((camera.scale - fit_scale).abs() < 0.01);
    }

    #[test]
    fn set_rotation_snaps_and_refits() {
        let mut camera = camera();
        let unrotated = camera.scale;

        camera.set_rotation(FRAC_PI_2);

        assert_eq!(camera.rotation_degrees(), 90);
        assert!((camera.angle - FRAC_PI_2).abs() < 1e-6);
        assert!((camera.target_angle - FRAC_PI_2).abs() < 1e-6);
        assert!(camera.scale < unrotated);
        assert!((camera.scale - camera.fit_scale(FRAC_PI_2)).abs() < 1e-6);
        assert!(!camera.is_animating());
    }

    #[test]
    fn reset_fit_keeps_the_view_rotation() {
        let mut camera = camera();
        camera.rotate(FRAC_PI_2);
        settle(&mut camera);

        camera.reset_fit();
        settle(&mut camera);

        assert_eq!(camera.rotation_degrees(), 90);
        assert!((camera.scale - camera.fit_scale(FRAC_PI_2)).abs() < 0.01);
    }
}
