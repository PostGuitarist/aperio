//! Color conversions shared by decoding, adjustments, and export.

use std::sync::OnceLock;

/// sRGB transfer function, encoded value to linear.
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

/// sRGB transfer function, linear to encoded value. Input is clamped to 0..=1.
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

/// [`linear_to_srgb`] as an 8-bit value, looked up in a table. For previews
/// and thumbnails, which convert every pixel of every frame; it can differ
/// from the exact conversion by one step.
pub fn linear_to_srgb8(v: f32) -> u8 {
    const STEPS: usize = 1 << 16;
    static TABLE: OnceLock<Vec<u8>> = OnceLock::new();
    let table = TABLE.get_or_init(|| {
        (0..STEPS)
            .map(|i| (linear_to_srgb(i as f32 / (STEPS - 1) as f32) * 255.0).round() as u8)
            .collect()
    });
    // NaN becomes 0.
    table[(v.clamp(0.0, 1.0) * (STEPS - 1) as f32 + 0.5) as usize]
}

/// Rec. 709 luminance of a linear RGB value.
pub fn luminance(rgb: [f32; 3]) -> f32 {
    0.2126 * rgb[0] + 0.7152 * rgb[1] + 0.0722 * rgb[2]
}

/// Linear sRGB color of a light source at the given temperature, normalized so
/// green is 1. Uses the Kim et al. cubic fit of the Planckian locus
/// (1667 K to 25000 K) for the CIE xy chromaticity.
pub fn blackbody_rgb(kelvin: f32) -> [f32; 3] {
    let t = kelvin.clamp(1667.0, 25000.0) as f64;
    let (t2, t3) = (t * t, t * t * t);
    let x = if t <= 4000.0 {
        -0.266_123_9e9 / t3 - 0.234_358_9e6 / t2 + 0.877_695_6e3 / t + 0.179_910
    } else {
        -3.025_846_9e9 / t3 + 2.107_037_9e6 / t2 + 0.222_634_7e3 / t + 0.240_390
    };
    let (x2, x3) = (x * x, x * x * x);
    let y = if t <= 2222.0 {
        -1.106_381_4 * x3 - 1.348_110_20 * x2 + 2.185_558_32 * x - 0.202_196_83
    } else if t <= 4000.0 {
        -0.954_947_6 * x3 - 1.374_185_93 * x2 + 2.091_370_15 * x - 0.167_488_67
    } else {
        3.081_758_0 * x3 - 5.873_386_70 * x2 + 3.751_129_97 * x - 0.370_014_83
    };
    // xyY (Y = 1) to XYZ, then XYZ to linear sRGB (D65).
    let (cx, cy, cz) = (x / y, 1.0, (1.0 - x - y) / y);
    let r = 3.240_454_2 * cx - 1.537_138_5 * cy - 0.498_531_4 * cz;
    let g = -0.969_266_0 * cx + 1.876_010_8 * cy + 0.041_556_0 * cz;
    let b = 0.055_643_4 * cx - 0.204_025_9 * cy + 1.057_225_2 * cz;
    [(r / g) as f32, 1.0, (b / g) as f32]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb8_table_is_within_one_step() {
        for i in 0..=100_000 {
            let v = i as f32 / 100_000.0;
            let exact = (linear_to_srgb(v) * 255.0).round() as i32;
            assert!((linear_to_srgb8(v) as i32 - exact).abs() <= 1, "{v}");
        }
        assert_eq!(linear_to_srgb8(-1.0), 0);
        assert_eq!(linear_to_srgb8(2.0), 255);
        assert_eq!(linear_to_srgb8(f32::NAN), 0);
    }

    #[test]
    fn srgb_round_trips() {
        for i in 0..=100 {
            let v = i as f32 / 100.0;
            assert!((linear_to_srgb(srgb_to_linear(v)) - v).abs() < 1e-5);
        }
    }

    #[test]
    fn d65_is_close_to_neutral() {
        let [r, g, b] = blackbody_rgb(6504.0);
        assert_eq!(g, 1.0);
        // The Planckian locus passes slightly off the D65 point.
        assert!((r - 1.0).abs() < 0.1 && (b - 1.0).abs() < 0.1, "{r} {b}");
    }

    #[test]
    fn warm_light_is_red_and_cool_light_is_blue() {
        let warm = blackbody_rgb(3000.0);
        let cool = blackbody_rgb(10000.0);
        assert!(warm[0] > 1.0 && warm[2] < 1.0);
        assert!(cool[0] < 1.0 && cool[2] > 1.0);
    }
}
