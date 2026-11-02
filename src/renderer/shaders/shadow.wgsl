struct ShadowUniforms {
    center: vec2<f32>,
    half_size: vec2<f32>,
    angle: f32,
    expand: f32,
    alpha: f32,
    radius: f32,
    screen_size: vec2<f32>,
    pad: vec2<f32>,
};

@group(0) @binding(0) var<uniform> u: ShadowUniforms;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
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
    let extent = u.half_size + vec2<f32>(u.expand);
    let local = (corner - vec2<f32>(0.5)) * 2.0 * extent;
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
    out.local = local;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // Signed distance to a rounded rectangle, in screen pixels.
    let inner = max(u.half_size - vec2<f32>(u.radius), vec2<f32>(0.0));
    let q = abs(in.local) - inner;
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - u.radius;

    let t = clamp(distance / max(u.expand, 0.001), 0.0, 1.0);
    let falloff = 1.0 - smoothstep(0.0, 1.0, t);
    let alpha = falloff * u.alpha;
    return vec4<f32>(0.0, 0.0, 0.0, alpha);
}
