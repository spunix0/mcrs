struct Uniforms {
    view_proj: mat4x4<f32>,
    // xyz = camera position, w = fog end distance
    camera: vec4<f32>,
    sky: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) dist: f32,
};

@vertex
fn vs(@location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4<f32>(pos, 1.0);
    o.uv = uv;
    // Tints and shading are authored in sRGB; the atlas is sampled as linear, so match spaces before multiplying.
    o.color = vec4<f32>(pow(color.rgb, vec3<f32>(2.2)), color.a);
    o.dist = length(pos.xz - u.camera.xz);
    return o;
}

fn fogged(rgb: vec3<f32>, dist: f32) -> vec3<f32> {
    let start = u.camera.w * 0.7;
    let f = clamp((dist - start) / (u.camera.w - start), 0.0, 1.0);
    return mix(rgb, u.sky.rgb, f);
}

@fragment
fn fs_opaque(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(atlas, atlas_sampler, i.uv) * i.color;
    if t.a < 0.5 {
        discard;
    }
    return vec4<f32>(fogged(t.rgb, i.dist), 1.0);
}

struct LineOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_line(@location(0) pos: vec3<f32>, @location(1) color: vec4<f32>) -> LineOut {
    var o: LineOut;
    o.clip = u.view_proj * vec4<f32>(pos, 1.0);
    o.color = vec4<f32>(pow(color.rgb, vec3<f32>(2.2)), color.a);
    return o;
}

@fragment
fn fs_line(i: LineOut) -> @location(0) vec4<f32> {
    return i.color;
}

@fragment
fn fs_translucent(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(atlas, atlas_sampler, i.uv) * i.color;
    return vec4<f32>(fogged(t.rgb, i.dist), t.a);
}
