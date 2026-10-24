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
    op_count: u32,
};

const KIND_GAIN: u32 = 1u;
const KIND_CONTRAST: u32 = 2u;
const KIND_COLOR: u32 = 3u;
const KIND_CURVE: u32 = 4u;
const LUT_SIZE: u32 = 256u;
const PIVOT: f32 = 0.18;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> ops: array<AdjustOp>;
@group(0) @binding(2) var<storage, read_write> pixels: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> luts: array<f32>;

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

fn apply(op: AdjustOp, px: vec3<f32>) -> vec3<f32> {
    switch op.kind {
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
            var out = px;
            out[channel - 1u] = tone(start, px[channel - 1u]);
            return out;
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
    var px = pixels[i];
    var rgb = px.rgb;
    for (var k = 0u; k < params.op_count; k++) {
        rgb = apply(ops[k], rgb);
    }
    pixels[i] = vec4<f32>(rgb, px.a);
}
