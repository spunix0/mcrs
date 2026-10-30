struct Uniforms {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    // xyz = camera position, w = fog end distance
    camera: vec4<f32>,
    // rgb = sky color overhead
    sky: vec4<f32>,
    // rgb = fog / horizon color, w = daylight factor for sky light (0.2 at night .. 1)
    fog: vec4<f32>,
    // xyz = direction to the sun, w = sunrise glow strength
    sun: vec4<f32>,
    // rgb = sunrise glow color, w = brightness option (gamma)
    sunrise: vec4<f32>,
    // xy = window size in pixels
    screen: vec4<f32>,
    sky_light: vec4<f32>,
    ambient: vec4<f32>,
    block_tint: vec4<f32>,
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
    o.dist = length(pos - u.camera.xyz);
    return o;
}

// Vanilla 26.3's lightmap.fsh; mesh.rs `lightmap` is the CPU twin. Levels are 0..15 (fractional from smooth light).
fn lightmap(sky_level: f32, block_level: f32) -> vec3<f32> {
    let sl = clamp(sky_level / 15.0, 0.0, 1.0);
    let bl = clamp(block_level / 15.0, 0.0, 1.0);
    let sky = sl / (4.0 - 3.0 * sl) * u.fog.w;
    let block = bl / (4.0 - 3.0 * bl) * 1.4;
    let p = 2.0 * bl - 1.0;
    let block_color = mix(u.block_tint.rgb, vec3<f32>(1.0), 0.9 * p * p);
    let c = clamp(u.ambient.rgb + u.sky_light.rgb * sky + block_color * block, vec3<f32>(0.0), vec3<f32>(1.0));
    let m = max(max(max(c.x, c.y), c.z), 1e-4);
    let inv = 1.0 - m;
    let scaled = 1.0 - inv * inv * inv * inv;
    return mix(c, c * (scaled / m), u.sunrise.w);
}

@vertex
fn vs_chunk(@location(0) pos: vec3<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>, @location(3) light: vec4<f32>) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4<f32>(pos, 1.0);
    o.uv = uv;
    let l = lightmap(light.x * 15.0, light.y * 15.0);
    o.color = vec4<f32>(pow(color.rgb * l, vec3<f32>(2.2)), color.a);
    o.dist = length(pos - u.camera.xyz);
    return o;
}

fn fogged(rgb: vec3<f32>, dist: f32) -> vec3<f32> {
    // sky.w holds the fog start, camera.w the end.
    let start = u.sky.w;
    let f = clamp((dist - start) / (u.camera.w - start), 0.0, 1.0);
    return mix(rgb, u.fog.rgb, f);
}

@fragment
fn fs_opaque(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(atlas, atlas_sampler, i.uv) * i.color;
    if t.a < 0.5 {
        discard;
    }
    return vec4<f32>(fogged(t.rgb, i.dist), 1.0);
}

// Entities and the hand: alpha-tested, not fogged (the hand is always close; entities fade with the chunks behind them).
@fragment
fn fs_cutout(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(atlas, atlas_sampler, i.uv) * i.color;
    if t.a < 0.1 {
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

// The first-person hand: alpha-tested and never fogged.
@fragment
fn fs_hand(i: VOut) -> @location(0) vec4<f32> {
    let t = textureSample(atlas, atlas_sampler, i.uv) * i.color;
    if t.a < 0.1 {
        discard;
    }
    return vec4<f32>(t.rgb, 1.0);
}

// Sun, moon and stars: added on top of the sky, never fogged.
@fragment
fn fs_celestial(i: VOut) -> @location(0) vec4<f32> {
    return textureSample(atlas, atlas_sampler, i.uv) * i.color;
}

struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

// One triangle covering the screen.
@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    var o: SkyOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    o.clip = vec4<f32>(p, 1.0, 1.0);
    o.ndc = p;
    return o;
}

@fragment
fn fs_sky(i: SkyOut) -> @location(0) vec4<f32> {
    let far = u.inv_view_proj * vec4<f32>(i.ndc, 1.0, 1.0);
    let near = u.inv_view_proj * vec4<f32>(i.ndc, 0.0, 1.0);
    let dir = normalize(far.xyz / far.w - near.xyz / near.w);
    // Vanilla: sky color overhead fading into the fog color at the horizon, darker below it.
    let up = smoothstep(-0.02, 0.45, dir.y);
    var c = mix(u.fog.rgb, u.sky.rgb, up);
    if dir.y < 0.0 {
        c = mix(c, u.sky.rgb * vec3<f32>(0.2, 0.2, 0.6), smoothstep(0.0, -0.4, dir.y));
    }
    // Sunrise and sunset glow on the sun's side of the sky.
    let toward = max(dot(normalize(vec2<f32>(dir.x, dir.z) + vec2<f32>(1e-5, 0.0)), normalize(u.sun.xz + vec2<f32>(1e-5, 0.0))), 0.0);
    let band = exp(-abs(dir.y - 0.05) * 6.0);
    c = mix(c, u.sunrise.rgb, clamp(u.sun.w * toward * toward * band, 0.0, 1.0));
    return vec4<f32>(c, 1.0);
}

struct GuiOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
};

@vertex
fn vs_gui(@location(0) pos: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) color: vec4<f32>) -> GuiOut {
    var o: GuiOut;
    let ndc = pos / u.screen.xy * 2.0 - 1.0;
    o.clip = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    o.uv = uv;
    o.color = vec4<f32>(pow(color.rgb, vec3<f32>(2.2)), color.a);
    return o;
}

@fragment
fn fs_gui(i: GuiOut) -> @location(0) vec4<f32> {
    // GUI pixels map to many screen pixels; always sample the full-resolution level.
    return textureSampleLevel(atlas, atlas_sampler, i.uv, 0.0) * i.color;
}

@fragment
fn fs_gui_blur(i: GuiOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(atlas, atlas_sampler, i.uv, 3.5) * i.color;
}
