//! The F3 debug screen: vanilla's left column (version, FPS, chunks, entities, position, facing, light) and right
//! column (memory, CPU, display, targeted block and entity), each line on a translucent gray box.

use crate::hud::Painter;

pub struct Info {
    pub fps: f32,
    pub max_fps: String,
    pub vsync: bool,
    pub server: String,
    pub chunks_drawn: usize,
    pub chunks_loaded: usize,
    pub view_distance: u8,
    pub triangles: u64,
    pub gpu_mb: u64,
    pub entities: usize,
    pub dimension: String,
    pub feet: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub sky_light: u8,
    pub block_light: u8,
    pub day_time: f64,
    pub game_mode: String,
    pub display: (u32, u32),
    pub gpu: String,
    pub target_block: Option<([i32; 3], String)>,
    pub target_entity: Option<String>,
    pub particles: usize,
}

fn facing(yaw: f32) -> (&'static str, &'static str) {
    match (((yaw / 90.0).round() as i32) % 4 + 4) % 4 {
        0 => ("south", "Towards positive Z"),
        1 => ("west", "Towards negative X"),
        2 => ("north", "Towards negative Z"),
        _ => ("east", "Towards positive X"),
    }
}

/// Resident memory of this process in bytes (Windows), for the "Mem" line.
fn memory() -> Option<u64> {
    #[repr(C)]
    struct Counters {
        cb: u32,
        page_faults: u32,
        peak_working_set: usize,
        working_set: usize,
        quota_peak_paged: usize,
        quota_paged: usize,
        quota_peak_nonpaged: usize,
        quota_nonpaged: usize,
        pagefile: usize,
        peak_pagefile: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, page_faults: 0, peak_working_set: 0, working_set: 0, quota_peak_paged: 0, quota_paged: 0, quota_peak_nonpaged: 0, quota_nonpaged: 0, pagefile: 0, peak_pagefile: 0 };
    // SAFETY: the struct matches PROCESS_MEMORY_COUNTERS and its size is passed in.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then_some(c.working_set as u64)
}

fn lines_box(p: &mut Painter, lines: &[String], right: bool) {
    let mut y = 2.0;
    for l in lines {
        if !l.is_empty() {
            let w = p.text_width(l);
            let x = if right { p.width - 2.0 - w } else { 2.0 };
            p.fill(x - 1.0, y - 1.0, w + 2.0, 9.0, [0x50, 0x50, 0x50, 0x90]);
            p.text_flat(l, x, y, [0xe0, 0xe0, 0xe0, 255]);
        }
        y += 9.0;
    }
}

pub fn draw(p: &mut Painter, i: &Info) {
    let [x, y, z] = i.feet;
    let (bx, by, bz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let (dir, towards) = facing(i.yaw);
    let yaw = (i.yaw + 180.0).rem_euclid(360.0) - 180.0;
    let left = vec![
        "Minecraft 26.3 (mcrs/vanilla)".to_string(),
        format!("{:.0} fps T: {}{}", i.fps, i.max_fps, if i.vsync { " vsync" } else { "" }),
        i.server.clone(),
        format!("C: {}/{} D: {} pT: {:.1}M  GPU: {} MB", i.chunks_drawn, i.chunks_loaded, i.view_distance, i.triangles as f64 / 1e6, i.gpu_mb),
        format!("E: {}, P: {}", i.entities, i.particles),
        i.dimension.clone(),
        String::new(),
        format!("XYZ: {x:.3} / {y:.5} / {z:.3}"),
        format!("Block: {bx} {by} {bz} [{} {} {}]", bx.rem_euclid(16), by.rem_euclid(16), bz.rem_euclid(16)),
        format!("Chunk: {} {} {} [{} {} in r.{}.{}.mca]", bx.div_euclid(16), by.div_euclid(16), bz.div_euclid(16), bx.div_euclid(16).rem_euclid(32), bz.div_euclid(16).rem_euclid(32), bx.div_euclid(512), bz.div_euclid(512)),
        format!("Facing: {dir} ({towards}) ({yaw:.1} / {:.1})", i.pitch),
        format!("Light: {} ({} sky, {} block)", i.sky_light.max(i.block_light), i.sky_light, i.block_light),
        format!("Day {} ({} ticks)", (i.day_time / 24000.0).floor() as i64, (i.day_time.rem_euclid(24000.0)) as i64),
        format!("Game mode: {}", i.game_mode),
        String::new(),
        "F3 + B: hitboxes  F3 + G: chunk borders  F1: hide HUD".to_string(),
    ];
    let mem = memory().map_or("Mem: ?".into(), |b| format!("Mem: {} MB", b / 1_048_576));
    let threads = std::thread::available_parallelism().map_or(0, |n| n.get());
    let mut right = vec![
        format!("Rust edition, {} 64bit", std::env::consts::OS),
        mem,
        String::new(),
        format!("CPU: {threads} threads"),
        String::new(),
        format!("Display: {}x{}", i.display.0, i.display.1),
        i.gpu.clone(),
        "wgpu".to_string(),
    ];
    if let Some((pos, name)) = &i.target_block {
        right.push(String::new());
        right.push(format!("\u{a7}nTargeted Block: {}, {}, {}", pos[0], pos[1], pos[2]));
        right.push(format!("minecraft:{name}"));
    }
    if let Some(e) = &i.target_entity {
        right.push(String::new());
        right.push("\u{a7}nTargeted Entity".to_string());
        right.push(format!("minecraft:{e}"));
    }
    lines_box(p, &left, false);
    lines_box(p, &right, true);
}
