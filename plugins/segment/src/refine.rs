//! Image helpers around the network: shrinking the photo to the network's
//! input, and growing its low-resolution mask back to full size with edges
//! that follow the photo.

/// Shrink `src` (w x h) by a whole factor so its longer edge is at most
/// `max_edge`, averaging each block of pixels and dropping leftover edge
/// pixels. Returns `None` if it already fits. Must match
/// `LinearImage::downscale` in aperio-render, which apps use for previews,
/// so a full-size image reduces to exactly its preview.
pub fn box_reduce(
    src: &[[f32; 4]],
    w: u32,
    h: u32,
    max_edge: u32,
) -> Option<(Vec<[f32; 4]>, u32, u32)> {
    let factor = w.max(h).div_ceil(max_edge.max(1)).max(1);
    if factor == 1 {
        return None;
    }
    let (ow, oh) = (w / factor, h / factor);
    let inv = 1.0 / (factor * factor) as f32;
    let mut out = Vec::with_capacity((ow * oh) as usize);
    for y in 0..oh {
        for x in 0..ow {
            let mut sum = [0.0f32; 4];
            for sy in y * factor..(y + 1) * factor {
                let row = (sy * w) as usize;
                for sx in x * factor..(x + 1) * factor {
                    let px = src[row + sx as usize];
                    for c in 0..4 {
                        sum[c] += px[c];
                    }
                }
            }
            out.push(sum.map(|v| v * inv));
        }
    }
    Some((out, ow, oh))
}

/// Average `src` (w x h, `C` channels) down to `ow` x `oh`. Each output pixel
/// averages the source pixels it covers, which avoids aliasing when the
/// network input is much smaller than the photo.
pub fn resize_area<const C: usize>(
    src: &[[f32; C]],
    w: u32,
    h: u32,
    ow: u32,
    oh: u32,
) -> Vec<[f32; C]> {
    let (w, h, ow, oh) = (w as usize, h as usize, ow as usize, oh as usize);
    let mut out = Vec::with_capacity(ow * oh);
    for oy in 0..oh {
        let y0 = oy * h / oh;
        let y1 = ((oy + 1) * h / oh).max(y0 + 1).min(h);
        for ox in 0..ow {
            let x0 = ox * w / ow;
            let x1 = ((ox + 1) * w / ow).max(x0 + 1).min(w);
            let mut sum = [0.0f32; C];
            for y in y0..y1 {
                for px in &src[y * w + x0..y * w + x1] {
                    for c in 0..C {
                        sum[c] += px[c];
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)) as f32;
            out.push(sum.map(|v| v / n));
        }
    }
    out
}

/// Bilinear upscale of a single-channel `w` x `h` map to `ow` x `oh`.
pub fn upscale(src: &[f32], w: u32, h: u32, ow: u32, oh: u32) -> Vec<f32> {
    let (w, h) = (w as usize, h as usize);
    let sx = w as f32 / ow as f32;
    let sy = h as f32 / oh as f32;
    let mut out = Vec::with_capacity(ow as usize * oh as usize);
    for oy in 0..oh {
        let fy = ((oy as f32 + 0.5) * sy - 0.5).clamp(0.0, (h - 1) as f32);
        let y0 = fy as usize;
        let y1 = (y0 + 1).min(h - 1);
        let ty = fy - y0 as f32;
        for ox in 0..ow {
            let fx = ((ox as f32 + 0.5) * sx - 0.5).clamp(0.0, (w - 1) as f32);
            let x0 = fx as usize;
            let x1 = (x0 + 1).min(w - 1);
            let tx = fx - x0 as f32;
            let top = src[y0 * w + x0] + (src[y0 * w + x1] - src[y0 * w + x0]) * tx;
            let bottom = src[y1 * w + x0] + (src[y1 * w + x1] - src[y1 * w + x0]) * tx;
            out.push(top + (bottom - top) * ty);
        }
    }
    out
}

/// Guided filter (He, Sun & Tang, 2010): smooths `p` while snapping its
/// edges to edges in the grayscale `guide`. Turns a blocky upscaled mask into
/// one that follows branches and rooflines.
pub fn guided_filter(
    guide: &[f32],
    p: &[f32],
    w: u32,
    h: u32,
    radius: usize,
    eps: f32,
) -> Vec<f32> {
    let n = guide.len();
    let ip: Vec<f32> = guide.iter().zip(p).map(|(i, p)| i * p).collect();
    let ii: Vec<f32> = guide.iter().map(|i| i * i).collect();
    // The box means are independent, and at full size each takes a while,
    // so they run side by side.
    let mean = |src: &[f32]| box_mean(src, w, h, radius);
    let (mean_i, mean_p, mean_ip, mean_ii) = std::thread::scope(|s| {
        let i = s.spawn(|| mean(guide));
        let pp = s.spawn(|| mean(p));
        let ip = s.spawn(|| mean(&ip));
        let ii = mean(&ii);
        (
            i.join().unwrap(),
            pp.join().unwrap(),
            ip.join().unwrap(),
            ii,
        )
    });
    let mut a = vec![0.0f32; n];
    let mut b = vec![0.0f32; n];
    for k in 0..n {
        let var = mean_ii[k] - mean_i[k] * mean_i[k];
        let cov = mean_ip[k] - mean_i[k] * mean_p[k];
        a[k] = cov / (var + eps);
        b[k] = mean_p[k] - a[k] * mean_i[k];
    }
    let (mean_a, mean_b) = std::thread::scope(|s| {
        let a = s.spawn(|| mean(&a));
        let b = mean(&b);
        (a.join().unwrap(), b)
    });
    (0..n)
        .map(|k| (mean_a[k] * guide[k] + mean_b[k]).clamp(0.0, 1.0))
        .collect()
}

/// Mean over a (2r+1)² window, clipped at the borders, via a summed-area
/// table (so the cost doesn't depend on the radius).
fn box_mean(src: &[f32], w: u32, h: u32, r: usize) -> Vec<f32> {
    let (w, h) = (w as usize, h as usize);
    // f64 sums keep precision on 20+ megapixel images.
    let mut sat = vec![0.0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0.0f64;
        for x in 0..w {
            row += src[y * w + x] as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let sum = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0]
                + sat[y0 * (w + 1) + x0];
            out.push((sum / ((y1 - y0) * (x1 - x0)) as f64) as f32);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn area_resize_averages_blocks() {
        let src: Vec<[f32; 1]> = vec![[0.0], [1.0], [2.0], [3.0], [4.0], [5.0], [6.0], [7.0]];
        assert_eq!(resize_area(&src, 4, 2, 2, 1), vec![[2.5], [4.5]]);
    }

    #[test]
    fn upscale_keeps_constant_maps_and_corners() {
        assert_eq!(upscale(&[0.5; 4], 2, 2, 5, 3), vec![0.5; 15]);
        let up = upscale(&[0.0, 1.0], 2, 1, 4, 1);
        assert_eq!((up[0], up[3]), (0.0, 1.0));
    }

    #[test]
    fn guided_filter_snaps_a_soft_edge_to_the_guide() {
        // Guide: hard edge between columns 4 and 5. Mask: a blurry ramp.
        let (w, h) = (10u32, 4u32);
        let guide: Vec<f32> = (0..w * h)
            .map(|i| if i % w < 5 { 0.0 } else { 1.0 })
            .collect();
        let p: Vec<f32> = (0..w * h).map(|i| (i % w) as f32 / 9.0).collect();
        let q = guided_filter(&guide, &p, w, h, 2, 1e-4);
        let at = |x: u32| q[(w + x) as usize];
        assert!(at(5) - at(4) > p[5] - p[4] + 0.2, "{q:?}");
        assert!(q.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
