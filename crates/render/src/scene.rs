//! Everything the compositor needs for one output frame, derived on the CPU from the project
//! and the render track. Pure and unit-tested; no GPU types in here.

use crate::layout;
use crate::overlay::{self, Geom, Layers};
use crate::wallpapers::{parse_color, wallpaper};
use screencraft_core::input::CursorKind;
use screencraft_core::project::{ClickEffect, Project, Rect};
use screencraft_core::track::RenderTrack;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BackgroundKind {
    Solid,
    Gradient,
    Wallpaper,
    Image,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CursorDraw {
    /// Normalised to the full source frame.
    pub pos: [f32; 2],
    pub size_px: f32,
    pub rotation: f32,
    pub alpha: f32,
    pub scale: f32,
    pub kind: u32,
    pub style: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ripple {
    pub pos: [f32; 2],
    /// 0..1 through the effect.
    pub progress: f32,
    pub ring: bool,
}

/// The camera picture: a rounded box that can be the whole frame, with its look.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraDraw {
    pub rect: [f32; 4],
    pub alpha: f32,
    pub radius: f32,
    pub shadow: f32,
    pub border: f32,
    pub mirror: f32,
    pub border_color: [f32; 4],
    /// How much of the other clip's camera picture to blend in (cross-dissolve).
    pub blend2: f32,
}

#[derive(Clone, Debug)]
pub struct Scene {
    pub out: (u32, u32),
    pub source: (u32, u32),
    /// The plain content rectangle in output px (before a camera layout moves the screen).
    pub content: [f32; 4],
    /// Where the screen is drawn: x, y, w, h (follows the camera layout).
    pub screen: [f32; 4],
    pub screen_alpha: f32,
    /// What the shadow and device frame hug (the current layout's screen).
    pub shadow_rect: [f32; 4],
    /// The shape the drop shadow follows: the screen, plus its device frame (x, y, w, h) and corner radius.
    pub shadow_box: [f32; 4],
    pub shadow_radius_px: f32,
    pub cam: Option<CameraDraw>,
    /// Cross-dissolve: how much of the other clip's screen picture shows.
    pub dissolve: f32,
    /// Cut transition on the finished frame: (kind 1 fade, 2 flash, 3 blur, 4 slide, 5 zoom, signed progress).
    pub transition: Option<(u32, f32)>,
    pub layers: Layers,
    pub bg_motion: u32,
    pub bg_blur_px: f32,
    pub bg_image: Option<Arc<(u32, u32, Vec<u8>)>>,
    pub radius_px: f32,
    pub shadow_blur_px: f32,
    pub shadow_intensity: f32,
    pub shadow_offset: [f32; 2],
    /// Visible crop of the source, normalised: x, y, w, h.
    pub crop: [f32; 4],
    /// Camera: centre (normalised to the full source) and zoom.
    pub camera: [f32; 3],
    pub bg_kind: BackgroundKind,
    pub bg_angle_rad: f32,
    pub bg_drift: bool,
    pub bg0: [f32; 4],
    pub bg1: [f32; 4],
    pub blobs: [[f32; 3]; 3],
    pub blob_colors: [[f32; 4]; 3],
    pub time_s: f32,
    pub cursor: Option<CursorDraw>,
    pub highlight_radius_px: f32,
    pub highlight_alpha: f32,
    pub highlight_color: [f32; 4],
    pub ripples: Vec<Ripple>,
    pub ripple_color: [f32; 4],
    pub points_to_px: f32,
    pub ripple_line_px: f32,
}

pub const ASPECTS: &[(&str, f64)] =
    &[("16:9", 16.0 / 9.0), ("9:16", 9.0 / 16.0), ("1:1", 1.0), ("4:3", 4.0 / 3.0), ("3:4", 3.0 / 4.0), ("4:5", 4.0 / 5.0), ("21:9", 21.0 / 9.0)];

fn even(n: f64) -> u32 {
    ((n / 2.0).round() as i64 * 2).max(2) as u32
}

/// Aspect ratio of the visible recording after cropping.
pub fn content_aspect(p: &Project) -> f64 {
    let c = p.config.crop;
    (c.width * p.media.width as f64) / (c.height * p.media.height as f64).max(1.0)
}

/// Output pixel size when the shorter side is `resolution` px.
pub fn output_size(p: &Project, resolution: u32) -> (u32, u32) {
    let a = if p.config.output.aspect_ratio == "auto" {
        content_aspect(p)
    } else {
        ASPECTS.iter().find(|(n, _)| *n == p.config.output.aspect_ratio).map(|x| x.1).unwrap_or(16.0 / 9.0)
    };
    let r = resolution as f64;
    if a >= 1.0 {
        (even(r * a), even(r))
    } else {
        (even(r), even(r / a))
    }
}

/// The screen rectangle inside an output of `w` x `h`: padded, aspect-correct, centred.
pub fn content_rect(p: &Project, w: f64, h: f64) -> [f32; 4] {
    let pad = p.config.styles.padding_ratio * w.min(h);
    let (aw, ah) = (w - 2.0 * pad, h - 2.0 * pad);
    let a = content_aspect(p);
    let dv = crate::devices::insets(&p.config.styles.device);
    // the screen's height, with the device frame around it fitting the padded area
    let ch = (ah / (1.0 + dv.top + dv.bottom)).min(aw / (a + 2.0 * dv.side));
    let cw = ch * a;
    [((w - cw) / 2.0) as f32, ((h - (1.0 + dv.top + dv.bottom) * ch) / 2.0 + dv.top * ch) as f32, cw as f32, ch as f32]
}

fn kind_index(k: u8) -> u32 {
    (k as usize).min(CursorKind::ALL.len() - 1) as u32
}

/// A camera / cursor state that replaces the track's for one frame (motion blur sub-frames).
#[derive(Clone, Copy, Debug)]
pub struct CamOverride {
    pub zoom: f64,
    pub cx: f64,
    pub cy: f64,
    pub mx: f64,
    pub my: f64,
}

impl CamOverride {
    pub fn at(track: &RenderTrack, i: usize) -> Self {
        let i = i.min(track.count.saturating_sub(1));
        Self { zoom: track.zoom[i], cx: track.cx[i], cy: track.cy[i], mx: track.mx[i], my: track.my[i] }
    }

    pub fn lerp(a: Self, b: Self, f: f64) -> Self {
        let l = |x: f64, y: f64| x + (y - x) * f;
        Self { zoom: l(a.zoom, b.zoom), cx: l(a.cx, b.cx), cy: l(a.cy, b.cy), mx: l(a.mx, b.mx), my: l(a.my, b.my) }
    }
}

/// Build the scene for output frame `i` of `track`.
pub fn build_scene(p: &Project, track: &RenderTrack, i: usize, out: (u32, u32)) -> Scene {
    build_scene_with(p, track, i, out, None)
}

pub fn build_scene_with(p: &Project, track: &RenderTrack, i: usize, out: (u32, u32), cam_override: Option<CamOverride>) -> Scene {
    let i = i.min(track.count.saturating_sub(1));
    let (w, h) = (out.0 as f64, out.1 as f64);
    let s = (h / 1080.0) as f32;
    let st = &p.config.styles;
    let crop: Rect = p.config.crop;
    let content = content_rect(p, w, h);

    // camera layout: where the screen and the camera go, eased between layouts
    let has_cam = p.media.camera.is_some();
    let (geo, g_cur) = layout::frame_geo(p, track, i, out.0 as f32, out.1 as f32, content, has_cam);
    let sr = geo.sr;

    let bg = &st.background;
    let wp = wallpaper(&bg.wallpaper);
    let (bg_kind, bg0, bg1) = match bg.kind.as_str() {
        "color" => (BackgroundKind::Solid, parse_color(&bg.color), parse_color(&bg.color)),
        "gradient" => (BackgroundKind::Gradient, parse_color(&bg.gradient_from), parse_color(&bg.gradient_to)),
        "image" => (BackgroundKind::Image, [0.07, 0.07, 0.07, 1.0], [0.07, 0.07, 0.07, 1.0]),
        _ => (BackgroundKind::Wallpaper, parse_color(wp.base[0]), parse_color(wp.base[1])),
    };
    let mut blobs = [[0f32; 3]; 3];
    let mut blob_colors = [[0f32; 4]; 3];
    for (k, b) in wp.blobs.iter().enumerate() {
        blobs[k] = [b.1, b.2, b.3];
        blob_colors[k] = parse_color(b.0);
    }
    let bg_motion = if bg.kind == "wallpaper" {
        match bg.motion.as_str() {
            "drift" => 1,
            "bokeh" => 2,
            "waves" => 3,
            "dust" => 4,
            _ => 0,
        }
    } else {
        0
    };
    let bg_image = if bg.kind == "image" { bg.image.as_deref().and_then(crate::assets::bg_image) } else { None };

    let t_out_ms = i as f64 * 1000.0 / track.fps.max(1.0);
    let cfg = &p.config.cursor;
    // a device's screen has corners of its own
    let corner = crate::devices::screen_radius(&st.device, st.corner_radius as f32 * s, content[3])
        * st.screen_scale.clamp(screencraft_core::project::SCREEN_SCALE_MIN, screencraft_core::project::SCREEN_SCALE_MAX) as f32;

    // Points (recording units) to output px, through the camera.
    let rec_h_pts = (crop.height * p.media.bounds.height).max(1.0);
    let zoom = cam_override.map(|c| c.zoom).unwrap_or_else(|| track.zoom.get(i).copied().unwrap_or(1.0));
    let points_to_px = (sr[3] as f64 / rec_h_pts * zoom) as f32;

    let cursor = if cfg.hide || track.cursor_alpha.get(i).copied().unwrap_or(0.0) < 0.01 {
        None
    } else {
        Some(CursorDraw {
            pos: [cam_override.map(|c| c.mx).unwrap_or(track.mx[i]) as f32, cam_override.map(|c| c.my).unwrap_or(track.my[i]) as f32],
            size_px: (cfg.size as f32) * points_to_px,
            rotation: track.cursor_rotation.get(i).copied().unwrap_or(0.0) as f32,
            alpha: track.cursor_alpha[i] as f32,
            scale: track.cursor_scale.get(i).copied().unwrap_or(1.0) as f32,
            kind: kind_index(track.cursor_kind.get(i).copied().unwrap_or(0)),
            style: crate::cursors::style_index(&cfg.style),
        })
    };

    let mut ripples = Vec::new();
    if cfg.click_effect != ClickEffect::None {
        for c in &track.clicks {
            let age = t_out_ms - c.t_out_ms;
            if (0.0..=650.0).contains(&age) {
                ripples.push(Ripple { pos: [c.x as f32, c.y as f32], progress: (age / 650.0) as f32, ring: cfg.click_effect == ClickEffect::Ripple });
                if ripples.len() == 4 {
                    break;
                }
            }
        }
    }

    let sh = &st.shadow;
    let ang = (sh.angle as f32).to_radians();
    let dist = sh.distance as f32 * s;
    let shadow_offset = if sh.directional { [ang.cos() * dist * 0.5, ang.sin() * dist * 0.5] } else { [0.0, dist * 0.3] };

    // the shadow hugs the screen and, with a device frame, the whole device
    let dv = crate::devices::insets(&st.device);
    let shadow_box = if dv.side + dv.top + dv.bottom > 0.0 {
        let r = g_cur.sr;
        (
            [r[0] - dv.side as f32 * r[3], r[1] - dv.top as f32 * r[3], r[2] + 2.0 * dv.side as f32 * r[3], r[3] * (1.0 + (dv.top + dv.bottom) as f32)],
            corner + dv.side as f32 * r[3],
        )
    } else {
        (g_cur.sr, corner)
    };

    // camera box
    let cam = if has_cam && geo.ca > 0.01 {
        let other = track.other_alpha.get(i).copied().unwrap_or(0.0) as f32;
        Some(CameraDraw { rect: geo.cr, alpha: geo.ca, radius: geo.rad, shadow: geo.shadow, border: geo.border, mirror: geo.mir, border_color: geo.bcol, blend2: if other > 0.003 { other.min(1.0) } else { 0.0 } })
    } else {
        None
    };
    let dissolve = track.other_alpha.get(i).copied().unwrap_or(0.0) as f32;
    let dissolve = if dissolve > 0.003 { dissolve.min(1.0) } else { 0.0 };
    let transition = match (track.trans_kind.get(i).copied().unwrap_or(0), track.trans_p.get(i).copied().unwrap_or(0.0) as f32) {
        (k, pr) if k > 0 && k != 6 && ease_k(pr.abs()) > 0.002 => Some((k as u32, pr)),
        _ => None,
    };

    // CPU layers: device frame, annotations, shortcuts, watermark, captions
    let (cx, cy) = match cam_override {
        Some(c) => (c.cx, c.cy),
        None => (track.cx.get(i).copied().unwrap_or(0.5), track.cy.get(i).copied().unwrap_or(0.5)),
    };
    let (vw, vh) = (crop.width / zoom, crop.height / zoom);
    let geom = Geom {
        w: out.0 as f32,
        h: out.1 as f32,
        sr,
        content_r: content,
        view: [(cx - vw / 2.0) as f32, (cy - vh / 2.0) as f32, vw as f32, vh as f32],
        zoom: zoom as f32,
        screen_radius: corner,
        t_src: track.t_src.get(i).copied().unwrap_or(0.0),
        t_out_ms,
        screen_alpha: geo.sa,
    };
    let layers = overlay::build(p, &geom, &track.combos, out, g_cur.sr, corner);

    Scene {
        out,
        source: (p.media.width, p.media.height),
        content,
        screen: sr,
        screen_alpha: geo.sa,
        shadow_rect: g_cur.sr,
        shadow_box: shadow_box.0,
        shadow_radius_px: shadow_box.1,
        cam,
        dissolve,
        transition,
        layers,
        bg_motion,
        bg_blur_px: if bg.kind == "color" { 0.0 } else { bg.blur as f32 * s },
        bg_image,
        radius_px: corner,
        shadow_blur_px: sh.blur as f32 * 2.0 * s,
        shadow_intensity: sh.intensity as f32,
        shadow_offset,
        crop: [crop.x as f32, crop.y as f32, crop.width as f32, crop.height as f32],
        camera: [cx as f32, cy as f32, zoom as f32],
        bg_kind,
        bg_angle_rad: (bg.gradient_angle as f32).to_radians(),
        bg_drift: bg_motion == 1,
        bg0,
        bg1,
        blobs,
        blob_colors,
        time_s: (t_out_ms / 1000.0) as f32,
        cursor,
        highlight_radius_px: if cfg.highlight { (cfg.highlight_size as f32 * points_to_px).max(2.0) } else { 0.0 },
        highlight_alpha: cfg.highlight_opacity as f32,
        highlight_color: parse_color(&cfg.highlight_color),
        ripples,
        ripple_color: parse_color(&cfg.click_effect_color),
        points_to_px,
        ripple_line_px: (3.0 * s * points_to_px / 1.5).max(1.5),
    }
}

pub fn ease_k(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A scene with nothing in it but the screen, for tests and thumbnails.
pub fn plain_scene(out: (u32, u32), source: (u32, u32)) -> Scene {
    let a = source.0 as f32 / source.1 as f32;
    let (w, h) = (out.0 as f32, out.1 as f32);
    let ch = h.min(w / a);
    let cw = ch * a;
    Scene {
        out,
        source,
        content: [(w - cw) / 2.0, (h - ch) / 2.0, cw, ch],
        screen: [(w - cw) / 2.0, (h - ch) / 2.0, cw, ch],
        screen_alpha: 1.0,
        shadow_rect: [(w - cw) / 2.0, (h - ch) / 2.0, cw, ch],
        shadow_box: [(w - cw) / 2.0, (h - ch) / 2.0, cw, ch],
        shadow_radius_px: 0.0,
        cam: None,
        dissolve: 0.0,
        transition: None,
        layers: Layers::default(),
        bg_motion: 0,
        bg_blur_px: 0.0,
        bg_image: None,
        radius_px: 0.0,
        shadow_blur_px: 1.0,
        shadow_intensity: 0.0,
        shadow_offset: [0.0, 0.0],
        crop: [0.0, 0.0, 1.0, 1.0],
        camera: [0.5, 0.5, 1.0],
        bg_kind: BackgroundKind::Solid,
        bg_angle_rad: 0.0,
        bg_drift: false,
        bg0: [0.1, 0.1, 0.1, 1.0],
        bg1: [0.1, 0.1, 0.1, 1.0],
        blobs: [[0.0; 3]; 3],
        blob_colors: [[0.0; 4]; 3],
        time_s: 0.0,
        cursor: None,
        highlight_radius_px: 0.0,
        highlight_alpha: 0.0,
        highlight_color: [1.0, 1.0, 1.0, 1.0],
        ripples: Vec::new(),
        ripple_color: [1.0, 1.0, 1.0, 1.0],
        points_to_px: 1.0,
        ripple_line_px: 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use screencraft_core::project::Media;

    fn project() -> Project {
        Project::from_recording("t", Media { width: 1920, height: 1080, duration_ms: 2000.0, ..Default::default() })
    }

    #[test]
    fn output_sizes_follow_aspect() {
        let mut p = project();
        assert_eq!(output_size(&p, 1080), (1920, 1080));
        p.config.output.aspect_ratio = "9:16".into();
        assert_eq!(output_size(&p, 1080), (1080, 1920));
        p.config.output.aspect_ratio = "1:1".into();
        assert_eq!(output_size(&p, 720), (720, 720));
    }

    #[test]
    fn content_is_centred_and_padded() {
        let p = project();
        let r = content_rect(&p, 1920.0, 1080.0);
        let pad = 0.02 * 1080.0;
        assert!((r[1] - pad as f32).abs() < 0.01);
        assert!((r[0] + r[2] / 2.0 - 960.0).abs() < 0.01);
        assert!((r[2] / r[3] - 16.0 / 9.0).abs() < 1e-3);
        // A portrait output letterboxes a landscape recording.
        let r2 = content_rect(&p, 1080.0, 1920.0);
        assert!(r2[1] > 100.0);
    }
}
