// RKB RUST GPU shader. Written by this project; no Minecraft asset or shader is
// included. Geometry, UVs, tint, and per-vertex light come from the M6 mesher.

struct Camera {
    // Column-major camera.view_projection, exactly as the software path uses.
    view_proj: mat4x4<f32>,
    // Eye position in blocks (xyz).
    eye: vec4<f32>,
    // Fog colour (rgb) and fog start distance (w).
    fog: vec4<f32>,
    // Fog end distance (x), 1.0 when the surface is sRGB (y).
    fog_end: vec4<f32>,
    // Viewport width (x), height (y).
    viewport: vec4<f32>,
    // Sky zenith colour (rgb).
    sky_top: vec4<f32>,
};

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var atlas_tex: texture_2d<f32>;
@group(0) @binding(2) var atlas_samp: sampler;

struct WorldVertex {
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) light: f32,
    @location(3) tint: vec3<f32>,
};

struct WorldOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) light: f32,
    @location(2) tint: vec3<f32>,
    @location(3) distance: f32,
};

@vertex
fn vs_world(vertex: WorldVertex) -> WorldOut {
    var out: WorldOut;
    out.clip = camera.view_proj * vec4<f32>(vertex.position, 1.0);
    out.uv = vertex.uv;
    out.light = vertex.light;
    out.tint = vertex.tint;
    out.distance = distance(vertex.position, camera.eye.xyz);
    return out;
}

// The software renderer writes sRGB byte values directly. A non-sRGB surface
// keeps that identical; an sRGB surface needs linear values, so this function
// applies the inverse transfer curve when the backend reported one.
fn encode(color: vec4<f32>) -> vec4<f32> {
    if camera.fog_end.y > 0.5 {
        return vec4<f32>(pow(max(color.rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2)), color.a);
    }
    return color;
}

fn shade(input: WorldOut) -> vec4<f32> {
    let texel = textureSample(atlas_tex, atlas_samp, input.uv);
    var rgb = texel.rgb * input.tint * input.light;
    let start = camera.fog.w;
    let end = max(camera.fog_end.x, start + 0.001);
    let fog = clamp((input.distance - start) / (end - start), 0.0, 1.0);
    rgb = mix(rgb, camera.fog.rgb, fog);
    return encode(vec4<f32>(rgb, texel.a));
}

@fragment
fn fs_opaque(input: WorldOut) -> @location(0) vec4<f32> {
    let color = shade(input);
    return vec4<f32>(color.rgb, 1.0);
}

@fragment
fn fs_cutout(input: WorldOut) -> @location(0) vec4<f32> {
    let color = shade(input);
    if color.a < 0.5 {
        discard;
    }
    return vec4<f32>(color.rgb, 1.0);
}

@fragment
fn fs_translucent(input: WorldOut) -> @location(0) vec4<f32> {
    return shade(input);
}

// Fullscreen sky: zenith to horizon at 62% of the viewport, matching the
// software renderer's gradient, then the horizon colour below.
@vertex
fn vs_sky(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(positions[index], 0.0, 1.0);
}

@fragment
fn fs_sky(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let horizon = max(camera.viewport.y * 0.62, 1.0);
    let t = clamp(position.y / horizon, 0.0, 1.0);
    let rgb = mix(camera.sky_top.rgb, camera.fog.rgb, t);
    return encode(vec4<f32>(rgb, 1.0));
}

// 2D interface geometry in framebuffer pixels (origin top-left), converted to
// clip space here so the game's existing HUD layout is reused unchanged.
struct UiVertex {
    @location(0) position: vec2<f32>,
    @location(1) color: vec4<f32>,
};

struct UiOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_ui(vertex: UiVertex) -> UiOut {
    var out: UiOut;
    let width = max(camera.viewport.x, 1.0);
    let height = max(camera.viewport.y, 1.0);
    let ndc = vec2<f32>(
        vertex.position.x / width * 2.0 - 1.0,
        1.0 - vertex.position.y / height * 2.0,
    );
    out.clip = vec4<f32>(ndc, 0.0, 1.0);
    out.color = vertex.color;
    return out;
}

@fragment
fn fs_ui(input: UiOut) -> @location(0) vec4<f32> {
    return encode(input.color);
}
