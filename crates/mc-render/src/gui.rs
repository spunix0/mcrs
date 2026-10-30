//! Screen-space quads for the HUD and menus. Quads are batched per texture and clip rectangle in draw order.

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
    /// HUD sprites, widgets and the font.
    Gui,
    /// The block/item atlas, for item icons.
    Blocks,
    /// The title screen panorama (six faces side by side), sharp.
    Panorama,
    /// The panorama sampled from a low mip level: vanilla's blurred menu background.
    PanoramaBlur,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Batch {
    pub texture: GuiTexture,
    /// `[x, y, w, h]` in pixels; `None` draws everywhere.
    pub clip: Option<[u32; 4]>,
    pub start: u32,
    pub end: u32,
}

#[derive(Default)]
pub struct Gui {
    pub(crate) vertices: Vec<GuiVertex>,
    pub(crate) batches: Vec<Batch>,
    clip: Option<[u32; 4]>,
}

impl Gui {
    /// Adds a quad covering `rect = [x0, y0, x1, y1]` (pixels) with `uv = [u0, v0, u1, v1]`.
    pub fn quad(&mut self, texture: GuiTexture, rect: [f32; 4], uv: [f32; 4], color: [u8; 4]) {
        let [x0, y0, x1, y1] = rect;
        let [u0, v0, u1, v1] = uv;
        self.poly(texture, [[x0, y0], [x0, y1], [x1, y1], [x1, y0]], [[u0, v0], [u0, v1], [u1, v1], [u1, v0]], [color; 4]);
    }

    /// Adds any four-cornered shape (corners in order around it).
    pub fn poly(&mut self, texture: GuiTexture, pos: [[f32; 2]; 4], uv: [[f32; 2]; 4], color: [[u8; 4]; 4]) {
        let start = self.vertices.len() as u32;
        let v = |i: usize| GuiVertex { pos: pos[i], uv: uv[i], color: color[i] };
        self.vertices.extend_from_slice(&[v(0), v(1), v(2), v(0), v(2), v(3)]);
        let end = self.vertices.len() as u32;
        match self.batches.last_mut() {
            Some(b) if b.texture == texture && b.clip == self.clip && b.end == start => b.end = end,
            _ => self.batches.push(Batch { texture, clip: self.clip, start, end }),
        }
    }

    /// Clips everything drawn after this to `[x, y, w, h]` pixels (`None` to stop clipping).
    pub fn set_clip(&mut self, clip: Option<[f32; 4]>) {
        self.clip = clip.map(|[x, y, w, h]| [x.max(0.0) as u32, y.max(0.0) as u32, w.max(0.0) as u32, h.max(0.0) as u32]);
    }

    pub fn is_empty(&self) -> bool {
        self.vertices.is_empty()
    }
}
