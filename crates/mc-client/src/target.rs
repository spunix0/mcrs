//! Finds the block under the crosshair: steps through the voxel grid along the view ray and hit-tests each block's boxes.

use crate::physics::Aabb;

/// Vanilla survival block reach.
pub const REACH: f64 = 4.5;

#[derive(Clone, Debug)]
pub struct Hit {
    pub pos: [i32; 3],
    /// Face that was hit, as Minecraft's 3D data value: down 0, up 1, north 2, south 3, west 4, east 5.
    pub face: u8,
    /// Exact hit point in world coordinates.
    pub point: [f64; 3],
    /// The block's boxes in world coordinates (for the outline).
    pub boxes: Vec<Aabb>,
}

/// Ray vs. box (slab method): distance and the face entered, if it hits in front of the origin.
fn hit_box(o: [f64; 3], d: [f64; 3], b: &Aabb) -> Option<(f64, u8)> {
    let (mut t_near, mut t_far, mut face) = (f64::MIN, f64::MAX, 0u8);
    for axis in 0..3 {
        if d[axis].abs() < 1e-12 {
            if o[axis] < b.min[axis] || o[axis] > b.max[axis] {
                return None;
            }
            continue;
        }
        let (t1, t2) = ((b.min[axis] - o[axis]) / d[axis], (b.max[axis] - o[axis]) / d[axis]);
        // Entering through the min side means we hit the face pointing to -axis.
        let (enter, exit, enter_face) = if t1 < t2 { (t1, t2, [4, 0, 2][axis]) } else { (t2, t1, [5, 1, 3][axis]) };
        if enter > t_near {
            t_near = enter;
            face = enter_face;
        }
        t_far = t_far.min(exit);
    }
    (t_near <= t_far && t_far >= 0.0).then_some((t_near.max(0.0), face))
}

/// `boxes_at` returns a block's targetable boxes in world coordinates (empty for air and plain fluid).
pub fn raycast(eye: [f64; 3], dir: [f64; 3], reach: f64, mut boxes_at: impl FnMut(i32, i32, i32) -> Vec<Aabb>) -> Option<Hit> {
    let mut cell = [eye[0].floor() as i32, eye[1].floor() as i32, eye[2].floor() as i32];
    let step = dir.map(|v| if v > 0.0 { 1 } else { -1 });
    let next = |axis: usize, c: i32| if dir[axis] > 0.0 { (c + 1) as f64 } else { c as f64 };
    let mut t_max = [0.0f64; 3];
    let mut t_delta = [f64::MAX; 3];
    for a in 0..3 {
        if dir[a].abs() > 1e-12 {
            t_max[a] = (next(a, cell[a]) - eye[a]) / dir[a];
            t_delta[a] = 1.0 / dir[a].abs();
        } else {
            t_max[a] = f64::MAX;
        }
    }
    loop {
        let boxes = boxes_at(cell[0], cell[1], cell[2]);
        let best = boxes.iter().filter_map(|b| hit_box(eye, dir, b)).min_by(|a, b| a.0.total_cmp(&b.0));
        if let Some((t, face)) = best {
            if t <= reach {
                let point = [eye[0] + dir[0] * t, eye[1] + dir[1] * t, eye[2] + dir[2] * t];
                return Some(Hit { pos: cell, face, point, boxes });
            }
        }
        let a = if t_max[0] < t_max[1] && t_max[0] < t_max[2] { 0 } else if t_max[1] < t_max[2] { 1 } else { 2 };
        if t_max[a] > reach {
            return None;
        }
        cell[a] += step[a];
        t_max[a] += t_delta[a];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid_at(x: i32, y: i32, z: i32, target: [i32; 3]) -> Vec<Aabb> {
        if [x, y, z] == target {
            let (fx, fy, fz) = (x as f64, y as f64, z as f64);
            vec![Aabb { min: [fx, fy, fz], max: [fx + 1.0, fy + 1.0, fz + 1.0] }]
        } else {
            Vec::new()
        }
    }

    #[test]
    fn hits_the_top_of_the_block_below() {
        let hit = raycast([0.5, 65.62, 0.5], [0.0, -1.0, 0.0], REACH, |x, y, z| solid_at(x, y, z, [0, 63, 0])).unwrap();
        assert_eq!((hit.pos, hit.face), ([0, 63, 0], 1));
        assert!((hit.point[1] - 64.0).abs() < 1e-9);
    }

    #[test]
    fn hits_the_west_face_looking_east() {
        let hit = raycast([0.5, 64.5, 0.5], [1.0, 0.0, 0.0], REACH, |x, y, z| solid_at(x, y, z, [3, 64, 0])).unwrap();
        assert_eq!((hit.pos, hit.face), ([3, 64, 0], 4));
    }

    #[test]
    fn misses_beyond_reach() {
        assert!(raycast([0.5, 64.5, 0.5], [1.0, 0.0, 0.0], REACH, |x, y, z| solid_at(x, y, z, [6, 64, 0])).is_none());
    }
}
