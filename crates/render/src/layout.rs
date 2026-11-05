//! Where the screen and the camera go for a camera layout (bubble, large bubble, camera only,
//! side by side ...), eased between layouts. A port of `layoutGeom` / `mixGeom` from the 0.3
//! compositor.

use crate::scene::content_aspect;
use screencraft_core::project::{CameraConfig, Project};

pub type R = [f32; 4]; // x, y, w, h

#[derive(Clone, Copy, Debug)]
pub struct Geo {
    pub sr: R,
    pub sa: f32,
    pub cr: R,
    pub ca: f32,
    pub rad: f32,
    pub shadow: f32,
    pub border: f32,
    pub mir: f32,
    pub bcol: [f32; 4],
}

/// The camera settings in force for layout block `idx` (-1 = none): its own, else the project's.
pub fn cam_settings(p: &Project, idx: i32) -> CameraConfig {
    if idx >= 0 {
        if let Some(s) = p.scene.camera_layouts.get(idx as usize).and_then(|r| r.settings.clone()) {
            return s;
        }
    }
    p.config.camera.clone()
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn cam_scale(cc: &CameraConfig, p: &Project, zoom: f64) -> f32 {
    let max_z = p.scene.zooms.iter().filter(|z| !z.is_disabled).map(|z| z.zoom).fold(1.0001f64, f64::max);
    let t = ((zoom - 1.0) / (max_z - 1.0)).clamp(0.0, 1.0) as f32;
    let f = smooth(t);
    1.0 + (cc.zoomed_scale as f32 - 1.0) * f
}

struct Bubble {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    rad: f32,
}

fn bubble_box(p: &Project, cc: &CameraConfig, w: f32, h: f32, zoom: f64, mul: f32) -> Bubble {
    let s = h / 1080.0;
    let a = if cc.aspect_ratio > 0.0 { cc.aspect_ratio as f32 } else { 1.0 };
    let m = w.min(h);
    let mut base = cc.size_ratio as f32 * m * cam_scale(cc, p, zoom) * mul;
    base = base.min(m * 0.8);
    let (bw, bh) = if a >= 1.0 { (base * a, base) } else { (base, base / a) };
    // "flush" pins the camera to the frame edge, ignoring the padding
    let pad = if cc.edge_flush { 0.0 } else { cc.padding_ratio as f32 * 0.08 * m + 8.0 * s };
    Bubble {
        x: pad + cc.position.x as f32 * (w - bw - 2.0 * pad),
        y: pad + cc.position.y as f32 * (h - bh - 2.0 * pad),
        w: bw,
        h: bh,
        rad: cc.corner_radius_ratio as f32 * bw.min(bh),
    }
}

/// Geometry of layout `id` (0 bubble, 1 hidden, 2 large, 3 camera only, 4 side by side with the
/// camera on the right, 5 camera left) under camera settings `cc`.
pub fn layout_geom(id: u8, cc: &CameraConfig, p: &Project, w: f32, h: f32, r: R, zoom: f64, has_cam: bool) -> Geo {
    let s = h / 1080.0;
    let rad = crate::devices::screen_radius(&p.config.styles.device, p.config.styles.corner_radius as f32 * s, r[3]);
    let bcol = crate::wallpapers::parse_color(if cc.border_color.is_empty() { "#ffffff" } else { &cc.border_color });
    let look = (if cc.shadow { 1.0 } else { 0.0 }, if cc.border { 1.0 } else { 0.0 }, if cc.mirror { -1.0 } else { 1.0 });
    let mk = |sr: R, sa: f32, cr: R, ca: f32, rad: f32| Geo { sr, sa, cr, ca, rad, shadow: look.0, border: look.1, mir: look.2, bcol };
    if !has_cam || id == 0 || id == 1 {
        let b = bubble_box(p, cc, w, h, zoom, 1.0);
        return mk(r, 1.0, [b.x, b.y, b.w, b.h], if has_cam && id == 0 && !cc.hide { 1.0 } else { 0.0 }, b.rad);
    }
    if id == 2 {
        let b = bubble_box(p, cc, w, h, zoom, 1.7);
        return mk(r, 1.0, [b.x, b.y, b.w, b.h], if cc.hide { 0.0 } else { 1.0 }, b.rad);
    }
    if id == 3 {
        return mk(r, 0.0, r, 1.0, rad);
    }
    let gap = r[2] * 0.025;
    let sw = r[2] * 0.64;
    let cw = r[2] - sw - gap;
    let asp = content_aspect(p) as f32;
    let fw = sw.min(r[3] * asp);
    let fh = fw / asp;
    let left = id == 5;
    let screen_x = if left { r[0] + cw + gap } else { r[0] };
    let sr = [screen_x + (sw - fw) / 2.0, r[1] + (r[3] - fh) / 2.0, fw, fh];
    let cr = [if left { r[0] } else { r[0] + sw + gap }, r[1], cw, r[3]];
    mk(sr, 1.0, cr, 1.0, rad)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn mix_r(a: R, b: R, t: f32) -> R {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t), lerp(a[3], b[3], t)]
}

pub fn mix_geo(a: &Geo, b: &Geo, t: f32) -> Geo {
    Geo {
        sr: mix_r(a.sr, b.sr, t),
        sa: lerp(a.sa, b.sa, t),
        cr: mix_r(a.cr, b.cr, t),
        ca: lerp(a.ca, b.ca, t),
        rad: lerp(a.rad, b.rad, t),
        shadow: lerp(a.shadow, b.shadow, t),
        border: lerp(a.border, b.border, t),
        // mirroring flips through zero width, so a change of mirror turns the picture around smoothly
        mir: lerp(a.mir, b.mir, t),
        bcol: if t < 0.5 { a.bcol } else { b.bcol },
    }
}

/// The eased geometry for output frame `i`, plus the current layout's own geometry (the shadow and
/// device frame follow that one).
pub fn frame_geo(p: &Project, track: &screencraft_core::track::RenderTrack, i: usize, w: f32, h: f32, r: R, has_cam: bool) -> (Geo, Geo) {
    let zoom = track.zoom.get(i).copied().unwrap_or(1.0);
    let cur_id = track.layout_cur.get(i).copied().unwrap_or(0);
    let prev_id = track.layout_prev.get(i).copied().unwrap_or(0);
    let cur_idx = track.layout_cur_idx.get(i).copied().unwrap_or(-1);
    let prev_idx = track.layout_prev_idx.get(i).copied().unwrap_or(-1);
    let g_cur = layout_geom(cur_id, &cam_settings(p, cur_idx), p, w, h, r, zoom, has_cam);
    let mix = track.layout_mix.get(i).copied().unwrap_or(1.0) as f32;
    let t = smooth(mix.clamp(0.0, 1.0));
    let mut geo = if t >= 1.0 || (prev_id == cur_id && prev_idx == cur_idx) {
        g_cur
    } else {
        let g_prev = layout_geom(prev_id, &cam_settings(p, prev_idx), p, w, h, r, zoom, has_cam);
        mix_geo(&g_prev, &g_cur, t)
    };
    transform_screen(&mut geo, p, w, h);
    let mut g_cur = g_cur;
    transform_screen(&mut g_cur, p, w, h);
    (geo, g_cur)
}

fn transform_screen(geo: &mut Geo, p: &Project, w: f32, h: f32) {
    let scale = p.config.styles.screen_scale.clamp(0.25, 3.0) as f32;
    let x = p.config.styles.screen_offset.x.clamp(-1.0, 1.0) as f32 * w;
    let y = p.config.styles.screen_offset.y.clamp(-1.0, 1.0) as f32 * h;
    geo.sr[0] += x + geo.sr[2] * (1.0 - scale) * 0.5;
    geo.sr[1] += y + geo.sr[3] * (1.0 - scale) * 0.5;
    geo.sr[2] *= scale;
    geo.sr[3] *= scale;
    geo.rad *= scale;
}
