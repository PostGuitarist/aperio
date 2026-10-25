//! Converting decoded HEIF pixels from their color space to linear light
//! with sRGB primaries, for the decoders that don't do it themselves.
//!
//! Only the primaries are converted. The transfer curve is taken to be
//! sRGB's, which Display P3 and the other common photo color spaces share.

use crate::color::srgb_to_linear;

type Mat3 = [[f32; 3]; 3];

/// XYZ (D65) to linear sRGB.
const XYZ_TO_SRGB: Mat3 = [
    [3.240_454_2, -1.537_138_5, -0.498_531_4],
    [-0.969_266, 1.876_010_8, 0.041_556],
    [0.055_643_4, -0.204_025_9, 1.057_225_2],
];

const D65: [f32; 2] = [0.3127, 0.3290];
const D50_XYZ: [f32; 3] = [0.9642, 1.0, 0.8249];

/// How to turn a photo's pixels into linear sRGB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Conversion {
    /// Linear RGB in the photo's primaries to linear sRGB, or `None` when
    /// the photo is already sRGB.
    matrix: Option<Mat3>,
}

impl Conversion {
    pub const SRGB: Self = Self { matrix: None };

    /// From the chromaticities of the red, green, and blue primaries and the
    /// white point, as an `nclx` color profile gives them. (Only libheif
    /// reports these; WIC gives ICC profiles.)
    #[cfg_attr(windows, allow(dead_code))]
    pub fn from_primaries(red: [f32; 2], green: [f32; 2], blue: [f32; 2], white: [f32; 2]) -> Self {
        let to_xyz = rgb_to_xyz(red, green, blue, white);
        let adapt = bradford(xy_to_xyz(white), xy_to_xyz(D65));
        Self::from_xyz(mul(adapt, to_xyz))
    }

    /// From an ICC profile, using its red, green, and blue colorants. Gives
    /// `None` for profiles without them (such as ones built from lookup
    /// tables), which callers treat as sRGB.
    pub fn from_icc(icc: &[u8]) -> Option<Self> {
        if icc.get(16..20)? != b"RGB " {
            return None;
        }
        let colorant = |sig: &[u8; 4]| -> Option<[f32; 3]> {
            let data = icc_tag(icc, sig)?;
            if data.get(0..4)? != b"XYZ " {
                return None;
            }
            let num = |i: usize| -> Option<f32> {
                let b = data.get(8 + i * 4..12 + i * 4)?;
                Some(i32::from_be_bytes(b.try_into().ok()?) as f32 / 65536.0)
            };
            Some([num(0)?, num(1)?, num(2)?])
        };
        let (r, g, b) = (colorant(b"rXYZ")?, colorant(b"gXYZ")?, colorant(b"bXYZ")?);
        // ICC colorants are adapted to D50.
        let to_xyz = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
        let adapt = bradford(D50_XYZ, xy_to_xyz(D65));
        Some(Self::from_xyz(mul(adapt, to_xyz)))
    }

    fn from_xyz(to_xyz_d65: Mat3) -> Self {
        let m = mul(XYZ_TO_SRGB, to_xyz_d65);
        let identity =
            (0..3).all(|i| (0..3).all(|j| (m[i][j] - (i == j) as u8 as f32).abs() < 2e-3));
        Self {
            matrix: (!identity).then_some(m),
        }
    }

    /// Linearize an sRGB-encoded pixel (0..1) and convert it to sRGB primaries.
    pub fn apply(&self, [r, g, b, a]: [f32; 4]) -> [f32; 4] {
        let rgb = [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)];
        let rgb = match &self.matrix {
            Some(m) => mul_vec(m, rgb),
            None => rgb,
        };
        [rgb[0], rgb[1], rgb[2], a]
    }
}

fn icc_tag<'a>(icc: &'a [u8], sig: &[u8; 4]) -> Option<&'a [u8]> {
    let u32_at = |i: usize| -> Option<usize> {
        Some(u32::from_be_bytes(icc.get(i..i + 4)?.try_into().ok()?) as usize)
    };
    let count = u32_at(128)?;
    (0..count.min(256)).find_map(|n| {
        let entry = 132 + n * 12;
        if icc.get(entry..entry + 4)? != sig {
            return None;
        }
        let (offset, size) = (u32_at(entry + 4)?, u32_at(entry + 8)?);
        icc.get(offset..offset.checked_add(size)?)
    })
}

fn xy_to_xyz([x, y]: [f32; 2]) -> [f32; 3] {
    [x / y, 1.0, (1.0 - x - y) / y]
}

#[cfg_attr(windows, allow(dead_code))]
fn rgb_to_xyz(red: [f32; 2], green: [f32; 2], blue: [f32; 2], white: [f32; 2]) -> Mat3 {
    let (r, g, b) = (xy_to_xyz(red), xy_to_xyz(green), xy_to_xyz(blue));
    let m = [[r[0], g[0], b[0]], [r[1], g[1], b[1]], [r[2], g[2], b[2]]];
    let s = mul_vec(&invert(m), xy_to_xyz(white));
    [
        [m[0][0] * s[0], m[0][1] * s[1], m[0][2] * s[2]],
        [m[1][0] * s[0], m[1][1] * s[1], m[1][2] * s[2]],
        [m[2][0] * s[0], m[2][1] * s[1], m[2][2] * s[2]],
    ]
}

/// Bradford chromatic adaptation from one white (XYZ) to another.
fn bradford(from: [f32; 3], to: [f32; 3]) -> Mat3 {
    const B: Mat3 = [
        [0.8951, 0.2664, -0.1614],
        [-0.7502, 1.7135, 0.0367],
        [0.0389, -0.0685, 1.0296],
    ];
    let (f, t) = (mul_vec(&B, from), mul_vec(&B, to));
    let scale = [
        [t[0] / f[0], 0.0, 0.0],
        [0.0, t[1] / f[1], 0.0],
        [0.0, 0.0, t[2] / f[2]],
    ];
    mul(invert(B), mul(scale, B))
}

fn mul(a: Mat3, b: Mat3) -> Mat3 {
    let mut out = [[0.0; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    out
}

fn mul_vec(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [0, 1, 2].map(|i| m[i][0] * v[0] + m[i][1] * v[1] + m[i][2] * v[2])
}

fn invert(m: Mat3) -> Mat3 {
    let [[a, b, c], [d, e, f], [g, h, i]] = m;
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    let inv = 1.0 / det;
    [
        [
            (e * i - f * h) * inv,
            (c * h - b * i) * inv,
            (b * f - c * e) * inv,
        ],
        [
            (f * g - d * i) * inv,
            (a * i - c * g) * inv,
            (c * d - a * f) * inv,
        ],
        [
            (d * h - e * g) * inv,
            (b * g - a * h) * inv,
            (a * e - b * d) * inv,
        ],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    const P3: ([f32; 2], [f32; 2], [f32; 2]) = ([0.680, 0.320], [0.265, 0.690], [0.150, 0.060]);

    #[test]
    fn srgb_primaries_need_no_conversion() {
        let c = Conversion::from_primaries([0.64, 0.33], [0.30, 0.60], [0.15, 0.06], D65);
        assert_eq!(c, Conversion::SRGB);
    }

    #[test]
    fn display_p3_red_is_outside_srgb() {
        let c = Conversion::from_primaries(P3.0, P3.1, P3.2, D65);
        let [r, g, b, a] = c.apply([1.0, 0.0, 0.0, 1.0]);
        // Known values for P3 red in linear sRGB.
        assert!((r - 1.2249).abs() < 1e-3, "{r}");
        assert!((g + 0.0420).abs() < 1e-3, "{g}");
        assert!((b + 0.0196).abs() < 1e-3, "{b}");
        assert_eq!(a, 1.0);
        // White stays white.
        let w = c.apply([1.0, 1.0, 1.0, 1.0]);
        assert!(w[..3].iter().all(|v| (v - 1.0).abs() < 1e-3), "{w:?}");
    }

    /// A minimal ICC profile holding just the three colorant tags.
    fn icc_with_colorants(colorants: [[f32; 3]; 3]) -> Vec<u8> {
        let mut icc = vec![0u8; 132 + 3 * 12];
        icc[16..20].copy_from_slice(b"RGB ");
        icc[128..132].copy_from_slice(&3u32.to_be_bytes());
        for (n, (sig, xyz)) in [b"rXYZ", b"gXYZ", b"bXYZ"]
            .iter()
            .zip(colorants)
            .enumerate()
        {
            let offset = icc.len() as u32;
            let entry = 132 + n * 12;
            icc[entry..entry + 4].copy_from_slice(*sig);
            icc[entry + 4..entry + 8].copy_from_slice(&offset.to_be_bytes());
            icc[entry + 8..entry + 12].copy_from_slice(&20u32.to_be_bytes());
            icc.extend_from_slice(b"XYZ \0\0\0\0");
            for v in xyz {
                icc.extend_from_slice(&((v * 65536.0).round() as i32).to_be_bytes());
            }
        }
        icc
    }

    #[test]
    fn icc_colorants_match_the_primaries() {
        // Display P3's colorants, as Apple's profile stores them (D50).
        let icc = icc_with_colorants([
            [0.5151, 0.2412, -0.0011],
            [0.2920, 0.6922, 0.0419],
            [0.1571, 0.0666, 0.7841],
        ]);
        let from_icc = Conversion::from_icc(&icc)
            .unwrap()
            .apply([1.0, 0.0, 0.0, 1.0]);
        let from_xy = Conversion::from_primaries(P3.0, P3.1, P3.2, D65).apply([1.0, 0.0, 0.0, 1.0]);
        for c in 0..3 {
            assert!(
                (from_icc[c] - from_xy[c]).abs() < 5e-3,
                "{from_icc:?} {from_xy:?}"
            );
        }
    }

    #[test]
    fn icc_srgb_colorants_need_no_conversion() {
        let icc = icc_with_colorants([
            [0.4361, 0.2225, 0.0139],
            [0.3851, 0.7169, 0.0971],
            [0.1431, 0.0606, 0.7141],
        ]);
        assert_eq!(Conversion::from_icc(&icc), Some(Conversion::SRGB));
        assert_eq!(Conversion::from_icc(b"too short"), None);
    }
}
