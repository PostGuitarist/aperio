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
const PIVOT: f32 = 0.18;

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> ops: array<AdjustOp>;
@group(0) @binding(2) var<storage, read_write> pixels: array<vec4<f32>>;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
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
