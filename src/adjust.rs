//! Non-destructive photo adjustments shared by the GPU shader and saving.

use rayon::prelude::*;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Adjustments {
    /// Additive brightness in linear light, roughly -0.3..=0.3.
    pub brightness: f32,
    /// Contrast around mid-grey, 1.0 is neutral.
    pub contrast: f32,
    /// Midtone gamma, 1.0 is neutral.
    pub gamma: f32,
    pub red: f32,
    pub green: f32,
    pub blue: f32,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self {
            brightness: 0.0,
            contrast: 1.0,
            gamma: 1.0,
            red: 1.0,
            green: 1.0,
            blue: 1.0,
        }
    }
}

impl Adjustments {
    pub fn is_neutral(self) -> bool {
        (self.brightness.abs() < 1e-4)
            && (self.contrast - 1.0).abs() < 1e-4
            && (self.gamma - 1.0).abs() < 1e-4
            && (self.red - 1.0).abs() < 1e-4
            && (self.green - 1.0).abs() < 1e-4
            && (self.blue - 1.0).abs() < 1e-4
    }

    pub fn uniforms(self) -> ([f32; 4], [f32; 4]) {
        (
            [self.brightness, self.contrast, self.gamma, 0.0],
            [self.red, self.green, self.blue, 0.0],
        )
    }

    pub fn auto() -> Self {
        Self {
            contrast: 1.15,
            gamma: 1.05,
            ..Self::default()
        }
    }

    pub fn lighten() -> Self {
        Self {
            brightness: 0.08,
            gamma: 1.18,
            ..Self::default()
        }
    }

    pub fn darken() -> Self {
        Self {
            brightness: -0.08,
            gamma: 0.85,
            ..Self::default()
        }
    }

    pub fn warm() -> Self {
        Self {
            red: 1.08,
            blue: 0.93,
            ..Self::default()
        }
    }

    pub fn cool() -> Self {
        Self {
            red: 0.93,
            blue: 1.08,
            ..Self::default()
        }
    }

    pub fn clamp(&mut self) {
        self.brightness = self.brightness.clamp(-0.3, 0.3);
        self.contrast = self.contrast.clamp(0.25, 2.5);
        self.gamma = self.gamma.clamp(0.2, 3.0);
        self.red = self.red.clamp(0.5, 1.5);
        self.green = self.green.clamp(0.5, 1.5);
        self.blue = self.blue.clamp(0.5, 1.5);
    }
}

/// Linear-light value of sRGB mid-grey (0.5); contrast pivots around it. Keep
/// in sync with `photo.wgsl`.
const CONTRAST_PIVOT: f32 = 0.214_041;

/// Apply adjustments to an sRGB RGBA8 buffer. This mirrors `photo.wgsl`, which
/// receives linear values because the photo texture uses an sRGB format.
pub fn apply_to_rgba(rgba: &mut [u8], adjustments: Adjustments) {
    if adjustments.is_neutral() {
        return;
    }
    let (adjust, gains) = adjustments.uniforms();

    // Every channel is mapped independently from an 8-bit input, so a table
    // per channel replaces three `powf` calls per pixel.
    let build = |gain: f32| -> [u8; 256] {
        let mut table = [0u8; 256];
        for (input, output) in table.iter_mut().enumerate() {
            let value = srgb_to_linear(input as f32 / 255.0) * gain;
            let value = (value - CONTRAST_PIVOT) * adjust[1] + CONTRAST_PIVOT;
            let value = value + adjust[0];
            let value = value.max(0.0).powf(1.0 / adjust[2].max(0.01));
            *output = (linear_to_srgb(value.clamp(0.0, 1.0)) * 255.0).round() as u8;
        }
        table
    };
    let tables = [build(gains[0]), build(gains[1]), build(gains[2])];

    rgba.par_chunks_mut(4).for_each(|pixel| {
        pixel[0] = tables[0][pixel[0] as usize];
        pixel[1] = tables[1][pixel[1] as usize];
        pixel[2] = tables[2][pixel[2] as usize];
    });
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_adjustments_leave_pixels_untouched() {
        let mut rgba = vec![0, 64, 128, 255, 255, 200, 10, 7];
        let original = rgba.clone();
        apply_to_rgba(&mut rgba, Adjustments::default());
        assert_eq!(rgba, original);
    }

    #[test]
    fn lighten_raises_midtones_and_keeps_alpha() {
        let mut rgba = vec![128, 128, 128, 42];
        apply_to_rgba(&mut rgba, Adjustments::lighten());
        assert!(rgba[0] > 128);
        assert_eq!(rgba[0], rgba[1]);
        assert_eq!(rgba[3], 42);
    }

    #[test]
    fn warm_and_cool_shift_channels_in_opposite_directions() {
        let mut warm = vec![128, 128, 128, 255];
        apply_to_rgba(&mut warm, Adjustments::warm());
        assert!(warm[0] > warm[2]);

        let mut cool = vec![128, 128, 128, 255];
        apply_to_rgba(&mut cool, Adjustments::cool());
        assert!(cool[2] > cool[0]);
    }

    #[test]
    fn extreme_adjustments_stay_inside_the_byte_range() {
        let mut rgba = vec![0, 255, 128, 255];
        let extreme = Adjustments {
            brightness: 1.0,
            contrast: 5.0,
            gamma: 0.1,
            ..Adjustments::default()
        };
        apply_to_rgba(&mut rgba, extreme);
        assert_eq!(rgba[0], 0);
        assert_eq!(rgba[1], 255);
        // Mid-grey sits on the contrast pivot, so only brightness moves it.
        assert_eq!(rgba[2], 255);
        assert_eq!(rgba[3], 255);
    }

    #[test]
    fn presets_are_non_neutral_and_clamped() {
        for mut preset in [
            Adjustments::auto(),
            Adjustments::lighten(),
            Adjustments::darken(),
            Adjustments::warm(),
            Adjustments::cool(),
        ] {
            assert!(!preset.is_neutral());
            preset.clamp();
            preset.gamma = 99.0;
            preset.clamp();
            assert_eq!(preset.gamma, 3.0);
        }
    }
}
