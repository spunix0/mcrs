//! Vanilla's entity models as data: boxes with texture offsets, parts with pivots and rotations, children. Built
//! from the `createBodyLayer` definitions in the game's model classes and rendered the way `ModelPart` does.

use glam::{Mat4, Quat, Vec3};
use mc_assets::Sprite;
use mc_render::Vertex;

#[derive(Clone)]
pub struct Cube {
    uv: [f32; 2],
    min: [f32; 3],
    size: [f32; 3],
    grow: f32,
    mirror: bool,
}

#[derive(Clone)]
pub struct Part {
    pub name: &'static str,
    pivot: [f32; 3],
    rot: [f32; 3],
    cubes: Vec<Cube>,
    children: Vec<Part>,
}

#[derive(Clone)]
pub struct Model {
    /// Texture size in pixels (`LayerDefinition.create(mesh, w, h)`).
    pub tex: [f32; 2],
    pub parts: Vec<Part>,
}

/// `texOffs(u, v).addBox(x, y, z, w, h, d)`.
fn c(u: f32, v: f32, x: f32, y: f32, z: f32, w: f32, h: f32, d: f32) -> Cube {
    Cube { uv: [u, v], min: [x, y, z], size: [w, h, d], grow: 0.0, mirror: false }
}

impl Cube {
    fn grow(mut self, g: f32) -> Self {
        self.grow = g;
        self
    }

    fn mirror(mut self) -> Self {
        self.mirror = true;
        self
    }
}

fn p(name: &'static str, pivot: [f32; 3], rot: [f32; 3], cubes: Vec<Cube>) -> Part {
    Part { name, pivot, rot, cubes, children: Vec::new() }
}

impl Part {
    fn with(mut self, children: Vec<Part>) -> Self {
        self.children = children;
        self
    }
}

const HALF_PI: f32 = std::f32::consts::FRAC_PI_2;

/// Per-frame pose: rotation overrides by part name (radians), like `setupAnim`.
pub type Pose<'a> = &'a dyn Fn(&str, &mut [f32; 3]);

/// Where model geometry goes and how it's lit.
pub struct Draw<'a> {
    pub out: &'a mut Vec<Vertex>,
    pub sprite: Sprite,
    /// 0..1 brightness at the entity.
    pub light: f32,
    /// Multiplied into the texture (red when hurt).
    pub tint: [f32; 3],
}

/// Vanilla's two entity light directions.
pub fn face_light(n: Vec3) -> f32 {
    let l0 = Vec3::new(0.2, 1.0, -0.7).normalize();
    let l1 = Vec3::new(-0.2, 1.0, 0.7).normalize();
    (0.4 + 0.6 * (n.dot(l0).max(0.0) + n.dot(l1).max(0.0))).min(1.0)
}

impl Model {
    /// Renders every part with `m` mapping model units (1/16 block) to world space.
    pub fn render(&self, m: Mat4, pose: Pose, d: &mut Draw) {
        for part in &self.parts {
            render_part(part, m, pose, d, self.tex);
        }
    }

    /// Renders one top-level part (the first-person arm).
    pub fn render_part(&self, name: &str, m: Mat4, pose: Pose, d: &mut Draw) {
        if let Some(part) = self.parts.iter().find(|p| p.name == name) {
            render_part(part, m, pose, d, self.tex);
        }
    }
}

fn render_part(part: &Part, parent: Mat4, pose: Pose, d: &mut Draw, tex: [f32; 2]) {
    let mut rot = part.rot;
    pose(part.name, &mut rot);
    // translateAndRotate: offset in pixels, then rotationZYX(z, y, x).
    let m = parent
        * Mat4::from_translation(Vec3::from(part.pivot) / 16.0)
        * Mat4::from_quat(Quat::from_rotation_z(rot[2]) * Quat::from_rotation_y(rot[1]) * Quat::from_rotation_x(rot[0]));
    for cube in &part.cubes {
        emit_cube(cube, m, d, tex);
    }
    for child in &part.children {
        render_part(child, m, pose, d, tex);
    }
}

/// `ModelPart.Cube`: eight corners and the standard box UV layout.
fn emit_cube(cube: &Cube, m: Mat4, d: &mut Draw, tex: [f32; 2]) {
    let [x, y, z] = cube.min;
    let [w, h, dd] = cube.size;
    let g = cube.grow;
    let (mut x0, y0, z0, mut x1, y1, z1) = (x - g, y - g, z - g, x + w + g, y + h + g, z + dd + g);
    if cube.mirror {
        std::mem::swap(&mut x0, &mut x1);
    }
    let t0 = Vec3::new(x0, y0, z0);
    let t1 = Vec3::new(x1, y0, z0);
    let t2 = Vec3::new(x1, y1, z0);
    let t3 = Vec3::new(x0, y1, z0);
    let l0 = Vec3::new(x0, y0, z1);
    let l1 = Vec3::new(x1, y0, z1);
    let l2 = Vec3::new(x1, y1, z1);
    let l3 = Vec3::new(x0, y1, z1);
    let [u, v] = cube.uv;
    let (u0, u1, u2, u22, u3, u4) = (u, u + dd, u + dd + w, u + dd + w + w, u + dd + w + dd, u + dd + w + dd + w);
    let (v0, v1, v2) = (v, v + dd, v + dd + h);
    // (corners, u0, v0, u1, v1, normal in model space)
    let faces: [([Vec3; 4], f32, f32, f32, f32, Vec3); 6] = [
        ([l1, l0, t0, t1], u1, v0, u2, v1, Vec3::NEG_Y),
        ([t2, t3, l3, l2], u2, v1, u22, v0, Vec3::Y),
        ([t0, l0, l3, t3], u0, v1, u1, v2, Vec3::NEG_X),
        ([t1, t0, t3, t2], u1, v1, u2, v2, Vec3::NEG_Z),
        ([l1, t1, t2, l2], u2, v1, u3, v2, Vec3::X),
        ([l0, l1, l2, l3], u3, v1, u4, v2, Vec3::Z),
    ];
    let [su0, sv0, su1, sv1] = d.sprite.uv;
    let map = |pu: f32, pv: f32| [su0 + (su1 - su0) * pu / tex[0], sv0 + (sv1 - sv0) * pv / tex[1]];
    for (corners, fu0, fv0, fu1, fv1, n) in faces {
        // Polygon: vertex 0 gets (u1, v0), 1 (u0, v0), 2 (u0, v1), 3 (u1, v1); mirrored boxes swap left and right.
        let mut uv = [map(fu1, fv0), map(fu0, fv0), map(fu0, fv1), map(fu1, fv1)];
        let mut corners = corners;
        let mut n = n;
        if cube.mirror {
            corners.reverse();
            uv.reverse();
            if n.x != 0.0 {
                n = -n;
            }
        }
        let normal = m.transform_vector3(n).normalize_or_zero();
        let b = d.light * face_light(normal);
        let color = [(255.0 * b * d.tint[0]) as u8, (255.0 * b * d.tint[1]) as u8, (255.0 * b * d.tint[2]) as u8, 255];
        let vert = |i: usize| Vertex { pos: m.transform_point3(corners[i] / 16.0).into(), uv: uv[i], color };
        d.out.extend_from_slice(&[vert(0), vert(1), vert(2), vert(0), vert(2), vert(3)]);
    }
}

/// Player model (64x64 skin, wide arms) with the outer layers.
pub fn player() -> Model {
    let head = p("head", [0.0, 0.0, 0.0], [0.0; 3], vec![c(0., 0., -4., -8., -4., 8., 8., 8.)])
        .with(vec![p("hat", [0.0; 3], [0.0; 3], vec![c(32., 0., -4., -8., -4., 8., 8., 8.).grow(0.5)])]);
    Model {
        tex: [64.0, 64.0],
        parts: vec![
            head,
            p("body", [0.0; 3], [0.0; 3], vec![c(16., 16., -4., 0., -2., 8., 12., 4.), c(16., 32., -4., 0., -2., 8., 12., 4.).grow(0.25)]),
            p("right_arm", [-5., 2., 0.], [0.0; 3], vec![c(40., 16., -3., -2., -2., 4., 12., 4.), c(40., 32., -3., -2., -2., 4., 12., 4.).grow(0.25)]),
            p("left_arm", [5., 2., 0.], [0.0; 3], vec![c(32., 48., -1., -2., -2., 4., 12., 4.), c(48., 48., -1., -2., -2., 4., 12., 4.).grow(0.25)]),
            p("right_leg", [-1.9, 12., 0.], [0.0; 3], vec![c(0., 16., -2., 0., -2., 4., 12., 4.), c(0., 32., -2., 0., -2., 4., 12., 4.).grow(0.25)]),
            p("left_leg", [1.9, 12., 0.], [0.0; 3], vec![c(16., 48., -2., 0., -2., 4., 12., 4.), c(0., 48., -2., 0., -2., 4., 12., 4.).grow(0.25)]),
        ],
    }
}

/// `HumanoidModel` (zombies and friends, 64x64 with mirrored left limbs).
pub fn humanoid() -> Model {
    Model {
        tex: [64.0, 64.0],
        parts: vec![
            p("head", [0.0; 3], [0.0; 3], vec![c(0., 0., -4., -8., -4., 8., 8., 8.)])
                .with(vec![p("hat", [0.0; 3], [0.0; 3], vec![c(32., 0., -4., -8., -4., 8., 8., 8.).grow(0.5)])]),
            p("body", [0.0; 3], [0.0; 3], vec![c(16., 16., -4., 0., -2., 8., 12., 4.)]),
            p("right_arm", [-5., 2., 0.], [0.0; 3], vec![c(40., 16., -3., -2., -2., 4., 12., 4.)]),
            p("left_arm", [5., 2., 0.], [0.0; 3], vec![c(40., 16., -1., -2., -2., 4., 12., 4.).mirror()]),
            p("right_leg", [-1.9, 12., 0.], [0.0; 3], vec![c(0., 16., -2., 0., -2., 4., 12., 4.)]),
            p("left_leg", [1.9, 12., 0.], [0.0; 3], vec![c(0., 16., -2., 0., -2., 4., 12., 4.).mirror()]),
        ],
    }
}

/// `SkeletonModel`: thin limbs, 64x32.
pub fn skeleton() -> Model {
    Model {
        tex: [64.0, 32.0],
        parts: vec![
            p("head", [0.0; 3], [0.0; 3], vec![c(0., 0., -4., -8., -4., 8., 8., 8.)]),
            p("body", [0.0; 3], [0.0; 3], vec![c(16., 16., -4., 0., -2., 8., 12., 4.)]),
            p("right_arm", [-5., 2., 0.], [0.0; 3], vec![c(40., 16., -1., -2., -1., 2., 12., 2.)]),
            p("left_arm", [5., 2., 0.], [0.0; 3], vec![c(40., 16., -1., -2., -1., 2., 12., 2.).mirror()]),
            p("right_leg", [-2., 12., 0.], [0.0; 3], vec![c(0., 16., -1., 0., -1., 2., 12., 2.)]),
            p("left_leg", [2., 12., 0.], [0.0; 3], vec![c(0., 16., -1., 0., -1., 2., 12., 2.).mirror()]),
        ],
    }
}

/// `QuadrupedModel.createBodyMesh` with a custom head.
fn quadruped(leg: f32, tex: [f32; 2], head: Part, body: Part, mirror_left: bool, mirror_right: bool) -> Model {
    let legc = |m: bool| {
        let l = c(0., 16., -2., 0., -2., 4., leg, 4.);
        if m { l.mirror() } else { l }
    };
    Model {
        tex,
        parts: vec![
            head,
            body,
            p("right_hind_leg", [-3., 24. - leg, 7.], [0.0; 3], vec![legc(mirror_right)]),
            p("left_hind_leg", [3., 24. - leg, 7.], [0.0; 3], vec![legc(mirror_left)]),
            p("right_front_leg", [-3., 24. - leg, -5.], [0.0; 3], vec![legc(mirror_right)]),
            p("left_front_leg", [3., 24. - leg, -5.], [0.0; 3], vec![legc(mirror_left)]),
        ],
    }
}

pub fn pig() -> Model {
    let head = p("head", [0., 12., -6.], [0.0; 3], vec![c(0., 0., -4., -4., -8., 8., 8., 8.), c(16., 16., -2., 0., -9., 4., 3., 1.)]);
    let body = p("body", [0., 11., 2.], [HALF_PI, 0., 0.], vec![c(28., 8., -5., -10., -7., 10., 16., 8.)]);
    quadruped(6.0, [64.0, 64.0], head, body, true, false)
}

pub fn cow() -> Model {
    let head = p(
        "head",
        [0., 4., -8.],
        [0.0; 3],
        vec![c(0., 0., -4., -4., -6., 8., 8., 6.), c(1., 33., -3., 1., -7., 6., 3., 1.), c(22., 0., -5., -5., -5., 1., 3., 1.), c(22., 0., 4., -5., -5., 1., 3., 1.)],
    );
    let body = p("body", [0., 5., 2.], [HALF_PI, 0., 0.], vec![c(18., 4., -6., -10., -7., 12., 18., 10.), c(52., 0., -2., 2., -8., 4., 6., 1.)]);
    let mut m = quadruped(12.0, [64.0, 64.0], head, body, true, false);
    // Cows stand wider than the generic quadruped.
    for part in &mut m.parts {
        if part.name.contains("leg") {
            part.pivot[0] = part.pivot[0].signum() * 4.0;
        }
    }
    m
}

pub fn sheep() -> Model {
    let head = p("head", [0., 6., -8.], [0.0; 3], vec![c(0., 0., -3., -4., -6., 6., 6., 8.)]);
    let body = p("body", [0., 5., 2.], [HALF_PI, 0., 0.], vec![c(28., 8., -4., -10., -7., 8., 16., 6.)]);
    quadruped(12.0, [64.0, 32.0], head, body, false, true)
}

/// The sheep's wool layer (`SheepFurModel`), drawn over the body with `sheep_wool`.
pub fn sheep_wool() -> Model {
    let leg = c(0., 16., -2., 0., -2., 4., 6., 4.).grow(0.5);
    Model {
        tex: [64.0, 32.0],
        parts: vec![
            p("head", [0., 6., -8.], [0.0; 3], vec![c(0., 0., -3., -4., -4., 6., 6., 6.).grow(0.6)]),
            p("body", [0., 5., 2.], [HALF_PI, 0., 0.], vec![c(28., 8., -4., -10., -7., 8., 16., 6.).grow(1.75)]),
            p("right_hind_leg", [-3., 12., 7.], [0.0; 3], vec![leg.clone()]),
            p("left_hind_leg", [3., 12., 7.], [0.0; 3], vec![leg.clone()]),
            p("right_front_leg", [-3., 12., -5.], [0.0; 3], vec![leg.clone()]),
            p("left_front_leg", [3., 12., -5.], [0.0; 3], vec![leg]),
        ],
    }
}

pub fn chicken() -> Model {
    let leg = c(26., 0., -1., 0., -3., 3., 5., 3.);
    Model {
        tex: [64.0, 32.0],
        parts: vec![
            p("head", [0., 15., -4.], [0.0; 3], vec![c(0., 0., -2., -6., -2., 4., 6., 3.)]).with(vec![
                p("beak", [0.0; 3], [0.0; 3], vec![c(14., 0., -2., -4., -4., 4., 2., 2.)]),
                p("red_thing", [0.0; 3], [0.0; 3], vec![c(14., 4., -1., -2., -3., 2., 2., 2.)]),
            ]),
            p("body", [0., 16., 0.], [HALF_PI, 0., 0.], vec![c(0., 9., -3., -4., -3., 6., 8., 6.)]),
            p("right_leg", [-2., 19., 1.], [0.0; 3], vec![leg.clone()]),
            p("left_leg", [1., 19., 1.], [0.0; 3], vec![leg]),
            p("right_wing", [-4., 13., 0.], [0.0; 3], vec![c(24., 13., 0., 0., -3., 1., 4., 6.)]),
            p("left_wing", [4., 13., 0.], [0.0; 3], vec![c(24., 13., -1., 0., -3., 1., 4., 6.)]),
        ],
    }
}

pub fn creeper() -> Model {
    let leg = c(0., 16., -2., 0., -2., 4., 6., 4.);
    Model {
        tex: [64.0, 32.0],
        parts: vec![
            p("head", [0., 6., 0.], [0.0; 3], vec![c(0., 0., -4., -8., -4., 8., 8., 8.)]),
            p("body", [0., 6., 0.], [0.0; 3], vec![c(16., 16., -4., 0., -2., 8., 12., 4.)]),
            p("right_hind_leg", [-2., 18., 4.], [0.0; 3], vec![leg.clone()]),
            p("left_hind_leg", [2., 18., 4.], [0.0; 3], vec![leg.clone()]),
            p("right_front_leg", [-2., 18., -4.], [0.0; 3], vec![leg.clone()]),
            p("left_front_leg", [2., 18., -4.], [0.0; 3], vec![leg]),
        ],
    }
}

pub fn spider() -> Model {
    let right = c(18., 0., -15., -1., -1., 16., 2., 2.);
    let left = c(18., 0., -1., -1., -1., 16., 2., 2.).mirror();
    let q = std::f32::consts::FRAC_PI_4;
    let e = std::f32::consts::FRAC_PI_8;
    let m = 0.581_194_6;
    Model {
        tex: [64.0, 32.0],
        parts: vec![
            p("head", [0., 15., -3.], [0.0; 3], vec![c(32., 4., -4., -4., -8., 8., 8., 8.)]),
            p("body0", [0., 15., 0.], [0.0; 3], vec![c(0., 0., -3., -3., -3., 6., 6., 6.)]),
            p("body1", [0., 15., 9.], [0.0; 3], vec![c(0., 12., -5., -4., -6., 10., 8., 12.)]),
            p("right_hind_leg", [-4., 15., 2.], [0., q, -q], vec![right.clone()]),
            p("left_hind_leg", [4., 15., 2.], [0., -q, q], vec![left.clone()]),
            p("right_middle_hind_leg", [-4., 15., 1.], [0., e, -m], vec![right.clone()]),
            p("left_middle_hind_leg", [4., 15., 1.], [0., -e, m], vec![left.clone()]),
            p("right_middle_front_leg", [-4., 15., 0.], [0., -e, -m], vec![right.clone()]),
            p("left_middle_front_leg", [4., 15., 0.], [0., e, m], vec![left.clone()]),
            p("right_front_leg", [-4., 15., -1.], [0., -q, -q], vec![right]),
            p("left_front_leg", [4., 15., -1.], [0., q, q], vec![left]),
        ],
    }
}

pub fn enderman() -> Model {
    let limb = |x: f32, y: f32, z: f32| c(56., 0., x, y, z, 2., 30., 2.);
    Model {
        tex: [64.0, 32.0],
        parts: vec![
            p("head", [0., -13., 0.], [0.0; 3], vec![c(0., 0., -4., -8., -4., 8., 8., 8.)])
                .with(vec![p("hat", [0.0; 3], [0.0; 3], vec![c(0., 16., -4., -8., -4., 8., 8., 8.).grow(-0.5)])]),
            p("body", [0., -14., 0.], [0.0; 3], vec![c(32., 16., -4., 0., -2., 8., 12., 4.)]),
            p("right_arm", [-5., -12., 0.], [0.0; 3], vec![limb(-1., -2., -1.)]),
            p("left_arm", [5., -12., 0.], [0.0; 3], vec![limb(-1., -2., -1.).mirror()]),
            p("right_leg", [-2., -5., 0.], [0.0; 3], vec![limb(-1., 0., -1.)]),
            p("left_leg", [2., -5., 0.], [0.0; 3], vec![limb(-1., 0., -1.).mirror()]),
        ],
    }
}

pub fn villager() -> Model {
    Model {
        tex: [64.0, 64.0],
        parts: vec![
            p("head", [0.0; 3], [0.0; 3], vec![c(0., 0., -4., -10., -4., 8., 10., 8.)]).with(vec![
                p("hat", [0.0; 3], [0.0; 3], vec![c(32., 0., -4., -10., -4., 8., 10., 8.).grow(0.51)]),
                p("nose", [0., -2., 0.], [0.0; 3], vec![c(24., 0., -1., -1., -6., 2., 4., 2.)]),
            ]),
            p("body", [0.0; 3], [0.0; 3], vec![c(16., 20., -4., 0., -3., 8., 12., 6.)])
                .with(vec![p("jacket", [0.0; 3], [0.0; 3], vec![c(0., 38., -4., 0., -3., 8., 20., 6.).grow(0.5)])]),
            p(
                "arms",
                [0., 3., -1.],
                [-0.75, 0., 0.],
                vec![c(44., 22., -8., -2., -2., 4., 8., 4.), c(44., 22., 4., -2., -2., 4., 8., 4.).mirror(), c(40., 38., -4., 2., -2., 8., 4., 4.)],
            ),
            p("right_leg", [-2., 12., 0.], [0.0; 3], vec![c(0., 22., -2., 0., -2., 4., 12., 4.)]),
            p("left_leg", [2., 12., 0.], [0.0; 3], vec![c(0., 22., -2., 0., -2., 4., 12., 4.).mirror()]),
        ],
    }
}

pub fn wolf() -> Model {
    let left = c(0., 18., 0., 0., -1., 2., 8., 2.);
    let right = c(0., 18., 0., 0., -1., 2., 8., 2.).mirror();
    Model {
        tex: [64.0, 32.0],
        parts: vec![
            p("head", [-1., 13.5, -7.], [0.0; 3], vec![]).with(vec![p(
                "real_head",
                [0.0; 3],
                [0.0; 3],
                vec![c(0., 0., -2., -3., -2., 6., 6., 4.), c(16., 14., -2., -5., 0., 2., 2., 1.), c(16., 14., 2., -5., 0., 2., 2., 1.), c(0., 10., -0.5, -0.001, -5., 3., 3., 4.)],
            )]),
            p("body", [0., 14., 2.], [HALF_PI, 0., 0.], vec![c(18., 14., -3., -2., -3., 6., 9., 6.)]),
            p("upper_body", [-1., 14., -3.], [HALF_PI, 0., 0.], vec![c(21., 0., -3., -3., -3., 8., 6., 7.)]),
            p("right_hind_leg", [-2.5, 16., 7.], [0.0; 3], vec![right.clone()]),
            p("left_hind_leg", [0.5, 16., 7.], [0.0; 3], vec![left.clone()]),
            p("right_front_leg", [-2.5, 16., -4.], [0.0; 3], vec![right]),
            p("left_front_leg", [0.5, 16., -4.], [0.0; 3], vec![left]),
            p("tail", [-1., 12., 8.], [std::f32::consts::PI / 5.0, 0., 0.], vec![]).with(vec![p("real_tail", [0.0; 3], [0.0; 3], vec![c(9., 18., 0., 0., -1., 2., 8., 2.)])]),
        ],
    }
}

pub fn squid() -> Model {
    let mut parts = vec![p("body", [0., 8., 0.], [0.0; 3], vec![c(0., 0., -6., -8., -6., 12., 16., 12.)])];
    const NAMES: [&str; 8] = ["tentacle0", "tentacle1", "tentacle2", "tentacle3", "tentacle4", "tentacle5", "tentacle6", "tentacle7"];
    for (i, name) in NAMES.iter().enumerate() {
        let a = i as f32 * std::f32::consts::TAU / 8.0;
        let (x, z) = (a.cos() * 5.0, a.sin() * 5.0);
        let yrot = i as f32 * -std::f32::consts::TAU / 8.0 + HALF_PI;
        parts.push(p(name, [x, 15., z], [0., yrot, 0.], vec![c(48., 0., -1., 0., -1., 2., 18., 2.)]));
    }
    Model { tex: [64.0, 32.0], parts }
}

pub fn slime() -> Model {
    Model { tex: [64.0, 32.0], parts: vec![p("cube", [0.0; 3], [0.0; 3], vec![c(0., 0., -4., 16., -4., 8., 8., 8.)])] }
}

pub fn iron_golem() -> Model {
    Model {
        tex: [128.0, 128.0],
        parts: vec![
            p("head", [0., -7., -2.], [0.0; 3], vec![c(0., 0., -4., -12., -5.5, 8., 10., 8.), c(24., 0., -1., -5., -7.5, 2., 4., 2.)]),
            p("body", [0., -7., 0.], [0.0; 3], vec![c(0., 40., -9., -2., -6., 18., 12., 11.), c(0., 70., -4.5, 10., -3., 9., 5., 6.).grow(0.5)]),
            p("right_arm", [0., -7., 0.], [0.0; 3], vec![c(60., 21., -13., -2.5, -3., 4., 30., 6.)]),
            p("left_arm", [0., -7., 0.], [0.0; 3], vec![c(60., 58., 9., -2.5, -3., 4., 30., 6.)]),
            p("right_leg", [-4., 11., 0.], [0.0; 3], vec![c(37., 0., -3.5, -3., -3., 6., 16., 5.)]),
            p("left_leg", [5., 11., 0.], [0.0; 3], vec![c(60., 0., -3.5, -3., -3., 6., 16., 5.).mirror()]),
        ],
    }
}

/// Which model and texture an entity type uses, and how it animates.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Kind {
    Player,
    Zombie,
    Skeleton,
    Quadruped,
    Sheep,
    Chicken,
    Creeper,
    Spider,
    Enderman,
    Villager,
    Wolf,
    Squid,
    Slime,
    IronGolem,
}

pub struct Mob {
    pub kind: Kind,
    pub model: Model,
    pub skin: &'static str,
    /// Extra scale (cave spiders are smaller spiders).
    pub scale: f32,
}

pub fn for_entity(name: &str) -> Option<Mob> {
    let m = |kind, model, skin| Some(Mob { kind, model, skin, scale: 1.0 });
    match name {
        "player" => m(Kind::Player, player(), "steve"),
        "zombie" | "zombie_villager" => m(Kind::Zombie, humanoid(), "zombie"),
        "husk" => m(Kind::Zombie, humanoid(), "husk"),
        "drowned" => m(Kind::Zombie, humanoid(), "drowned"),
        "giant" => Some(Mob { kind: Kind::Zombie, model: humanoid(), skin: "zombie", scale: 6.0 }),
        "skeleton" | "bogged" => m(Kind::Skeleton, skeleton(), "skeleton"),
        "stray" => m(Kind::Skeleton, skeleton(), "stray"),
        "wither_skeleton" => Some(Mob { kind: Kind::Skeleton, model: skeleton(), skin: "wither_skeleton", scale: 1.2 }),
        "pig" => m(Kind::Quadruped, pig(), "pig"),
        "cow" => m(Kind::Quadruped, cow(), "cow"),
        "mooshroom" => m(Kind::Quadruped, cow(), "mooshroom"),
        "sheep" => m(Kind::Sheep, sheep(), "sheep"),
        "chicken" => m(Kind::Chicken, chicken(), "chicken"),
        "creeper" => m(Kind::Creeper, creeper(), "creeper"),
        "spider" => m(Kind::Spider, spider(), "spider"),
        "cave_spider" => Some(Mob { kind: Kind::Spider, model: spider(), skin: "cave_spider", scale: 0.7 }),
        "enderman" => m(Kind::Enderman, enderman(), "enderman"),
        "villager" => m(Kind::Villager, villager(), "villager"),
        "wandering_trader" => m(Kind::Villager, villager(), "wandering_trader"),
        "wolf" => m(Kind::Wolf, wolf(), "wolf"),
        "squid" => m(Kind::Squid, squid(), "squid"),
        "glow_squid" => m(Kind::Squid, squid(), "glow_squid"),
        "slime" => m(Kind::Slime, slime(), "slime"),
        "iron_golem" => m(Kind::IronGolem, iron_golem(), "iron_golem"),
        _ => None,
    }
}

/// Animation inputs, as vanilla's render state has them.
#[derive(Clone, Copy, Default)]
pub struct Anim {
    /// `walkAnimation.position` and `.speed`.
    pub walk_pos: f32,
    pub walk_speed: f32,
    /// Head yaw relative to the body and pitch, radians.
    pub head_yaw: f32,
    pub head_pitch: f32,
    /// Ticks alive (idle motion).
    pub age: f32,
    /// 0..1 arm swing (attacks).
    pub attack: f32,
}

/// `setupAnim` for each kind: sets part rotations from the animation inputs.
pub fn animate(kind: Kind, a: Anim, name: &str, rot: &mut [f32; 3]) {
    let (pos, speed) = (a.walk_pos, a.walk_speed.min(1.0));
    let cos = |off: f32| (pos * 0.6662 + off).cos();
    let pi = std::f32::consts::PI;
    match (kind, name) {
        (_, "head") if kind != Kind::Slime => {
            rot[0] += a.head_pitch;
            rot[1] += a.head_yaw;
        }
        (Kind::Wolf, "real_head") => {}
        (Kind::Player | Kind::Zombie | Kind::Skeleton | Kind::Enderman, "right_arm") => {
            if matches!(kind, Kind::Zombie) {
                // AnimationUtils.animateZombieArms: held out in front.
                rot[0] = -HALF_PI + (a.age * 0.067).sin() * 0.05 - a.attack * 1.2;
            } else {
                rot[0] = cos(pi) * 2.0 * speed * 0.5 - a.attack * 1.2;
                rot[2] = (a.age * 0.09).cos() * 0.05 + 0.05;
            }
        }
        (Kind::Player | Kind::Zombie | Kind::Skeleton | Kind::Enderman, "left_arm") => {
            if matches!(kind, Kind::Zombie) {
                rot[0] = -HALF_PI - (a.age * 0.067).sin() * 0.05;
            } else {
                rot[0] = cos(0.0) * 2.0 * speed * 0.5;
                rot[2] = -((a.age * 0.09).cos() * 0.05 + 0.05);
            }
        }
        (Kind::Player | Kind::Zombie | Kind::Skeleton | Kind::Villager | Kind::Enderman, "right_leg") => rot[0] = cos(0.0) * 1.4 * speed,
        (Kind::Player | Kind::Zombie | Kind::Skeleton | Kind::Villager | Kind::Enderman, "left_leg") => rot[0] = cos(pi) * 1.4 * speed,
        (Kind::IronGolem, "right_leg") => rot[0] = -1.5 * triangle(pos, 13.0) * speed,
        (Kind::IronGolem, "left_leg") => rot[0] = 1.5 * triangle(pos, 13.0) * speed,
        (Kind::IronGolem, "right_arm") => rot[0] = (-0.2 + 1.5 * triangle(pos, 13.0)) * speed,
        (Kind::IronGolem, "left_arm") => rot[0] = (-0.2 - 1.5 * triangle(pos, 13.0)) * speed,
        (_, "right_hind_leg" | "left_front_leg") => rot[0] += cos(0.0) * 1.4 * speed,
        (_, "left_hind_leg" | "right_front_leg") => rot[0] += cos(pi) * 1.4 * speed,
        (Kind::Spider, n) if n.ends_with("leg") => {
            // A small walk wobble around the resting splay.
            let side = if n.starts_with("right") { 1.0 } else { -1.0 };
            rot[1] += -(pos * 0.6662 * 2.0).cos() * 0.4 * speed * side;
        }
        (Kind::Chicken, "right_leg") => rot[0] = cos(0.0) * 1.4 * speed,
        (Kind::Chicken, "left_leg") => rot[0] = cos(pi) * 1.4 * speed,
        (Kind::Wolf, "tail") => rot[1] = cos(0.0) * 1.4 * speed,
        (Kind::Squid, n) if n.starts_with("tentacle") => rot[0] = 0.4 + (a.age * 0.2).sin().abs() * 0.4,
        _ => {}
    }
}

/// `Mth.triangleWave`.
fn triangle(x: f32, period: f32) -> f32 {
    ((x % period) - period * 0.5).abs() / (period * 0.25) - 1.0
}

/// Model space (1/16 block, Y down, facing -Z) to world for an entity at `pos` facing `yaw`, like
/// `LivingEntityRenderer`: rotate by 180 - yaw, flip X and Y, and lift by 1.501 blocks.
pub fn entity_matrix(pos: Vec3, yaw: f32, scale: f32) -> Mat4 {
    Mat4::from_translation(pos)
        * Mat4::from_rotation_y((180.0 - yaw).to_radians())
        * Mat4::from_scale(Vec3::splat(scale))
        * Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0))
        * Mat4::from_translation(Vec3::new(0.0, -1.501, 0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cow_faces_where_it_looks() {
        // Yaw 0 faces +Z (south): the head (model -Z) must end up on the +Z side.
        let m = entity_matrix(Vec3::ZERO, 0.0, 1.0);
        let head = m.transform_point3(Vec3::new(0.0, 4.0, -12.0) / 16.0);
        assert!(head.z > 0.5 && head.y > 1.0);
        // Feet (model y 24) at the entity's feet.
        assert!(m.transform_point3(Vec3::new(0.0, 24.0, 0.0) / 16.0).y.abs() < 0.01);
    }

    #[test]
    fn every_model_builds_triangles() {
        let sprite = Sprite { w: 64.0, h: 64.0, uv: [0.0, 0.0, 1.0, 1.0] };
        for name in ["player", "pig", "cow", "sheep", "chicken", "creeper", "spider", "enderman", "villager", "wolf", "squid", "slime", "iron_golem"] {
            let mob = for_entity(name).unwrap();
            let mut out = Vec::new();
            let mut d = Draw { out: &mut out, sprite, light: 1.0, tint: [1.0; 3] };
            mob.model.render(entity_matrix(Vec3::ZERO, 0.0, 1.0), &|_, _| {}, &mut d);
            assert!(!out.is_empty() && out.len() % 6 == 0, "{name}");
        }
    }
}
