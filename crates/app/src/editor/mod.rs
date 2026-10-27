//! The editor: top bar, inspector with its icon rail, preview stage, transport and the six-track
//! timeline, laid out exactly like the 0.3 editor.

mod actions;
mod dialogs;
mod export_ui;
pub mod ops;
mod panels;
mod playback;
mod preview;
mod timeline;

use crate::icons;
use crate::kit;
use crate::theme::{self, *};
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use screencraft_core::input::InputLog;
use screencraft_core::project::Project;
use screencraft_core::timeline::{display_to_effective, effective_duration_ms, effective_to_display, output_to_source};
use screencraft_core::track::{compute_track, RenderTrack};
use screencraft_engine::{build_soundtrack, FrameTable, Package};
use screencraft_render::{build_scene, output_size, Compositor, Slot};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub use export_ui::ExportState;
use playback::AudioPlayback;
use preview::{PreviewWorker, Want};

/// What is selected on the timeline.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Sel {
    None,
    Slice(String),
    Zoom(String),
    Layout(String),
    Annot(String),
    Caption(String),
    Music(String),
}

impl Sel {
    pub fn id(&self) -> Option<&str> {
        match self {
            Sel::None => None,
            Sel::Slice(s) | Sel::Zoom(s) | Sel::Layout(s) | Sel::Annot(s) | Sel::Caption(s) | Sel::Music(s) => Some(s),
        }
    }
}

/// What the save prompt is guarding.
#[derive(Clone, PartialEq, Debug)]
pub enum CloseIntent {
    /// This window is closing.
    Window,
    /// Another project is about to replace this one.
    Open(std::path::PathBuf),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Save,
    Discard,
    Cancel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Appearance,
    Cursor,
    Camera,
    Zoom,
    Audio,
    Crop,
    Annotate,
    Captions,
    Pace,
}

pub const TABS: [(Tab, &str, &str); 9] = [
    (Tab::Appearance, "appearance", "Appearance"),
    (Tab::Cursor, "cursor", "Cursor & motion"),
    (Tab::Camera, "camera", "Camera"),
    (Tab::Zoom, "zoom", "Zoom"),
    (Tab::Audio, "audio", "Audio"),
    (Tab::Crop, "crop", "Crop"),
    (Tab::Annotate, "annotate", "Annotate"),
    (Tab::Captions, "captions", "Captions"),
    (Tab::Pace, "pace", "Pace & transitions"),
];

#[derive(Clone)]
struct PreviewGeo {
    out: (u32, u32),
    sr: [f32; 4],
    view_w: f32,
    view_h: f32,
    bounds: Vec<screencraft_render::overlay::Bound>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum DragMode {
    Move,
    Resize,
    ArrowStart,
    ArrowEnd,
}

#[derive(Clone)]
struct AnnotDrag {
    id: String,
    mode: DragMode,
    p0: Pos2,
    start: (f64, f64, f64, f64, f64, f64),
    content: bool,
}

/// Preview frame rate of the render track (editing); exports use their own.
const TRACK_FPS: f64 = 60.0;

pub struct EditorState {
    pub package: Package,
    pub project: Project,
    pub input: InputLog,
    track: RenderTrack,
    track_dirty: bool,
    /// The track of what plays (jump cuts left out), for the sound effects under the soundtrack.
    eff_track: Option<RenderTrack>,
    undo: Vec<Project>,
    redo: Vec<Project>,
    last_undo_push: Instant,
    last_undo_key: Option<String>,
    pub sel: Sel,
    pub multi: Vec<String>,
    pub tab: Tab,
    /// Playhead in output (edited) time.
    pub playhead_ms: f64,
    playing: bool,
    rate: f64,
    play_anchor: Option<(Instant, f64)>,
    /// True while the soundtrack is playing: the playhead then follows the audio device's clock.
    audio_clock: bool,
    table: FrameTable,
    worker: PreviewWorker,
    uploaded_key: Option<(usize, Option<usize>, Option<usize>)>,
    cam_table: Option<FrameTable>,
    compositor: Compositor,
    render_state: eframe::egui_wgpu::RenderState,
    tex: Option<(egui::TextureId, (u32, u32))>,
    audio: Option<AudioPlayback>,
    pub timeline: timeline::View,
    pub waveform: Option<Arc<Vec<f32>>>,
    wave_rx: Option<std::sync::mpsc::Receiver<Vec<f32>>>,
    pub export: ExportState,
    dirty_since: Option<Instant>,
    /// Edits not yet saved: switching projects or closing asks first.
    pub dirty: bool,
    /// Asking what to do with unsaved edits before this window closes or another project opens.
    pub close_prompt: Option<CloseIntent>,
    /// The user's answer to `close_prompt`, collected by the app.
    pub close_answer: Option<Answer>,
    /// Set once the user decided to replace this project (its edits are settled).
    pub close_after_open: bool,
    pub error: Option<String>,
    id_counter: u64,
    /// Last rendered content rect in screen space (for the zoom-target picker).
    preview_content: Option<Rect>,
    pub preview_shown: Option<Rect>,
    /// Last scene's annotation boxes and geometry (for the drag handles on the preview).
    preview_geo: Option<PreviewGeo>,
    annot_drag: Option<AnnotDrag>,
    pub preview_res: u32,
    pub swatches: HashMap<String, egui::TextureHandle>,
    pub panel: panels::PanelState,
    pub dialogs: dialogs::Dialogs,
    pub title_edit: Option<String>,
    pub side_w: f32,
    pub tl_h: f32,
    pub wants_back: bool,
    /// A project the user asked to open from this window's Projects menu.
    pub open_request: Option<std::path::PathBuf>,
    /// Raw video to open in the compressor window.
    pub compress_request: Option<std::path::PathBuf>,
    /// Waveform peaks (0..1, 3000 bins) of music files, filled in by a worker.
    pub music_peaks: HashMap<String, Arc<Vec<f32>>>,
    peaks_pending: std::collections::HashSet<String>,
    peaks_tx: std::sync::mpsc::Sender<(String, Vec<f32>)>,
    peaks_rx: std::sync::mpsc::Receiver<(String, Vec<f32>)>,
}

impl EditorState {
    pub fn open(package: Package, ctx: &egui::Context, render_state: eframe::egui_wgpu::RenderState) -> Result<Self, String> {
        let mut project = package.load_project()?;
        let input = package.load_input(&project);
        let screen = package.screen_path(&project);
        let table = FrameTable::open(&screen)?;
        if project.scene.slices.is_empty() {
            project.scene.slices.push(screencraft_core::project::Slice {
                id: "slice-0".into(),
                source_start_ms: 0.0,
                source_end_ms: project.media.duration_ms,
                ..Default::default()
            });
        }
        let worker = PreviewWorker::spawn(package.clone(), project.clone(), ctx.clone());
        let compositor = Compositor::new(&render_state.device, &render_state.queue);

        // Waveform peaks, computed once in the background.
        let (wtx, wrx) = std::sync::mpsc::channel();
        {
            let (path, ctx) = (screen.clone(), ctx.clone());
            let track = project.media.system_track.or(project.media.mic_track);
            std::thread::spawn(move || {
                if let Some(t) = track {
                    if let Ok(peaks) = waveform_peaks(&path, t as usize) {
                        let _ = wtx.send(peaks);
                        ctx.request_repaint();
                    }
                }
            });
        }
        let (ptx, prx) = std::sync::mpsc::channel();
        let mut s = Self {
            package,
            project,
            input,
            track: RenderTrack::default(),
            track_dirty: true,
            eff_track: None,
            undo: Vec::new(),
            redo: Vec::new(),
            last_undo_push: Instant::now() - Duration::from_secs(10),
            last_undo_key: None,
            sel: Sel::None,
            multi: Vec::new(),
            tab: Tab::Appearance,
            playhead_ms: 0.0,
            playing: false,
            rate: 1.0,
            play_anchor: None,
            audio_clock: false,
            table,
            worker,
            uploaded_key: None,
            cam_table: None,
            compositor,
            render_state,
            tex: None,
            audio: None,
            timeline: timeline::View::default(),
            waveform: None,
            wave_rx: Some(wrx),
            export: ExportState::default(),
            dirty_since: None,
            dirty: false,
            close_prompt: None,
            close_answer: None,
            close_after_open: false,
            error: None,
            id_counter: 0,
            preview_content: None,
            preview_shown: None,
            preview_geo: None,
            annot_drag: None,
            preview_res: 720,
            swatches: HashMap::new(),
            panel: panels::PanelState::default(),
            dialogs: dialogs::Dialogs::default(),
            title_edit: None,
            side_w: 340.0,
            tl_h: 350.0,
            wants_back: false,
            open_request: None,
            compress_request: None,
            music_peaks: HashMap::new(),
            peaks_pending: Default::default(),
            peaks_tx: ptx,
            peaks_rx: prx,
        };
        s.timeline.fit_pending = true;
        Ok(s)
    }

    /// Developer hook: a tab name, or `zoom:N` / `slice:N` to select a block.
    pub fn apply_dev_selection(&mut self, what: Option<&str>) {
        let Some(w) = what else { return };
        if w == "auto-zoom" {
            self.auto_zoom();
        } else if w == "jump-cuts" || w == "jump-cuts-zoom" {
            if w == "jump-cuts-zoom" {
                self.panel.pace.jc_silence = 0.25;
                self.panel.pace.jc_sens = 0.9;
            }
            let o = &self.panel.pace;
            let settings = screencraft_core::jumpcut::JumpCutSettings { min_silence_ms: o.jc_silence * 1000.0, padding_ms: o.jc_padding * 1000.0, sensitivity: o.jc_sens };
            self.panel.pace.jc_status = match self.auto_jump_cuts(settings) {
                Ok((n, ms)) => format!("{n} jump cuts found, {:.1} s removed from the final video.", ms / 1000.0),
                Err(e) => e,
            };
            self.tab = Tab::Pace;
        } else if let Some(n) = w.strip_prefix("zoom:").and_then(|n| n.parse::<usize>().ok()) {
            if let Some(z) = self.project.scene.zooms.get(n) {
                self.sel = Sel::Zoom(z.id.clone());
                self.tab = Tab::Zoom;
            }
        } else if let Some(n) = w.strip_prefix("slice:").and_then(|n| n.parse::<usize>().ok()) {
            if let Some(s) = self.project.scene.slices.get(n) {
                self.sel = Sel::Slice(s.id.clone());
                self.tab = Tab::Pace;
            }
        } else if let Some(n) = w.strip_prefix("layout:").and_then(|n| n.parse::<usize>().ok()) {
            if let Some(s) = self.project.scene.camera_layouts.get(n) {
                self.sel = Sel::Layout(s.id.clone());
                self.tab = Tab::Camera;
            }
        } else if let Some(n) = w.strip_prefix("annot:").and_then(|n| n.parse::<usize>().ok()) {
            if let Some(s) = self.project.scene.annotations.get(n) {
                self.sel = Sel::Annot(s.id.clone());
                self.tab = Tab::Annotate;
            }
        } else if let Some(k) = w.strip_prefix("add-annot:") {
            self.add_annotation(k);
        } else if w == "export" {
            self.export.open = true;
        } else if w == "about" {
            self.dialogs.about = true;
        } else if w == "presets" {
            self.dialogs.presets_open = true;
        } else if w == "raw" {
            self.dialogs.raw = true;
        } else {
            self.tab = match w {
                "cursor" => Tab::Cursor,
                "camera" => Tab::Camera,
                "zoom" => Tab::Zoom,
                "audio" => Tab::Audio,
                "crop" => Tab::Crop,
                "annotate" => Tab::Annotate,
                "captions" => Tab::Captions,
                "pace" => Tab::Pace,
                _ => Tab::Appearance,
            };
        }
    }

    pub fn duration_ms(&self) -> f64 {
        ops::out_duration(&self.project.scene.slices)
    }

    pub fn new_id(&mut self, prefix: &str) -> String {
        self.id_counter += 1;
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros()).unwrap_or(0);
        format!("{prefix}-{t:x}-{}", self.id_counter)
    }

    fn ensure_track(&mut self) {
        if self.track_dirty {
            self.track = compute_track(&self.project, &self.input, TRACK_FPS);
            self.track_dirty = false;
        }
    }

    pub fn track(&mut self) -> &RenderTrack {
        self.ensure_track();
        &self.track
    }

    /// Mark the project edited by the UI: recompute the track and schedule a save.
    fn changed(&mut self) {
        self.track_dirty = true;
        self.eff_track = None;
        self.dirty = true;
        self.dirty_since = Some(Instant::now());
        if self.playing {
            self.restart_audio();
        }
    }

    /// Run `edit` on the project. History is pushed once per burst of edits.
    pub fn edit(&mut self, edit: impl FnOnce(&mut Project)) {
        self.edit_keyed(None, edit)
    }

    /// Like `edit`, but successive edits with the same key (a slider drag) share one undo step.
    pub fn edit_keyed(&mut self, key: Option<&str>, edit: impl FnOnce(&mut Project)) {
        let before = self.project.clone();
        edit(&mut self.project);
        if self.project != before {
            let same = key.is_some() && self.last_undo_key.as_deref() == key && self.last_undo_push.elapsed() < Duration::from_millis(900);
            if !same && self.last_undo_push.elapsed() > Duration::from_millis(250) || (!same && key.is_some()) {
                self.undo.push(before);
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
            }
            self.last_undo_key = key.map(String::from);
            self.last_undo_push = Instant::now();
            self.redo.clear();
            self.changed();
            let d = self.duration_ms();
            self.playhead_ms = self.playhead_ms.min(d);
        }
    }

    pub fn undo(&mut self) {
        if let Some(p) = self.undo.pop() {
            self.redo.push(std::mem::replace(&mut self.project, p));
            self.sel = Sel::None;
            self.changed();
        }
    }

    pub fn redo(&mut self) {
        if let Some(p) = self.redo.pop() {
            self.undo.push(std::mem::replace(&mut self.project, p));
            self.sel = Sel::None;
            self.changed();
        }
    }

    pub fn save_now(&mut self) {
        if let Err(e) = self.package.save_project(&self.project) {
            self.error = Some(e);
        }
        self.dirty_since = None;
        self.dirty = false;
    }

    /// Length of what plays: the timeline without its jump cuts.
    pub fn play_duration_ms(&self) -> f64 {
        effective_duration_ms(&self.project.scene.slices)
    }

    /// Timeline position -> play position (jump cuts skipped), and back.
    fn to_play(&self, t: f64) -> f64 {
        display_to_effective(&self.project.scene.slices, t)
    }

    fn to_timeline(&self, t: f64) -> f64 {
        effective_to_display(&self.project.scene.slices, t)
    }

    fn restart_audio(&mut self) {
        self.ensure_track();
        self.audio_clock = false;
        if self.audio.is_some() {
            if let Some(a) = self.audio.as_mut() {
                a.stop();
            }
            if self.playing {
                // the soundtrack is that of the project without its jump cuts
                let eff = self.project.effective();
                let has_cuts = eff.scene.slices.len() != self.project.scene.slices.len();
                if has_cuts && self.eff_track.is_none() {
                    self.eff_track = Some(compute_track(&eff, &self.input, TRACK_FPS));
                }
                let track = if has_cuts { self.eff_track.as_ref().unwrap_or(&self.track) } else { &self.track };
                let start = self.to_play(self.playhead_ms);
                if let Ok(Some(st)) = build_soundtrack(&self.package, &eff, track) {
                    if let Some(a) = self.audio.as_mut() {
                        a.play(st, start);
                        self.audio_clock = self.rate == 1.0;
                    }
                }
            }
        }
    }

    pub fn play(&mut self) {
        if self.play_duration_ms() <= 0.0 {
            return;
        }
        if self.playhead_ms >= self.duration_ms() - 1.0 {
            self.playhead_ms = 0.0;
        }
        self.playing = true;
        self.play_anchor = Some((Instant::now(), self.to_play(self.playhead_ms)));
        if self.audio.is_none() {
            self.audio = AudioPlayback::new();
        }
        self.restart_audio();
    }

    pub fn pause(&mut self) {
        self.playing = false;
        self.rate = 1.0;
        self.audio_clock = false;
        self.play_anchor = None;
        if let Some(a) = self.audio.as_mut() {
            a.stop();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    pub fn seek(&mut self, t_ms: f64) {
        self.playhead_ms = t_ms.clamp(0.0, self.duration_ms());
        if self.playing {
            self.play_anchor = Some((Instant::now(), self.to_play(self.playhead_ms)));
            self.restart_audio();
        }
    }

    /// Source time currently shown at the playhead.
    pub fn playhead_source_ms(&self) -> f64 {
        output_to_source(&self.project.scene.slices, self.playhead_ms).map(|x| x.0).unwrap_or(0.0)
    }

    fn frame_index(&mut self) -> usize {
        self.ensure_track();
        let n = self.track.count.max(1);
        ((self.playhead_ms * TRACK_FPS / 1000.0).round() as usize).min(n - 1)
    }

    /// Per-frame housekeeping: clock, decode requests, autosave, async results.
    pub fn tick(&mut self, ctx: &egui::Context) {
        if self.playing {
            if let Some((t0, p0)) = self.play_anchor {
                // with sound, the device's own clock drives the picture; it waits for the first samples
                let t = match (&self.audio, self.audio_clock) {
                    (Some(a), true) => p0 + a.position_ms(),
                    _ => p0 + t0.elapsed().as_secs_f64() * 1000.0 * self.rate,
                };
                // `t` counts play time (jump cuts skipped); the playhead sits on the timeline
                if t >= self.play_duration_ms() {
                    self.playhead_ms = self.duration_ms();
                    self.pause();
                } else {
                    self.playhead_ms = self.to_timeline(t);
                }
            }
            ctx.request_repaint();
        }
        if let Some(rx) = &self.wave_rx {
            if let Ok(p) = rx.try_recv() {
                self.waveform = Some(Arc::new(p));
                self.wave_rx = None;
            }
        }
        while let Ok((f, p)) = self.peaks_rx.try_recv() {
            self.music_peaks.insert(f, Arc::new(p));
            ctx.request_repaint();
        }
        self.export.poll(ctx);
        self.panel.poll(ctx);
        self.take_caption_results();
    }

    /// Waveform of a music file, decoded in the background the first time it is asked for.
    pub fn peaks_for(&mut self, file: &str, ctx: &egui::Context) -> Option<Arc<Vec<f32>>> {
        if let Some(p) = self.music_peaks.get(file) {
            return Some(p.clone());
        }
        if self.peaks_pending.insert(file.to_string()) {
            let (tx, path, name, ctx) = (self.peaks_tx.clone(), self.package.dir.join(file), file.to_string(), ctx.clone());
            std::thread::spawn(move || {
                if let Some(pcm) = screencraft_engine::decode_music(&path) {
                    const BINS: usize = 3000;
                    let frames = pcm.len() / 2;
                    let per = (frames / BINS).max(1);
                    let mut out = vec![0f32; frames.div_ceil(per)];
                    for (i, o) in out.iter_mut().enumerate() {
                        let (a, b) = (i * per, ((i + 1) * per).min(frames));
                        *o = (a..b).map(|f| pcm[f * 2].abs().max(pcm[f * 2 + 1].abs())).fold(0.0, f32::max);
                    }
                    let peak = out.iter().cloned().fold(0.0, f32::max);
                    if peak > 0.0 {
                        out.iter_mut().for_each(|v| *v /= peak);
                    }
                    let _ = tx.send((name, out));
                    ctx.request_repaint();
                }
            });
        }
        None
    }

    // ---- selection helpers ------------------------------------------------------------

    pub fn is_selected(&self, sel: &Sel) -> bool {
        &self.sel == sel
            || match sel {
                Sel::Zoom(id) | Sel::Caption(id) => self.multi.contains(id) && std::mem::discriminant(&self.sel) == std::mem::discriminant(sel),
                _ => false,
            }
    }

    pub fn select(&mut self, sel: Sel) {
        self.sel = sel;
        self.multi.clear();
    }

    /// Ids of every selected zoom region (the first one drives the panel).
    pub fn selected_zoom_ids(&self) -> Vec<String> {
        match &self.sel {
            Sel::Zoom(id) => {
                if self.multi.len() > 1 {
                    self.multi.clone()
                } else {
                    vec![id.clone()]
                }
            }
            _ => vec![],
        }
    }

    pub fn selected_caption_ids(&self) -> Vec<String> {
        match &self.sel {
            Sel::Caption(id) => {
                if self.multi.len() > 1 {
                    self.multi.clone()
                } else {
                    vec![id.clone()]
                }
            }
            _ => vec![],
        }
    }

    // ---- operations -------------------------------------------------------------------

    pub fn auto_zoom(&mut self) {
        let opts = screencraft_core::zoom::AutoZoomOptions::for_ease(
            self.project.config.animations.default_zoom,
            self.project.config.animations.ease_in_ms,
        );
        let mut regions = screencraft_core::zoom::generate(&self.input, self.project.media.duration_ms, &opts);
        for r in regions.iter_mut() {
            r.id = self.new_id("zoom");
        }
        self.edit(|pr| pr.scene.zooms = regions);
        self.select(Sel::None);
    }

    pub fn add_zoom_at_playhead(&mut self) {
        let (src, idx) = ops::out_to_src(&self.project.scene.slices, self.playhead_ms);
        let end = (src + 2500.0).min(self.project.scene.slices[idx].source_end_ms);
        if end - src < 400.0 {
            return;
        }
        let id = self.new_id("zoom");
        let zoom = self.project.config.animations.default_zoom;
        self.edit(|p| {
            p.scene.zooms.push(screencraft_core::project::ZoomRegion { id: id.clone(), zoom, source_start_ms: src, source_end_ms: end, ..Default::default() })
        });
        self.select(Sel::Zoom(id));
        self.tab = Tab::Zoom;
    }

    pub fn add_layout(&mut self, layout: &str) {
        let (start, end) = ops::region_at(&self.project, self.playhead_ms, 4000.0);
        let id = self.new_id("layout");
        let l = layout.to_string();
        self.edit(|p| {
            p.scene.camera_layouts.push(screencraft_core::project::CameraLayoutRegion {
                id: id.clone(),
                source_start_ms: start,
                source_end_ms: end,
                layout: l,
                settings: None,
            })
        });
        self.select(Sel::Layout(id));
        self.tab = Tab::Camera;
    }

    pub fn add_annotation(&mut self, kind: &str) {
        let len = if kind == "blur" || kind == "pixelate" { 6000.0 } else { 3500.0 };
        let (start, end) = ops::region_at(&self.project, self.playhead_ms, len);
        let id = self.new_id("annot");
        let a = ops::new_annotation(kind, id.clone(), start, end);
        self.edit(|p| p.scene.annotations.push(a));
        self.select(Sel::Annot(id));
        self.tab = Tab::Annotate;
    }

    pub fn add_caption(&mut self) {
        let (start, end) = ops::region_at(&self.project, self.playhead_ms, 2500.0);
        let id = self.new_id("cap");
        self.edit(|p| {
            p.scene.subtitles.push(screencraft_core::project::Subtitle {
                id: id.clone(),
                source_start_ms: start,
                source_end_ms: end,
                text: "Your caption".into(),
                word_starts: vec![],
            })
        });
        self.select(Sel::Caption(id));
        self.tab = Tab::Captions;
    }

    /// Split at the playhead. Nothing selected: everything (clips, zooms, music) is cut.
    pub fn split_at_playhead(&mut self) {
        let t_out = self.playhead_ms;
        let (src, idx) = ops::out_to_src(&self.project.scene.slices, t_out);
        let sel = self.sel.clone();
        // layout / annotation / caption blocks
        match &sel {
            Sel::Layout(id) | Sel::Annot(id) | Sel::Caption(id) => {
                let new = self.new_id("split");
                let id = id.clone();
                let kind = sel.clone();
                let mut right_id = None;
                self.edit(|p| match kind {
                    Sel::Layout(_) => {
                        if let Some(i) = p.scene.camera_layouts.iter().position(|x| x.id == id) {
                            let it = p.scene.camera_layouts[i].clone();
                            if src > it.source_start_ms + 50.0 && src < it.source_end_ms - 50.0 {
                                let mut r = it;
                                r.id = new.clone();
                                r.source_start_ms = src;
                                p.scene.camera_layouts[i].source_end_ms = src;
                                p.scene.camera_layouts.insert(i + 1, r);
                                right_id = Some(new.clone());
                            }
                        }
                    }
                    Sel::Annot(_) => {
                        if let Some(i) = p.scene.annotations.iter().position(|x| x.id == id) {
                            let it = p.scene.annotations[i].clone();
                            if src > it.source_start_ms + 50.0 && src < it.source_end_ms - 50.0 {
                                let mut r = it;
                                r.id = new.clone();
                                r.source_start_ms = src;
                                p.scene.annotations[i].source_end_ms = src;
                                p.scene.annotations.insert(i + 1, r);
                                right_id = Some(new.clone());
                            }
                        }
                    }
                    Sel::Caption(_) => {
                        if let Some(i) = p.scene.subtitles.iter().position(|x| x.id == id) {
                            let it = p.scene.subtitles[i].clone();
                            if src > it.source_start_ms + 50.0 && src < it.source_end_ms - 50.0 {
                                let mut r = it;
                                r.id = new.clone();
                                r.source_start_ms = src;
                                p.scene.subtitles[i].source_end_ms = src;
                                p.scene.subtitles.insert(i + 1, r);
                                right_id = Some(new.clone());
                            }
                        }
                    }
                    _ => {}
                });
                if let Some(r) = right_id {
                    self.select(match sel {
                        Sel::Layout(_) => Sel::Layout(r),
                        Sel::Annot(_) => Sel::Annot(r),
                        _ => Sel::Caption(r),
                    });
                }
                return;
            }
            _ => {}
        }
        let do_clips = matches!(sel, Sel::None | Sel::Slice(_));
        let do_zooms = matches!(sel, Sel::None | Sel::Zoom(_));
        let do_music = matches!(sel, Sel::None | Sel::Music(_));
        let mut clip_idx: Option<usize> = None;
        if do_clips {
            let cand = if let Sel::Slice(id) = &sel {
                self.project.scene.slices.iter().position(|x| &x.id == id).filter(|&i| i == idx)
            } else {
                Some(idx)
            };
            if let Some(i) = cand {
                let s = &self.project.scene.slices[i];
                if src > s.source_start_ms + 50.0 && src < s.source_end_ms - 50.0 {
                    clip_idx = Some(i);
                }
            }
        }
        let zoom_ids = self.selected_zoom_ids();
        let zooms: Vec<String> = if do_zooms {
            self.project
                .scene
                .zooms
                .iter()
                .filter(|z| (matches!(sel, Sel::None) || zoom_ids.contains(&z.id)) && src > z.source_start_ms + 50.0 && src < z.source_end_ms - 50.0)
                .map(|z| z.id.clone())
                .collect()
        } else {
            vec![]
        };
        let music: Vec<String> = if do_music {
            self.project
                .scene
                .music_clips
                .iter()
                .filter(|c| (matches!(sel, Sel::None) || Some(c.id.as_str()) == sel.id()) && t_out > c.offset_ms + 50.0 && t_out < c.offset_ms + c.dur_ms - 50.0)
                .map(|c| c.id.clone())
                .collect()
        } else {
            vec![]
        };
        if clip_idx.is_none() && zooms.is_empty() && music.is_empty() {
            return;
        }
        let mut new_ids: Vec<String> = (0..(1 + zooms.len() + music.len())).map(|_| self.new_id("split")).collect();
        let mut next_sel: Option<Sel> = None;
        self.edit(|p| {
            if let Some(ci) = clip_idx {
                let right_id = new_ids.pop().unwrap();
                let mut right = p.scene.slices[ci].clone();
                right.id = right_id.clone();
                right.source_start_ms = src;
                p.scene.slices[ci].source_end_ms = src;
                p.scene.slices.insert(ci + 1, right);
                if matches!(sel, Sel::Slice(_)) {
                    next_sel = Some(Sel::Slice(right_id));
                }
            }
            for id in &music {
                let nid = new_ids.pop().unwrap();
                if let Some(i) = p.scene.music_clips.iter().position(|c| &c.id == id) {
                    let c = p.scene.music_clips[i].clone();
                    let cut = t_out - c.offset_ms;
                    let mut right = c.clone();
                    right.id = nid.clone();
                    right.offset_ms = t_out;
                    right.in_ms = c.in_ms + cut;
                    right.dur_ms = c.dur_ms - cut;
                    p.scene.music_clips[i].dur_ms = cut;
                    p.scene.music_clips.insert(i + 1, right);
                    if matches!(sel, Sel::Music(_)) {
                        next_sel = Some(Sel::Music(nid));
                    }
                }
            }
            let mut rights = Vec::new();
            for id in &zooms {
                let nid = new_ids.pop().unwrap();
                if let Some(i) = p.scene.zooms.iter().position(|z| &z.id == id) {
                    let mut right = p.scene.zooms[i].clone();
                    right.id = nid.clone();
                    right.source_start_ms = src;
                    // tiny eases at the cut so the camera doesn't dip between the two halves
                    p.scene.zooms[i].source_end_ms = src;
                    p.scene.zooms[i].ease_out_ms = Some(1.0);
                    right.ease_in_ms = Some(1.0);
                    p.scene.zooms.insert(i + 1, right);
                    rights.push(nid);
                }
            }
            if matches!(sel, Sel::Zoom(_)) && !rights.is_empty() {
                next_sel = Some(Sel::Zoom(rights[0].clone()));
            }
        });
        if let Some(s) = next_sel {
            self.select(s);
        }
    }

    pub fn duplicate_selected(&mut self) {
        match self.sel.clone() {
            Sel::Annot(id) => {
                let nid = self.new_id("annot");
                let n2 = nid.clone();
                self.edit(|p| {
                    if let Some(a) = p.scene.annotations.iter().find(|x| x.id == id).cloned() {
                        let mut c = a.clone();
                        c.id = n2;
                        c.x = (a.x + 0.04).min(0.9);
                        c.y = (a.y + 0.05).min(0.9);
                        p.scene.annotations.push(c);
                    }
                });
                self.select(Sel::Annot(nid));
            }
            Sel::Slice(id) => {
                let nid = self.new_id("slice");
                let n2 = nid.clone();
                self.edit(|p| {
                    if let Some(i) = p.scene.slices.iter().position(|x| x.id == id) {
                        let mut c = p.scene.slices[i].clone();
                        c.id = n2;
                        p.scene.slices.insert(i + 1, c);
                    }
                });
                self.select(Sel::Slice(nid));
            }
            _ => {}
        }
    }

    pub fn delete_selected(&mut self) {
        let sel = self.sel.clone();
        match sel {
            Sel::None => {}
            Sel::Music(id) => self.edit(|p| p.scene.music_clips.retain(|c| c.id != id)),
            Sel::Caption(_) => {
                let ids = self.selected_caption_ids();
                self.edit(|p| p.scene.subtitles.retain(|x| !ids.contains(&x.id)));
            }
            Sel::Layout(id) => self.edit(|p| p.scene.camera_layouts.retain(|x| x.id != id)),
            Sel::Annot(id) => self.edit(|p| p.scene.annotations.retain(|x| x.id != id)),
            Sel::Zoom(_) => {
                let ids = self.selected_zoom_ids();
                self.edit(|p| p.scene.zooms.retain(|x| !ids.contains(&x.id)));
            }
            Sel::Slice(id) => {
                if self.project.scene.slices.len() > 1 {
                    self.edit(|p| p.scene.slices.retain(|s| s.id != id));
                    let d = self.duration_ms();
                    self.seek(self.playhead_ms.min(d));
                }
            }
        }
        self.select(Sel::None);
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let k = |key: egui::Key| ctx.input(|i| i.key_pressed(key) && !i.modifiers.command);
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |key: egui::Key| ctx.input(|i| i.key_pressed(key));
        // Space: tap toggles, hold plays at 2x
        let (space_down, space_up) = ctx.input(|i| (i.key_pressed(egui::Key::Space), i.key_released(egui::Key::Space)));
        if space_down {
            self.panel.space_since = Some(Instant::now());
            self.panel.space_was_playing = self.playing;
            self.panel.space_held = false;
        }
        if let Some(t) = self.panel.space_since {
            if !self.panel.space_held && t.elapsed() > Duration::from_millis(250) {
                self.panel.space_held = true;
                if !self.playing {
                    self.play();
                }
                if self.playing {
                    self.rate = 2.0;
                    self.audio_clock = false;
                    self.play_anchor = Some((Instant::now(), self.to_play(self.playhead_ms)));
                }
            }
            if space_up {
                if self.panel.space_held {
                    self.rate = 1.0;
                    self.play_anchor = Some((Instant::now(), self.to_play(self.playhead_ms)));
                    if !self.panel.space_was_playing {
                        self.pause();
                    }
                } else if self.playing {
                    self.pause();
                } else {
                    self.play();
                }
                self.panel.space_since = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(60));
            }
        }
        if k(egui::Key::S) && !cmd {
            self.split_at_playhead();
        }
        if k(egui::Key::X) && !cmd {
            self.toggle_jump_cut();
        }
        if k(egui::Key::Z) {
            self.add_zoom_at_playhead();
        }
        if pressed(egui::Key::D) && cmd {
            self.duplicate_selected();
        }
        if k(egui::Key::Q) {
            self.pause();
            self.seek(0.0);
        }
        if k(egui::Key::E) {
            self.pause();
            let d = self.duration_ms();
            self.seek(d);
        }
        if k(egui::Key::A) || k(egui::Key::D) {
            let prev = pressed(egui::Key::A);
            let mut starts: Vec<f64> =
                self.project.scene.zooms.iter().filter_map(|z| ops::src_to_out(&self.project.scene.slices, z.source_start_ms)).collect();
            starts.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let eps = 30.0;
            let target = if prev { starts.iter().rev().find(|t| **t < self.playhead_ms - eps) } else { starts.iter().find(|t| **t > self.playhead_ms + eps) };
            if let Some(t) = target.copied() {
                self.pause();
                self.seek(t);
            }
        }
        if pressed(egui::Key::Delete) || pressed(egui::Key::Backspace) {
            self.delete_selected();
        }
        let step = if shift { 1000.0 } else { 1000.0 / 30.0 };
        if pressed(egui::Key::ArrowLeft) {
            self.pause();
            self.seek(self.playhead_ms - step);
        }
        if pressed(egui::Key::ArrowRight) {
            self.pause();
            self.seek(self.playhead_ms + step);
        }
        if cmd && pressed(egui::Key::A) {
            self.select_all();
        }
        if cmd && pressed(egui::Key::Z) {
            if shift {
                self.redo();
            } else {
                self.undo();
            }
        }
        if cmd && pressed(egui::Key::E) {
            self.export.open = true;
        }
        if cmd && pressed(egui::Key::S) {
            self.save_now();
        }
    }

    fn select_all(&mut self) {
        if self.tab == Tab::Captions {
            let ids: Vec<String> = self.project.scene.subtitles.iter().map(|c| c.id.clone()).collect();
            if let Some(f) = ids.first().cloned() {
                self.sel = Sel::Caption(f);
                self.multi = if ids.len() > 1 { ids } else { vec![] };
            }
            return;
        }
        let ids: Vec<String> = self.project.scene.zooms.iter().map(|z| z.id.clone()).collect();
        if let Some(f) = ids.first().cloned() {
            self.sel = Sel::Zoom(f);
            self.multi = if ids.len() > 1 { ids } else { vec![] };
            self.tab = Tab::Zoom;
        }
    }

    /// Draw the preview into `rect`.
    fn show_preview(&mut self, ui: &mut egui::Ui, rect: Rect) {
        self.ensure_track();
        if self.track.count == 0 {
            return;
        }
        let i = self.frame_index();
        let t_src = self.track.t_src[i];
        let want_screen = self.table.index_at(t_src);
        let off = self.project.media.camera_offset_ms;
        if self.cam_table.is_none() && self.worker.has_camera {
            if let Some(c) = &self.project.media.camera {
                self.cam_table = FrameTable::open(&self.package.dir.join(c)).ok();
            }
        }
        let want_cam = self.cam_table.as_ref().map(|t| t.index_at(t_src + off));
        let other = self.track.other_alpha.get(i).filter(|a| **a > 0.003).map(|_| self.track.other_t_src[i]);
        let want_other = other.map(|t| self.table.index_at(t));
        let key = (want_screen, want_cam, want_other);
        self.worker.request(Want { t_src, other_t: other, key });
        if let Some(r) = self.worker.poll() {
            match r {
                Ok(r) => {
                    // Upload whatever arrived, even if the playhead moved on: it is the freshest.
                    self.compositor.upload(&r.frames.screen);
                    for (slot, f) in [(Slot::Screen2, &r.frames.screen2), (Slot::Camera, &r.frames.camera), (Slot::Camera2, &r.frames.camera2)] {
                        match f {
                            Some(f) => self.compositor.upload_slot(slot, f),
                            None => self.compositor.clear_slot(slot),
                        }
                    }
                    self.uploaded_key = Some(r.key);
                }
                Err(e) => self.error = Some(e),
            }
        }
        if self.uploaded_key != Some(key) {
            ui.ctx().request_repaint_after(Duration::from_millis(8));
        }

        let (w, h) = output_size(&self.project, self.preview_res);
        let aspect = w as f32 / h as f32;
        let mut size = Vec2::new(rect.width(), rect.width() / aspect);
        if size.y > rect.height() {
            size = Vec2::new(rect.height() * aspect, rect.height());
        }
        let shown = Rect::from_center_size(rect.center(), size);
        let scene = build_scene(&self.project, &self.track, i, (w, h));
        self.preview_content = Some(Rect::from_min_size(
            shown.min + Vec2::new(scene.screen[0] / w as f32 * shown.width(), scene.screen[1] / h as f32 * shown.height()),
            Vec2::new(scene.screen[2] / w as f32 * shown.width(), scene.screen[3] / h as f32 * shown.height()),
        ));
        self.preview_geo = Some(PreviewGeo {
            out: (w, h),
            sr: scene.screen,
            view_w: self.project.config.crop.width as f32 / self.track.zoom[i] as f32,
            view_h: self.project.config.crop.height as f32 / self.track.zoom[i] as f32,
            bounds: scene.layers.bounds.clone(),
        });
        self.compositor.render(&scene);
        let view = self.compositor.target_view().unwrap();
        let mut renderer = self.render_state.renderer.write();
        let id = match self.tex {
            Some((id, sz)) if sz == (w, h) => id,
            Some((id, _)) => {
                renderer.update_egui_texture_from_wgpu_texture(&self.render_state.device, view, eframe::wgpu::FilterMode::Linear, id);
                self.tex = Some((id, (w, h)));
                id
            }
            None => {
                let id = renderer.register_native_texture(&self.render_state.device, view, eframe::wgpu::FilterMode::Linear);
                self.tex = Some((id, (w, h)));
                id
            }
        };
        drop(renderer);
        ui.painter().image(id, shown, Rect::from_min_max(Pos2::new(0.0, 0.0), Pos2::new(1.0, 1.0)), Color32::WHITE);
        self.preview_shown = Some(shown);
        self.draw_preview_overlays(ui, i);
    }

    /// The editor's whole UI. Returns `true` when the user asked to go back to the library.
    pub fn ui(&mut self, ui: &mut egui::Ui) -> bool {
        let ctx = ui.ctx().clone();
        self.tick(&ctx);
        self.handle_keys(&ctx);
        self.wants_back = false;

        egui::Panel::top("editor-top")
            .exact_size(48.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(8, 0)))
            .show_separator_line(false)
            .show(ui, |ui| self.top_bar(ui));
        {
            let r = ui.min_rect();
            ui.painter().line_segment([Pos2::new(r.min.x, 48.0), Pos2::new(r.max.x, 48.0)], Stroke::new(1.0, LINE));
        }

        egui::Panel::bottom("editor-timeline")
            .resizable(true)
            .default_size(self.tl_h)
            .size_range(170.0..=640.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin { left: 16, right: 16, top: 0, bottom: 10 }))
            .show_separator_line(true)
            .show(ui, |ui| {
                self.tl_h = ui.max_rect().height();
                timeline::show(self, ui);
            });

        egui::Panel::left("editor-side")
            .resizable(true)
            .default_size(self.side_w)
            .size_range(280.0..=560.0)
            .frame(egui::Frame::new().fill(PANEL))
            .show_separator_line(true)
            .show(ui, |ui| {
                self.side_w = ui.max_rect().width();
                self.side(ui);
            });

        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(BG)).show(ui, |ui| self.stage(ui));

        if let Some(e) = self.error.clone() {
            let mut open = true;
            egui::Window::new("Something went wrong").open(&mut open).collapsible(false).show(&ctx, |ui| {
                ui.label(e);
            });
            if !open {
                self.error = None;
            }
        }
        export_ui::show(self, &ctx);
        dialogs::show(self, &ctx);
        dialogs::save_prompt(self, &ctx);
        self.wants_back
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            for (name, tip, url) in [
                ("x", "ScreenCraft on X (@mdnlabs)", "https://x.com/mdnlabs"),
                ("github", "ScreenCraft on GitHub", "https://github.com/MarshallNickolauson/ScreenCraft"),
                ("bug", "Report a bug or request a feature", "https://github.com/MarshallNickolauson/ScreenCraft/issues"),
            ] {
                if kit::icon_btn_sized(ui, name, tip, 32.0, 17.0, true).clicked() {
                    let _ = open_url(url);
                }
            }
            ui.add_space(2.0);
            // Projects button
            let tw = kit::text_width(ui, "Projects", 13.0);
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(tw + 10.0 * 2.0 + 16.0 + 8.0, 32.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(rect, CornerRadius::same(7), RAISE);
            }
            let fg = if resp.hovered() { Color32::WHITE } else { Color32::from_rgb(0xcd, 0xd5, 0xe5) };
            icons::paint(ui, "folder", Rect::from_center_size(Pos2::new(rect.min.x + 10.0 + 8.0, rect.center().y), Vec2::splat(16.0)), fg);
            ui.painter().text(Pos2::new(rect.min.x + 10.0 + 16.0 + 8.0, rect.center().y), Align2::LEFT_CENTER, "Projects", kit::font(13.0), fg);
            if resp.clone().on_hover_text("Open another project or show this one in Finder").clicked() {
                self.dialogs.projects = !self.dialogs.projects;
                self.dialogs.projects_anchor = Some(rect);
            }
            // Title
            ui.add_space(6.0);
            self.title(ui);
            // right side
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(6.0);
                if kit::primary(ui, Some("export"), "Export").clicked() {
                    self.export.open = true;
                }
                let r = kit::pill(ui, Some("sparkle"), "Presets", self.dialogs.presets_open, false, true, None);
                if r.clicked() {
                    self.dialogs.presets_open = !self.dialogs.presets_open;
                    self.dialogs.presets_anchor = Some(r.rect);
                } else if self.dialogs.presets_open && self.dialogs.presets_anchor.is_none() {
                    self.dialogs.presets_anchor = Some(r.rect);
                }
                let r = kit::pill(ui, Some("file"), "Raw", false, false, false, None);
                if r.clone().on_hover_text("Raw recording files").clicked() {
                    self.dialogs.raw = !self.dialogs.raw;
                    self.dialogs.raw_anchor = Some(r.rect);
                }
                let upd = self.dialogs.update_available();
                let label = if upd { "Update available" } else { "Updates" };
                let col = if upd { Some(Color32::from_rgb(0x5f, 0xd3, 0x8d)) } else { None };
                if kit::pill(ui, Some("info"), label, false, false, false, col).on_hover_text("About, privacy & updates").clicked() {
                    self.dialogs.about = true;
                }
                let can_redo = !self.redo.is_empty();
                let can_undo = !self.undo.is_empty();
                if kit::icon_btn_sized(ui, "redo", "Redo (⇧⌘Z)", 32.0, 18.0, can_redo).clicked() && can_redo {
                    self.redo();
                }
                if kit::icon_btn_sized(ui, "undo", "Undo (⌘Z)", 32.0, 18.0, can_undo).clicked() && can_undo {
                    self.undo();
                }
            });
        });
    }

    fn title(&mut self, ui: &mut egui::Ui) {
        let name = self.project.name.clone();
        if let Some(buf) = self.title_edit.as_mut() {
            let r = ui.add(
                egui::TextEdit::singleline(buf)
                    .desired_width(220.0)
                    .font(kit::font(13.5))
                    .margin(egui::Margin::symmetric(9, 4)),
            );
            r.request_focus();
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
            if esc {
                self.title_edit = None;
            } else if enter || r.lost_focus() {
                let new = buf.trim().to_string();
                self.title_edit = None;
                if !new.is_empty() && new != name {
                    self.rename(&new);
                }
            }
            return;
        }
        let max_w = (ui.available_width() * 0.38).min(420.0);
        let galley = ui.painter().layout_no_wrap(name.clone(), kit::font(13.5), TEXT);
        let w = (galley.size().x + 18.0).min(max_w);
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 28.0), Sense::click());
        ui.painter().line_segment([Pos2::new(rect.min.x, rect.min.y + 1.0), Pos2::new(rect.min.x, rect.max.y - 1.0)], Stroke::new(1.0, LINE_2));
        if resp.hovered() {
            ui.painter().rect_filled(rect.shrink2(Vec2::new(0.0, 0.0)), CornerRadius { nw: 0, sw: 0, ne: 6, se: 6 }, Color32::from_white_alpha(15));
        }
        ui.painter().with_clip_rect(rect).text(Pos2::new(rect.min.x + 9.0, rect.center().y), Align2::LEFT_CENTER, &name, kit::font(13.5), TEXT);
        if resp.on_hover_cursor(egui::CursorIcon::Text).clicked() {
            self.title_edit = Some(name);
        }
    }

    fn rename(&mut self, name: &str) {
        self.save_now();
        let safe: String = name.chars().map(|c| if "/\\:".contains(c) { '-' } else { c }).collect();
        let parent = self.package.dir.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let mut dest = parent.join(format!("{safe}.screencraft"));
        let mut n = 2;
        while dest.exists() && dest != self.package.dir {
            dest = parent.join(format!("{safe} {n}.screencraft"));
            n += 1;
        }
        if dest != self.package.dir {
            if let Err(e) = std::fs::rename(&self.package.dir, &dest) {
                self.error = Some(format!("Could not rename: {e}"));
                return;
            }
            self.package = Package::open(dest);
        }
        self.project.name = name.to_string();
        self.save_now();
    }

    fn side(&mut self, ui: &mut egui::Ui) {
        // icon rail
        let rail_h = 8.0 + 38.0;
        let (rail, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), rail_h + 1.0), Sense::hover());
        ui.painter().line_segment([Pos2::new(rail.min.x, rail.max.y - 0.5), Pos2::new(rail.max.x, rail.max.y - 0.5)], Stroke::new(1.0, LINE));
        let inner = Rect::from_min_max(Pos2::new(rail.min.x + 10.0, rail.min.y + 8.0), Pos2::new(rail.max.x - 10.0, rail.max.y - 1.0));
        let n = TABS.len() as f32;
        let gap = 2.0;
        let bw = (inner.width() - gap * (n - 1.0)) / n;
        for (k, (tab, icon, title)) in TABS.iter().enumerate() {
            let r = Rect::from_min_size(Pos2::new(inner.min.x + k as f32 * (bw + gap), inner.min.y), Vec2::new(bw, 38.0));
            let resp = ui.interact(r, ui.id().with(("rail", k)), Sense::click());
            let on = self.tab == *tab;
            let cr = CornerRadius { nw: 7, ne: 7, sw: 0, se: 0 };
            if on {
                ui.painter().rect_filled(r, cr, BG);
                let ul = Rect::from_min_max(Pos2::new(r.min.x + r.width() * 0.2, r.max.y - 2.0), Pos2::new(r.max.x - r.width() * 0.2, r.max.y));
                ui.painter().rect_filled(ul, CornerRadius::same(2), ACCENT_2);
            } else if resp.hovered() {
                ui.painter().rect_filled(r, cr, PANEL_2);
            }
            let col = if on || resp.hovered() { Color32::WHITE } else { Color32::from_rgb(0x7f, 0x8b, 0xa1) };
            icons::paint(ui, icon, Rect::from_center_size(r.center(), Vec2::splat(19.0)), col);
            if resp.on_hover_text(*title).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.tab = *tab;
            }
        }
        // panel
        let rect = ui.available_rect_before_wrap();
        ui.painter().rect_filled(rect, 0.0, BG);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
            egui::Frame::new().inner_margin(egui::Margin { left: 18, right: 18, top: 16, bottom: 16 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 2.0);
                panels::show(self, ui);
            });
        });
    }

    fn stage(&mut self, ui: &mut egui::Ui) {
        let full = ui.max_rect();
        // radial gradient: ellipse at 50% 30%, #0e1624 -> bg at 70%
        paint_stage_bg(ui, full);
        let inner = Rect::from_min_max(Pos2::new(full.min.x + 18.0, full.min.y + 10.0), Pos2::new(full.max.x - 18.0, full.max.y));
        // stage bar
        let bar = Rect::from_min_size(inner.min, Vec2::new(inner.width(), 40.0));
        let mut bar_ui = ui.new_child(egui::UiBuilder::new().max_rect(bar).layout(egui::Layout::left_to_right(egui::Align::Center)));
        bar_ui.spacing_mut().item_spacing.x = 10.0;
        bar_ui.label(egui::RichText::new("Aspect").color(MUTED).size(13.0));
        let aspects: [(&str, &str); 8] = [
            ("auto", "Auto"),
            ("16:9", "16:9 Wide"),
            ("9:16", "9:16 Vertical"),
            ("1:1", "1:1 Square"),
            ("4:3", "4:3"),
            ("3:4", "3:4"),
            ("4:5", "4:5"),
            ("21:9", "21:9 Cinema"),
        ];
        let cur = aspects.iter().find(|a| a.0 == self.project.config.output.aspect_ratio).map(|a| a.1).unwrap_or("Auto");
        let labels: Vec<&str> = aspects.iter().map(|a| a.1).collect();
        if let Some(i) = kit::select(&mut bar_ui, "aspect", cur, &labels, 110.0) {
            let v = aspects[i].0.to_string();
            self.edit(|p| p.config.output.aspect_ratio = v);
        }
        if kit::pill(&mut bar_ui, None, "Crop", self.tab == Tab::Crop, false, false, None).clicked() {
            self.tab = Tab::Crop;
        }
        bar_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let opts = [(540u32, "Fast"), (720, "Balanced"), (1080, "Full")];
            let cur = opts.iter().find(|o| o.0 == self.preview_res).map(|o| o.1).unwrap_or("Balanced");
            if let Some(i) = kit::select(ui, "preview", cur, &["Fast", "Balanced", "Full"], 90.0) {
                self.preview_res = opts[i].0;
            }
            ui.label(egui::RichText::new("Preview").color(MUTED).size(13.0));
        });
        // transport (46px + margins) at the bottom
        let tr_h = 46.0;
        let canvas = Rect::from_min_max(Pos2::new(inner.min.x, inner.min.y + 40.0), Pos2::new(inner.max.x, inner.max.y - tr_h - 18.0));
        let ui_clip = ui.clip_rect();
        let _ = ui_clip;
        self.show_preview(ui, canvas);
        self.transport(ui, Rect::from_min_max(Pos2::new(inner.min.x, inner.max.y - tr_h - 10.0), Pos2::new(inner.max.x, inner.max.y - 10.0)));
    }

    fn transport(&mut self, ui: &mut egui::Ui, area: Rect) {
        let time_w = 62.0;
        let zoom_label_w = kit::text_width(ui, "Add Zoom", 13.0) + 11.0 * 2.0 + 18.0 + 6.0;
        let content_w = 12.0 * 2.0 + time_w + 8.0 + 32.0 + 8.0 + 36.0 + 8.0 + 32.0 + 8.0 + time_w + 8.0 + 6.0 + zoom_label_w;
        let pill = Rect::from_center_size(area.center(), Vec2::new(content_w, 46.0));
        ui.painter().rect_filled(pill, CornerRadius::same(12), PANEL);
        ui.painter().rect_stroke(pill, CornerRadius::same(12), Stroke::new(1.0, LINE), StrokeKind::Inside);
        let mut x = pill.min.x + 12.0;
        let cy = pill.center().y;
        let t_cur = fmt_time(self.playhead_ms);
        ui.painter().text(Pos2::new(x + time_w / 2.0, cy), Align2::CENTER_CENTER, t_cur, kit::font(12.0), MUTED);
        x += time_w + 8.0;
        let mut btn = |ui: &mut egui::Ui, x: &mut f32, w: f32, name: &str, tip: &str| -> bool {
            let r = Rect::from_min_size(Pos2::new(*x, cy - 16.0), Vec2::new(w, 32.0));
            *x += w + 8.0;
            let resp = ui.interact(r, ui.id().with(("tr", name)), Sense::click());
            if name == "play" || name == "pause" {
                ui.painter().rect_filled(r, CornerRadius::same(8), if resp.hovered() { ACCENT_2 } else { ACCENT });
                icons::paint(ui, name, Rect::from_center_size(r.center(), Vec2::splat(18.0)), Color32::WHITE);
            } else {
                if resp.hovered() {
                    ui.painter().rect_filled(r, CornerRadius::same(7), RAISE);
                }
                icons::paint(ui, name, Rect::from_center_size(r.center(), Vec2::splat(18.0)), if resp.hovered() { Color32::WHITE } else { ICON });
            }
            resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
        };
        if btn(ui, &mut x, 32.0, "start", "Back to start") {
            self.pause();
            self.seek(0.0);
        }
        let icon = if self.playing { "pause" } else { "play" };
        if btn(ui, &mut x, 36.0, icon, "Play / Pause (Space)") {
            if self.playing {
                self.pause();
            } else {
                self.play();
            }
        }
        if btn(ui, &mut x, 32.0, "end", "Go to end") {
            self.pause();
            let d = self.duration_ms();
            self.seek(d);
        }
        ui.painter().text(Pos2::new(x + time_w / 2.0, cy), Align2::CENTER_CENTER, fmt_time(self.duration_ms()), kit::font(12.0), MUTED);
        x += time_w + 8.0 + 6.0;
        let zr = Rect::from_min_size(Pos2::new(x, cy - 15.0), Vec2::new(zoom_label_w, 30.0));
        let resp = ui.interact(zr, ui.id().with("zoomhere"), Sense::click());
        ui.painter().rect_filled(zr, CornerRadius::same(7), alpha(ACCENT, if resp.hovered() { 0.32 } else { 0.16 }));
        ui.painter().rect_stroke(zr, CornerRadius::same(7), Stroke::new(1.0, Color32::from_rgba_unmultiplied(106, 174, 232, 115)), StrokeKind::Inside);
        let fg = Color32::from_rgb(0xd9, 0xea, 0xff);
        icons::paint(ui, "zoom-plus", Rect::from_center_size(Pos2::new(zr.min.x + 11.0 + 9.0, zr.center().y), Vec2::splat(18.0)), fg);
        ui.painter().text(Pos2::new(zr.min.x + 11.0 + 24.0, zr.center().y), Align2::LEFT_CENTER, "Add Zoom", kit::font(13.0), fg);
        if resp.on_hover_text("Add a zoom on the mouse at the playhead (Z)").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.add_zoom_at_playhead();
        }
    }

    /// Hit areas and outlines drawn over the preview: the zoom-target marker and annotation handles.
    fn draw_preview_overlays(&mut self, ui: &mut egui::Ui, i: usize) {
        let Some(content) = self.preview_content else { return };
        // Manual zoom target picker.
        if let Sel::Zoom(id) = self.sel.clone() {
            let zi = self.project.scene.zooms.iter().position(|z| z.id == id);
            let manual = zi.map(|k| self.project.scene.zooms[k].kind == screencraft_core::project::ZoomKind::Manual).unwrap_or(false);
            if let (Some(zi), true) = (zi, manual) {
                let resp = ui.interact(content, ui.id().with("zoom-target"), Sense::click_and_drag());
                if resp.dragged() || resp.clicked() {
                    if let Some(pos) = resp.interact_pointer_pos() {
                        let local = ((pos - content.min) / content.size()).clamp(Vec2::ZERO, Vec2::splat(1.0));
                        let c = self.project.config.crop;
                        let (cx, cy, z) = (self.track.cx[i], self.track.cy[i], self.track.zoom[i]);
                        let sx = cx + (local.x as f64 - 0.5) * c.width / z;
                        let sy = cy + (local.y as f64 - 0.5) * c.height / z;
                        self.edit_keyed(Some("zoom-target"), |pr| {
                            if let Some(zr) = pr.scene.zooms.get_mut(zi) {
                                zr.manual_target_point.x = sx.clamp(0.0, 1.0);
                                zr.manual_target_point.y = sy.clamp(0.0, 1.0);
                            }
                        });
                    }
                }
                if let Some(z) = self.project.scene.zooms.get(zi) {
                    let c = self.project.config.crop;
                    let (cx, cy, zz) = (self.track.cx[i], self.track.cy[i], self.track.zoom[i]);
                    let lx = (z.manual_target_point.x - cx) * zz / c.width + 0.5;
                    let ly = (z.manual_target_point.y - cy) * zz / c.height + 0.5;
                    if (0.0..=1.0).contains(&lx) && (0.0..=1.0).contains(&ly) {
                        let pt = content.min + Vec2::new(lx as f32 * content.width(), ly as f32 * content.height());
                        ui.painter().circle_stroke(pt, 13.0, Stroke::new(2.0, Color32::WHITE));
                        ui.painter().circle_stroke(pt, 14.5, Stroke::new(2.0, ACCENT));
                    }
                }
            }
        }
        self.annotation_handles(ui);
    }

    /// Outline and drag handles of the selected annotation: the body moves it, the corner resizes
    /// it, and an arrow's ends move on their own.
    fn annotation_handles(&mut self, ui: &mut egui::Ui) {
        let (Sel::Annot(id), Some(shown), Some(geo)) = (self.sel.clone(), self.preview_shown, self.preview_geo.clone()) else { return };
        let Some(a) = self.project.scene.annotations.iter().find(|x| x.id == id).cloned() else { return };
        let Some(b) = geo.bounds.iter().find(|x| x.id == id).cloned() else { return };
        let k = shown.width() / geo.out.0 as f32; // canvas px -> screen points
        let to_scr = |x: f32, y: f32| Pos2::new(shown.min.x + x * k, shown.min.y + y * k);
        let u = geo.out.1 as f32 / 1080.0;
        let accent = ACCENT_2;
        // outline
        let (r_outer, is_arrow) = (Rect::from_min_size(to_scr(b.rect[0], b.rect[1]), Vec2::new(b.rect[2] * k, b.rect[3] * k)), b.arrow.is_some());
        if let Some([x1, y1, x2, y2]) = b.arrow {
            for (x, y) in [(x1, y1), (x2, y2)] {
                ui.painter().circle_filled(to_scr(x, y), 7.0 * u * k, accent);
            }
        } else {
            let corners = [r_outer.left_top(), r_outer.right_top(), r_outer.right_bottom(), r_outer.left_bottom(), r_outer.left_top()];
            for w in corners.windows(2) {
                ui.painter().extend(egui::Shape::dashed_line(&[w[0], w[1]], Stroke::new(2.0 * u * k, accent), 6.0 * u * k, 4.0 * u * k));
            }
            if a.kind != "text" {
                ui.painter().rect_filled(Rect::from_center_size(r_outer.right_bottom(), Vec2::splat(10.0 * u * k)), 0.0, accent);
            }
        }
        // interaction
        let resp = ui.interact(shown, ui.id().with("annot-drag"), Sense::click_and_drag());
        let to_canvas = |p: Pos2| ((p.x - shown.min.x) / k, (p.y - shown.min.y) / k);
        if resp.drag_started() {
            if let Some(pos) = resp.interact_pointer_pos() {
                let (px, py) = to_canvas(pos);
                let near = |x: f32, y: f32, r: f32| ((px - x).powi(2) + (py - y).powi(2)).sqrt() <= r * u;
                let mut mode = None;
                if let Some([x1, y1, x2, y2]) = b.arrow {
                    if near(x2, y2, 18.0) {
                        mode = Some(DragMode::ArrowEnd);
                    } else if near(x1, y1, 18.0) {
                        mode = Some(DragMode::ArrowStart);
                    } else {
                        let (dx, dy) = (x2 - x1, y2 - y1);
                        let len2 = (dx * dx + dy * dy).max(1e-6);
                        let t = (((px - x1) * dx + (py - y1) * dy) / len2).clamp(0.0, 1.0);
                        if ((px - (x1 + dx * t)).powi(2) + (py - (y1 + dy * t)).powi(2)).sqrt() <= 14.0 * u {
                            mode = Some(DragMode::Move);
                        }
                    }
                } else {
                    let pad = 10.0 * u;
                    let [bx, by, bw, bh] = b.rect;
                    if px >= bx - pad && px <= bx + bw + pad && py >= by - pad && py <= by + bh + pad {
                        mode = Some(if a.kind != "text" && near(bx + bw, by + bh, 18.0) { DragMode::Resize } else { DragMode::Move });
                    }
                }
                let _ = is_arrow;
                if let Some(mode) = mode {
                    self.annot_drag = Some(AnnotDrag { id: id.clone(), mode, p0: pos, start: (a.x, a.y, a.w, a.h, a.x2, a.y2), content: a.space == "content" });
                }
            }
        }
        if resp.dragged() {
            if let (Some(d), Some(pos)) = (self.annot_drag.clone(), resp.interact_pointer_pos()) {
                let (dxc, dyc) = ((pos.x - d.p0.x) / k, (pos.y - d.p0.y) / k);
                // canvas px -> annotation units
                let (kx, ky) = if d.content { (geo.view_w as f64 / geo.sr[2] as f64, geo.view_h as f64 / geo.sr[3] as f64) } else { (1.0 / geo.out.0 as f64, 1.0 / geo.out.1 as f64) };
                let (dx, dy) = (dxc as f64 * kx, dyc as f64 * ky);
                let (id2, mode, s) = (d.id.clone(), d.mode, d.start);
                self.edit_keyed(Some(&format!("annot-drag-{id2}")), |p| {
                    if let Some(x) = p.scene.annotations.iter_mut().find(|x| x.id == id2) {
                        match mode {
                            DragMode::Move => {
                                x.x = s.0 + dx;
                                x.y = s.1 + dy;
                                if x.kind == "arrow" {
                                    x.x2 = s.4 + dx;
                                    x.y2 = s.5 + dy;
                                }
                            }
                            DragMode::Resize => {
                                x.w = (s.2 + dx).max(0.01);
                                x.h = (s.3 + dy).max(0.01);
                            }
                            DragMode::ArrowStart => {
                                x.x = s.0 + dx;
                                x.y = s.1 + dy;
                            }
                            DragMode::ArrowEnd => {
                                x.x2 = s.4 + dx;
                                x.y2 = s.5 + dy;
                            }
                        }
                    }
                });
            }
        }
        if resp.drag_stopped() {
            self.annot_drag = None;
        }
    }
}

fn paint_stage_bg(ui: &egui::Ui, rect: Rect) {
    let p = ui.painter();
    p.rect_filled(rect, 0.0, BG);
    let c = Pos2::new(rect.center().x, rect.min.y + rect.height() * 0.3);
    let (rx, ry) = (rect.width() * 0.5 * 1.4142, rect.height() * 0.7 * 1.4142);
    let (rx, ry) = (rx * 0.7, ry * 0.7);
    let center = Color32::from_rgb(0x0e, 0x16, 0x24);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(c, center);
    let n = 64;
    for k in 0..=n {
        let a = k as f32 / n as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(Pos2::new(c.x + rx * a.cos(), c.y + ry * a.sin()), BG);
    }
    for k in 0..n {
        mesh.add_triangle(0, 1 + k as u32, 2 + k as u32);
    }
    p.with_clip_rect(rect).add(egui::Shape::mesh(mesh));
}

pub use dialogs::reveal;

pub fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut c = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut c = {
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", ""]);
        c
    };
    #[cfg(all(unix, not(target_os = "macos")))]
    let mut c = std::process::Command::new("xdg-open");
    c.arg(url).spawn().map(|_| ())
}

/// One peak value per 20 ms packet of audio track `track`, 0..1.
pub(crate) fn waveform_peaks(path: &std::path::Path, track: usize) -> Result<Vec<f32>, String> {
    use screencraft_encode::mp4read::Mp4File;
    use screencraft_encode::opus::OpusDecoder;
    let file = Mp4File::open(path).map_err(|e| e.to_string())?;
    let t = file
        .tracks
        .iter()
        .filter(|t| t.kind == screencraft_encode::mp4read::TrackKind::Audio)
        .nth(track)
        .ok_or("no audio track")?;
    let mut dec = OpusDecoder::new(t.channels.clamp(1, 2) as u8).map_err(|e| e.to_string())?;
    let mut peaks = Vec::with_capacity(t.samples.len());
    for s in &t.samples {
        let raw = file.read_sample(s).map_err(|e| e.to_string())?;
        let pcm = dec.decode(&raw).map_err(|e| e.to_string())?;
        peaks.push(pcm.iter().fold(0f32, |m, v| m.max(v.abs())).min(1.0));
    }
    Ok(peaks)
}
