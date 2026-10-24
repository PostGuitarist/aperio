//! Orientation, crop, and straighten. These run on the CPU, outside the
//! per-pixel pipeline, because they move pixels rather than change them.

use crate::LinearImage;

/// A crop rectangle in normalized coordinates of the (oriented) source,
/// rotated by `angle_deg` around its own center.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub angle_deg: f32,
}

impl LinearImage {
    /// Apply an EXIF-style orientation: flip horizontally and/or vertically,
    /// then optionally transpose (swap x and y).
    pub fn orient(&self, transpose: bool, hflip: bool, vflip: bool) -> Self {
        if !(transpose || hflip || vflip) {
            return self.clone();
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let (ow, oh) = if transpose { (h, w) } else { (w, h) };
        let mut pixels = vec![[0.0; 4]; w * h];
        for y in 0..h {
            for x in 0..w {
                let fx = if hflip { w - 1 - x } else { x };
                let fy = if vflip { h - 1 - y } else { y };
                let (ox, oy) = if transpose { (fy, fx) } else { (fx, fy) };
                pixels[oy * ow + ox] = self.pixels[y * w + x];
            }
        }
        Self::new(ow as u32, oh as u32, pixels)
    }

    /// Cut out `rect`, resampling bilinearly when it is rotated. Areas of the
    /// rectangle outside the image come out transparent black.
    pub fn crop(&self, rect: CropRect) -> Self {
        let (sw, sh) = (self.width as f32, self.height as f32);
        let ow = ((rect.width * sw).round() as u32).max(1);
        let oh = ((rect.height * sh).round() as u32).max(1);
        let (cx, cy) = (
            (rect.x + rect.width / 2.0) * sw,
            (rect.y + rect.height / 2.0) * sh,
        );
        let (sin, cos) = rect.angle_deg.to_radians().sin_cos();
        let mut pixels = Vec::with_capacity((ow * oh) as usize);
        for y in 0..oh {
            for x in 0..ow {
                // Offset from the crop center, rotated into source space.
                let dx = x as f32 + 0.5 - ow as f32 / 2.0;
                let dy = y as f32 + 0.5 - oh as f32 / 2.0;
                let sx = cx + dx * cos - dy * sin;
                let sy = cy + dx * sin + dy * cos;
                pixels.push(self.sample(sx, sy));
            }
        }
        Self::new(ow, oh, pixels)
    }

    /// Bilinear sample at pixel-space coordinates (pixel centers at +0.5).
    fn sample(&self, x: f32, y: f32) -> [f32; 4] {
        let (w, h) = (self.width as i64, self.height as i64);
        let (fx, fy) = (x - 0.5, y - 0.5);
        if fx < -1.0 || fy < -1.0 || fx > w as f32 || fy > h as f32 {
            return [0.0; 4];
        }
        let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - x0 as f32, fy - y0 as f32);
        let at = |x: i64, y: i64| self.pixels[(y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize];
        let (a, b, c, d) = (
            at(x0, y0),
            at(x0 + 1, y0),
            at(x0, y0 + 1),
            at(x0 + 1, y0 + 1),
        );
        std::array::from_fn(|i| {
            let top = a[i] + (b[i] - a[i]) * tx;
            let bottom = c[i] + (d[i] - c[i]) * tx;
            top + (bottom - top) * ty
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 3x2 image whose red channel is the pixel index.
    fn numbered() -> LinearImage {
        LinearImage::new(3, 2, (0..6).map(|i| [i as f32, 0.0, 0.0, 1.0]).collect())
    }

    fn reds(img: &LinearImage) -> Vec<f32> {
        img.pixels.iter().map(|p| p[0]).collect()
    }

    #[test]
    fn orientations() {
        let img = numbered(); // 0 1 2 / 3 4 5
        assert_eq!(
            reds(&img.orient(false, true, false)),
            [2., 1., 0., 5., 4., 3.]
        );
        assert_eq!(
            reds(&img.orient(false, true, true)),
            [5., 4., 3., 2., 1., 0.]
        );
        // Rotate 90° clockwise: flip vertically, then transpose.
        let cw = img.orient(true, false, true);
        assert_eq!((cw.width, cw.height), (2, 3));
        assert_eq!(reds(&cw), [3., 0., 4., 1., 5., 2.]);
    }

    #[test]
    fn axis_aligned_crop_copies_pixels_exactly() {
        let img = numbered();
        let out = img.crop(CropRect {
            x: 1.0 / 3.0,
            y: 0.0,
            width: 2.0 / 3.0,
            height: 0.5,
            angle_deg: 0.0,
        });
        assert_eq!((out.width, out.height), (2, 1));
        assert_eq!(reds(&out), [1., 2.]);
    }

    #[test]
    fn half_turn_crop_reverses_pixels() {
        let img = numbered();
        let full = CropRect {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
            angle_deg: 180.0,
        };
        let out = img.crop(full);
        let r = reds(&out);
        for (got, want) in r.iter().zip([5., 4., 3., 2., 1., 0.]) {
            assert!((got - want).abs() < 1e-4, "{r:?}");
        }
    }
}
