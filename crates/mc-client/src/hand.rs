//! First-person hand, ported from vanilla's `FirstPersonHandsAndItemsRenderer`: the same pose-stack transforms for
//! the empty arm and for held items (swing, equip dip, view-sway lag), and the item model's own
//! `firstperson_righthand` display transform. Also draws item models for dropped items.

use crate::models::{self, Model};
use glam::{Mat4, Quat, Vec3};
use mc_assets::{ItemInfo, Quad, Sprite, Transform};
use mc_render::Vertex;

pub struct Pose {
    /// Camera space (x right, y up, -z forward, already bobbed) to world space.
    pub cam_to_world: Mat4,
    /// Swing progress 0..1, interpolated.
    pub attack: f32,
    /// `1 - equip height`: 0 when the item is fully raised, 1 when fully lowered.
    pub inverse_height: f32,
    /// Degrees of hand lag behind the view: `(viewXRot - xBob) * 0.1`, `(viewYRot - yBob) * 0.1`.
    pub sway: [f32; 2],
    /// 0..1 brightness at the player.
    pub light: f32,
}

fn deg(a: f32) -> f32 {
    a.to_radians()
}

/// The lag rotation every hand render starts with.
fn base(p: &Pose) -> Mat4 {
    p.cam_to_world * Mat4::from_rotation_x(deg(p.sway[0])) * Mat4::from_rotation_y(deg(p.sway[1]))
}

/// Steve's right arm, as `renderPlayerArm` + `AvatarRenderer.renderHand`.
pub fn arm(out: &mut Vec<Vertex>, p: &Pose, player: &Model, skin: Sprite) {
    let a = p.attack;
    let sq = a.sqrt();
    let pi = std::f32::consts::PI;
    let x_swing = -0.3 * (sq * pi).sin();
    let y_swing = 0.4 * (sq * pi * 2.0).sin();
    let z_swing = -0.4 * (a * pi).sin();
    let m = base(p)
        * Mat4::from_translation(Vec3::new(x_swing + 0.640_000_05, y_swing - 0.6 + p.inverse_height * -0.6, z_swing - 0.719_999_97))
        * Mat4::from_rotation_y(deg(45.0))
        * Mat4::from_rotation_y(deg((sq * pi).sin() * 70.0))
        * Mat4::from_rotation_z(deg((a * a * pi).sin() * -20.0))
        * Mat4::from_translation(Vec3::new(-1.0, 3.6, 3.5))
        * Mat4::from_rotation_z(deg(120.0))
        * Mat4::from_rotation_x(deg(200.0))
        * Mat4::from_rotation_y(deg(-135.0))
        * Mat4::from_translation(Vec3::new(5.6, 0.0, 0.0));
    let mut d = models::Draw { out, sprite: skin, light: p.light, tint: [1.0; 3] };
    // renderHand resets the arm's pose and tilts it by zRot 0.1.
    player.render_part("right_arm", m, &|_, rot| *rot = [0.0, 0.0, 0.1], &mut d);
}

/// `ItemTransform.apply`: translate, rotate X then Y then Z, scale.
pub fn display(t: &Transform) -> Mat4 {
    let [rx, ry, rz] = t.rotation.map(deg);
    Mat4::from_translation(Vec3::from(t.translation)) * Mat4::from_quat(Quat::from_euler(glam::EulerRot::XYZ, rx, ry, rz)) * Mat4::from_scale(Vec3::from(t.scale))
}

/// The held item, as `submitArmWithItem` for a plain item (no use animation).
pub fn item(out: &mut Vec<Vertex>, p: &Pose, item: &ItemInfo) {
    let a = p.attack;
    let pi = std::f32::consts::PI;
    let mut m = base(p) * Mat4::from_translation(Vec3::new(0.56, -0.52 + p.inverse_height * -0.6, -0.72));
    if a > 0.0 {
        // swingArm + applyItemArmAttackTransform.
        let sq = a.sqrt();
        m = m
            * Mat4::from_translation(Vec3::new(-0.4 * (sq * pi).sin(), 0.2 * (sq * pi * 2.0).sin(), -0.2 * (a * pi).sin()))
            * Mat4::from_rotation_y(deg(45.0 + (a * a * pi).sin() * -20.0))
            * Mat4::from_rotation_z(deg((sq * pi).sin() * -20.0))
            * Mat4::from_rotation_x(deg((sq * pi).sin() * -80.0))
            * Mat4::from_rotation_y(deg(-45.0));
    }
    model(out, item, m * display(&item.hand), p.light);
}

/// Draws an item model with `m` mapping model space (0..1, centered on 0.5) like vanilla's item renderer.
pub fn model(out: &mut Vec<Vertex>, item: &ItemInfo, m: Mat4, light: f32) {
    let Some(quads) = &item.model else { return };
    let m = m * Mat4::from_translation(Vec3::splat(-0.5));
    let tint = item.tint.unwrap_or([255, 255, 255]);
    for q in quads.iter() {
        emit(out, q, m, light, if q.tinted { item.model_tint } else { tint });
    }
}

fn emit(out: &mut Vec<Vertex>, q: &Quad, m: Mat4, light: f32, tint: [u8; 3]) {
    let pts = q.pos.map(|p| m.transform_point3(Vec3::from(p)));
    let n = (pts[1] - pts[0]).cross(pts[3] - pts[0]).normalize_or_zero();
    let b = light * models::face_light(n);
    let color = [(tint[0] as f32 * b) as u8, (tint[1] as f32 * b) as u8, (tint[2] as f32 * b) as u8, 255];
    let v = |i: usize| Vertex { pos: pts[i].into(), uv: q.uv[i], color };
    out.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
}

/// A block state's model drawn like a block (falling sand, primed TNT), feet-centered at `pos`.
pub fn block(out: &mut Vec<Vertex>, quads: &[Quad], pos: Vec3, light: f32, tint: [u8; 3]) {
    let m = Mat4::from_translation(pos - Vec3::new(0.5, 0.0, 0.5));
    for q in quads {
        let c = (255.0 * light * q.shade) as u8;
        let t = if q.tinted { tint } else { [255; 3] };
        let color = [(t[0] as u32 * c as u32 / 255) as u8, (t[1] as u32 * c as u32 / 255) as u8, (t[2] as u32 * c as u32 / 255) as u8, 255];
        let v = |i: usize| Vertex { pos: m.transform_point3(Vec3::from(q.pos[i])).into(), uv: q.uv[i], color };
        out.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
    }
}

/// A dropped item: bobbing and spinning like `ItemEntityRenderer`, with the model's `ground` transform.
pub fn dropped(out: &mut Vec<Vertex>, item: &ItemInfo, pos: Vec3, age: f32, offset: f32, count: u32, light: f32) {
    let bob = ((age / 10.0 + offset).sin() * 0.1 + 0.1) as f32;
    let spin = age / 20.0 + offset;
    // Stacks show more copies, as vanilla does (1, 2, 3, 4 or 5 models).
    let copies = match count {
        0..=1 => 1,
        2..=16 => 2,
        17..=32 => 3,
        33..=48 => 4,
        _ => 5,
    };
    let mut seed = (offset * 1000.0) as u32 | 1;
    let mut rand = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed % 1000) as f32 / 1000.0 * 2.0 - 1.0
    };
    let ground = display(&item.ground);
    let scale_y = item.ground.scale[1];
    for i in 0..copies {
        let jitter = if i == 0 { Vec3::ZERO } else if item.block { Vec3::new(rand(), rand(), rand()) * 0.15 } else { Vec3::new(rand() * 0.15, rand() * 0.15, -0.09375 * i as f32) };
        let m = Mat4::from_translation(pos + Vec3::new(0.0, bob + 0.25 * scale_y, 0.0)) * Mat4::from_rotation_y(spin) * Mat4::from_translation(jitter) * ground;
        model(out, item, m, light);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose() -> Pose {
        Pose { cam_to_world: Mat4::IDENTITY, attack: 0.0, inverse_height: 0.0, sway: [0.0; 2], light: 1.0 }
    }

    /// In camera space (x right, y up, -z forward) the hand must sit low, to the right and in front.
    fn check(out: &[Vertex]) {
        assert!(!out.is_empty());
        let n = out.len() as f32;
        let c = out.iter().fold(Vec3::ZERO, |a, v| a + Vec3::from(v.pos)) / n;
        assert!(c.x > 0.1 && c.y < 0.0 && c.z < -0.2, "hand center {c}");
    }

    #[test]
    fn arm_is_in_view() {
        let mut out = Vec::new();
        arm(&mut out, &pose(), &models::player(), Sprite { w: 64.0, h: 64.0, uv: [0.0, 0.0, 1.0, 1.0] });
        check(&out);
    }

    #[test]
    fn held_item_is_in_view() {
        let quad = Quad { pos: [[0.0, 1.0, 0.5], [0.0, 0.0, 0.5], [1.0, 0.0, 0.5], [1.0, 1.0, 0.5]], uv: [[0.0; 2]; 4], cull: None, shade: 1.0, tinted: false, translucent: false };
        let hand = Transform { rotation: [0.0, -90.0, 25.0], translation: [1.13 / 16.0, 3.2 / 16.0, 1.13 / 16.0], scale: [0.68; 3] };
        let item = ItemInfo { name: "x".into(), icon: None, tint: None, tool: None, block: false, model: Some(std::sync::Arc::new(vec![quad])), model_tint: [255; 3], hand, gui: Transform::default(), ground: Transform::default() };
        let mut out = Vec::new();
        self::item(&mut out, &pose(), &item);
        check(&out);
    }
}
