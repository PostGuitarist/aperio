// Per-pixel adjustments. Must match `AdjustOp::apply` in adjust.rs.

struct AdjustOp {
    kind: u32,
    _pad: u32,
    a: f32,
    b: f32,
    c: f32,
    d: f32,
    _pad2: vec2<f32>,
};

struct Params {
    pixel_count: u32,
    op_start: u32,
    op_count: u32,
    has_mask: u32,
    opacity: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
};

const KIND_GAIN: u32 = 1u;
const KIND_CONTRAST: u32 = 2u;
const KIND_COLOR: u32 = 3u;
const KIND_CURVE: u32 = 4u;
const KIND_HSL: u32 = 5u;
const KIND_GRADE: u32 = 6u;
const HSL_HUE_RANGE: f32 = 30.0 / 360.0;
const GRADE_CENTER: u32 = 16u;
const GRADE_WIDTH: u32 = 17u;
const LUT_SIZE: u32 = 256u;
const PIVOT: f32 = 0.18;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> ops: array<AdjustOp>;
@group(0) @binding(2) var<storage, read_write> pixels: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> luts: array<f32>;
@group(0) @binding(4) var<storage, read> mask: array<f32>;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 {
        return v / 12.92;
    }
    return pow((v + 0.055) / 1.055, 2.4);
}

fn linear_to_srgb(v: f32) -> f32 {
    let c = clamp(v, 0.0, 1.0);
    if c <= 0.0031308 {
        return c * 12.92;
    }
    return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
}

// Linear interpolation in a baked curve. Matches `curve::lookup`.
fn lut(start: u32, v: f32) -> f32 {
    let p = clamp(v, 0.0, 1.0) * f32(LUT_SIZE - 1u);
    let i = min(u32(floor(p)), LUT_SIZE - 2u);
    let t = p - f32(i);
    let a = luts[start + i];
    return a + (luts[start + i + 1u] - a) * t;
}

// Tone curve on the sRGB-encoded value. Matches `KIND_CURVE` in adjust.rs.
fn tone(start: u32, v: f32) -> f32 {
    if v > 1.0 {
        return v * srgb_to_linear(luts[start + LUT_SIZE - 1u]);
    }
    return srgb_to_linear(lut(start, linear_to_srgb(v)));
}

// sRGB-encoded channels, with overbright values scaled into range first.
// Matches `encode` and `decode` in adjust.rs.
fn encode(px: vec3<f32>, scale: f32) -> vec3<f32> {
    let v = px / scale;
    return vec3<f32>(linear_to_srgb(v.r), linear_to_srgb(v.g), linear_to_srgb(v.b));
}

fn decode(c: vec3<f32>, scale: f32) -> vec3<f32> {
    let v = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec3<f32>(srgb_to_linear(v.r), srgb_to_linear(v.g), srgb_to_linear(v.b)) * scale;
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> vec3<f32> {
    let n = vec3<f32>(5.0, 3.0, 1.0) + vec3<f32>(h * 6.0);
    let k = n - 6.0 * floor(n / 6.0);
    return vec3<f32>(v) - v * s * clamp(min(k, vec3<f32>(4.0) - k), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Matches `hsl_pixel` in adjust.rs.
fn hsl(start: u32, px: vec3<f32>) -> vec3<f32> {
    let scale = max(max(px.r, max(px.g, px.b)), 1.0);
    let c = encode(px, scale);
    let mx = max(c.r, max(c.g, c.b));
    let mn = min(c.r, min(c.g, c.b));
    let chroma = mx - mn;
    if chroma <= 1e-6 {
        return px;
    }
    var h: f32;
    if mx == c.r {
        h = (c.g - c.b) / chroma;
    } else if mx == c.g {
        h = (c.b - c.r) / chroma + 2.0;
    } else {
        h = (c.r - c.g) / chroma + 4.0;
    }
    h = h / 6.0;
    h = h - floor(h);
    let s = chroma / mx;
    let dh = lut(start, h);
    let ds = lut(start + LUT_SIZE, h);
    let dl = lut(start + 2u * LUT_SIZE, h);
    var h2 = h + dh * HSL_HUE_RANGE;
    h2 = h2 - floor(h2);
    let turned = hsv_to_rgb(h2, s, mx);
    let lum = luminance(turned);
    let k = 1.0 + dl * 0.5 * s;
    return decode(max(vec3<f32>(lum) + (turned - vec3<f32>(lum)) * (1.0 + ds), vec3<f32>(0.0)) * k, scale);
}

// Matches `grade_pixel` in adjust.rs.
fn grade(start: u32, px: vec3<f32>) -> vec3<f32> {
    let scale = max(max(px.r, max(px.g, px.b)), 1.0);
    var c = encode(px, scale);
    let l = linear_to_srgb(luminance(px) / scale);
    let center = luts[start + GRADE_CENTER];
    let width = luts[start + GRADE_WIDTH];
    let shadows = 1.0 - smoothstep(0.0, center + width, l);
    let highlights = smoothstep(center - width, 1.0, l);
    let mid = max(1.0 - shadows - highlights, 0.0);
    // Unrolled: Direct3D's shader compiler rejects dynamic array indexing here.
    c += shadows * tint(start);
    c += mid * tint(start + 4u);
    c += highlights * tint(start + 8u);
    c += tint(start + 12u);
    return decode(c, scale);
}

// One wheel of a grade block: its RGB tint plus its luminance shift.
fn tint(t: u32) -> vec3<f32> {
    return vec3<f32>(luts[t], luts[t + 1u], luts[t + 2u]) + vec3<f32>(luts[t + 3u]);
}

fn apply(op: AdjustOp, px: vec3<f32>) -> vec3<f32> {
    switch op.kind {
        case KIND_HSL: {
            return hsl(u32(op.a) * LUT_SIZE, px);
        }
        case KIND_GRADE: {
            return grade(u32(op.a) * LUT_SIZE, px);
        }
        case KIND_GAIN: {
            return px * vec3<f32>(op.a, op.b, op.c);
        }
        case KIND_CONTRAST: {
            return PIVOT * pow(max(px, vec3<f32>(0.0)) / PIVOT, vec3<f32>(1.0 + op.a));
        }
        case KIND_COLOR: {
            let lum = luminance(px);
            let mx = max(px.r, max(px.g, px.b));
            let mn = min(px.r, min(px.g, px.b));
            var sat = 0.0;
            if mx > 0.0 {
                sat = (mx - mn) / mx;
            }
            let s = (1.0 + op.a) * (1.0 + op.b * (1.0 - sat));
            return max(vec3<f32>(lum) + (px - vec3<f32>(lum)) * s, vec3<f32>(0.0));
        }
        case KIND_CURVE: {
            let start = u32(op.a) * LUT_SIZE;
            let channel = u32(op.b);
            if channel == 0u {
                return vec3<f32>(tone(start, px.r), tone(start, px.g), tone(start, px.b));
            }
            if channel == 4u {
                let lum = luminance(px);
                if lum <= 0.0 {
                    return px;
                }
                return px * (tone(start, lum) / lum);
            }
            // No dynamic vector index here: Direct3D's shader compiler can't
            // compile one inside the per-pixel loop.
            if channel == 1u {
                return vec3<f32>(tone(start, px.r), px.g, px.b);
            }
            if channel == 2u {
                return vec3<f32>(px.r, tone(start, px.g), px.b);
            }
            return vec3<f32>(px.r, px.g, tone(start, px.b));
        }
        default: {
            return px;
        }
    }
}

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x + id.y * 65535u * 256u;
    if i >= params.pixel_count {
        return;
    }
    // One dispatch per layer: run its steps, then blend over what's below.
    var weight = params.opacity;
    if params.has_mask != 0u {
        weight *= mask[i];
    }
    if weight <= 0.0 {
        return;
    }
    let below = pixels[i];
    var rgb = below.rgb;
    for (var k = params.op_start; k < params.op_start + params.op_count; k++) {
        rgb = apply(ops[k], rgb);
    }
    pixels[i] = mix(below, vec4<f32>(rgb, below.a), weight);
}
