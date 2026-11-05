//! The standalone compressor window (a port of `compress.html`): pick a video, choose a preset or
//! fine-tune, and re-encode it smaller.

use crate::kit;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2, ViewportBuilder, ViewportId};
use screencraft_encode::mp4read::{Mp4File, TrackKind};
use screencraft_engine::compress::{compress_video, CompressOptions};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

struct Preset {
    name: &'static str,
    sub: &'static str,
    quality: f32,
    height: Option<u32>,
    fps: Option<u32>,
}

const PRESETS: [Preset; 6] = [
    Preset { name: "Fast 1080p30", sub: "H.264 · quality 55", quality: 55.0, height: Some(1080), fps: Some(30) },
    Preset { name: "HQ 1080p30", sub: "H.264 · quality 75", quality: 75.0, height: Some(1080), fps: Some(30) },
    Preset { name: "Super HQ 1080p60", sub: "H.264 · quality 85", quality: 85.0, height: Some(1080), fps: None },
    Preset { name: "Fast 720p30", sub: "H.264 · quality 50", quality: 50.0, height: Some(720), fps: Some(30) },
    Preset { name: "Very Fast 480p30", sub: "H.264 · quality 40", quality: 40.0, height: Some(480), fps: Some(30) },
    Preset { name: "Tiny share", sub: "H.264 · 720p · quality 30", quality: 30.0, height: Some(720), fps: Some(30) },
];

struct Probe {
    path: PathBuf,
    width: u32,
    height: u32,
    fps: f64,
    duration_s: f64,
    size: u64,
    kbps: f64,
    has_audio: bool,
}

fn probe(path: &PathBuf) -> Result<Probe, String> {
    let f = Mp4File::open(path).map_err(|e| e.to_string())?;
    let v = f.video().ok_or("no video track")?;
    let n = v.samples.len();
    let dur = (f.duration_us as f64 / 1e6).max(0.001);
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let fps = if n > 1 { (n as f64 - 1.0) * 1e6 / (v.samples.last().unwrap().pts_us - v.samples[0].pts_us).max(1) as f64 } else { 30.0 };
    Ok(Probe { path: path.clone(), width: v.width, height: v.height, fps, duration_s: dur, size, kbps: size as f64 * 8.0 / dur / 1000.0, has_audio: f.tracks.iter().any(|t| t.kind == TrackKind::Audio) })
}

#[derive(Default)]
struct Job {
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<(PathBuf, u64), String>>>>,
}

pub struct Compressor {
    src: Option<Probe>,
    preset: Option<usize>,
    quality: f32,
    height: Option<u32>,
    fps: Option<u32>,
    audio: bool,
    job: Option<Job>,
    status: String,
    done: Option<PathBuf>,
}

impl Compressor {
    pub fn new(input: Option<PathBuf>) -> Self {
        let mut c = Self { src: None, preset: Some(0), quality: 55.0, height: Some(1080), fps: Some(30), audio: true, job: None, status: String::new(), done: None };
        if let Some(p) = input {
            c.load(p);
        }
        c
    }

    fn load(&mut self, p: PathBuf) {
        match probe(&p) {
            Ok(pr) => {
                self.src = Some(pr);
                self.status.clear();
                self.done = None;
            }
            Err(e) => self.status = format!("Could not read that file: {e}"),
        }
    }

    fn apply(&mut self, i: usize) {
        let p = &PRESETS[i];
        self.preset = Some(i);
        self.quality = p.quality;
        self.height = p.height;
        self.fps = p.fps;
    }

    fn estimate(&self) -> Option<(f64, f64)> {
        let s = self.src.as_ref()?;
        let k = self.height.filter(|h| *h < s.height).map(|h| h as f64 / s.height as f64).unwrap_or(1.0);
        let (w, h) = ((s.width as f64 * k) as u32, (s.height as f64 * k) as u32);
        let fps = self.fps.map(|f| f as f64).unwrap_or(s.fps).min(s.fps);
        let mut rate = screencraft_encode::default_bitrate_kbps(w, h, fps.round() as u32) as f64 * 1000.0 * (self.quality as f64 / 60.0).powf(1.6);
        if s.kbps > 0.0 {
            rate = rate.min(s.kbps * 1000.0 * 0.98);
        }
        let audio = if s.has_audio && self.audio { 128_000.0 } else { 0.0 };
        Some(((rate + audio) * s.duration_s / 8.0, rate))
    }

    /// Draw the window; returns false once it is closed.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        let mut keep = true;
        let finished = self.job.as_ref().map(|j| j.result.lock().unwrap().take());
        if let Some(res) = finished {
            if let Some(r) = res {
                match r {
                    Ok((p, size)) => {
                        let before = self.src.as_ref().map(|s| s.size).unwrap_or(0);
                        self.status = format!(
                            "Done — {} → {} ({}% smaller)",
                            mb(before as f64),
                            mb(size as f64),
                            if before > 0 {
                                ((1.0 - size as f64 / before as f64) * 100.0)
                                    .max(0.0)
                                    .round()
                            } else {
                                0.0
                            }
                        );
                        self.done = Some(p);
                    }
                    Err(e) => {
                        self.status = if e == "cancelled" {
                            "Cancelled".into()
                        } else {
                            format!("Failed: {e}")
                        }
                    }
                }
                self.job = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
        let builder = ViewportBuilder::default()
            .with_title("Compress video")
            .with_inner_size([1000.0, 720.0])
            .with_min_inner_size([820.0, 620.0])
            .with_resizable(true);
        ctx.show_viewport_immediate(ViewportId::from_hash_of("compressor"), builder, |ui, _| {
            egui::Panel::right("comp-presets")
                .exact_size(270.0)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin { left: 12, right: 12, top: 20, bottom: 12 }))
                .show_inside(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    ui.label(egui::RichText::new("Presets").size(13.0).strong().color(TEXT));
                    ui.label(egui::RichText::new("Choose a starting point").size(12.0).color(MUTED));
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        for (i, p) in PRESETS.iter().enumerate() {
                            let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 56.0), Sense::click());
                            let selected = self.preset == Some(i);
                            if selected || response.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(7), PANEL_2);
                            }
                            if selected {
                                ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())), 0.0, ACCENT_2);
                            }
                            ui.painter().text(Pos2::new(rect.min.x + 12.0, rect.min.y + 19.0), Align2::LEFT_CENTER, p.name, kit::font(13.0), if selected { Color32::WHITE } else { Color32::from_rgb(0xcd, 0xd5, 0xe5) });
                            ui.painter().text(Pos2::new(rect.min.x + 12.0, rect.min.y + 39.0), Align2::LEFT_CENTER, p.sub, kit::font(12.0), MUTED);
                            ui.painter().rect_stroke(
                                rect.expand(1.0),
                                CornerRadius::same(8),
                                Stroke::new(1.0, if response.has_focus() { TEXT } else { Color32::TRANSPARENT }),
                                StrokeKind::Outside,
                            );
                            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                            let chosen = activate(ui, &response);
                            response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::RadioButton, ui.is_enabled(), self.preset == Some(i), format!("{}, {}", p.name, p.sub)));
                            if chosen {
                                self.apply(i);
                            }
                        }
                    });
                });
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin { left: 26, right: 26, top: 22, bottom: 18 }))
                .show_inside(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(8.0, 12.0);
                    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                        ui.label(egui::RichText::new("Compress video").size(21.0).strong().color(TEXT));
                        ui.label(egui::RichText::new("Choose a preset or adjust the output settings.").size(13.0).color(MUTED));

                        card(ui, |ui| {
                            ui.horizontal(|ui| {
                                crate::icons::paint(ui, "captions", Rect::from_min_size(ui.cursor().min + Vec2::new(0.0, 4.0), Vec2::splat(30.0)), Color32::from_rgb(0xb9, 0xc4, 0xd9));
                                ui.add_space(40.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(egui::RichText::new("Source video").size(11.5).strong().color(MUTED));
                                    match &self.src {
                                        Some(s) => {
                                            ui.label(egui::RichText::new(s.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()).strong().size(13.0).color(TEXT));
                                            ui.label(egui::RichText::new(format!("{}×{} · {:.0} fps · {}:{:02} · {} · {} kbps{}", s.width, s.height, s.fps, s.duration_s as u64 / 60, s.duration_s as u64 % 60, mb(s.size as f64), s.kbps.round(), if s.has_audio { " · audio" } else { "" })).size(12.0).color(MUTED));
                                        }
                                        None => {
                                            ui.label(egui::RichText::new("No video selected").strong().size(13.0).color(TEXT));
                                            ui.label(egui::RichText::new("Choose an MP4, M4V, or MOV file.").size(12.0).color(MUTED));
                                        }
                                    }
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    let response = kit::pill(ui, None, "Choose file…", false, false, false, None);
                                    let response = mark_button(ui, response, "Choose a video file", true);
                                    if activate(ui, &response) {
                                        if let Some(path) = rfd::FileDialog::new().add_filter("Video", &["mp4", "m4v", "mov"]).pick_file() {
                                            self.load(path);
                                        }
                                    }
                                });
                            });
                        });

                        card(ui, |ui| {
                            ui.label(egui::RichText::new("Output settings").size(14.0).strong().color(TEXT));
                            ui.add_space(4.0);
                            let heights: [(Option<u32>, &str); 7] = [(None, "Same as source"), (Some(2160), "2160p (4K)"), (Some(1440), "1440p"), (Some(1080), "1080p"), (Some(720), "720p"), (Some(480), "480p"), (Some(360), "360p")];
                            let fpss: [(Option<u32>, &str); 5] = [(None, "Same as source"), (Some(60), "60 fps"), (Some(30), "30 fps"), (Some(24), "24 fps"), (Some(15), "15 fps")];
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    let label = ui.label(egui::RichText::new("Resolution").size(12.5).color(LABEL));
                                    let current = heights.iter().find(|entry| entry.0 == self.height).map(|entry| entry.1).unwrap_or("Same as source");
                                    if let Some(index) = select(ui, "comp-h", current, &heights.iter().map(|entry| entry.1).collect::<Vec<_>>(), 200.0, label.id) {
                                        self.height = heights[index].0;
                                        self.preset = None;
                                    }
                                });
                                ui.add_space(20.0);
                                ui.vertical(|ui| {
                                    let label = ui.label(egui::RichText::new("Frame rate").size(12.5).color(LABEL));
                                    let current = fpss.iter().find(|entry| entry.0 == self.fps).map(|entry| entry.1).unwrap_or("Same as source");
                                    if let Some(index) = select(ui, "comp-f", current, &fpss.iter().map(|entry| entry.1).collect::<Vec<_>>(), 200.0, label.id) {
                                        self.fps = fpss[index].0;
                                        self.preset = None;
                                    }
                                });
                            });
                            quality_slider(ui, &mut self.quality, &mut self.preset);
                            audio_toggle(ui, &mut self.audio);
                        });

                        let (size, rate) = self.estimate().unwrap_or((0.0, 0.0));
                        card(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.label(egui::RichText::new("Estimated output").size(12.5).color(MUTED));
                                    let text = match (&self.src, size > 0.0) {
                                        (Some(source), true) if size < source.size as f64 => {
                                            format!("{} · {}% smaller", mb(size), ((1.0 - size / source.size.max(1) as f64) * 100.0).round())
                                        }
                                        (Some(_), true) => format!("{} · may be larger than source", mb(size)),
                                        _ => "—".into(),
                                    };
                                    ui.label(egui::RichText::new(text).size(18.0).color(ACCENT_2));
                                });
                                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                                    ui.vertical(|ui| {
                                        ui.label(egui::RichText::new("Estimated video bitrate").size(12.5).color(MUTED));
                                        ui.label(egui::RichText::new(if rate > 0.0 { format!("{:.2} Mbps", rate / 1e6) } else { "—".into() }).size(18.0).color(ACCENT_2));
                                    });
                                });
                            });
                        });

                        if let Some(job) = &self.job {
                            card(ui, |ui| {
                                let progress = job.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                                let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 8.0), Sense::hover());
                                ui.painter().rect_filled(rect, CornerRadius::same(4), LINE_2);
                                ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(rect.width() * progress, rect.height())), CornerRadius::same(4), ACCENT_2);
                                response.widget_info(|| egui::WidgetInfo::new(egui::WidgetType::ProgressIndicator));
                                ui.label(egui::RichText::new(format!("Compressing… {}%", (progress * 100.0).round())).size(13.0).color(MUTED));
                            });
                        } else if !self.status.is_empty() {
                            ui.label(egui::RichText::new(&self.status).size(13.0).color(MUTED));
                        }
                    });

                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(job) = &self.job {
                            let response = kit::pill(ui, None, "Cancel", false, false, false, None);
                            let response = mark_button(ui, response, "Cancel compression", true);
                            if activate(ui, &response) {
                                job.cancel.store(true, Ordering::Relaxed);
                            }
                        } else {
                            let ready = self.src.is_some();
                            let response = ui.add_enabled_ui(ready, |ui| kit::primary(ui, None, "Compress…")).inner;
                            let response = mark_button(ui, response, "Compress video and choose a save location", ready);
                            if activate(ui, &response) {
                                self.start(ui.ctx());
                            }
                            if let Some(path) = self.done.clone() {
                                let response = kit::pill(ui, None, "Show in folder", false, false, false, None);
                                let response = mark_button(ui, response, "Show compressed video in folder", true);
                                if activate(ui, &response) {
                                    crate::editor::reveal(&path);
                                }
                            }
                        }
                    });
                });
            if ui.ctx().input(|i| i.viewport().close_requested() || (i.key_pressed(egui::Key::Escape) && self.job.is_none())) {
                if let Some(j) = &self.job {
                    j.cancel.store(true, Ordering::Relaxed);
                }
                keep = false;
            }
        });
        keep
    }

    fn start(&mut self, ctx: &egui::Context) {
        let Some(s) = &self.src else { return };
        let base = s.path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(out) = rfd::FileDialog::new().set_file_name(format!("{base} (compressed).mp4")).add_filter("MP4", &["mp4"]).save_file() else { return };
        let job = Job::default();
        let (pg, cf, rs, ctx2) = (job.progress.clone(), job.cancel.clone(), job.result.clone(), ctx.clone());
        let opts = CompressOptions { input: s.path.clone(), output: out.clone(), hevc: false, quality: self.quality, height: self.height, fps: self.fps.map(|f| f as f32), audio_kbps: if self.audio { Some(128) } else { None } };
        std::thread::spawn(move || {
            let r = compress_video(&opts, |f| {
                pg.store((f * 1000.0) as u32, Ordering::Relaxed);
                ctx2.request_repaint();
                !cf.load(Ordering::Relaxed)
            });
            *rs.lock().unwrap() = Some(r.map(|n| (out, n)));
            ctx2.request_repaint();
        });
        self.job = Some(job);
        self.status.clear();
    }
}

fn mb(b: f64) -> String {
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else {
        format!("{:.*} MB", if b < 1e7 { 2 } else { 1 }, b / 1e6)
    }
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0, LINE))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn select(
    ui: &mut egui::Ui,
    id: &str,
    current: &str,
    items: &[&str],
    min_width: f32,
    label: egui::Id,
) -> Option<usize> {
    let mut selected = None;
    let combo = egui::ComboBox::from_id_salt(id)
        .width(min_width)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (index, item) in items.iter().enumerate() {
                if ui.selectable_label(*item == current, *item).clicked() {
                    selected = Some(index);
                }
            }
        });
    combo.response.labelled_by(label);
    selected
}

fn quality_slider(ui: &mut egui::Ui, quality: &mut f32, preset: &mut Option<usize>) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Quality").size(12.5).color(LABEL));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(format!("{} / 100", quality.round() as u32))
                    .size(12.5)
                    .color(ACCENT_2),
            );
        });
    });

    let (rect, response) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 28.0),
        Sense::click_and_drag(),
    );
    let mut next = *quality;
    if response.clicked() || response.dragged() || response.drag_started() {
        if let Some(pointer) = response.interact_pointer_pos() {
            next = 5.0 + ((pointer.x - rect.min.x) / rect.width()).clamp(0.0, 1.0) * 95.0;
        }
    }
    if response.has_focus() {
        ui.input(|input| {
            if input.key_pressed(egui::Key::ArrowLeft) || input.key_pressed(egui::Key::ArrowDown) {
                next -= 1.0;
            }
            if input.key_pressed(egui::Key::ArrowRight) || input.key_pressed(egui::Key::ArrowUp) {
                next += 1.0;
            }
            if input.key_pressed(egui::Key::PageDown) {
                next -= 10.0;
            }
            if input.key_pressed(egui::Key::PageUp) {
                next += 10.0;
            }
            if input.key_pressed(egui::Key::Home) {
                next = 5.0;
            }
            if input.key_pressed(egui::Key::End) {
                next = 100.0;
            }
        });
    }
    next = next.round().clamp(5.0, 100.0);
    if next != *quality {
        *quality = next;
        *preset = None;
    }

    let track = Rect::from_min_max(
        Pos2::new(rect.min.x, rect.center().y - 2.0),
        Pos2::new(rect.max.x, rect.center().y + 2.0),
    );
    let t = (*quality - 5.0) / 95.0;
    let center_x = rect.min.x + t * rect.width();
    ui.painter()
        .rect_filled(track, CornerRadius::same(2), LINE_2);
    ui.painter().rect_filled(
        Rect::from_min_max(track.min, Pos2::new(center_x, track.max.y)),
        CornerRadius::same(2),
        ACCENT,
    );
    let thumb = Rect::from_center_size(Pos2::new(center_x, rect.center().y), Vec2::splat(15.0));
    ui.painter()
        .rect_filled(thumb.expand(2.0), CornerRadius::same(6), ACCENT_2);
    ui.painter().rect_filled(
        thumb,
        CornerRadius::same(5),
        if response.has_focus() || response.hovered() {
            Color32::WHITE
        } else {
            ACCENT_2
        },
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            CornerRadius::same(6),
            Stroke::new(1.0, TEXT),
            StrokeKind::Outside,
        );
    }
    response.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), *quality as f64, "Quality"));
}

fn audio_toggle(ui: &mut egui::Ui, keep_audio: &mut bool) {
    ui.add_space(6.0);
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 42.0), Sense::click());
    let label_rect = Rect::from_min_size(rect.min, Vec2::new(width - 52.0, 20.0));
    ui.put(
        label_rect,
        egui::Label::new(egui::RichText::new("Keep audio").size(13.0).color(TEXT)),
    );
    let hint_rect = Rect::from_min_size(
        Pos2::new(rect.min.x, rect.min.y + 19.0),
        Vec2::new(width - 52.0, 22.0),
    );
    ui.put(
        hint_rect,
        egui::Label::new(
            egui::RichText::new("Opus audio is carried over; other audio formats are left out.")
                .size(11.5)
                .color(MUTED),
        )
        .wrap(),
    );
    let switch = Rect::from_min_size(
        Pos2::new(rect.max.x - 38.0, rect.center().y - 11.0),
        Vec2::new(36.0, 22.0),
    );
    ui.painter().rect_filled(
        switch,
        CornerRadius::same(7),
        if *keep_audio { ACCENT } else { LINE_2 },
    );
    let knob_x = switch.min.x + 3.0 + if *keep_audio { 14.0 } else { 0.0 };
    ui.painter().rect_filled(
        Rect::from_min_size(Pos2::new(knob_x, switch.min.y + 3.0), Vec2::splat(16.0)),
        CornerRadius::same(5),
        Color32::WHITE,
    );
    if response.has_focus() {
        ui.painter().rect_stroke(
            rect.expand(1.0),
            CornerRadius::same(6),
            Stroke::new(1.0, TEXT),
            StrokeKind::Outside,
        );
    }
    let toggled = activate(ui, &response);
    if toggled {
        *keep_audio = !*keep_audio;
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            ui.is_enabled(),
            *keep_audio,
            "Keep audio",
        )
    });
}

fn mark_button(
    ui: &egui::Ui,
    response: egui::Response,
    label: &str,
    enabled: bool,
) -> egui::Response {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            CornerRadius::same(9),
            Stroke::new(1.0, TEXT),
            StrokeKind::Outside,
        );
    }
    response
}

fn activate(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.clicked()
        || (response.has_focus()
            && ui.input(|input| {
                input.key_pressed(egui::Key::Enter) || input.key_pressed(egui::Key::Space)
            }))
}
