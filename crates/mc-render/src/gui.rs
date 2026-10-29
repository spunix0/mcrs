//! Screen-space quads for the HUD and menus. Quads are batched per texture in draw order.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct GuiVertex {
    /// Pixels from the top-left of the window.
    pub pos: [f32; 2],
    pub uv: [f32; 2],
    pub color: [u8; 4],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GuiTexture {
    /// HUD sprites and the font.
    Gui,
    /// The block/item atlas, for item icons.
    Blocks,
}

#[derive(Default)]
pub struct Gui {
    pub(crate) vertices: Vec<GuiVertex>,
    /// `(texture, first vertex, end vertex)`, in draw order.
    pub(crate) batches: Vec<(GuiTexture, u32, u32)>,
}

impl Gui {
    /// Adds a quad covering `rect = [x0, y0, x1, y1]` (pixels) with `uv = [u0, v0, u1, v1]`.
    pub fn quad(&mut self, texture: GuiTexture, rect: [f32; 4], uv: [f32; 4], color: [u8; 4]) {
        let [x0, y0, x1, y1] = rect;
        let [u0, v0, u1, v1] = uv;
        let start = self.vertices.len() as u32;
        let v = |x, y, u, w| GuiVertex { pos: [x, y], uv: [u, w], color };
        self.vertices.extend_from_slice(&[v(x0, y0, u0, v0), v(x0, y1, u0, v1), v(x1, y1, u1, v1), v(x0, y0, u0, v0), v(x1, y1, u1, v1), v(x1, y0, u1, v0)]);
        let end = self.vertices.len() as u32;
        match self.batches.last_mut() {
            Some((t, _, e)) if *t == texture && *e == start => *e = end,
            _ => self.batches.push((texture, start, end)),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}
