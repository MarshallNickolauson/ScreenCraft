//! The six-track timeline: video, zoom, camera, notes, captions and music, drawn and edited the
//! way the 0.3 timeline works (blocks with trim handles, drag to move, shift/cmd multi-select,
//! "+" transition markers on cuts, scrubbing on the ruler and any empty space).

use super::ops::{self, slice_len};
use super::{EditorState, Sel, Tab};
use crate::icons;
use crate::kit;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Vec2};
use screencraft_core::project::{Slice, ZoomRegion};
use std::sync::Arc;

const LABEL_W: f32 = 58.0;
const RULER_H: f32 = 24.0;
const TRACK_H: f32 = 38.0;
const TRACK_GAP: f32 = 6.0;
const HANDLE_W: f32 = 11.0;
const ZOOM_MAX: f64 = 60.0;

#[derive(Default)]
pub struct View {
    /// 1 = the whole project fits the viewport.
    pub zoom: f64,
    /// Horizontal scroll in px when zoomed in.
    pub scroll_x: f32,
    pub fit_pending: bool,
    drag: Option<Drag>,
    preview: Option<Preview>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Move,
    Left,
    Right,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Clip,
    Ramp,
    Zoom,
    Layout,
    Annot,
    Caption,
    Music,
}

#[derive(Clone)]
enum Drag {
    Scrub { was_playing: bool },
    Block { kind: Kind, id: String, mode: Mode, x0: f32, a0: f64, b0: f64, moved: bool, group: bool, music: (f64, f64, f64) },
    Bar { grab: f32 },
}

#[derive(Clone)]
struct Preview {
    id: String,
    a: f64,
    b: f64,
    /// Slot a dragged clip would drop into.
    drop: Option<usize>,
    music: Option<(f64, f64, f64)>,
}

struct Block {
    kind: Kind,
    id: String,
    a: f64,
    b: f64,
    track: usize,
    lane: usize,
    label: String,
    sub: Option<String>,
    sel: bool,
    color: Color32,
    text: Color32,
    disabled: bool,
    pill: Option<String>,
    freeze: bool,
    /// For clips: the slice index.
    index: usize,
    /// Music: waveform and the clip's window into the file (in ms, duration ms, file ms).
    wave: Option<(Arc<Vec<f32>>, f64, f64, f64)>,
}

fn rng_to_zoom(v: f64) -> f64 {
    ZOOM_MAX.powf(v / 100.0)
}
fn zoom_to_rng(z: f64) -> f64 {
    z.ln() / ZOOM_MAX.ln() * 100.0
}

fn x_text(k: f64) -> String {
    format!("{}×", (k * 100.0).round() / 100.0)
}

pub fn fmt_ruler(t: f64) -> String {
    let s = (t / 1000.0).max(0.0);
    let (m, sec) = ((s / 60.0).floor() as u64, s % 60.0);
    let txt = format!("{m}:{:04.1}", sec);
    txt.strip_suffix(".0").map(String::from).unwrap_or(txt)
}

fn set_zoom(st: &mut EditorState, z: f64, anchor_frac_x: Option<(f32, f32)>, tracks_w: f32) {
    let z = z.clamp(1.0, ZOOM_MAX);
    let old_w = tracks_w as f64 * st.timeline.zoom.max(1.0);
    let new_w = tracks_w as f64 * z;
    if let Some((ax, _)) = anchor_frac_x {
        // keep the time under `ax` (px from the left of the tracks) fixed
        let frac = (st.timeline.scroll_x as f64 + ax as f64) / old_w;
        st.timeline.scroll_x = (frac * new_w - ax as f64) as f32;
    }
    st.timeline.zoom = z;
    let max_scroll = (new_w - tracks_w as f64).max(0.0) as f32;
    st.timeline.scroll_x = st.timeline.scroll_x.clamp(0.0, max_scroll);
}

pub fn show(st: &mut EditorState, ui: &mut egui::Ui) {
    if st.timeline.zoom < 1.0 {
        st.timeline.zoom = 1.0;
    }
    tools(st, ui);
    body(st, ui);
}

fn tools(st: &mut EditorState, ui: &mut egui::Ui) {
    let r = ui.allocate_ui(Vec2::new(ui.available_width(), 42.0), |ui| {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if kit::icon_btn(ui, "split", "Split clip at playhead (S)").clicked() {
                st.split_at_playhead();
            }
            if kit::icon_btn(ui, "duplicate", "Duplicate selected clip (⌘D)").clicked() {
                st.duplicate_selected();
            }
            if kit::icon_btn(ui, "trash", "Delete selected (⌫)").clicked() {
                st.delete_selected();
            }
            if kit::pill(ui, None, "Auto zooms", false, true, false, None).on_hover_text("Regenerate zooms from clicks").clicked() {
                st.auto_zoom();
            }
            if kit::pill(ui, None, "Auto jump cuts", false, true, false, None).on_hover_text("Switch off the pauses in your speech (they stay on the timeline, greyed out). X toggles the selected clip.").clicked() {
                let o = &st.panel.pace;
                let settings = screencraft_core::jumpcut::JumpCutSettings { min_silence_ms: o.jc_silence * 1000.0, padding_ms: o.jc_padding * 1000.0, sensitivity: o.jc_sens };
                st.panel.pace.jc_status = match st.auto_jump_cuts(settings) {
                    Ok((0, _)) => "No pauses that long. Try a shorter silence or a higher sensitivity.".into(),
                    Ok((n, ms)) => format!("{n} jump cut{} found, {:.1} s removed from the final video.", if n == 1 { "" } else { "s" }, ms / 1000.0),
                    Err(e) => e,
                };
                st.tab = Tab::Pace;
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                // Speed of the selected clip
                let speeds = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0, 8.0];
                let labels: Vec<String> = speeds.iter().map(|s| format!("{s}×")).collect();
                let cur_speed = match &st.sel {
                    Sel::Slice(id) => st.project.scene.slices.iter().find(|s| &s.id == id).map(|s| s.time_scale).unwrap_or(1.0),
                    _ => 1.0,
                };
                let cur_label = labels.iter().zip(speeds).find(|(_, s)| (s - cur_speed).abs() < 0.01).map(|(l, _)| l.clone()).unwrap_or_else(|| format!("{}×", cur_speed));
                let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
                if let Some(i) = kit::select(ui, "tl-speed", &cur_label, &refs, 70.0) {
                    if let Sel::Slice(id) = st.sel.clone() {
                        let v = speeds[i];
                        st.edit(|p| {
                            if let Some(s) = p.scene.slices.iter_mut().find(|s| s.id == id) {
                                s.time_scale = v;
                            }
                        });
                    }
                }
                ui.label(egui::RichText::new("Speed").color(MUTED).size(13.0));
                ui.add_space(10.0);
                if kit::pill(ui, None, "Fit", false, true, false, None).on_hover_text("Fit the whole project").clicked() {
                    st.timeline.fit_pending = true;
                }
                if kit::icon_btn(ui, "zoom-plus", "Zoom timeline in").clicked() {
                    st.timeline.zoom = (st.timeline.zoom * 1.6).min(ZOOM_MAX);
                }
                let mut v = zoom_to_rng(st.timeline.zoom);
                let (rect, resp) = ui.allocate_exact_size(Vec2::new(110.0, 18.0), Sense::click_and_drag());
                if resp.dragged() || resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        v = (((p.x - rect.min.x - 7.0) / (rect.width() - 14.0)).clamp(0.0, 1.0) * 100.0) as f64;
                        st.timeline.zoom = rng_to_zoom(v);
                    }
                }
                let t = (zoom_to_rng(st.timeline.zoom) / 100.0) as f32;
                ui.painter().rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.center().y - 2.0), Pos2::new(rect.max.x, rect.center().y + 2.0)), CornerRadius::same(2), LINE_2);
                let cx = rect.min.x + 7.0 + t * (rect.width() - 14.0);
                let th = Rect::from_center_size(Pos2::new(cx, rect.center().y), Vec2::splat(14.0));
                ui.painter().rect_filled(th.expand(1.0), CornerRadius::same(5), ACCENT_2);
                ui.painter().rect_filled(th, CornerRadius::same(4), BG);
                ui.painter().rect_filled(th.shrink(2.0), CornerRadius::same(3), if resp.hovered() { Color32::WHITE } else { ACCENT_2 });
                if kit::icon_btn(ui, "zoom-minus", "Zoom timeline out").clicked() {
                    st.timeline.zoom = (st.timeline.zoom / 1.6).max(1.0);
                }
            });
        });
    });
    let _ = r;
}

fn body(st: &mut EditorState, ui: &mut egui::Ui) {
    let total = st.duration_ms().max(1.0);
    let slices = st.project.scene.slices.clone();
    let scene = st.project.scene.clone();
    let sel_id = st.sel.clone();

    // ---- build the blocks ------------------------------------------------------------
    let mut blocks: Vec<Block> = Vec::new();
    // video track
    let mut acc = 0.0;
    let mut markers: Vec<(f64, usize)> = Vec::new();
    let mut si = 0;
    while si < slices.len() {
        let s = &slices[si];
        if s.ramp {
            let mut sj = si;
            let mut glen = 0.0;
            while sj < slices.len() && slices[sj].ramp {
                glen += slice_len(&slices[sj]);
                sj += 1;
            }
            let (first, last) = (&slices[si], &slices[sj - 1]);
            let selected = matches!(&sel_id, Sel::Slice(id) if slices[si..sj].iter().any(|x| &x.id == id));
            blocks.push(Block {
                kind: Kind::Ramp,
                id: first.id.clone(),
                a: acc,
                b: acc + glen,
                track: 0,
                lane: 0,
                label: String::new(),
                sub: None,
                sel: selected,
                color: CLIP,
                text: Color32::from_rgb(0x2d, 0x14, 0x05),
                disabled: false,
                pill: Some(format!("{} → {}", x_text(first.time_scale), x_text(last.time_scale))),
                freeze: false,
                index: si,
                wave: None,
            });
            if si > 0 {
                markers.push((acc, si));
            }
            acc += glen;
            si = sj;
            continue;
        }
        let len = slice_len(s);
        let sped = (s.time_scale - 1.0).abs() > 1e-9 && !s.freeze;
        blocks.push(Block {
            kind: Kind::Clip,
            id: s.id.clone(),
            a: acc,
            b: acc + len,
            track: 0,
            lane: 0,
            label: if s.disabled { "Jump cut".into() } else if sped { String::new() } else if s.freeze { "Freeze".into() } else { "Clip".into() },
            sub: if sped && !s.disabled { None } else { Some(format!("{:.1}s", len / 1000.0)) },
            sel: matches!(&sel_id, Sel::Slice(id) if id == &s.id),
            color: CLIP,
            text: if s.disabled { Color32::from_rgb(0xa9, 0xb4, 0xc8) } else { Color32::from_rgb(0x2d, 0x14, 0x05) },
            disabled: s.disabled,
            pill: if sped && !s.disabled { Some(x_text(s.time_scale)) } else { None },
            freeze: s.freeze,
            index: si,
                wave: None,
        });
        if si > 0 && !s.disabled && !slices[si - 1].disabled {
            markers.push((acc, si));
        }
        acc += len;
        si += 1;
    }
    // zoom track
    for z in &scene.zooms {
        for sp in ops::spans_out(&slices, z.source_start_ms, z.source_end_ms) {
            if sp[1] - sp[0] < 1.0 {
                continue;
            }
            blocks.push(Block {
                kind: Kind::Zoom,
                id: z.id.clone(),
                a: sp[0],
                b: sp[1],
                track: 1,
                lane: 0,
                label: "Zoom".into(),
                sub: Some(format!("{:.1}× · {}", z.zoom, if z.kind == screencraft_core::project::ZoomKind::Manual { "Manual" } else { "Auto" })),
                sel: st.is_selected(&Sel::Zoom(z.id.clone())),
                color: ACCENT,
                text: Color32::WHITE,
                disabled: z.is_disabled,
                pill: None,
                freeze: false,
                index: 0,
                wave: None,
            });
        }
    }
    for r in &scene.camera_layouts {
        for sp in ops::spans_out(&slices, r.source_start_ms, r.source_end_ms) {
            if sp[1] - sp[0] < 1.0 {
                continue;
            }
            blocks.push(Block {
                kind: Kind::Layout,
                id: r.id.clone(),
                a: sp[0],
                b: sp[1],
                track: 2,
                lane: 0,
                label: ops::layout_name(&r.layout).to_string(),
                sub: None,
                sel: matches!(&sel_id, Sel::Layout(id) if id == &r.id),
                color: BLOCK_LAYOUT,
                text: Color32::WHITE,
                disabled: false,
                pill: None,
                freeze: false,
                index: 0,
                wave: None,
            });
        }
    }
    // notes: greedy lanes so overlapping ones stack
    let mut items: Vec<(usize, [f64; 4])> = Vec::new();
    for (i, an) in scene.annotations.iter().enumerate() {
        for sp in ops::spans_out(&slices, an.source_start_ms, an.source_end_ms) {
            if sp[1] - sp[0] >= 1.0 {
                items.push((i, sp));
            }
        }
    }
    items.sort_by(|x, y| x.1[0].partial_cmp(&y.1[0]).unwrap());
    let mut lanes: Vec<f64> = Vec::new();
    for (i, sp) in items {
        let lane = lanes.iter().position(|end| *end <= sp[0] + 1.0).unwrap_or_else(|| {
            lanes.push(0.0);
            lanes.len() - 1
        });
        lanes[lane] = sp[1];
        let an = &scene.annotations[i];
        let color = match an.kind.as_str() {
            "blur" | "pixelate" => BLOCK_NOTE_BLUR,
            "spotlight" => BLOCK_NOTE_SPOT,
            "arrow" | "box" | "ellipse" | "highlight" => BLOCK_NOTE_SHAPE,
            _ => BLOCK_NOTE,
        };
        let label = if an.kind == "text" { format!("“{}”", an.text.chars().take(28).collect::<String>()) } else { ops::note_name(&an.kind).to_string() };
        blocks.push(Block {
            kind: Kind::Annot,
            id: an.id.clone(),
            a: sp[0],
            b: sp[1],
            track: 3,
            lane,
            label,
            sub: None,
            sel: matches!(&sel_id, Sel::Annot(id) if id == &an.id),
            color,
            text: Color32::WHITE,
            disabled: false,
            pill: None,
            freeze: false,
            index: 0,
                wave: None,
        });
    }
    let note_lanes = lanes.len().max(1);
    for c in &scene.subtitles {
        for sp in ops::spans_out(&slices, c.source_start_ms, c.source_end_ms) {
            if sp[1] - sp[0] < 1.0 {
                continue;
            }
            let label: String = c.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(40).collect();
            blocks.push(Block {
                kind: Kind::Caption,
                id: c.id.clone(),
                a: sp[0],
                b: sp[1],
                track: 4,
                lane: 0,
                label: if label.is_empty() { "(empty)".into() } else { label },
                sub: None,
                sel: st.is_selected(&Sel::Caption(c.id.clone())),
                color: BLOCK_CAPTION,
                text: Color32::WHITE,
                disabled: false,
                pill: None,
                freeze: false,
                index: 0,
                wave: None,
            });
        }
    }
    for c in &scene.music_clips {
        blocks.push(Block {
            kind: Kind::Music,
            id: c.id.clone(),
            a: c.offset_ms,
            b: c.offset_ms + c.dur_ms,
            track: 5,
            lane: 0,
            label: format!("♪ {}", if c.name.is_empty() { "Music" } else { &c.name }),
            sub: None,
            sel: matches!(&sel_id, Sel::Music(id) if id == &c.id),
            color: BLOCK_MUSIC,
            text: Color32::WHITE,
            disabled: false,
            pill: None,
            freeze: false,
            index: 0,
            wave: st.peaks_for(&c.file, ui.ctx()).map(|p| (p, c.in_ms, c.dur_ms, c.file_dur_ms)),
        });
    }

    // ---- geometry --------------------------------------------------------------------
    let track_h = |t: usize| -> f32 {
        if t == 3 {
            (8.0 + note_lanes as f32 * 32.0).max(TRACK_H)
        } else {
            TRACK_H
        }
    };
    let mut track_y = [0f32; 6];
    let mut y = RULER_H;
    for t in 0..6 {
        y += TRACK_GAP;
        track_y[t] = y;
        y += track_h(t);
    }
    let content_h = y + 4.0;

    let avail = ui.available_rect_before_wrap();
    let tracks_w = (avail.width() - LABEL_W).max(100.0);
    if st.timeline.fit_pending {
        st.timeline.zoom = 1.0;
        st.timeline.scroll_x = 0.0;
        st.timeline.fit_pending = false;
    }
    let zoom = st.timeline.zoom;
    let inner_w = tracks_w * zoom as f32;
    let ppm = inner_w as f64 / total;
    let max_scroll = (inner_w - tracks_w).max(0.0);
    st.timeline.scroll_x = st.timeline.scroll_x.clamp(0.0, max_scroll);

    let scrollbar_h = if zoom > 1.0 { 12.0 } else { 0.0 };
    let body_rect = Rect::from_min_max(avail.min, Pos2::new(avail.max.x, avail.max.y - scrollbar_h));
    let resp_all = ui.interact(body_rect, ui.id().with("tl-body"), Sense::click_and_drag());

    // wheel: cmd/ctrl zooms around the pointer; horizontal / shift scrolls
    if resp_all.hovered() {
        let (scroll, mods, hover) = ui.ctx().input(|i| (i.smooth_scroll_delta, i.modifiers, i.pointer.hover_pos()));
        if mods.command || mods.ctrl {
            if scroll.y != 0.0 {
                let ax = hover.map(|p| p.x - body_rect.min.x - LABEL_W).unwrap_or(tracks_w / 2.0);
                let nz = zoom * (scroll.y as f64 * 0.01).exp();
                set_zoom(st, nz, Some((ax, 0.0)), tracks_w);
            }
        } else if scroll.x != 0.0 || (mods.shift && scroll.y != 0.0) {
            let d = if scroll.x != 0.0 { scroll.x } else { scroll.y };
            st.timeline.scroll_x = (st.timeline.scroll_x - d).clamp(0.0, max_scroll);
        }
    }
    let zoom = st.timeline.zoom;
    let inner_w = tracks_w * zoom as f32;
    let ppm = (inner_w as f64 / total).max(1e-6);
    let _ = ppm;
    let ppm = inner_w as f64 / total;
    let scroll_x = st.timeline.scroll_x;

    let origin_y = body_rect.min.y; // vertical scrolling handled below
    let v_off = ui.ctx().memory_mut(|m| m.data.get_temp::<f32>(ui.id().with("tl-voff")).unwrap_or(0.0));
    let visible_h = body_rect.height();
    let max_v = (content_h - visible_h).max(0.0);
    let mut v_off = v_off.clamp(0.0, max_v);
    if resp_all.hovered() && max_v > 0.0 {
        let (scroll, mods) = ui.ctx().input(|i| (i.smooth_scroll_delta, i.modifiers));
        if !(mods.command || mods.ctrl || mods.shift) && scroll.y != 0.0 {
            v_off = (v_off - scroll.y).clamp(0.0, max_v);
        }
    }
    ui.ctx().memory_mut(|m| m.data.insert_temp(ui.id().with("tl-voff"), v_off));

    let tracks_left = body_rect.min.x + LABEL_W;
    let x_of = |t: f64| tracks_left - scroll_x + (t * ppm) as f32;
    let t_of = |x: f32| ((x - tracks_left + scroll_x) as f64 / ppm).clamp(0.0, total);
    let ty = |t: usize| origin_y - v_off + track_y[t];
    let painter = ui.painter().with_clip_rect(body_rect);
    let tracks_clip = Rect::from_min_max(Pos2::new(tracks_left, body_rect.min.y), body_rect.max);
    let tpainter = ui.painter().with_clip_rect(tracks_clip);

    // labels
    for (i, name) in ["Video", "Zoom", "Camera", "Notes", "Captions", "Music"].iter().enumerate() {
        let r = Rect::from_min_size(Pos2::new(body_rect.min.x, ty(i)), Vec2::new(LABEL_W, track_h(i)));
        crate::kit::spaced_text(ui, Pos2::new(r.min.x, r.center().y - 6.0), &name.to_uppercase(), 10.0, MUTED, 1.2);
        let _ = &painter;
    }
    // track backgrounds (faint 80px grid, like the web UI's repeating gradient)
    for t in 0..6 {
        let r = Rect::from_min_size(Pos2::new(tracks_left, ty(t)), Vec2::new(inner_w, track_h(t)));
        let r = Rect::from_min_max(Pos2::new(r.min.x - scroll_x, r.min.y), Pos2::new(r.max.x - scroll_x, r.max.y));
        let mut x = r.min.x + 79.0;
        while x < r.max.x {
            if x >= tracks_clip.min.x - 1.0 && x <= tracks_clip.max.x {
                tpainter.line_segment([Pos2::new(x, r.min.y), Pos2::new(x, r.max.y)], Stroke::new(1.0, Color32::from_white_alpha(7)));
            }
            x += 80.0;
        }
    }
    // ruler
    {
        let steps = [100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10_000.0, 15_000.0, 30_000.0, 60_000.0, 120_000.0, 300_000.0];
        let step = steps.iter().copied().find(|s| s * ppm >= 80.0).unwrap_or(300_000.0);
        let mut t = 0.0;
        while t <= total {
            let x = x_of(t);
            if x > tracks_clip.min.x - 40.0 && x < tracks_clip.max.x + 40.0 {
                tpainter.line_segment([Pos2::new(x, body_rect.min.y - v_off + 14.0), Pos2::new(x, body_rect.min.y - v_off + 20.0)], Stroke::new(1.0, Color32::from_rgb(0x35, 0x43, 0x5c)));
                tpainter.text(Pos2::new(x, body_rect.min.y - v_off), Align2::CENTER_TOP, fmt_ruler(t), kit::font(10.5), MUTED);
            }
            t += step;
        }
    }
    // hints for empty tracks
    let hint = |t: usize, text: &str| {
        tpainter.text(Pos2::new(tracks_left + 12.0, ty(t) + track_h(t) / 2.0), Align2::LEFT_CENTER, text, kit::font(11.5), MUTED);
    };
    if scene.camera_layouts.is_empty() {
        hint(2, "Change how the camera looks over time: add a layout from the Camera tab.");
    }
    if scene.annotations.is_empty() {
        hint(3, "Text, arrows, blur and spotlight appear here: add them from the Annotate tab.");
    }
    if scene.subtitles.is_empty() {
        hint(4, "Captions go here: add them from the Captions tab, or import an SRT.");
    }
    let mut add_music_rect = None;
    if scene.music_clips.is_empty() {
        let r = Rect::from_min_size(Pos2::new(tracks_left, ty(5)), Vec2::new(tracks_w, TRACK_H));
        add_music_rect = Some(r);
    }

    // ---- interaction ------------------------------------------------------------------
    let pointer = resp_all.interact_pointer_pos().or(resp_all.hover_pos());
    let shift = ui.ctx().input(|i| i.modifiers.shift);
    let cmd = ui.ctx().input(|i| i.modifiers.command || i.modifiers.ctrl);

    let rect_of = |b: &Block, pv: &Option<Preview>| -> Rect {
        let (mut a, mut bb) = (b.a, b.b);
        if let Some(p) = pv {
            if p.id == b.id && b.kind != Kind::Clip && b.kind != Kind::Ramp {
                a = p.a;
                bb = p.b;
            }
        }
        let x0 = x_of(a);
        let x1 = x_of(bb);
        let (top, h) = if b.kind == Kind::Annot {
            (ty(3) + 4.0 + b.lane as f32 * 32.0, 28.0)
        } else {
            (ty(b.track), TRACK_H)
        };
        let min_w = match b.kind {
            Kind::Zoom | Kind::Layout | Kind::Annot | Kind::Caption | Kind::Music => 14.0,
            Kind::Clip => 4.0,
            Kind::Ramp => 6.0,
        };
        Rect::from_min_max(Pos2::new(x0, top), Pos2::new((x1 - 2.0).max(x0 + min_w), top + h))
    };

    // hit test at pointer
    let hit_block = |pos: Pos2, pv: &Option<Preview>| -> Option<(usize, Mode)> {
        // topmost = last drawn
        blocks.iter().enumerate().rev().find_map(|(i, b)| {
            let r = rect_of(b, pv);
            if !r.expand2(Vec2::new(0.0, 0.0)).contains(pos) {
                return None;
            }
            let tiny = r.width() < 24.0;
            let mode = if !tiny && pos.x < r.min.x + HANDLE_W && b.kind != Kind::Ramp {
                Mode::Left
            } else if !tiny && pos.x > r.max.x - HANDLE_W && b.kind != Kind::Ramp {
                Mode::Right
            } else {
                Mode::Move
            };
            Some((i, mode))
        })
    };
    let hit_marker = |pos: Pos2| -> Option<usize> {
        markers.iter().find_map(|(t, si)| {
            let c = Pos2::new(x_of(*t), ty(0) + TRACK_H / 2.0);
            if (pos - c).length() <= 10.0 {
                Some(*si)
            } else {
                None
            }
        })
    };

    let preview_now = st.timeline.preview.clone();
    if resp_all.drag_started() || (resp_all.clicked() && st.timeline.drag.is_none()) {
        if let Some(pos) = pointer {
            let in_tracks = pos.x >= tracks_left;
            let in_ruler = pos.y < body_rect.min.y - v_off + RULER_H + 1.0;
            if !in_tracks {
                // label column: nothing
            } else if let Some(r) = add_music_rect.filter(|r| r.translate(Vec2::new(0.0, 0.0)).contains(pos) && pos.y > ty(5) - 1.0) {
                let _ = r;
                st.add_music();
            } else if let (false, Some(si)) = (in_ruler, hit_marker(pos)) {
                let id = st.project.scene.slices[si].id.clone();
                st.select(Sel::Slice(id));
                st.tab = Tab::Pace;
            } else if let (false, Some((bi, mode))) = (in_ruler, hit_block(pos, &preview_now)) {
                let b = &blocks[bi];
                let mut group = false;
                // selection rules
                match b.kind {
                    Kind::Zoom => {
                        st.tab = Tab::Zoom;
                        if shift || cmd {
                            let mut ids = st.selected_zoom_ids();
                            if let Some(p) = ids.iter().position(|x| x == &b.id) {
                                ids.remove(p);
                            } else {
                                ids.push(b.id.clone());
                            }
                            if ids.is_empty() {
                                st.sel = Sel::None;
                                st.multi.clear();
                            } else {
                                st.sel = Sel::Zoom(ids[0].clone());
                                st.multi = if ids.len() > 1 { ids } else { vec![] };
                            }
                            st.timeline.drag = None;
                            return finish(st, ui, &blocks, v_off);
                        }
                        group = st.is_selected(&Sel::Zoom(b.id.clone())) && st.multi.len() > 1;
                        if !group {
                            st.select(Sel::Zoom(b.id.clone()));
                        }
                    }
                    Kind::Caption => {
                        st.tab = Tab::Captions;
                        let mut list = st.project.scene.subtitles.clone();
                        list.sort_by(|a, b| a.source_start_ms.partial_cmp(&b.source_start_ms).unwrap());
                        if shift || cmd {
                            let mut ids = st.selected_caption_ids();
                            if shift && !ids.is_empty() {
                                let i0 = list.iter().position(|c| c.id == ids[0]).unwrap_or(0);
                                let i1 = list.iter().position(|c| c.id == b.id).unwrap_or(0);
                                ids = list[i0.min(i1)..=i0.max(i1)].iter().map(|c| c.id.clone()).collect();
                                if i0 > i1 {
                                    ids.reverse();
                                }
                            } else if let Some(p) = ids.iter().position(|x| x == &b.id) {
                                ids.remove(p);
                            } else {
                                ids.push(b.id.clone());
                            }
                            if ids.is_empty() {
                                st.sel = Sel::None;
                                st.multi.clear();
                            } else {
                                st.sel = Sel::Caption(ids[0].clone());
                                st.multi = if ids.len() > 1 { ids } else { vec![] };
                            }
                            st.timeline.drag = None;
                            return finish(st, ui, &blocks, v_off);
                        }
                        group = st.is_selected(&Sel::Caption(b.id.clone())) && st.multi.len() > 1;
                        if !group {
                            st.select(Sel::Caption(b.id.clone()));
                        }
                    }
                    Kind::Layout => {
                        st.select(Sel::Layout(b.id.clone()));
                        st.tab = Tab::Camera;
                    }
                    Kind::Annot => {
                        st.select(Sel::Annot(b.id.clone()));
                        st.tab = Tab::Annotate;
                    }
                    Kind::Music => {
                        st.select(Sel::Music(b.id.clone()));
                        st.tab = Tab::Audio;
                    }
                    Kind::Clip | Kind::Ramp => {
                        st.select(Sel::Slice(b.id.clone()));
                    }
                }
                let music = st.project.scene.music_clips.iter().find(|c| c.id == b.id).map(|c| (c.offset_ms, c.in_ms, c.dur_ms)).unwrap_or((0.0, 0.0, 0.0));
                st.timeline.drag = Some(Drag::Block { kind: b.kind, id: b.id.clone(), mode, x0: pos.x, a0: b.a, b0: b.b, moved: false, group, music });
            } else {
                // empty space or the ruler: scrub, and deselect
                st.select(Sel::None);
                let was_playing = st.is_playing();
                if was_playing {
                    st.pause();
                }
                st.timeline.drag = Some(Drag::Scrub { was_playing });
                st.seek(t_of(pos.x));
            }
        }
    }

    if resp_all.dragged() {
        if let (Some(pos), Some(drag)) = (pointer, st.timeline.drag.clone()) {
            match drag {
                Drag::Scrub { .. } => st.seek(t_of(pos.x)),
                Drag::Block { kind, id, mode, x0, a0, b0, mut moved, group, music } => {
                    if (pos.x - x0).abs() >= 3.0 || moved {
                        moved = true;
                        let d = (pos.x - x0) as f64 / ppm;
                        let mut prev = Preview { id: id.clone(), a: a0, b: b0, drop: None, music: None };
                        match kind {
                            Kind::Zoom | Kind::Layout | Kind::Annot | Kind::Caption => {
                                let (min_len, ) = (if kind == Kind::Zoom { 300.0 } else { 200.0 },);
                                match mode {
                                    Mode::Move => {
                                        let w = b0 - a0;
                                        prev.a = (a0 + d).clamp(0.0, (total - w).max(0.0));
                                        prev.b = prev.a + w;
                                    }
                                    Mode::Left => prev.a = (a0 + d).clamp(0.0, b0 - min_len),
                                    Mode::Right => prev.b = (b0 + d).clamp(a0 + min_len, total),
                                }
                            }
                            Kind::Music => {
                                let (o0, i0, d0) = music;
                                let file_dur = st.project.scene.music_clips.iter().find(|c| c.id == id).map(|c| c.file_dur_ms).unwrap_or(0.0);
                                let v = match mode {
                                    Mode::Move => ((o0 + d).clamp(0.0, (total - 200.0).max(0.0)), i0, d0),
                                    Mode::Left => {
                                        let delta = d.clamp((-i0).max(-o0), d0 - 200.0);
                                        (o0 + delta, i0 + delta, d0 - delta)
                                    }
                                    Mode::Right => (o0, i0, (d0 + d).clamp(200.0, (if file_dur > 0.0 { file_dur } else { d0 + i0 } - i0).max(200.0))),
                                };
                                prev.music = Some(v);
                                prev.a = v.0;
                                prev.b = v.0 + v.2;
                            }
                            Kind::Clip => {
                                let sl = st.project.scene.slices.iter().find(|s| s.id == id).cloned();
                                if let Some(s) = sl {
                                    match mode {
                                        Mode::Move => {
                                            // drop slot = number of other clips whose centre is left of the pointer
                                            let n = blocks.iter().filter(|b| matches!(b.kind, Kind::Clip | Kind::Ramp) && b.id != id).filter(|b| (x_of(b.a) + x_of(b.b)) / 2.0 < pos.x).count();
                                            prev.drop = Some(n);
                                            prev.a = a0 + d;
                                            prev.b = b0 + d;
                                        }
                                        Mode::Left => {
                                            let ns = (s.source_start_ms + d * s.time_scale).clamp(0.0, s.source_end_ms - 200.0);
                                            prev.a = b0 - (s.source_end_ms - ns) / s.time_scale;
                                        }
                                        Mode::Right => {
                                            let ne = (s.source_end_ms + d * s.time_scale).clamp(s.source_start_ms + 200.0, st.project.media.duration_ms);
                                            prev.b = a0 + (ne - s.source_start_ms) / s.time_scale;
                                        }
                                    }
                                }
                            }
                            Kind::Ramp => {}
                        }
                        st.timeline.preview = Some(prev);
                        st.timeline.drag = Some(Drag::Block { kind, id, mode, x0, a0, b0, moved, group, music });
                    }
                }
                Drag::Bar { grab } => {
                    let track = tracks_w;
                    let thumb_w = (track * track / inner_w).max(24.0);
                    let x = (pos.x - tracks_left - grab).clamp(0.0, track - thumb_w);
                    st.timeline.scroll_x = (x / (track - thumb_w).max(1.0) * max_scroll).clamp(0.0, max_scroll);
                }
            }
        }
    }

    let released = resp_all.drag_stopped() || (resp_all.clicked() && st.timeline.drag.is_some());
    if released {
        if let Some(drag) = st.timeline.drag.take() {
            let prev = st.timeline.preview.take();
            match drag {
                Drag::Scrub { was_playing } => {
                    if was_playing {
                        st.play();
                    }
                }
                Drag::Bar { .. } => {}
                Drag::Block { kind, id, mode, moved, group, a0, b0, .. } => {
                    if !moved {
                        // a click on a clip seeks there
                        if matches!(kind, Kind::Clip | Kind::Ramp) {
                            if let Some(p) = pointer {
                                st.seek(t_of(p.x));
                            }
                        }
                    } else if let Some(pv) = prev {
                        commit(st, kind, &id, mode, group, a0, b0, &pv);
                    }
                }
            }
        }
        st.timeline.preview = None;
    }

    // ---- paint blocks -----------------------------------------------------------------
    let pv = st.timeline.preview.clone();
    for b in &blocks {
        let mut r = rect_of(b, &pv);
        let mut dragging = false;
        if let Some(p) = &pv {
            if p.id == b.id && matches!(b.kind, Kind::Clip) {
                if p.drop.is_some() {
                    r = r.translate(Vec2::new(((p.a - b.a) * ppm) as f32, 0.0));
                    dragging = true;
                } else {
                    r = Rect::from_min_max(Pos2::new(x_of(p.a), r.min.y), Pos2::new((x_of(p.b) - 2.0).max(x_of(p.a) + 8.0), r.max.y));
                }
            }
        }
        // group followers
        if let Some(Drag::Block { group: true, id, mode: Mode::Move, a0, .. }) = &st.timeline.drag {
            if let Some(p) = &pv {
                if st.is_selected(&Sel::Zoom(b.id.clone())) && b.kind == Kind::Zoom || st.is_selected(&Sel::Caption(b.id.clone())) && b.kind == Kind::Caption {
                    if &b.id != id {
                        r = r.translate(Vec2::new(((p.a - a0) * ppm) as f32, 0.0));
                    }
                }
            }
        }
        if r.max.x < tracks_clip.min.x || r.min.x > tracks_clip.max.x || r.max.y < body_rect.min.y || r.min.y > body_rect.max.y {
            continue;
        }
        paint_block(ui, &tpainter, b, r, dragging);
    }
    // transition markers
    for (t, si) in &markers {
        let c = Pos2::new(x_of(*t), ty(0) + TRACK_H / 2.0);
        if c.x < tracks_clip.min.x - 12.0 || c.x > tracks_clip.max.x + 12.0 {
            continue;
        }
        let has = st.project.scene.slices.get(*si).map(|s| s.transition.is_some()).unwrap_or(false);
        let a = if has { 255 } else { 140 };
        tpainter.circle_filled(c + Vec2::new(0.0, 2.0), 9.0, Color32::from_black_alpha(60));
        tpainter.circle_filled(c, 9.0, Color32::from_white_alpha(a));
        tpainter.text(c, Align2::CENTER_CENTER, if has { "◐" } else { "+" }, kit::font(10.0), Color32::from_rgb(0x22, 0x22, 0x22));
    }
    if let Some(r) = add_music_rect {
        let r = Rect::from_min_max(Pos2::new(r.min.x, ty(5)), Pos2::new(r.max.x, ty(5) + TRACK_H));
        let hov = ui.rect_contains_pointer(r);
        let col = if hov { BLOCK_MUSIC } else { LINE_2 };
        if hov {
            tpainter.rect_filled(r, CornerRadius::same(6), alpha(BLOCK_MUSIC, 0.08));
        }
        for w in [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()].windows(2) {
            tpainter.extend(Shape::dashed_line(&[w[0], w[1]], Stroke::new(1.0, col), 4.0, 3.0));
        }
        tpainter.text(r.center(), Align2::CENTER_CENTER, "+ Add music", kit::font(13.0), if hov { Color32::from_rgb(0xd3, 0xf5, 0xe5) } else { MUTED });
    }

    // playhead
    let px = x_of(st.playhead_ms);
    if px >= tracks_clip.min.x - 8.0 && px <= tracks_clip.max.x + 8.0 {
        let top = body_rect.min.y - v_off;
        tpainter.rect_filled(Rect::from_min_max(Pos2::new(px - 1.0, top + 20.0), Pos2::new(px + 1.0, top + content_h)), 0.0, ACCENT_2);
        tpainter.rect_filled(Rect::from_min_size(Pos2::new(px - 6.0, top + 9.0), Vec2::splat(12.0)), CornerRadius { nw: 3, ne: 3, sw: 6, se: 6 }, ACCENT_2);
    }
    // keep the playhead visible while playing a zoomed-in timeline
    if st.is_playing() && zoom > 1.0 && (px > tracks_clip.max.x - 20.0 || px < tracks_clip.min.x) {
        st.timeline.scroll_x = ((st.playhead_ms * ppm) as f32 - tracks_w * 0.2).clamp(0.0, max_scroll);
    }

    // horizontal scrollbar when zoomed
    if scrollbar_h > 0.0 {
        let bar = Rect::from_min_size(Pos2::new(tracks_left, body_rect.max.y + 2.0), Vec2::new(tracks_w, 8.0));
        let thumb_w = (tracks_w * tracks_w / inner_w).max(24.0);
        let x = bar.min.x + scroll_x / max_scroll.max(1.0) * (tracks_w - thumb_w);
        let thumb = Rect::from_min_size(Pos2::new(x, bar.min.y), Vec2::new(thumb_w, 8.0));
        let r = ui.interact(thumb.expand2(Vec2::new(0.0, 4.0)), ui.id().with("tl-hbar"), Sense::drag());
        if r.drag_started() {
            if let Some(p) = r.interact_pointer_pos() {
                st.timeline.drag = Some(Drag::Bar { grab: p.x - thumb.min.x });
            }
        }
        if r.dragged() {
            if let Some(p) = r.interact_pointer_pos() {
                let grab = if let Some(Drag::Bar { grab }) = st.timeline.drag { grab } else { thumb_w / 2.0 };
                let x = (p.x - bar.min.x - grab).clamp(0.0, tracks_w - thumb_w);
                st.timeline.scroll_x = (x / (tracks_w - thumb_w).max(1.0) * max_scroll).clamp(0.0, max_scroll);
            }
        }
        if r.drag_stopped() {
            st.timeline.drag = None;
        }
        ui.painter().rect_filled(thumb, CornerRadius::same(4), Color32::from_rgb(0x2b, 0x38, 0x50));
    }
    ui.advance_cursor_after_rect(Rect::from_min_max(avail.min, avail.max));

    // cursor feedback on trim handles
    if let Some(h) = resp_all.hover_pos() {
        if let Some((_, mode)) = hit_block(h, &pv) {
            if mode != Mode::Move {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
            } else {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            }
        }
    }
    if st.timeline.drag.is_some() && ui.ctx().input(|i| i.pointer.any_down()) {
        ui.ctx().request_repaint();
    }
}

fn finish(_st: &mut EditorState, _ui: &mut egui::Ui, _b: &[Block], _v: f32) {}

fn paint_block(ui: &egui::Ui, p: &egui::Painter, b: &Block, r: Rect, dragging: bool) {
    let cr = CornerRadius::same(6);
    let mut fill = b.color;
    if b.disabled {
        fill = alpha(fill, 0.35);
    }
    if dragging {
        p.rect_filled(r.translate(Vec2::new(0.0, 8.0)).expand(2.0), CornerRadius::same(8), Color32::from_black_alpha(90));
        fill = alpha(fill, 0.88);
    }
    let jump = b.kind == Kind::Clip && b.disabled;
    if b.freeze || b.kind == Kind::Ramp || jump {
        // diagonal hatch
        let (c0, c1) = if jump { (Color32::from_rgb(0x24, 0x2c, 0x3b), Color32::from_rgb(0x2c, 0x35, 0x46)) } else if b.freeze { (Color32::from_rgb(0x5d, 0x6b, 0x82), Color32::from_rgb(0x4f, 0x5c, 0x72)) } else { (Color32::from_rgb(0xc9, 0x8a, 0x63), Color32::from_rgb(0xbe, 0x7d, 0x57)) };
        p.rect_filled(r, cr, c0);
        let clipped = p.with_clip_rect(r);
        let step = if b.freeze || jump { 16.0 } else { 20.0 };
        let mut x = r.min.x - r.height();
        while x < r.max.x {
            clipped.add(Shape::convex_polygon(
                vec![Pos2::new(x + step / 2.0, r.min.y), Pos2::new(x + step, r.min.y), Pos2::new(x + step - r.height(), r.max.y), Pos2::new(x + step / 2.0 - r.height(), r.max.y)],
                c1,
                Stroke::NONE,
            ));
            x += step;
        }
    } else {
        p.rect_filled(r, cr, fill);
    }
    let tiny = r.width() < 24.0;
    let clip = p.with_clip_rect(r);
    if !tiny {
        // trim handles
        if b.kind != Kind::Ramp {
            let hl = Rect::from_min_size(r.min, Vec2::new(HANDLE_W, r.height()));
            let hr = Rect::from_min_max(Pos2::new(r.max.x - HANDLE_W, r.min.y), r.max);
            clip.rect_filled(hl, CornerRadius { nw: 6, sw: 6, ne: 0, se: 0 }, Color32::from_black_alpha(56));
            clip.rect_filled(hr, CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }, Color32::from_black_alpha(56));
            let bar = if jump { Color32::from_rgb(0x7d, 0x8a, 0xa3) } else if b.kind == Kind::Clip { Color32::from_rgb(0x2d, 0x14, 0x05) } else if b.kind == Kind::Music { Color32::from_rgb(0x0b, 0x3a, 0x28) } else { Color32::from_white_alpha(235) };
            for h in [hl, hr] {
                clip.rect_filled(Rect::from_center_size(h.center(), Vec2::new(2.0, 14.0)), CornerRadius::same(2), bar);
            }
        }
        let inner = Rect::from_min_max(Pos2::new(r.min.x + HANDLE_W, r.min.y), Pos2::new(r.max.x - HANDLE_W, r.max.y));
        let ic = p.with_clip_rect(inner);
        let cx = r.center().x;
        if let Some(pill) = &b.pill {
            let w = kit::text_width(ui, pill, 11.0) + 18.0;
            let pr = Rect::from_center_size(r.center(), Vec2::new(w.max(34.0), 20.0));
            let bg = if b.kind == Kind::Clip || b.kind == Kind::Ramp { Color32::from_rgba_unmultiplied(45, 20, 5, 41) } else { Color32::from_black_alpha(77) };
            ic.rect_filled(pr, CornerRadius::same(10), bg);
            ic.text(pr.center(), Align2::CENTER_CENTER, pill, kit::font(11.0), b.text);
        } else if b.kind == Kind::Music {
            if let Some((peaks, in_ms, dur_ms, file_ms)) = &b.wave {
                let total = peaks.len() as f64;
                let fd = if *file_ms > 0.0 { *file_ms } else { in_ms + dur_ms };
                let (i0, i1) = (in_ms / fd * total, (in_ms + dur_ms) / fd * total);
                let mid = r.center().y;
                let mut x = r.min.x.max(inner.min.x - HANDLE_W).floor();
                while x < r.max.x.min(inner.max.x + HANDLE_W) {
                    let f0 = (x - r.min.x) as f64 / r.width() as f64;
                    let f1 = (x + 1.0 - r.min.x) as f64 / r.width() as f64;
                    let a = (i0 + (i1 - i0) * f0).floor().max(0.0) as usize;
                    let bb = ((i0 + (i1 - i0) * f1).floor() as usize).max(a + 1);
                    let m = (a..bb.min(peaks.len())).map(|i| peaks[i]).fold(0.0f32, f32::max);
                    let bar = (m * r.height() * 0.9).max(1.0);
                    clip.rect_filled(Rect::from_min_max(Pos2::new(x, mid - bar / 2.0), Pos2::new(x + 1.0, mid + bar / 2.0)), 0.0, Color32::from_white_alpha(191));
                    x += 1.0;
                }
            }
            let mname = b.label.clone();
            let w = kit::text_width(ui, &mname, 11.0) + 12.0;
            let lr = Rect::from_min_size(Pos2::new(inner.min.x + 2.0, r.max.y - 3.0 - 17.0), Vec2::new(w.min(inner.width() - 4.0).max(10.0), 17.0));
            ic.rect_filled(lr, CornerRadius::same(4), Color32::from_black_alpha(82));
            ic.text(Pos2::new(lr.min.x + 6.0, lr.center().y), Align2::LEFT_CENTER, mname, kit::font(11.0), Color32::WHITE);
        } else if let Some(sub) = &b.sub {
            ic.text(Pos2::new(cx, r.center().y - 7.0), Align2::CENTER_CENTER, &b.label, kit::font(12.0), b.text);
            let sc = if jump { Color32::from_rgb(0x7d, 0x8a, 0xa3) } else if b.kind == Kind::Clip { Color32::from_rgba_unmultiplied(45, 20, 5, 178) } else { alpha(b.text, 0.85) };
            ic.text(Pos2::new(cx, r.center().y + 8.0), Align2::CENTER_CENTER, sub, kit::font(11.0), sc);
        } else {
            let size = if matches!(b.kind, Kind::Annot) { 11.5 } else { 12.0 };
            ic.text(r.center(), Align2::CENTER_CENTER, &b.label, kit::font(size), b.text);
        }
    }
    if b.sel {
        p.rect_stroke(r, cr, Stroke::new(2.0, Color32::WHITE), StrokeKind::Inside);
    }
}

/// Apply a finished drag.
fn commit(st: &mut EditorState, kind: Kind, id: &str, mode: Mode, group: bool, a0: f64, b0: f64, pv: &Preview) {
    let slices = st.project.scene.slices.clone();
    let dur = st.project.media.duration_ms;
    match kind {
        Kind::Zoom | Kind::Layout | Kind::Annot | Kind::Caption => {
            if group && mode == Mode::Move {
                let delta = ops::out_to_src(&slices, pv.a).0 - ops::out_to_src(&slices, a0).0;
                let ids = if kind == Kind::Zoom { st.selected_zoom_ids() } else { st.selected_caption_ids() };
                st.edit(|p| {
                    if kind == Kind::Zoom {
                        for r in p.scene.zooms.iter_mut().filter(|r| ids.contains(&r.id)) {
                            let len = r.source_end_ms - r.source_start_ms;
                            let s = (r.source_start_ms + delta).clamp(0.0, (dur - len).max(0.0));
                            r.source_start_ms = s;
                            r.source_end_ms = s + len;
                        }
                    } else {
                        for r in p.scene.subtitles.iter_mut().filter(|r| ids.contains(&r.id)) {
                            let len = r.source_end_ms - r.source_start_ms;
                            let s = (r.source_start_ms + delta).clamp(0.0, (dur - len).max(0.0));
                            r.source_start_ms = s;
                            r.source_end_ms = s + len;
                        }
                    }
                });
                return;
            }
            let ns = ops::out_to_src(&slices, pv.a).0;
            let ne = ops::out_to_src(&slices, pv.b).0;
            if group {
                st.select(match kind {
                    Kind::Zoom => Sel::Zoom(id.to_string()),
                    _ => Sel::Caption(id.to_string()),
                });
            }
            let id = id.to_string();
            let mut new_ids: Vec<String> = (0..8).map(|_| st.new_id("zoom")).collect();
            st.edit(|p| match kind {
                Kind::Zoom => {
                    let Some(i) = p.scene.zooms.iter().position(|z| z.id == id) else { return };
                    let z = p.scene.zooms[i].clone();
                    let spans = ops::spans_out(&slices, z.source_start_ms, z.source_end_ms);
                    if z.ease_in_ms == Some(1.0) {
                        p.scene.zooms[i].ease_in_ms = None;
                    }
                    if z.ease_out_ms == Some(1.0) {
                        p.scene.zooms[i].ease_out_ms = None;
                    }
                    if spans.len() <= 1 {
                        p.scene.zooms[i].source_start_ms = ns;
                        p.scene.zooms[i].source_end_ms = ne.max(ns + 100.0);
                        return;
                    }
                    // the region shows up in several places: detach the stretch that was dragged
                    let k = spans.iter().position(|sp| (sp[0] - a0).abs() < 2.0 && (sp[1] - b0).abs() < 2.0).unwrap_or(0);
                    let copies: Vec<ZoomRegion> = spans
                        .iter()
                        .enumerate()
                        .filter(|(j, _)| *j != k)
                        .map(|(_, sp)| {
                            let mut c = p.scene.zooms[i].clone();
                            c.id = new_ids.pop().unwrap_or_else(|| "zoom-x".into());
                            c.source_start_ms = sp[2];
                            c.source_end_ms = sp[3];
                            c
                        })
                        .collect();
                    p.scene.zooms[i].source_start_ms = ns;
                    p.scene.zooms[i].source_end_ms = ne.max(ns + 100.0);
                    for (n, c) in copies.into_iter().enumerate() {
                        p.scene.zooms.insert(i + 1 + n, c);
                    }
                }
                Kind::Layout => {
                    if let Some(x) = p.scene.camera_layouts.iter_mut().find(|x| x.id == id) {
                        x.source_start_ms = ns;
                        x.source_end_ms = ne.max(ns + 100.0);
                    }
                }
                Kind::Annot => {
                    if let Some(x) = p.scene.annotations.iter_mut().find(|x| x.id == id) {
                        x.source_start_ms = ns;
                        x.source_end_ms = ne.max(ns + 100.0);
                    }
                }
                _ => {
                    if let Some(x) = p.scene.subtitles.iter_mut().find(|x| x.id == id) {
                        x.source_start_ms = ns;
                        x.source_end_ms = ne.max(ns + 100.0);
                    }
                }
            });
        }
        Kind::Music => {
            if let Some(v) = pv.music {
                let id = id.to_string();
                st.edit(|p| {
                    if let Some(c) = p.scene.music_clips.iter_mut().find(|c| c.id == id) {
                        c.offset_ms = v.0.round();
                        c.in_ms = v.1.round();
                        c.dur_ms = v.2.round();
                    }
                });
            }
        }
        Kind::Clip => {
            let id = id.to_string();
            match mode {
                Mode::Move => {
                    if let Some(drop) = pv.drop {
                        st.edit(|p| {
                            if let Some(from) = p.scene.slices.iter().position(|x| x.id == id) {
                                if drop != from {
                                    let it = p.scene.slices.remove(from);
                                    p.scene.slices.insert(drop.min(p.scene.slices.len()), it);
                                }
                            }
                        });
                    }
                }
                Mode::Left | Mode::Right => {
                    let Some(s) = slices.iter().find(|s| s.id == id).cloned() else { return };
                    let scale = s.time_scale.max(1e-3);
                    let (ns, ne) = if mode == Mode::Left {
                        (s.source_end_ms - (b0 - pv.a) * scale, s.source_end_ms)
                    } else {
                        (s.source_start_ms, s.source_start_ms + (pv.b - a0) * scale)
                    };
                    st.edit(|p| {
                        let Some(i) = p.scene.slices.iter().position(|x| x.id == id) else { return };
                        let (old_s, old_e, dis) = (p.scene.slices[i].source_start_ms, p.scene.slices[i].source_end_ms, p.scene.slices[i].disabled);
                        // a jump cut's edge is shared with the clip beside it: that clip gives or takes the time
                        let joined = |o: &Slice, edge: f64, at_end: bool| (if at_end { o.source_end_ms } else { o.source_start_ms } - edge).abs() < 1.0 && o.disabled != dis && !o.freeze;
                        let (mut ns, mut ne) = (ns, ne);
                        let prev_joined = mode == Mode::Left && i > 0 && joined(&p.scene.slices[i - 1], old_s, true);
                        let next_joined = mode == Mode::Right && i + 1 < p.scene.slices.len() && joined(&p.scene.slices[i + 1], old_e, false);
                        if prev_joined {
                            ns = ns.max(p.scene.slices[i - 1].source_start_ms + 40.0);
                        }
                        if next_joined {
                            ne = ne.min(p.scene.slices[i + 1].source_end_ms - 40.0);
                        }
                        let x = &mut p.scene.slices[i];
                        x.source_start_ms = ns.clamp(0.0, ne - 200.0);
                        x.source_end_ms = ne.clamp(x.source_start_ms + 200.0, dur);
                        let (new_s, new_e) = (x.source_start_ms, x.source_end_ms);
                        if prev_joined {
                            p.scene.slices[i - 1].source_end_ms = new_s;
                        }
                        if next_joined {
                            p.scene.slices[i + 1].source_start_ms = new_e;
                        }
                    });
                }
            }
        }
        Kind::Ramp => {}
    }
    let _ = (icons::markup, Slice::default);
}
