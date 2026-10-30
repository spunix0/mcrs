//! Blockstate selection (variants + multipart) and model baking (parent chains, elements, rotations).

use crate::atlas::Atlas;
use crate::{Dir, Jar, Quad};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct VariantRef {
    pub model: String,
    pub x: i32,
    pub y: i32,
}

fn variant_ref(v: &Value) -> Option<VariantRef> {
    // A list means "pick randomly"; vanilla weights it, we take the first.
    let v = if let Some(arr) = v.as_array() { arr.first()? } else { v };
    Some(VariantRef {
        model: v["model"].as_str()?.to_string(),
        x: v["x"].as_i64().unwrap_or(0) as i32,
        y: v["y"].as_i64().unwrap_or(0) as i32,
    })
}

fn prop<'a>(props: &'a [(String, String)], key: &str) -> Option<&'a str> {
    props.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn value_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn condition(when: &Value, props: &[(String, String)]) -> bool {
    let Some(obj) = when.as_object() else { return true };
    if let Some(any) = obj.get("OR").and_then(Value::as_array) {
        return any.iter().any(|c| condition(c, props));
    }
    if let Some(all) = obj.get("AND").and_then(Value::as_array) {
        return all.iter().all(|c| condition(c, props));
    }
    obj.iter().all(|(k, v)| {
        let want = value_str(v);
        prop(props, k).is_some_and(|have| want.split('|').any(|w| w == have))
    })
}

/// Picks the model(s) a blockstate definition uses for these properties.
pub fn select(def: &Value, props: &[(String, String)]) -> Vec<VariantRef> {
    if let Some(variants) = def["variants"].as_object() {
        for (key, v) in variants {
            let matches = key.is_empty()
                || key == "normal"
                || key.split(',').all(|kv| kv.split_once('=').is_some_and(|(k, v)| prop(props, k) == Some(v)));
            if matches {
                return variant_ref(v).into_iter().collect();
            }
        }
        return Vec::new();
    }
    if let Some(parts) = def["multipart"].as_array() {
        return parts
            .iter()
            .filter(|p| p.get("when").is_none_or(|w| condition(w, props)))
            .filter_map(|p| variant_ref(&p["apply"]))
            .collect();
    }
    Vec::new()
}

fn model_file(name: &str) -> String {
    let path = name.strip_prefix("minecraft:").unwrap_or(name);
    format!("assets/minecraft/models/{path}.json")
}

fn texture_name(name: &str) -> String {
    if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") }
}

/// A model with its parent chain flattened.
struct Resolved {
    textures: HashMap<String, String>,
    elements: Vec<Value>,
    /// Display transforms by context (`gui`, `firstperson_righthand`, `ground`, ...), nearest definition wins.
    display: HashMap<String, Value>,


}

/// An item model's placement for one display context, like vanilla's `ItemTransform`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// Degrees around X, Y, Z (applied in that order).
    pub rotation: [f32; 3],
    /// In blocks.
    pub translation: [f32; 3],
    pub scale: [f32; 3],
}

impl Default for Transform {
    fn default() -> Self {
        Self { rotation: [0.0; 3], translation: [0.0; 3], scale: [1.0; 3] }
    }
}

impl Transform {
    fn parse(v: &Value) -> Self {
        let get = |k: &str, d: [f32; 3]| vec3(&v[k]).unwrap_or(d);
        let t = get("translation", [0.0; 3]).map(|c| c.clamp(-80.0, 80.0) / 16.0);
        let s = get("scale", [1.0; 3]).map(|c| c.clamp(-4.0, 4.0));
        Self { rotation: get("rotation", [0.0; 3]), translation: t, scale: s }
    }
}

/// How an item is drawn, before atlas UVs are known.
pub struct ItemModelDef {
    /// The model to bake for 3D items (block-shaped models with elements).
    pub elements_model: Option<String>,
    /// Flat icon texture (`layer0`, or a representative face of block models).
    pub icon: Option<String>,
    pub display: HashMap<String, Transform>,
}

/// A quad before atlas UVs are known.
pub struct PendingQuad {
    pos: [[f32; 3]; 4],
    uv: [[f32; 2]; 4],
    texture: String,
    cull: Option<Dir>,
    shade: f32,
    tinted: bool,
}

impl PendingQuad {
    pub fn finish(&self, atlas: &Atlas) -> Quad {
        let [u0, v0, u1, v1] = atlas.rect(&self.texture);
        let map = |[u, v]: [f32; 2]| [u0 + (u1 - u0) * (u / 16.0), v0 + (v1 - v0) * (v / 16.0)];
        Quad {
            pos: self.pos,
            uv: self.uv.map(map),
            cull: self.cull,
            shade: self.shade,
            tinted: self.tinted,
            translucent: atlas.is_translucent(&self.texture),
        }
    }
}

pub struct BakedPart {
    pub quads: Vec<PendingQuad>,
    /// Has an element covering the whole block on all six sides.
    pub full_cube: bool,
    /// Textures of that full-cube element, used to decide whether it hides neighbors.
    pub textures: Vec<String>,
    /// Element boxes after blockstate rotation, in block units: `[min x, y, z, max x, y, z]`.
    /// Rotated elements (plants, wall torches) and zero-thickness planes are left out; they don't collide.
    pub boxes: Vec<[f32; 6]>,
}

pub struct Baker {
    models: HashMap<String, Option<Arc<Resolved>>>,
    baked: HashMap<VariantRef, Arc<BakedPart>>,
    textures: Vec<String>,
    texture_ids: HashMap<String, usize>,
}

fn rotate(p: [f32; 3], axis: char, deg: f32, origin: [f32; 3]) -> [f32; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    let [x, y, z] = [p[0] - origin[0], p[1] - origin[1], p[2] - origin[2]];
    let r = match axis {
        'x' => [x, y * c - z * s, y * s + z * c],
        'y' => [x * c + z * s, y, -x * s + z * c],
        _ => [x * c - y * s, x * s + y * c, z],
    };
    [r[0] + origin[0], r[1] + origin[1], r[2] + origin[2]]
}

/// Applies the blockstate's 90° X/Y rotation to an element box and returns it in block units.
fn rotated_box(from: [f32; 3], to: [f32; 3], v: &VariantRef) -> [f32; 6] {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for corner in 0..8 {
        let mut p = [
            if corner & 1 == 0 { from[0] } else { to[0] },
            if corner & 2 == 0 { from[1] } else { to[1] },
            if corner & 4 == 0 { from[2] } else { to[2] },
        ];
        for (axis, deg) in [('x', -(v.x as f32)), ('y', -(v.y as f32))] {
            if deg != 0.0 {
                p = rotate(p, axis, deg, [8.0; 3]);
            }
        }
        for i in 0..3 {
            // Round away float noise from the 90° rotations.
            let c = (p[i] * 1000.0).round() / 16000.0;
            lo[i] = lo[i].min(c);
            hi[i] = hi[i].max(c);
        }
    }
    [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]]
}

fn vec3(v: &Value) -> Option<[f32; 3]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32])
}

fn default_uv(dir: Dir, f: [f32; 3], t: [f32; 3]) -> [f32; 4] {
    match dir {
        Dir::Down => [f[0], 16.0 - t[2], t[0], 16.0 - f[2]],
        Dir::Up => [f[0], f[2], t[0], t[2]],
        Dir::North => [16.0 - t[0], 16.0 - t[1], 16.0 - f[0], 16.0 - f[1]],
        Dir::South => [f[0], 16.0 - t[1], t[0], 16.0 - f[1]],
        Dir::West => [f[2], 16.0 - t[1], t[2], 16.0 - f[1]],
        Dir::East => [16.0 - t[2], 16.0 - t[1], 16.0 - f[2], 16.0 - f[1]],
    }
}

/// Corners top-left, bottom-left, bottom-right, top-right as seen from outside the face.
fn corners(dir: Dir, a: [f32; 3], b: [f32; 3]) -> [[f32; 3]; 4] {
    let ([x0, y0, z0], [x1, y1, z1]) = (a, b);
    match dir {
        Dir::North => [[x1, y1, z0], [x1, y0, z0], [x0, y0, z0], [x0, y1, z0]],
        Dir::South => [[x0, y1, z1], [x0, y0, z1], [x1, y0, z1], [x1, y1, z1]],
        Dir::West => [[x0, y1, z0], [x0, y0, z0], [x0, y0, z1], [x0, y1, z1]],
        Dir::East => [[x1, y1, z1], [x1, y0, z1], [x1, y0, z0], [x1, y1, z0]],
        Dir::Up => [[x0, y1, z0], [x0, y1, z1], [x1, y1, z1], [x1, y1, z0]],
        Dir::Down => [[x0, y0, z1], [x0, y0, z0], [x1, y0, z0], [x1, y0, z1]],
    }
}

fn normal(p: &[[f32; 3]; 4]) -> [f32; 3] {
    let a = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
    let b = [p[3][0] - p[0][0], p[3][1] - p[0][1], p[3][2] - p[0][2]];
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

impl Baker {
    pub fn new() -> Self {
        Self { models: HashMap::new(), baked: HashMap::new(), textures: Vec::new(), texture_ids: HashMap::new() }
    }

    fn resolve(&mut self, jar: &mut Jar, name: &str) -> Option<Arc<Resolved>> {
        if let Some(r) = self.models.get(name) {
            return r.clone();
        }
        let mut chain = Vec::new();
        let mut cur = Some(name.to_string());
        while let Some(n) = cur.take() {
            if n.starts_with("builtin/") || chain.len() > 16 {
                break;
            }
            let Some(json) = jar.json(&model_file(&n)) else { break };
            cur = json["parent"].as_str().map(str::to_string);
            chain.push(json);
        }
        let result = if chain.is_empty() {
            None
        } else {
            let mut textures = HashMap::new();
            for m in chain.iter().rev() {
                if let Some(t) = m["textures"].as_object() {
                    for (k, v) in t {
                        // 26.3 also allows {"sprite": ..., "force_translucent": true} (glass and panes).
                        if let Some(s) = v.as_str().or_else(|| v["sprite"].as_str()) {
                            textures.insert(k.clone(), s.to_string());
                        }
                    }
                }
            }
            let elements = chain
                .iter()
                .find_map(|m| m["elements"].as_array().cloned())
                .unwrap_or_default();
            let mut display = HashMap::new();
            for m in chain.iter().rev() {
                if let Some(d) = m["display"].as_object() {
                    for (k, v) in d {
                        display.insert(k.clone(), v.clone());
                    }
                }
            }

            Some(Arc::new(Resolved { textures, elements, display }))
        };
        self.models.insert(name.to_string(), result.clone());
        result
    }

    fn texture(&mut self, model: &Resolved, reference: &str) -> String {
        let mut r = reference.to_string();
        for _ in 0..12 {
            match r.strip_prefix('#') {
                Some(key) => match model.textures.get(key) {
                    Some(next) => r = next.clone(),
                    None => return "minecraft:missingno".into(),
                },
                None => break,
            }
        }
        let name = texture_name(&r);
        if !self.texture_ids.contains_key(&name) {
            self.texture_ids.insert(name.clone(), self.textures.len());
            self.textures.push(name.clone());
        }
        name
    }

    pub fn bake(&mut self, jar: &mut Jar, v: &VariantRef) -> Arc<BakedPart> {
        if let Some(b) = self.baked.get(v) {
            return b.clone();
        }
        let mut part = BakedPart { quads: Vec::new(), full_cube: false, textures: Vec::new(), boxes: Vec::new() };
        if let Some(model) = self.resolve(jar, &v.model) {
            for el in &model.elements {
                let (Some(from), Some(to)) = (vec3(&el["from"]), vec3(&el["to"])) else { continue };
                let faces = el["faces"].as_object().cloned().unwrap_or_default();
                let shade_el = el["shade"].as_bool().unwrap_or(true);
                let shade_up = el["shade_direction_override"].as_str() == Some("up");
                let rot = el.get("rotation").filter(|r| r.is_object());
                let is_full = from == [0.0; 3] && to == [16.0; 3] && faces.len() == 6;
                for (dname, face) in &faces {
                    let Some(dir) = Dir::parse(dname) else { continue };
                    let uvr = face["uv"]
                        .as_array()
                        .and_then(|a| Some([a.first()?.as_f64()? as f32, a.get(1)?.as_f64()? as f32, a.get(2)?.as_f64()? as f32, a.get(3)?.as_f64()? as f32]))
                        .unwrap_or_else(|| default_uv(dir, from, to));
                    let mut uv = [[uvr[0], uvr[1]], [uvr[0], uvr[3]], [uvr[2], uvr[3]], [uvr[2], uvr[1]]];
                    let steps = (face["rotation"].as_i64().unwrap_or(0) / 90).rem_euclid(4) as usize;
                    uv.rotate_left(steps);

                    let mut pos = corners(dir, from, to);
                    if let Some(r) = rot {
                        let origin = vec3(&r["origin"]).unwrap_or([8.0; 3]);
                        let axis = r["axis"].as_str().unwrap_or("y").chars().next().unwrap_or('y');
                        let angle = r["angle"].as_f64().unwrap_or(0.0) as f32;
                        let rescale = r["rescale"].as_bool().unwrap_or(false);
                        let scale = if rescale { 1.0 / angle.to_radians().cos() } else { 1.0 };
                        for p in &mut pos {
                            *p = rotate(*p, axis, angle, origin);
                            if rescale {
                                for (i, a) in ['x', 'y', 'z'].iter().enumerate() {
                                    if *a != axis {
                                        p[i] = origin[i] + (p[i] - origin[i]) * scale;
                                    }
                                }
                            }
                        }
                    }
                    let mut cull = face["cullface"].as_str().and_then(Dir::parse);
                    // Blockstate rotation: X first, then Y, both negated like vanilla.
                    for (axis, deg) in [('x', -(v.x as f32)), ('y', -(v.y as f32))] {
                        if deg != 0.0 {
                            for p in &mut pos {
                                *p = rotate(*p, axis, deg, [8.0; 3]);
                            }
                            cull = cull.map(|c| {
                                let o = c.offset();
                                Dir::from_normal(rotate([o[0] as f32, o[1] as f32, o[2] as f32], axis, deg, [0.0; 3]))
                            });
                        }
                    }
                    for p in &mut pos {
                        *p = [p[0] / 16.0, p[1] / 16.0, p[2] / 16.0];
                    }
                    let texture = self.texture(&model, face["texture"].as_str().unwrap_or("#missing"));
                    if is_full {
                        part.textures.push(texture.clone());
                    }
                    let shade = if !shade_el || shade_up { 1.0 } else { Dir::from_normal(normal(&pos)).shade() };
                    part.quads.push(PendingQuad { pos, uv, texture, cull, shade, tinted: face.get("tintindex").is_some() });
                }
                part.full_cube |= is_full;
                let tilted = rot.is_some_and(|r| r["angle"].as_f64().unwrap_or(0.0) != 0.0);
                let flat = (0..3).any(|i| (to[i] - from[i]).abs() < 0.01);
                if !tilted && !flat {
                    part.boxes.push(rotated_box(from, to, v));
                }
            }
        }
        let part = Arc::new(part);
        self.baked.insert(v.clone(), part.clone());
        part
    }

    /// Picks a flat icon texture for an item: `layer0` for drawn items, a representative face for block items.
    /// Registers the texture so it lands in the atlas.
    /// Returns the texture and whether the item is drawn as a block.
    /// Everything needed to draw an item: its 3D model (if block-shaped), flat icon and display transforms.
    pub fn item_model(&mut self, jar: &mut Jar, item: &str) -> Option<ItemModelDef> {
        fn first_model(v: &Value) -> Option<String> {
            match v {
                Value::Object(o) => {
                    if let (Some(Value::String(m)), true) = (o.get("model"), o.get("type").and_then(Value::as_str) == Some("minecraft:model")) {
                        return Some(m.clone());
                    }
                    o.values().find_map(first_model)
                }
                Value::Array(a) => a.iter().find_map(first_model),
                _ => None,
            }
        }
        let def = jar.json(&format!("assets/minecraft/items/{item}.json"))?;
        let model_name = first_model(&def)?;
        let model = self.resolve(jar, &model_name)?;
        let icon = self.item_icon(jar, item).map(|(t, _)| t);
        let display = model.display.iter().map(|(k, v)| (k.clone(), Transform::parse(v))).collect();
        let elements_model = (!model.elements.is_empty()).then(|| model_name.clone());
        if let Some(m) = &elements_model {
            // Bake now so its textures are registered before the atlas is stitched.
            self.bake(jar, &VariantRef { model: m.clone(), x: 0, y: 0 });
        }
        Some(ItemModelDef { elements_model, icon, display })
    }

    pub fn item_icon(&mut self, jar: &mut Jar, item: &str) -> Option<(String, bool)> {
        fn first_model(v: &Value) -> Option<String> {
            match v {
                Value::Object(o) => {
                    if let (Some(Value::String(m)), true) = (o.get("model"), o.get("type").and_then(Value::as_str) == Some("minecraft:model")) {
                        return Some(m.clone());
                    }
                    o.values().find_map(first_model)
                }
                Value::Array(a) => a.iter().find_map(first_model),
                _ => None,
            }
        }
        let def = jar.json(&format!("assets/minecraft/items/{item}.json"))?;
        let model_name = first_model(&def)?;
        let model = self.resolve(jar, &model_name)?;
        let key = ["layer0", "all", "side", "front", "north", "texture", "top", "particle"]
            .into_iter()
            .find(|k| model.textures.contains_key(*k))?;
        Some((self.texture(&model, &format!("#{key}")), model_name.contains("block/")))
    }

    /// Loads every referenced texture (plus fluids) and packs the atlas.
    pub fn stitch(&mut self, jar: &mut Jar) -> anyhow::Result<Atlas> {
        let fluids = ["water_still", "lava_still", "water_flow", "lava_flow"].map(|t| format!("minecraft:block/{t}"));
        let cracks = (0..10).map(|i| format!("minecraft:block/destroy_stage_{i}"));
        for extra in fluids.into_iter().chain(cracks) {
            if !self.texture_ids.contains_key(&extra) {
                self.texture_ids.insert(extra.clone(), self.textures.len());
                self.textures.push(extra);
            }
        }
        Atlas::build(jar, &self.textures)
    }
}
