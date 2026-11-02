struct PhotoUniforms {
    center: vec2<f32>,
    half_size: vec2<f32>,
    angle: f32,
    alpha: f32,
    gamma: f32,
    pad: f32,
    screen_size: vec2<f32>,
    uv_row0: vec2<f32>,
    uv_row1: vec2<f32>,
    uv_offset: vec2<f32>,
    adjust: vec4<f32>,
    gains: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: PhotoUniforms;

@group(1) @binding(0) var photo_texture: texture_2d<f32>;
@group(1) @binding(1) var photo_sampler: sampler;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );

    let corner = corners[vertex_index];
    let local = (corner - vec2<f32>(0.5)) * 2.0 * u.half_size;
    let s = sin(u.angle);
    let c = cos(u.angle);
    let rotated = vec2<f32>(
        local.x * c - local.y * s,
        local.x * s + local.y * c,
    );
    let screen = u.center + rotated;
    let ndc = vec2<f32>(
        screen.x / u.screen_size.x * 2.0 - 1.0,
        1.0 - screen.y / u.screen_size.y * 2.0,
    );

    var out: VertexOutput;
    out.position = vec4<f32>(ndc, 0.0, 1.0);
    out.uv = corner;
    return out;
}

fn gamma_from_linear(rgb: vec3<f32>) -> vec3<f32> {
    let cutoff = rgb < vec3<f32>(0.0031308);
    let lower = rgb * 12.92;
    let higher = 1.055 * pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(higher, lower, cutoff);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let source_uv = vec2<f32>(
        dot(u.uv_row0, in.uv),
        dot(u.uv_row1, in.uv),
    ) + u.uv_offset;

    var color = textureSample(photo_texture, photo_sampler, source_uv);

    var rgb = color.rgb * u.gains.rgb;
    rgb = (rgb - vec3<f32>(0.214041)) * u.adjust.y + vec3<f32>(0.214041);
    rgb = rgb + vec3<f32>(u.adjust.x);
    rgb = pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / max(u.adjust.z, 0.01)));
    color = vec4<f32>(rgb, color.a);
    color.a = color.a * u.alpha;

    // Rgba8UnormSrgb sampling returns linear values. A non-sRGB target needs
    // manual gamma encoding; an sRGB target does it in the hardware.
    if (u.gamma > 0.5) {
        color = vec4<f32>(gamma_from_linear(color.rgb), color.a);
    }

    return vec4<f32>(color.rgb * color.a, color.a);
}
