//! The Export dialog (format, size, rate, quality, motion blur, compress-after) and the
//! background job behind it, laid out like the 0.3 dialog.

use super::EditorState;
use crate::kit;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use screencraft_encode::{available_video_codecs, Container, Effort, HwPreference, VideoCodec};
use screencraft_engine::{export_project, ExportOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Quality {
    Small,
    Balanced,
    High,
    Max,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp4,
    WebM,
    Gif,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CPreset {
    Balanced,
    Small,
    Tiny,
}

struct Job {
    progress: Arc<AtomicU32>,
    text: Arc<Mutex<String>>,
    cancel: Arc<AtomicBool>,
    result: Arc<Mutex<Option<Result<(PathBuf, String), String>>>>,
}

pub struct ExportState {
    pub open: bool,
    format: Format,
    codec: VideoCodec,
    resolution: u32,
    fps: u32,
    quality: Quality,
    blur: bool,
    compress: bool,
    cpreset: CPreset,
    keep: bool,
    job: Option<Job>,
    finished: Option<Result<(PathBuf, String), String>>,
}

pub fn default_codec() -> VideoCodec {
    if available_video_codecs().contains(&VideoCodec::H264) {
        VideoCodec::H264
    } else {
        VideoCodec::Av1
    }
}

impl Default for ExportState {
    fn default() -> Self {
        Self {
            open: false,
            format: Format::Mp4,
            codec: default_codec(),
            resolution: 1080,
            fps: 60,
            quality: Quality::Balanced,
            blur: true,
            compress: false,
            cpreset: CPreset::Balanced,
            keep: false,
            job: None,
            finished: None,
        }
    }
}

impl ExportState {
    pub fn poll(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.job {
            let done = job.result.lock().unwrap().take();
            if let Some(r) = done {
                self.finished = Some(r);
                self.job = None;
            } else {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
        }
    }

    pub fn busy(&self) -> bool {
        self.job.is_some()
    }
}

fn quality_k(q: Quality) -> f64 {
    match q {
        Quality::Small => 0.5,
        Quality::Balanced => 1.0,
        Quality::High => 1.8,
        Quality::Max => 3.0,
    }
}

fn bitrate(q: Quality, w: u32, h: u32, fps: u32) -> u32 {
    (screencraft_encode::default_bitrate_kbps(w, h, fps) as f64 * quality_k(q)).round() as u32
}

fn fmt_mb(b: f64) -> String {
    format!("{:.*} MB", if b < 1e7 { 2 } else { 1 }, b / 1e6)
}

/// Compression presets, as in 0.3: (codec label, quality, height cap, audio kbps).
fn cpreset(c: CPreset) -> (u32, Option<u32>, u32) {
    match c {
        CPreset::Balanced => (60, None, 160),
        CPreset::Small => (45, None, 128),
        CPreset::Tiny => (30, Some(720), 96),
    }
}

fn row(ui: &mut egui::Ui, label: &str, sub: Option<&str>, add: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(7.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_max_width(270.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            ui.label(egui::RichText::new(label).size(13.0).color(TEXT));
            if let Some(s) = sub {
                ui.add(egui::Label::new(egui::RichText::new(s).size(12.0).color(MUTED)).wrap());
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add);
    });
    ui.add_space(7.0);
}

fn dialog_btn(ui: &mut egui::Ui, text: &str, primary: bool) -> bool {
    if primary {
        kit::primary(ui, None, text).clicked()
    } else {
        kit::pill(ui, None, text, false, false, false, None).clicked()
    }
}

pub fn show(st: &mut EditorState, ctx: &egui::Context) {
    if !st.export.open && st.export.job.is_none() && st.export.finished.is_none() {
        return;
    }
    // dim the editor behind the dialog
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("export-dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        ui.allocate_rect(screen, Sense::click());
        ui.painter().rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(4, 7, 12, 168));
    });
    let mut start: Option<PathBuf> = None;
    let mut cancel = false;
    let mut close = false;
    let (w_src, h_src) = screencraft_render::output_size(&st.project, st.export.resolution);
    let dur = st.play_duration_ms() / 1000.0;
    let has_audio = st.project.media.has_microphone || st.project.media.has_system_audio || st.project.media.mic.is_some() || !st.project.scene.music_clips.is_empty();
    let compress_ok = cfg!(any(target_os = "macos", target_os = "windows"));

    egui::Window::new("Export")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .frame(
            egui::Frame::new()
                .fill(PANEL)
                .stroke(Stroke::new(1.0, LINE_2))
                .corner_radius(CornerRadius::same(12))
                .inner_margin(egui::Margin::same(22))
                .shadow(egui::Shadow { offset: [0, 24], blur: 70, spread: 0, color: Color32::from_black_alpha(140) }),
        )
        .fixed_size(Vec2::new(426.0, 0.0))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 0.0);
            ui.label(egui::RichText::new("Export").size(16.0).strong().color(TEXT));
            ui.add_space(12.0);
            let (line, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, LINE);
            ui.add_space(4.0);

            if let Some(job) = &st.export.job {
                let v = job.progress.load(Ordering::Relaxed) as f32 / 1000.0;
                let text = job.text.lock().unwrap().clone();
                ui.add_space(14.0);
                bar(ui, v);
                ui.add_space(8.0);
                ui.label(egui::RichText::new(text).size(13.0).color(MUTED));
                ui.add_space(14.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if dialog_btn(ui, "Cancel", false) {
                        cancel = true;
                    }
                });
                return;
            }
            if let Some(fin) = &st.export.finished {
                ui.add_space(14.0);
                match fin {
                    Ok((path, summary)) => {
                        bar(ui, 1.0);
                        ui.add_space(8.0);
                        ui.add(egui::Label::new(egui::RichText::new(summary).size(13.0).color(MUTED)).wrap());
                        ui.add_space(14.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if dialog_btn(ui, "Done", true) {
                                close = true;
                            }
                            if dialog_btn(ui, "Show in folder", false) {
                                super::dialogs::reveal(path);
                            }
                        });
                    }
                    Err(e) => {
                        ui.add(egui::Label::new(egui::RichText::new(format!("Export failed: {e}")).size(13.0).color(DANGER)).wrap());
                        ui.add_space(14.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if dialog_btn(ui, "Close", true) {
                                close = true;
                            }
                        });
                    }
                }
                return;
            }

            let ex = &mut st.export;
            row(ui, "Format", None, |ui| {
                let mut o: Vec<(Format, &str)> = vec![(Format::Mp4, "MP4")];
                if available_video_codecs().contains(&VideoCodec::Av1) {
                    o.push((Format::WebM, "WebM"));
                }
                o.push((Format::Gif, "GIF"));
                kit::segmented(ui, None, &o, &mut ex.format);
            });
            row(ui, "Resolution", None, |ui| {
                kit::segmented(ui, None, &[(720u32, "720p"), (1080, "1080p"), (1440, "1440p"), (2160, "4K")], &mut ex.resolution);
            });
            if ex.format != Format::Gif {
                row(ui, "Frame rate", None, |ui| {
                    kit::segmented(ui, None, &[(30u32, "30"), (60, "60")], &mut ex.fps);
                });
                if ex.format == Format::Mp4 {
                    row(ui, "Codec", None, |ui| {
                        let mut o: Vec<(VideoCodec, &str)> = Vec::new();
                        if available_video_codecs().contains(&VideoCodec::H264) {
                            o.push((VideoCodec::H264, "H.264"));
                        }
                        o.push((VideoCodec::Av1, "AV1"));
                        if !o.iter().any(|(c, _)| *c == ex.codec) {
                            ex.codec = o[0].0;
                        }
                        kit::segmented(ui, None, &o, &mut ex.codec);
                    });
                }
                row(ui, "Quality", None, |ui| {
                    kit::segmented(ui, None, &[(Quality::Small, "Small"), (Quality::Balanced, "Balanced"), (Quality::High, "High"), (Quality::Max, "Max")], &mut ex.quality);
                });
                row(ui, "Motion blur", None, |ui| {
                    kit::segmented(ui, None, &[(false, "Off"), (true, "On")], &mut ex.blur);
                });
                if ex.format == Format::Mp4 && compress_ok {
                    row(ui, "Compress after export", Some("Re-encodes the finished file to make it smaller"), |ui| {
                        toggle(ui, &mut ex.compress);
                    });
                    if ex.compress {
                        row(ui, "Compression", None, |ui| {
                            kit::segmented(ui, None, &[(CPreset::Balanced, "Balanced"), (CPreset::Small, "Smaller"), (CPreset::Tiny, "Smallest")], &mut ex.cpreset);
                        });
                        row(ui, "Export both", Some("Saves “name” and “name compressed” side by side"), |ui| {
                            let mut k = ex.keep;
                            let (r, resp) = ui.allocate_exact_size(Vec2::splat(17.0), Sense::click());
                            ui.painter().rect_filled(r, CornerRadius::same(4), if k { ACCENT } else { PANEL_2 });
                            ui.painter().rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0, if k { ACCENT } else { LINE_2 }), StrokeKind::Inside);
                            if k {
                                crate::icons::paint(ui, "check", r.shrink(3.0), Color32::WHITE);
                            }
                            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                k = !k;
                            }
                            ex.keep = k;
                        });
                    }
                }
            }

            // estimate box
            ui.add_space(14.0);
            let est_original = if ex.format == Format::Gif {
                let (gw, gh) = screencraft_render::output_size(&st.project, ex.resolution.min(540));
                gw as f64 * gh as f64 * 15.0 * dur * 0.09
            } else {
                (bitrate(ex.quality, w_src, h_src, ex.fps) as f64 * 1000.0 + if has_audio { 160_000.0 } else { 0.0 }) * dur / 8.0
            };
            let show_c = ex.compress && ex.format == Format::Mp4 && compress_ok;
            let mut lines: Vec<(&str, String)> = vec![("Original export", format!("≈ {}", fmt_mb(est_original)))];
            if show_c {
                let (q, hcap, akbps) = cpreset(ex.cpreset);
                let scale = hcap.filter(|c| *c < h_src).map(|c| c as f64 / h_src as f64).unwrap_or(1.0);
                let (cw, ch) = ((w_src as f64 * scale) as u32, (h_src as f64 * scale) as u32);
                let mut rate = screencraft_encode::default_bitrate_kbps(cw, ch, ex.fps) as f64 * 1000.0 * (q as f64 / 60.0).powf(1.6);
                rate = rate.min(bitrate(ex.quality, w_src, h_src, ex.fps) as f64 * 1000.0 * 0.98);
                let comp = (rate + if has_audio { akbps as f64 * 1000.0 } else { 0.0 }) * dur / 8.0;
                let pct = ((1.0 - comp / est_original) * 100.0).round().max(0.0);
                lines.push(("Compressed", format!("≈ {}  ({pct}% smaller)", fmt_mb(comp))));
                if ex.keep {
                    lines.push(("Total on disk", format!("≈ {}", fmt_mb(est_original + comp))));
                }
            }
            let h = 24.0 + lines.len() as f32 * 24.0 - 6.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::hover());
            ui.painter().rect_filled(rect, CornerRadius::same(8), BG);
            ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, LINE), StrokeKind::Inside);
            ui.painter().rect_filled(Rect::from_min_size(rect.min, Vec2::new(3.0, rect.height())), CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, ACCENT);
            for (i, (k, v)) in lines.iter().enumerate() {
                let y = rect.min.y + 12.0 + i as f32 * 24.0 + 9.0;
                ui.painter().text(Pos2::new(rect.min.x + 14.0, y), Align2::LEFT_CENTER, k, kit::font(13.0), MUTED);
                ui.painter().text(Pos2::new(rect.max.x - 14.0, y), Align2::RIGHT_CENTER, v, kit::font(13.0), TEXT);
            }
            ui.add_space(20.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if dialog_btn(ui, "Export", true) {
                    let ext = match ex.format {
                        Format::Mp4 => "mp4",
                        Format::WebM => "webm",
                        Format::Gif => "gif",
                    };
                    let mut dlg = rfd::FileDialog::new().set_file_name(format!("{}.{ext}", st.project.name)).add_filter(ext.to_uppercase(), &[ext]);
                    if let Some(d) = crate::prefs::Prefs::load().export_dir.or_else(|| st.package.dir.parent().map(|d| d.to_path_buf())) {
                        dlg = dlg.set_directory(d);
                    }
                    if let Some(path) = dlg.save_file() {
                        start = Some(path);
                    }
                }
                if dialog_btn(ui, "Cancel", false) {
                    close = true;
                }
            });
        });

    if let Some(path) = start {
        launch(st, ctx, path);
    }
    if cancel {
        if let Some(j) = &st.export.job {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        if st.export.job.is_none() {
            st.export.open = false;
            st.export.finished = None;
        } else if let Some(j) = &st.export.job {
            j.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn toggle(ui: &mut egui::Ui, v: &mut bool) {
    let (r, resp) = ui.allocate_exact_size(Vec2::new(36.0, 20.0), Sense::click());
    ui.painter().rect_filled(r, CornerRadius::same(6), if *v { ACCENT } else { LINE_2 });
    let kx = r.min.x + 3.0 + if *v { 16.0 } else { 0.0 };
    ui.painter().rect_filled(Rect::from_min_size(Pos2::new(kx, r.min.y + 3.0), Vec2::splat(14.0)), CornerRadius::same(4), Color32::WHITE);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *v = !*v;
    }
}

fn bar(ui: &mut egui::Ui, v: f32) {
    ui.add_space(16.0);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 6.0), Sense::hover());
    ui.painter().rect_filled(r, CornerRadius::same(3), LINE_2);
    ui.painter().rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width() * v.clamp(0.0, 1.0), 6.0)), CornerRadius::same(3), ACCENT_2);
}

fn strip_ext(p: &Path) -> PathBuf {
    p.with_extension("")
}

fn launch(st: &mut EditorState, ctx: &egui::Context, path: PathBuf) {
    let ex = &st.export;
    let (w, h) = screencraft_render::output_size(&st.project, ex.resolution);
    let format = ex.format;
    let do_compress = ex.compress && format == Format::Mp4;
    let keep = ex.keep;
    let cpre = ex.cpreset;
    let mut project = st.project.clone();
    project.config.animations.motion_blur = if ex.blur { 0.8 } else { 0.0 };
    let container = if format == Format::WebM { Container::WebM } else { Container::Mp4 };
    let codec = if format == Format::WebM { if available_video_codecs().contains(&VideoCodec::Vp9) { VideoCodec::Vp9 } else { VideoCodec::Av1 } } else { ex.codec };
    let opts = ExportOptions {
        container,
        codec,
        resolution: ex.resolution,
        fps: ex.fps,
        bitrate_kbps: Some(bitrate(ex.quality, w, h, ex.fps)),
        effort: match ex.quality {
            Quality::Small => Effort::Best,
            _ => Effort::Balanced,
        },
        hardware: HwPreference::Auto,
        audio: true,
        threads: 0,
    };
    let resolution = ex.resolution;
    st.save_now();
    st.pause();
    let package = st.package.clone();
    let progress = Arc::new(AtomicU32::new(0));
    let text = Arc::new(Mutex::new("Preparing…".to_string()));
    let cancel = Arc::new(AtomicBool::new(false));
    let result = Arc::new(Mutex::new(None));
    let (pg, tx, cf, rs, ctx2) = (progress.clone(), text.clone(), cancel.clone(), result.clone(), ctx.clone());
    std::thread::spawn(move || {
        let scale = if do_compress { 0.55 } else { 1.0 };
        let raw = if do_compress && !keep { path.with_extension(format!("sc-tmp-{}.mp4", std::process::id())) } else { path.clone() };
        let set = |f: f32, t: String| {
            pg.store((f * scale * 1000.0) as u32, Ordering::Relaxed);
            *tx.lock().unwrap() = if do_compress { format!("Step 1 of 2 · {t}") } else { t };
            ctx2.request_repaint();
        };
        let step1: Result<String, String> = match format {
            Format::Gif => screencraft_engine::export_gif(&package, &project, resolution, &raw, |f| {
                set(f, format!("Rendering… {}%", (f * 100.0).round()));
                !cf.load(Ordering::Relaxed)
            })
            .map(|r| format!("Saved {} ({}×{}).", fmt_mb(r.bytes as f64), r.width, r.height)),
            _ => export_project(&package, &project, &opts, &raw, |f| {
                set(f, format!("Rendering… {}%", (f * 100.0).round()));
                !cf.load(Ordering::Relaxed)
            })
            .map(|r| format!("Saved {} · {}×{} · {} frames · {}", fmt_mb(r.summary.bytes as f64), r.width, r.height, r.frames, r.summary.encoder)),
        };
        let out: Result<(PathBuf, String), String> = match step1 {
            Err(e) if cf.load(Ordering::Relaxed) => {
                let _ = e;
                Err("Cancelled".into())
            }
            Err(e) => Err(e),
            Ok(summary) if !do_compress => Ok((path.clone(), summary)),
            Ok(_) => {
                let (q, hcap, akbps) = cpreset(cpre);
                let compressed = PathBuf::from(format!("{} compressed.mp4", strip_ext(&path).display()));
                let before = std::fs::metadata(&raw).map(|m| m.len()).unwrap_or(0);
                let r = screencraft_engine::compress::compress_video(
                    &screencraft_engine::compress::CompressOptions {
                        input: raw.clone(),
                        output: compressed.clone(),
                        hevc: !matches!(cpre, CPreset::Balanced),
                        quality: q as f32,
                        height: hcap,
                        fps: None,
                        audio_kbps: Some(akbps),
                    },
                    |f| {
                        pg.store(((0.55 + f * 0.45) * 1000.0) as u32, Ordering::Relaxed);
                        *tx.lock().unwrap() = format!("Step 2 of 2 · Compressing… {}%", (f * 100.0).round());
                        ctx2.request_repaint();
                        !cf.load(Ordering::Relaxed)
                    },
                );
                if !keep {
                    let _ = std::fs::remove_file(&raw);
                }
                r.map(|size| (compressed, format!("Saved! {} → {}", fmt_mb(before as f64), fmt_mb(size as f64))))
            }
        };
        *rs.lock().unwrap() = Some(out);
        ctx2.request_repaint();
    });
    st.export.job = Some(Job { progress, text, cancel, result });
    st.export.open = false;
}
