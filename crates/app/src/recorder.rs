//! The recorder: the floating bar (capture mode, inputs, output, Record), the countdown, the
//! recording HUD and the drop-up menus, laid out like the 0.3 recorder window.

use crate::icons;
use crate::kit;
use crate::prefs::Prefs;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use screencraft_audio::{list_devices, DeviceInfo, DeviceKind};
use screencraft_capture::{DisplayInfo, Permissions, Sources, Target, WindowInfo};
use screencraft_core::project::Rect as PRect;
use screencraft_encode::HwPreference;
use screencraft_engine::package::list_packages;
use screencraft_engine::{Finished, LiveStats, Package, RecordOptions, Recording};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const BAR_W: f32 = 1260.0;
pub const BAR_H: f32 = 76.0;

/// Resolution choices: cap on the long side in px (0 = the display's native resolution).
pub const RESOLUTIONS: [(u32, &str, &str); 5] = [
    (0, "Native", "Native"),
    (2560, "2560 px", "2560"),
    (1920, "1080p class · 1920 px", "1080p"),
    (1600, "1600 px", "1600"),
    (1280, "720p class · 1280 px", "720p"),
];
pub const FPS_CHOICES: [u32; 2] = [30, 60];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Display,
    Window,
    Area,
    /// No screen: the camera is the picture (a talking-head video).
    Camera,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Display => "display",
            Mode::Window => "window",
            Mode::Area => "area",
            Mode::Camera => "camera",
        }
    }
}

enum Cmd {
    Start(RecordOptions),
    Stop,
}

pub enum Evt {
    Started { stats: Arc<LiveStats>, width: u32, height: u32, encoder: String },
    StartFailed(String),
    Stopped(Result<Finished, String>),
}

/// Owns the `Recording` on its own thread (audio streams are not `Send`).
pub struct Controller {
    tx: Sender<Cmd>,
    pub rx: Receiver<Evt>,
}

impl Controller {
    pub fn spawn(ctx: egui::Context) -> Self {
        let (tx, cmd_rx) = channel::<Cmd>();
        let (evt_tx, rx) = channel::<Evt>();
        std::thread::Builder::new()
            .name("screencraft-controller".into())
            .spawn(move || {
                let mut current: Option<Recording> = None;
                while let Ok(cmd) = cmd_rx.recv() {
                    match cmd {
                        Cmd::Start(opts) => match Recording::start(opts) {
                            Ok(rec) => {
                                let info = rec.info().clone();
                                let _ = evt_tx.send(Evt::Started { stats: rec.stats.clone(), width: info.width, height: info.height, encoder: rec.encoder().to_string() });
                                current = Some(rec);
                            }
                            Err(e) => {
                                let _ = evt_tx.send(Evt::StartFailed(e));
                            }
                        },
                        Cmd::Stop => {
                            if let Some(rec) = current.take() {
                                let _ = evt_tx.send(Evt::Stopped(rec.stop()));
                            }
                        }
                    }
                    ctx.request_repaint();
                }
            })
            .expect("controller thread");
        Self { tx, rx }
    }

    pub fn start(&self, o: RecordOptions) {
        let _ = self.tx.send(Cmd::Start(o));
    }

    pub fn stop(&self) {
        let _ = self.tx.send(Cmd::Stop);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Idle,
    Countdown,
    Starting,
    Recording,
    Confirm,
    Finishing,
}

enum Probe {
    Sources(Result<Sources, String>),
    Mics(Vec<DeviceInfo>),
    Cameras(Vec<(String, String)>),
}

#[derive(Clone, Debug)]
pub enum MenuAction {
    None,
    PickWindow(u32, String),
    PickDisplay(u32),
    Resolution(u32),
    Fps(u32),
    CameraOff,
    Camera(String, String),
    MicOff,
    Mic(String, String),
    SysOff,
    SysOn,
    OpenProject(PathBuf),
    RevealFolder,
    ChooseFolder,
}

#[derive(Clone, Debug)]
pub enum MenuItem {
    Header(String),
    Item { label: String, sub: String, checked: bool, action: MenuAction },
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MenuKind {
    Windows,
    Displays,
    Camera,
    Mic,
    Sys,
    Quality,
    Folder,
}

pub struct Menu {
    pub kind: MenuKind,
    pub items: Vec<MenuItem>,
    /// Left edge of the menu within the bar window.
    pub left: f32,
}

/// What the bar asks the application to do.
pub enum Action {
    None,
    Record,
    Stop,
    Cancel,
    AskDiscard,
    KeepRecording,
    Discard,
    Quit,
    OpenProject(PathBuf),
    PickArea,
    HoverWindow(Option<WindowInfo>),
    ShowPermissions,
}

pub struct Recorder {
    pub mode: Mode,
    pub sources: Option<Result<Sources, String>>,
    sources_at: Option<Instant>,
    pub display: Option<u32>,
    pub window: Option<u32>,
    pub window_title: String,
    /// The picked area in global logical points.
    pub area: Option<PRect>,
    /// True while an editor window is open and the bar is parked out of sight.
    pub parked: bool,
    pub permissions: Permissions,
    perms_at: Option<Instant>,
    pub phase: Phase,
    pub countdown_at: Option<Instant>,
    pub stats: Option<Arc<LiveStats>>,
    pub started_at: Option<Instant>,
    pub encoder: String,
    pub size: (u32, u32),
    pub error: Option<(String, Instant)>,
    pub mics: Vec<DeviceInfo>,
    mics_loaded: bool,
    pub cameras: Vec<(String, String)>,
    cameras_loaded: bool,
    probe_rx: Receiver<Probe>,
    probe_tx: Sender<Probe>,
    sources_busy: bool,
    ctx: egui::Context,
    pub controller: Controller,
    pub menu: Option<Menu>,
    last_menu_close: Option<(MenuKind, Instant)>,
    pub finished: Option<Finished>,
    pub live: crate::live::Live,
}

impl Recorder {
    pub fn new(ctx: &egui::Context, prefs: &Prefs) -> Self {
        let (probe_tx, probe_rx) = channel();
        let mode = match prefs.mode.as_str() {
            "camera" if prefs.camera => Mode::Camera,
            // an area or window pick does not survive a restart, so always start on the display
            _ => Mode::Display,
        };
        Self {
            mode,
            sources: None,
            sources_at: None,
            display: None,
            window: None,
            window_title: String::new(),
            area: None,
            parked: false,
            permissions: Permissions::default(),
            perms_at: None,
            phase: Phase::Idle,
            countdown_at: None,
            stats: None,
            started_at: None,
            encoder: String::new(),
            size: (0, 0),
            error: None,
            mics: Vec::new(),
            mics_loaded: false,
            cameras: Vec::new(),
            cameras_loaded: false,
            probe_rx,
            probe_tx,
            sources_busy: false,
            ctx: ctx.clone(),
            controller: Controller::spawn(ctx.clone()),
            menu: None,
            last_menu_close: None,
            finished: None,
            live: crate::live::Live::new(ctx),
        }
    }

    pub fn show_error(&mut self, msg: impl Into<String>) {
        self.error = Some((msg.into(), Instant::now()));
    }

    /// Poll the slow platform queries (they run off the UI thread; they can block on prompts).
    pub fn refresh(&mut self, prefs: &Prefs) {
        let stale = |t: &Option<Instant>, s: u64| t.map(|t| t.elapsed().as_secs() >= s).unwrap_or(true);
        while let Ok(p) = self.probe_rx.try_recv() {
            match p {
                Probe::Sources(r) => {
                    if let Ok(s) = &r {
                        if self.display.is_none() {
                            self.display = s.displays.iter().find(|d| d.is_main).or(s.displays.first()).map(|d| d.id);
                        }
                    }
                    self.sources = Some(r);
                    self.sources_busy = false;
                }
                Probe::Mics(m) => {
                    self.mics = m;
                    self.mics_loaded = true;
                }
                Probe::Cameras(c) => {
                    self.cameras = c;
                    self.cameras_loaded = true;
                    // a phone plugged in while the menu is open shows up right away
                    if self.menu.as_ref().map(|m| m.kind == MenuKind::Camera).unwrap_or(false) {
                        let items = self.camera_items(prefs);
                        if let Some(m) = self.menu.as_mut() {
                            m.items = items;
                        }
                    }
                }
            }
        }
        if stale(&self.perms_at, 2) {
            self.permissions = screencraft_capture::permissions();
            self.perms_at = Some(Instant::now());
        }
        if !self.sources_busy && stale(&self.sources_at, 4) && self.phase == Phase::Idle && (self.permissions.screen || self.sources.is_none()) {
            self.sources_busy = true;
            self.sources_at = Some(Instant::now());
            let (tx, ctx) = (self.probe_tx.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Probe::Sources(screencraft_capture::list_sources()));
                ctx.request_repaint();
            });
        }
        // Only look at audio inputs once the microphone is wanted: enumerating them can trigger
        // the system's microphone permission prompt.
        if prefs.microphone && !self.mics_loaded {
            self.mics_loaded = true; // set again by the probe
            let (tx, ctx) = (self.probe_tx.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let m = list_devices().into_iter().filter(|d| d.kind == DeviceKind::Microphone).collect();
                let _ = tx.send(Probe::Mics(m));
                ctx.request_repaint();
            });
        }
        if let Some((_, t)) = &self.error {
            if t.elapsed() > Duration::from_secs(9) {
                self.error = None;
            }
        }
        let preset = match (self.mode, prefs.vertical) {
            (Mode::Camera, true) => screencraft_capture::camera::CamPreset::TalkingHeadVertical,
            (Mode::Camera, false) => screencraft_capture::camera::CamPreset::TalkingHead,
            _ => screencraft_capture::camera::CamPreset::Bubble,
        };
        self.live.sync(prefs, self.phase, self.parked, preset);
        if let Some(e) = self.live.take_error() {
            self.show_error(e);
        }
    }

    pub fn displays(&self) -> &[DisplayInfo] {
        match &self.sources {
            Some(Ok(s)) => &s.displays,
            _ => &[],
        }
    }

    pub fn windows(&self) -> &[WindowInfo] {
        match &self.sources {
            Some(Ok(s)) => &s.windows,
            _ => &[],
        }
    }

    /// Everything needed to start, from the current selections.
    pub fn build_options(&self, prefs: &Prefs) -> Result<RecordOptions, String> {
        let stamp = timestamp();
        let displays = self.displays();
        let display_id = self.display.or_else(|| displays.iter().find(|d| d.is_main).map(|d| d.id)).or_else(|| displays.first().map(|d| d.id));
        let target = match self.mode {
            Mode::Display => Target::Display(display_id),
            Mode::Window => Target::Window(self.window.ok_or("choose a window to record")?),
            Mode::Area => Target::Area { display: display_id, rect: self.area.ok_or("draw the area to record")? },
            Mode::Camera => {
                if !prefs.camera {
                    return Err("turn the camera on to record a talking-head video".into());
                }
                Target::Camera { vertical: prefs.vertical }
            }
        };
        let package = Package::create_in(&prefs.recordings_dir, &stamp).map_err(|e| format!("cannot create the recording folder: {e}"))?;
        Ok(RecordOptions {
            target,
            fps: prefs.fps,
            max_long_side: if prefs.max_long_side == 0 { None } else { Some(prefs.max_long_side) },
            system_audio: prefs.system_audio,
            microphone: prefs.microphone,
            microphone_id: prefs.microphone_id.clone(),
            camera: if prefs.camera { Some(screencraft_engine::CameraSpec { id: prefs.camera_id.clone(), hub: self.live.camera_source() }) } else { None },
            output: package,
            bitrate_kbps: None,
            hardware: HwPreference::Auto,
        })
    }

    pub fn elapsed_ms(&self) -> f64 {
        self.started_at.map(|t| t.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0)
    }

    /// The window size the current phase wants (the bar stays anchored at its bottom centre).
    pub fn window_size(&self, screen_w: f32, camera: bool) -> Vec2 {
        let menu_h = self.menu.as_ref().map(|m| menu_height(m) + 8.0 + 10.0).unwrap_or(0.0);
        match self.phase {
            Phase::Idle | Phase::Starting => Vec2::new(BAR_W.min(screen_w - 24.0), BAR_H + menu_h),
            Phase::Countdown => Vec2::new(240.0, 60.0),
            Phase::Recording | Phase::Finishing => Vec2::new(if camera { 380.0 } else { 340.0 }, 60.0),
            Phase::Confirm => Vec2::new(420.0, 60.0),
        }
    }

    /// A menu for screenshots, without clicking.
    pub fn menu_for_dev(&self, kind: MenuKind, prefs: &Prefs) -> Menu {
        let items = match kind {
            MenuKind::Quality => self.quality_items(prefs),
            MenuKind::Sys => self.sys_items(prefs),
            _ => self.folder_items(prefs),
        };
        Menu { kind, items, left: 640.0 }
    }

    pub fn close_menu(&mut self) {
        if let Some(m) = self.menu.take() {
            self.last_menu_close = Some((m.kind, Instant::now()));
        }
    }

    fn open_menu(&mut self, kind: MenuKind, items: Vec<MenuItem>, left: f32) {
        // the click that dismissed this very menu also lands on its button: don't reopen
        if let Some((k, t)) = self.last_menu_close {
            if k == kind && t.elapsed() < Duration::from_millis(350) {
                return;
            }
        }
        self.menu = Some(Menu { kind, items, left });
    }

    fn toggle_menu(&mut self, kind: MenuKind, items: Vec<MenuItem>, left: f32) {
        if self.menu.as_ref().map(|m| m.kind) == Some(kind) {
            self.close_menu();
        } else {
            self.open_menu(kind, items, left);
        }
    }

    // ---- menu builders -----------------------------------------------------------------

    fn camera_items(&self, prefs: &Prefs) -> Vec<MenuItem> {
        let mut v = vec![MenuItem::Header("Camera".into()), MenuItem::Item { label: "No camera".into(), sub: String::new(), checked: !prefs.camera, action: MenuAction::CameraOff }];
        for (id, name) in &self.cameras {
            let lower = name.to_lowercase();
            let phone = ["iphone", "ipad", "android", "pixel", "galaxy", "camo", "droidcam", "epoccam"].iter().any(|k| lower.contains(k));
            v.push(MenuItem::Item {
                label: name.clone(),
                sub: if phone { "Phone camera".into() } else { String::new() },
                checked: prefs.camera && prefs.camera_id.as_deref() == Some(id.as_str()),
                action: MenuAction::Camera(id.clone(), name.clone()),
            });
        }
        if !self.cameras.iter().any(|(_, n)| {
            let l = n.to_lowercase();
            l.contains("iphone") || l.contains("ipad") || l.contains("android")
        }) {
            v.push(MenuItem::Item {
                label: "Use your phone as the camera".into(),
                sub: "Plug it in by USB. iPhone: unlock it. Android: USB → Webcam.".into(),
                checked: false,
                action: MenuAction::None,
            });
        }
        v
    }

    fn mic_items(&self, prefs: &Prefs) -> Vec<MenuItem> {
        let mut v = vec![MenuItem::Header("Microphone".into()), MenuItem::Item { label: "No microphone".into(), sub: String::new(), checked: !prefs.microphone, action: MenuAction::MicOff }];
        for d in &self.mics {
            v.push(MenuItem::Item {
                label: d.name.clone(),
                sub: String::new(),
                checked: prefs.microphone && prefs.microphone_id.as_deref() == Some(d.id.as_str()),
                action: MenuAction::Mic(d.id.clone(), d.name.clone()),
            });
        }
        v
    }

    fn sys_items(&self, prefs: &Prefs) -> Vec<MenuItem> {
        vec![
            MenuItem::Header("Device audio".into()),
            MenuItem::Item { label: "Off".into(), sub: String::new(), checked: !prefs.system_audio, action: MenuAction::SysOff },
            MenuItem::Item {
                label: "System audio".into(),
                sub: format!("Everything your {} is playing (all apps)", if cfg!(target_os = "macos") { "Mac" } else { "PC" }),
                checked: prefs.system_audio,
                action: MenuAction::SysOn,
            },
        ]
    }

    fn quality_items(&self, prefs: &Prefs) -> Vec<MenuItem> {
        let d = self.displays().iter().find(|d| Some(d.id) == self.display);
        let dims = |long: u32| -> String {
            match (d, long) {
                (Some(d), l) if l > 0 => {
                    let k = l as f64 / d.width.max(d.height);
                    format!("{} × {}", (d.width * k).round(), (d.height * k).round())
                }
                _ => String::new(),
            }
        };
        let mut v = vec![MenuItem::Header("Resolution".into())];
        for (val, label, _) in RESOLUTIONS {
            let sub = if val > 0 {
                dims(val)
            } else if let Some(d) = d {
                format!("{} × {} on Retina", d.width as u32 * 2, d.height as u32 * 2)
            } else {
                "Full pixel size of the display".into()
            };
            v.push(MenuItem::Item { label: label.into(), sub, checked: prefs.max_long_side == val, action: MenuAction::Resolution(val) });
        }
        v.push(MenuItem::Header("Frame rate".into()));
        for f in FPS_CHOICES {
            v.push(MenuItem::Item {
                label: format!("{f} fps"),
                sub: if f == 60 { "Smoother motion, larger files".into() } else { "Lighter to record and edit".into() },
                checked: prefs.fps == f,
                action: MenuAction::Fps(f),
            });
        }
        v
    }

    fn folder_items(&self, prefs: &Prefs) -> Vec<MenuItem> {
        let mut v = vec![MenuItem::Header("Recent Projects".into())];
        let list = list_packages(&prefs.recordings_dir);
        for (pkg, modified) in list.iter().take(8) {
            let name = pkg.load_project().map(|p| p.name).unwrap_or_else(|_| pkg.dir.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
            v.push(MenuItem::Item { label: name, sub: format_time(*modified), checked: false, action: MenuAction::OpenProject(pkg.dir.clone()) });
        }
        if list.is_empty() {
            v.push(MenuItem::Item { label: "No projects yet".into(), sub: String::new(), checked: false, action: MenuAction::None });
        }
        v.push(MenuItem::Header("Raw recordings folder".into()));
        v.push(MenuItem::Item { label: "Open recordings folder".into(), sub: prefs.recordings_dir.display().to_string(), checked: false, action: MenuAction::RevealFolder });
        v.push(MenuItem::Item { label: "Choose recordings folder…".into(), sub: String::new(), checked: false, action: MenuAction::ChooseFolder });
        v
    }

    /// Apply a menu pick.
    pub fn apply(&mut self, action: MenuAction, prefs: &mut Prefs) -> Action {
        match action {
            MenuAction::None => {}
            MenuAction::PickWindow(id, title) => {
                self.window = Some(id);
                self.window_title = title;
            }
            MenuAction::PickDisplay(id) => self.display = Some(id),
            MenuAction::Resolution(v) => prefs.max_long_side = v,
            MenuAction::Fps(f) => prefs.fps = f,
            MenuAction::CameraOff => prefs.camera = false,
            MenuAction::Camera(id, name) => {
                prefs.camera = true;
                prefs.camera_id = Some(id);
                prefs.camera_label = name;
            }
            MenuAction::MicOff => prefs.microphone = false,
            MenuAction::Mic(id, name) => {
                prefs.microphone = true;
                prefs.microphone_id = Some(id);
                prefs.microphone_name = name;
            }
            MenuAction::SysOff => prefs.system_audio = false,
            MenuAction::SysOn => prefs.system_audio = true,
            MenuAction::OpenProject(dir) => return Action::OpenProject(dir),
            MenuAction::RevealFolder => {
                let _ = crate::editor::open_url(&prefs.recordings_dir.to_string_lossy());
            }
            MenuAction::ChooseFolder => {
                if let Some(d) = rfd::FileDialog::new().set_directory(&prefs.recordings_dir).pick_folder() {
                    prefs.recordings_dir = d;
                }
            }
        }
        prefs.mode = self.mode.name().into();
        prefs.save();
        Action::None
    }

    // ---- drawing --------------------------------------------------------------------------

    /// Draw the whole window content. `ui` is the root ui of the bar window.
    pub fn show(&mut self, ui: &mut egui::Ui, prefs: &mut Prefs) -> Action {
        let mut action = Action::None;
        let full = ui.max_rect();
        let bar_h = if matches!(self.phase, Phase::Idle | Phase::Starting) { BAR_H } else { 60.0 };
        let bar = Rect::from_min_max(Pos2::new(full.min.x, full.max.y - bar_h), full.max);
        // bar background: rgba(11,16,26,.96), 1px line-2, radius 14, soft shadow
        let painter = ui.painter();
        painter.rect_filled(bar.expand(1.0).translate(Vec2::new(0.0, 6.0)), CornerRadius::same(16), Color32::from_black_alpha(36));
        painter.rect_filled(bar, CornerRadius::same(14), Color32::from_rgba_unmultiplied(11, 16, 26, 245));
        painter.rect_stroke(bar, CornerRadius::same(14), Stroke::new(1.0, LINE_2), StrokeKind::Inside);

        // drag the bar anywhere you like by its background (the buttons drawn after this sit on top)
        let grab = ui.interact(bar, ui.id().with("bar-drag"), Sense::drag());
        if grab.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if grab.hovered() && self.menu.is_none() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        match self.phase {
            Phase::Idle | Phase::Starting => {
                let a = self.draw_idle(ui, bar, prefs);
                if !matches!(a, Action::None) {
                    action = a;
                }
            }
            Phase::Countdown => action = self.draw_countdown(ui, bar),
            Phase::Recording | Phase::Finishing => action = self.draw_hud(ui, bar),
            Phase::Confirm => action = self.draw_confirm(ui, bar),
        }
        // menu above the bar
        if self.phase == Phase::Idle {
            let a = self.draw_menu(ui, full, bar, prefs);
            if !matches!(a, Action::None) {
                action = a;
            }
        }
        // error toast over the bar
        if let Some((msg, _)) = self.error.clone() {
            let r = bar.shrink(0.0);
            let resp = ui.interact(r, ui.id().with("err"), Sense::click());
            ui.painter().rect_filled(r, CornerRadius::same(14), Color32::from_rgba_unmultiplied(58, 17, 23, 247));
            ui.painter().rect_stroke(r, CornerRadius::same(14), Stroke::new(1.0, Color32::from_rgb(0x7b, 0x22, 0x30)), StrokeKind::Inside);
            ui.painter().with_clip_rect(r.shrink(10.0)).text(Pos2::new(r.min.x + 16.0, r.center().y), Align2::LEFT_CENTER, format!("{msg}  (click to dismiss)"), kit::font(13.0), TEXT);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                self.error = None;
            }
        }
        action
    }

    fn draw_idle(&mut self, ui: &mut egui::Ui, bar: Rect, prefs: &mut Prefs) -> Action {
        let mut action = Action::None;
        let cy = bar.center().y;
        let mut x = bar.min.x + 12.0;
        let loading = self.live.any_loading(prefs);
        // close
        let close = Rect::from_center_size(Pos2::new(x + 15.0, cy), Vec2::splat(30.0));
        {
            let resp = ui.interact(close, ui.id().with("close"), Sense::click());
            let hov = resp.hovered();
            ui.painter().rect_filled(close, CornerRadius::same(9), if hov { alpha(DANGER, 0.15) } else { Color32::TRANSPARENT });
            ui.painter().rect_stroke(close, CornerRadius::same(9), Stroke::new(1.0, if hov { DANGER } else { LINE_2 }), StrokeKind::Inside);
            let col = if hov { Color32::from_rgb(0xff, 0x9a, 0xa5) } else { Color32::from_rgb(0xb9, 0xc3, 0xd6) };
            let c = close.center();
            for (a, b) in [((-5.0, -5.0), (5.0, 5.0)), ((5.0, -5.0), (-5.0, 5.0))] {
                ui.painter().line_segment([c + Vec2::new(a.0, a.1), c + Vec2::new(b.0, b.1)], Stroke::new(2.2, col));
            }
            if resp.on_hover_text("Quit ScreenCraft").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                action = Action::Quit;
            }
        }
        x += 30.0 + 10.0;
        let sep = |ui: &egui::Ui, x: &mut f32| {
            ui.painter().line_segment([Pos2::new(*x + 0.5, cy - 20.0), Pos2::new(*x + 0.5, cy + 20.0)], Stroke::new(1.0, LINE_2));
            *x += 1.0 + 4.0 + 10.0 - 2.0;
        };
        sep(ui, &mut x);

        // ---- capture group
        let cap_top = cy - 25.0;
        let group_caption = |ui: &egui::Ui, x: f32, text: &str| {
            kit::spaced_text(ui, Pos2::new(x + 2.0, cap_top), &text.to_uppercase(), 9.5, MUTED, 1.14);
        };
        group_caption(ui, x, "Capture");
        let seg_y = cap_top + 17.0;
        let modes = [(Mode::Display, "Display", "monitor"), (Mode::Window, "Window", "window"), (Mode::Area, "Area", "area"), (Mode::Camera, "Camera", "camera")];
        let widths: Vec<f32> = modes.iter().map(|(_, l, _)| 10.0 * 2.0 + 17.0 + 7.0 + kit::text_width(ui, l, 12.5) + 1.0).collect();
        let seg_w: f32 = widths.iter().sum();
        let seg = Rect::from_min_size(Pos2::new(x, seg_y), Vec2::new(seg_w, 34.0));
        let mut mx = x;
        let mut clicked_mode = None;
        for (i, (m, label, icon)) in modes.iter().enumerate() {
            let r = Rect::from_min_size(Pos2::new(mx, seg_y), Vec2::new(widths[i], 34.0));
            let resp = ui.interact(r, ui.id().with(("mode", i)), Sense::click());
            let on = self.mode == *m;
            let last = modes.len() - 1;
            let cr = CornerRadius { nw: if i == 0 { 9 } else { 0 }, sw: if i == 0 { 9 } else { 0 }, ne: if i == last { 9 } else { 0 }, se: if i == last { 9 } else { 0 } };
            if on {
                ui.painter().rect_filled(r.shrink(1.0), cr, ACCENT);
            } else if resp.hovered() {
                ui.painter().rect_filled(r.shrink(1.0), cr, Color32::from_white_alpha(13));
            }
            let fg = if on { Color32::WHITE } else { Color32::from_rgb(0xb9, 0xc3, 0xd6) };
            icons::paint(ui, icon, Rect::from_center_size(Pos2::new(r.min.x + 10.0 + 8.5, cy_of(r)), Vec2::splat(17.0)), fg);
            ui.painter().text(Pos2::new(r.min.x + 10.0 + 24.0, cy_of(r)), Align2::LEFT_CENTER, label, kit::font(12.5), fg);
            if i < last {
                ui.painter().line_segment([Pos2::new(r.max.x - 0.5, r.min.y), Pos2::new(r.max.x - 0.5, r.max.y)], Stroke::new(1.0, LINE_2));
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                clicked_mode = Some((*m, r));
            }
            mx += widths[i];
        }
        ui.painter().rect_stroke(seg, CornerRadius::same(9), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
        x += seg_w + 10.0;
        if let Some((m, r)) = clicked_mode {
            let left = (r.min.x - ui.max_rect().min.x).max(8.0);
            action = self.choose_mode(m, left, prefs);
        }
        sep(ui, &mut x);

        // ---- inputs group
        group_caption(ui, x, "Inputs");
        let oy = seg_y;
        let cam_label = if prefs.camera { if prefs.camera_label.is_empty() { "Camera".to_string() } else { prefs.camera_label.clone() } } else { "No camera".to_string() };
        let mic_label = if prefs.microphone { if prefs.microphone_name.is_empty() { "Microphone".to_string() } else { prefs.microphone_name.clone() } } else { "No microphone".to_string() };
        let sys_label = if prefs.system_audio { "System audio" } else { "No system audio" };
        let (mic_level, sys_level) = self.live.levels();
        let cam_tex = self.live.camera_texture();
        let left0 = ui.max_rect().min.x;
        let opts: [(&str, String, bool, MenuKind, f32, bool, Option<egui::TextureId>); 3] = [
            ("camera", cam_label, prefs.camera, MenuKind::Camera, 0.0, self.live.loading(crate::live::Input::Camera, prefs), cam_tex),
            ("mic", mic_label, prefs.microphone, MenuKind::Mic, mic_level, self.live.loading(crate::live::Input::Mic, prefs), None),
            ("audio", sys_label.to_string(), prefs.system_audio, MenuKind::Sys, sys_level, self.live.loading(crate::live::Input::Sys, prefs), None),
        ];
        let mut toggled: Option<(MenuKind, f32)> = None;
        for (icon, label, on, kind, level, is_loading, tex) in opts {
            // nothing plays "on the computer" in a talking-head video
            if self.mode == Mode::Camera && kind == MenuKind::Sys {
                continue;
            }
            let (r, resp) = opt_button(ui, Pos2::new(x, oy), icon, &label, on, 108.0, level, is_loading, tex);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !is_loading {
                toggled = Some((kind, (r.min.x - left0).clamp(8.0, BAR_W - 320.0)));
            }
            x += r.width() + 6.0;
        }
        x += 4.0;
        if let Some((kind, left)) = toggled {
            self.click_input(kind, left, prefs);
        }
        sep(ui, &mut x);

        // ---- output group
        group_caption(ui, x, "Output");
        let res = RESOLUTIONS.iter().find(|r| r.0 == prefs.max_long_side).unwrap_or(&RESOLUTIONS[0]);
        let qual = format!("{} · {} fps", res.2, prefs.fps);
        let (r, resp) = opt_button_chev(ui, Pos2::new(x, oy), "chart", &qual, 138.0);
        if resp.on_hover_text("Resolution & frame rate").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            let items = self.quality_items(prefs);
            self.toggle_menu(MenuKind::Quality, items, (r.min.x - left0).clamp(8.0, BAR_W - 320.0));
        }
        x += r.width() + 6.0;
        if matches!(self.mode, Mode::Area | Mode::Camera) {
            let tip = if self.mode == Mode::Camera { "Record vertically (9:16), like a phone video" } else { "Keep the area 9:16 while you draw it" };
            let (r, resp) = toggle_chip(ui, Pos2::new(x, oy), "9:16", prefs.vertical);
            if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                prefs.vertical = !prefs.vertical;
                prefs.save();
                // a different shape needs a fresh area pick
                if self.mode == Mode::Area {
                    self.area = None;
                }
            }
            x += r.width() + 6.0;
        }
        let (r, resp) = opt_button_icon_right(ui, Pos2::new(x, oy), "Projects", "folder");
        if resp.on_hover_text("Open projects").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            let items = self.folder_items(prefs);
            self.toggle_menu(MenuKind::Folder, items, (r.min.x - left0).clamp(8.0, BAR_W - 320.0));
        }

        // ---- record
        let rec_w = 16.0 * 2.0 + 14.0 + 9.0 + kit::text_width(ui, "Record", 13.5);
        let rec = Rect::from_min_size(Pos2::new(bar.max.x - 12.0 - rec_w, cy - 23.0), Vec2::new(rec_w, 46.0));
        let resp = ui.interact(rec, ui.id().with("rec"), if loading { Sense::hover() } else { Sense::click() });
        let hov = resp.hovered() && !loading;
        let col = if hov { ACCENT_2 } else { ACCENT };
        ui.painter().rect_filled(rec, CornerRadius::same(10), if loading { alpha(col, 0.5) } else { col });
        ui.painter().rect_stroke(rec, CornerRadius::same(10), Stroke::new(1.0, Color32::from_white_alpha(46)), StrokeKind::Inside);
        // the logo's four corner marks
        let marks = Rect::from_min_max(Pos2::new(rec.min.x + 6.0, rec.min.y + 8.0), Pos2::new(rec.max.x - 6.0, rec.max.y - 8.0));
        let mc = Color32::from_white_alpha(178);
        let l = 7.0;
        for (corner, dx, dy) in [(marks.left_top(), 1.0, 1.0), (marks.right_top(), -1.0, 1.0), (marks.left_bottom(), 1.0, -1.0), (marks.right_bottom(), -1.0, -1.0)] {
            ui.painter().line_segment([corner, corner + Vec2::new(l * dx, 0.0)], Stroke::new(1.0, mc));
            ui.painter().line_segment([corner, corner + Vec2::new(0.0, l * dy)], Stroke::new(1.0, mc));
        }
        let dot = Pos2::new(rec.min.x + 16.0 + 7.0, rec.center().y);
        ui.painter().circle_filled(dot, 10.0, Color32::from_white_alpha(71));
        ui.painter().circle_filled(dot, 7.0, Color32::WHITE);
        ui.painter().text(Pos2::new(rec.min.x + 16.0 + 14.0 + 9.0, rec.center().y), Align2::LEFT_CENTER, "Record", kit::font(13.5), Color32::WHITE);
        if resp.on_hover_cursor(if loading { egui::CursorIcon::NotAllowed } else { egui::CursorIcon::PointingHand }).clicked() && !loading {
            action = Action::Record;
        }
        action
    }

    fn choose_mode(&mut self, m: Mode, left: f32, prefs: &mut Prefs) -> Action {
        self.close_menu();
        self.mode = m;
        prefs.mode = m.name().into();
        prefs.save();
        match m {
            Mode::Display => {
                let ds = self.displays().to_vec();
                if ds.len() > 1 {
                    let items = std::iter::once(MenuItem::Header("Displays".into()))
                        .chain(ds.iter().map(|d| MenuItem::Item {
                            label: d.name.clone(),
                            sub: format!("{} × {}", d.width as u32, d.height as u32),
                            checked: self.display == Some(d.id),
                            action: MenuAction::PickDisplay(d.id),
                        }))
                        .collect();
                    self.open_menu(MenuKind::Displays, items, left);
                }
                Action::None
            }
            Mode::Window => {
                let mut items = vec![MenuItem::Header("Windows".into())];
                for w in self.windows() {
                    items.push(MenuItem::Item {
                        label: format!("{}{}", w.app, if w.title.is_empty() { String::new() } else { format!(" — {}", w.title) }),
                        sub: format!("{} × {}", w.width.round() as i64, w.height.round() as i64),
                        checked: self.window == Some(w.id),
                        action: MenuAction::PickWindow(w.id, if w.title.is_empty() { w.app.clone() } else { w.title.clone() }),
                    });
                }
                if items.len() == 1 {
                    items.push(MenuItem::Item { label: "No windows found".into(), sub: String::new(), checked: false, action: MenuAction::None });
                }
                self.open_menu(MenuKind::Windows, items, left);
                Action::None
            }
            Mode::Area => Action::PickArea,
            Mode::Camera => {
                // a talking-head video needs the camera
                if !prefs.camera {
                    prefs.camera = true;
                    prefs.save();
                }
                Action::None
            }
        }
    }

    fn click_input(&mut self, kind: MenuKind, left: f32, prefs: &mut Prefs) {
        match kind {
            MenuKind::Camera => {
                // look again every time the menu opens, so a phone just plugged in is found
                self.cameras_loaded = true;
                let (tx, ctx) = (self.probe_tx.clone(), self.ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Probe::Cameras(screencraft_capture::camera::list()));
                    ctx.request_repaint();
                });
                let items = self.camera_items(prefs);
                self.toggle_menu(kind, items, left);
            }
            MenuKind::Mic => {
                if !self.mics_loaded {
                    self.mics_loaded = true;
                    let (tx, ctx) = (self.probe_tx.clone(), self.ctx.clone());
                    std::thread::spawn(move || {
                        let m = list_devices().into_iter().filter(|d| d.kind == DeviceKind::Microphone).collect();
                        let _ = tx.send(Probe::Mics(m));
                        ctx.request_repaint();
                    });
                }
                let items = self.mic_items(prefs);
                self.toggle_menu(kind, items, left);
            }
            _ => {
                let items = self.sys_items(prefs);
                self.toggle_menu(kind, items, left);
            }
        }
    }

    /// The drop-up menu, rebuilt from live data each frame it is open (device lists arrive late).
    fn draw_menu(&mut self, ui: &mut egui::Ui, full: Rect, bar: Rect, prefs: &mut Prefs) -> Action {
        let Some(kind) = self.menu.as_ref().map(|m| m.kind) else { return Action::None };
        // refresh the items that depend on late-arriving lists
        let items = match kind {
            MenuKind::Camera => Some(self.camera_items(prefs)),
            MenuKind::Mic => Some(self.mic_items(prefs)),
            MenuKind::Sys => Some(self.sys_items(prefs)),
            MenuKind::Quality => Some(self.quality_items(prefs)),
            _ => None,
        };
        if let (Some(items), Some(m)) = (items, self.menu.as_mut()) {
            m.items = items;
        }
        let Some(menu) = self.menu.as_ref() else { return Action::None };
        let h = menu_height(menu);
        let w = menu_width(ui, menu);
        let left = (full.min.x + menu.left).min(full.max.x - w - 8.0).max(full.min.x + 8.0);
        let rect = Rect::from_min_size(Pos2::new(left, bar.min.y - 8.0 - h), Vec2::new(w, h));
        ui.painter().rect_filled(rect.translate(Vec2::new(0.0, 8.0)).expand(2.0), CornerRadius::same(13), Color32::from_black_alpha(60));
        ui.painter().rect_filled(rect, CornerRadius::same(11), Color32::from_rgba_unmultiplied(13, 19, 30, 250));
        ui.painter().rect_stroke(rect, CornerRadius::same(11), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
        let mut picked: Option<MenuAction> = None;
        let mut hover_window: Option<u32> = None;
        let items = menu.items.clone();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect.shrink(6.0)));
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for it in &items {
                match it {
                    MenuItem::Header(t) => {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 25.0), Sense::hover());
                        kit::spaced_text(ui, Pos2::new(r.min.x + 10.0, r.min.y + 9.0), &t.to_uppercase(), 10.0, MUTED, 1.0);
                    }
                    MenuItem::Item { label, sub, checked, action } => {
                        let h = if sub.is_empty() { 34.0 } else { 52.0 };
                        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(r, CornerRadius::same(7), Color32::from_white_alpha(18));
                            if let MenuAction::PickWindow(id, _) = action {
                                hover_window = Some(*id);
                            }
                        }
                        if *checked {
                            ui.painter().text(Pos2::new(r.min.x + 10.0, r.min.y + 17.0), Align2::LEFT_CENTER, "✓", kit::font(13.0), ACCENT_2);
                        }
                        let ty = if sub.is_empty() { r.center().y } else { r.min.y + 17.0 };
                        ui.painter().with_clip_rect(r).text(Pos2::new(r.min.x + 26.0, ty), Align2::LEFT_CENTER, label, kit::font(13.0), Color32::from_rgb(0xd5, 0xdc, 0xec));
                        if !sub.is_empty() {
                            ui.painter().with_clip_rect(r).text(Pos2::new(r.min.x + 26.0, r.min.y + 36.0), Align2::LEFT_CENTER, sub, kit::font(11.0), MUTED);
                        }
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            picked = Some(action.clone());
                        }
                    }
                }
            }
        });
        let mut action = Action::None;
        if let Some(a) = picked {
            self.close_menu();
            action = self.apply(a, prefs);
            if matches!(action, Action::None) {
                action = Action::HoverWindow(None);
            }
        } else {
            // a click anywhere else closes it
            let outside = ui.ctx().input(|i| i.pointer.any_pressed()) && ui.ctx().input(|i| i.pointer.interact_pos()).map(|p| !rect.contains(p)).unwrap_or(false);
            if outside {
                self.close_menu();
                action = Action::HoverWindow(None);
            } else if kind == MenuKind::Windows {
                let w = hover_window.and_then(|id| self.windows().iter().find(|w| w.id == id).cloned());
                action = Action::HoverWindow(w);
            }
        }
        action
    }

    fn draw_countdown(&mut self, ui: &mut egui::Ui, bar: Rect) -> Action {
        let started = self.countdown_at.unwrap_or_else(Instant::now);
        let left = 3.0 - started.elapsed().as_secs_f32();
        let n = left.ceil().max(1.0) as u32;
        ui.painter().text(Pos2::new(bar.min.x + 60.0, bar.center().y), Align2::CENTER_CENTER, n.to_string(), egui::FontId::proportional(30.0), TEXT);
        ui.ctx().request_repaint_after(Duration::from_millis(50));
        let mut a = Action::None;
        if stop_button(ui, bar, "Cancel", 0.0).clicked() {
            a = Action::Cancel;
        }
        a
    }

    fn draw_hud(&mut self, ui: &mut egui::Ui, bar: Rect) -> Action {
        let mut action = Action::None;
        let cy = bar.center().y;
        let mut x = bar.min.x + 12.0;
        let finishing = self.phase == Phase::Finishing;
        if let Some(tex) = self.live.camera_texture().filter(|_| !finishing) {
            let r = Rect::from_min_size(Pos2::new(x, cy - 17.0), Vec2::splat(34.0));
            ui.painter().image(tex, r, Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0)), Color32::WHITE);
            x += 34.0 + 14.0;
        }
        if !finishing {
            // pulse
            let t = ui.input(|i| i.time);
            let a = 0.35 + 0.65 * (0.5 + 0.5 * (t * std::f64::consts::TAU / 1.2).cos()) as f32;
            ui.painter().rect_filled(Rect::from_center_size(Pos2::new(x + 5.5, cy), Vec2::splat(11.0)), CornerRadius::same(3), alpha(DANGER, a));
            x += 11.0 + 14.0;
        }
        let secs = (self.elapsed_ms() / 1000.0) as u64;
        let text = if finishing { "Saving…".to_string() } else { format!("{}:{:02}", secs / 60, secs % 60) };
        let g = ui.painter().layout_no_wrap(text, kit::font(18.0), TEXT);
        let w = g.size().x;
        ui.painter().galley(Pos2::new(x, cy - g.size().y / 2.0), g, TEXT);
        x += w + 14.0;
        if !finishing {
            if let Some(s) = &self.stats {
                for (lvl, tip) in [(s.mic_peak(), "Microphone"), (s.system_peak(), "System audio")] {
                    let r = Rect::from_min_size(Pos2::new(x, cy - 2.5), Vec2::new(34.0, 5.0));
                    ui.painter().rect_filled(r, CornerRadius::same(3), Color32::from_white_alpha(36));
                    meter_fill(ui, r, lvl.sqrt());
                    let _ = tip;
                    x += 34.0 + 2.0 + 8.0;
                }
            }
            // trash + stop on the right
            let stop_w = 18.0 * 2.0 + kit::text_width(ui, "Stop", 13.0);
            let stop = Rect::from_min_size(Pos2::new(bar.max.x - 12.0 - stop_w, cy - 20.0), Vec2::new(stop_w, 40.0));
            let resp = ui.interact(stop, ui.id().with("stop"), Sense::click());
            ui.painter().rect_filled(stop, CornerRadius::same(9), if resp.hovered() { Color32::from_rgb(0xff, 0x6b, 0x7a) } else { DANGER });
            ui.painter().text(stop.center(), Align2::CENTER_CENTER, "Stop", kit::font(13.0), Color32::WHITE);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                action = Action::Stop;
            }
            let trash = Rect::from_min_size(Pos2::new(stop.min.x - 10.0 - 40.0, cy - 20.0), Vec2::splat(40.0));
            let resp = ui.interact(trash, ui.id().with("trash"), Sense::click());
            let hov = resp.hovered();
            ui.painter().rect_filled(trash, CornerRadius::same(9), if hov { alpha(DANGER, 0.16) } else { Color32::TRANSPARENT });
            ui.painter().rect_stroke(trash, CornerRadius::same(9), Stroke::new(1.0, if hov { alpha(DANGER, 0.5) } else { LINE_2 }), StrokeKind::Inside);
            icons::paint(ui, "trash", Rect::from_center_size(trash.center(), Vec2::splat(18.0)), if hov { Color32::from_rgb(0xff, 0x9a, 0xa6) } else { Color32::from_rgb(0xb9, 0xc4, 0xd9) });
            if resp.on_hover_text("Discard this recording").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                action = Action::AskDiscard;
            }
        }
        ui.ctx().request_repaint_after(Duration::from_millis(50));
        action
    }

    fn draw_confirm(&mut self, ui: &mut egui::Ui, bar: Rect) -> Action {
        let cy = bar.center().y;
        ui.painter().text(Pos2::new(bar.min.x + 14.0, cy), Align2::LEFT_CENTER, "Discard this recording?", kit::font(13.0), TEXT);
        let mut a = Action::None;
        let dw = 14.0 * 2.0 + kit::text_width(ui, "Discard", 13.0);
        let drop = Rect::from_min_size(Pos2::new(bar.max.x - 12.0 - dw, cy - 19.0), Vec2::new(dw, 38.0));
        let resp = ui.interact(drop, ui.id().with("drop"), Sense::click());
        ui.painter().rect_filled(drop, CornerRadius::same(9), if resp.hovered() { Color32::from_rgb(0xff, 0x6b, 0x7a) } else { DANGER });
        ui.painter().text(drop.center(), Align2::CENTER_CENTER, "Discard", kit::font(13.0), Color32::WHITE);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            a = Action::Discard;
        }
        let kw = 14.0 * 2.0 + kit::text_width(ui, "Keep recording", 13.0);
        let keep = Rect::from_min_size(Pos2::new(drop.min.x - 10.0 - kw, cy - 19.0), Vec2::new(kw, 38.0));
        let resp = ui.interact(keep, ui.id().with("keep"), Sense::click());
        ui.painter().rect_filled(keep, CornerRadius::same(9), if resp.hovered() { RAISE } else { PANEL_2 });
        ui.painter().rect_stroke(keep, CornerRadius::same(9), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
        ui.painter().text(keep.center(), Align2::CENTER_CENTER, "Keep recording", kit::font(13.0), Color32::from_rgb(0xe1, 0xe7, 0xf5));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            a = Action::KeepRecording;
        }
        a
    }
}

fn cy_of(r: Rect) -> f32 {
    r.center().y
}

fn stop_button(ui: &mut egui::Ui, bar: Rect, text: &str, _x: f32) -> egui::Response {
    let w = 18.0 * 2.0 + kit::text_width(ui, text, 13.0);
    let r = Rect::from_min_size(Pos2::new(bar.max.x - 12.0 - w, bar.center().y - 20.0), Vec2::new(w, 40.0));
    let resp = ui.interact(r, ui.id().with(("stopbtn", text)), Sense::click());
    ui.painter().rect_filled(r, CornerRadius::same(9), if resp.hovered() { Color32::from_rgb(0xff, 0x6b, 0x7a) } else { DANGER });
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, kit::font(13.0), Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The level bar of an input: green, amber from 80%, red from 90%.
fn meter_fill(ui: &egui::Ui, r: Rect, level: f32) {
    let f = level.clamp(0.0, 1.0);
    let w = r.width() * f;
    let stops = [(0.0f32, 0.65f32, Color32::from_rgb(0x3d, 0xdc, 0x97)), (0.65, 0.8, Color32::from_rgb(0xf6, 0xc4, 0x53)), (0.8, 1.0, Color32::from_rgb(0xff, 0x6b, 0x6b))];
    for (a, b, c) in stops {
        let (x0, x1) = (r.min.x + r.width() * a, (r.min.x + r.width() * b).min(r.min.x + w));
        if x1 > x0 {
            ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x0, r.min.y), Pos2::new(x1, r.max.y)), 0.0, c);
        }
    }
}

/// `.opt`: an icon (or live camera picture) and a label in a bordered button, with an optional
/// level meter along its bottom edge.
#[allow(clippy::too_many_arguments)]
fn opt_button(ui: &mut egui::Ui, at: Pos2, icon: &str, label: &str, on: bool, max_w: f32, level: f32, loading: bool, tex: Option<egui::TextureId>) -> (Rect, egui::Response) {
    let lab_w = kit::text_width(ui, label, 12.5);
    let w = (10.0 * 2.0 + 17.0 + 7.0 + lab_w).min(max_w);
    let r = Rect::from_min_size(at, Vec2::new(w, 34.0));
    let resp = ui.interact(r, ui.id().with(("opt", icon)), Sense::click());
    let hov = resp.hovered();
    ui.painter().rect_filled(r, CornerRadius::same(9), PANEL_2);
    let border = if on { Color32::from_rgba_unmultiplied(106, 174, 232, 140) } else if hov { LINE_2 } else { LINE };
    ui.painter().rect_stroke(r, CornerRadius::same(9), Stroke::new(1.0, border), StrokeKind::Inside);
    let op = if on { 1.0 } else { 0.65 };
    let fg = if on || hov { Color32::WHITE } else { Color32::from_rgb(0xb9, 0xc3, 0xd6) };
    let fg = Color32::from_rgba_unmultiplied(fg.r(), fg.g(), fg.b(), (255.0 * op) as u8);
    if loading {
        // a spinner instead of the contents
        let t = ui.input(|i| i.time) as f32;
        let c = r.center();
        for k in 0..12 {
            let a = k as f32 / 12.0 * std::f32::consts::TAU + t * 8.0;
            let alpha_ = ((k as f32 / 12.0) * 200.0) as u8;
            ui.painter().circle_filled(c + Vec2::new(a.cos(), a.sin()) * 7.0, 1.2, Color32::from_rgba_unmultiplied(106, 174, 232, alpha_));
        }
        ui.ctx().request_repaint_after(Duration::from_millis(40));
        return (r, resp);
    }
    if let (Some(t), true) = (tex, on) {
        let pr = Rect::from_center_size(Pos2::new(r.min.x + 10.0 + 13.0, r.center().y), Vec2::splat(26.0));
        ui.painter().image(t, pr, Rect::from_min_max(Pos2::new(1.0, 0.0), Pos2::new(0.0, 1.0)), Color32::WHITE);
        let shown = kit::fit_text(ui, label, 12.5, w - 10.0 - 26.0 - 7.0 - 10.0);
        ui.painter().text(Pos2::new(r.min.x + 10.0 + 26.0 + 7.0, r.center().y), Align2::LEFT_CENTER, shown, kit::font(12.5), fg);
    } else {
        icons::paint(ui, icon, Rect::from_center_size(Pos2::new(r.min.x + 10.0 + 8.5, r.center().y), Vec2::splat(17.0)), fg);
        let shown = kit::fit_text(ui, label, 12.5, w - 10.0 - 24.0 - 10.0);
        ui.painter().text(Pos2::new(r.min.x + 10.0 + 24.0, r.center().y), Align2::LEFT_CENTER, shown, kit::font(12.5), fg);
    }
    if on && (icon == "mic" || icon == "audio") {
        let m = Rect::from_min_max(Pos2::new(r.min.x + 8.0, r.max.y - 5.0), Pos2::new(r.max.x - 8.0, r.max.y - 3.0));
        ui.painter().rect_filled(m, CornerRadius::same(1), Color32::from_white_alpha(26));
        meter_fill(ui, m, level.sqrt());
    }
    (r, resp)
}

fn opt_button_chev(ui: &mut egui::Ui, at: Pos2, icon: &str, label: &str, max_w: f32) -> (Rect, egui::Response) {
    let lab_w = kit::text_width(ui, label, 12.5);
    let w = (10.0 * 2.0 + 17.0 + 7.0 + lab_w + 7.0 + 11.0).min(max_w);
    let r = Rect::from_min_size(at, Vec2::new(w, 34.0));
    let resp = ui.interact(r, ui.id().with(("optc", icon)), Sense::click());
    let hov = resp.hovered();
    ui.painter().rect_filled(r, CornerRadius::same(9), PANEL_2);
    ui.painter().rect_stroke(r, CornerRadius::same(9), Stroke::new(1.0, if hov { LINE_2 } else { LINE }), StrokeKind::Inside);
    let fg = if hov { Color32::WHITE } else { Color32::from_rgb(0xb9, 0xc3, 0xd6) };
    let svg_icon = if icon == "chart" { "chart" } else { icon };
    icons::paint(ui, svg_icon, Rect::from_center_size(Pos2::new(r.min.x + 10.0 + 8.5, r.center().y), Vec2::splat(17.0)), fg);
    ui.painter().text(Pos2::new(r.min.x + 10.0 + 24.0, r.center().y), Align2::LEFT_CENTER, label, kit::font(12.5), fg);
    icons::paint(ui, "chev", Rect::from_center_size(Pos2::new(r.max.x - 10.0 - 5.5, r.center().y), Vec2::splat(11.0)), fg);
    (r, resp)
}

/// A small on/off chip (the 9:16 switch).
fn toggle_chip(ui: &mut egui::Ui, at: Pos2, label: &str, on: bool) -> (Rect, egui::Response) {
    let w = 10.0 * 2.0 + kit::text_width(ui, label, 12.5) + 7.0 + 12.0;
    let r = Rect::from_min_size(at, Vec2::new(w, 34.0));
    let resp = ui.interact(r, ui.id().with(("chip", label)), Sense::click());
    let hov = resp.hovered();
    let fill = if on { ACCENT } else { PANEL_2 };
    ui.painter().rect_filled(r, CornerRadius::same(9), fill);
    ui.painter().rect_stroke(r, CornerRadius::same(9), Stroke::new(1.0, if on { Color32::from_white_alpha(46) } else if hov { LINE_2 } else { LINE }), StrokeKind::Inside);
    let fg = if on { Color32::WHITE } else if hov { Color32::WHITE } else { Color32::from_rgb(0xb9, 0xc3, 0xd6) };
    // a little upright rectangle
    let ic = Rect::from_center_size(Pos2::new(r.min.x + 10.0 + 4.0, r.center().y), Vec2::new(8.0, 13.0));
    ui.painter().rect_stroke(ic, CornerRadius::same(2), Stroke::new(1.6, fg), StrokeKind::Inside);
    ui.painter().text(Pos2::new(r.min.x + 10.0 + 12.0 + 6.0, r.center().y), Align2::LEFT_CENTER, label, kit::font(12.5), fg);
    (r, resp)
}

fn opt_button_icon_right(ui: &mut egui::Ui, at: Pos2, label: &str, icon: &str) -> (Rect, egui::Response) {
    let w = 10.0 * 2.0 + kit::text_width(ui, label, 12.5) + 7.0 + 17.0;
    let r = Rect::from_min_size(at, Vec2::new(w, 34.0));
    let resp = ui.interact(r, ui.id().with(("optr", icon)), Sense::click());
    let hov = resp.hovered();
    ui.painter().rect_filled(r, CornerRadius::same(9), PANEL_2);
    ui.painter().rect_stroke(r, CornerRadius::same(9), Stroke::new(1.0, if hov { LINE_2 } else { LINE }), StrokeKind::Inside);
    let fg = if hov { Color32::WHITE } else { Color32::from_rgb(0xb9, 0xc3, 0xd6) };
    ui.painter().text(Pos2::new(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, label, kit::font(12.5), fg);
    icons::paint(ui, icon, Rect::from_center_size(Pos2::new(r.max.x - 10.0 - 8.5, r.center().y), Vec2::splat(17.0)), fg);
    (r, resp)
}

fn menu_height(m: &Menu) -> f32 {
    let content: f32 = m
        .items
        .iter()
        .map(|i| match i {
            MenuItem::Header(_) => 25.0,
            MenuItem::Item { sub, .. } => {
                if sub.is_empty() {
                    34.0
                } else {
                    52.0
                }
            }
        })
        .sum();
    content.min(300.0) + 12.0
}

fn menu_width(ui: &egui::Ui, m: &Menu) -> f32 {
    let w = m
        .items
        .iter()
        .map(|i| match i {
            MenuItem::Header(_) => 0.0,
            MenuItem::Item { label, sub, .. } => kit::text_width(ui, label, 13.0).max(kit::text_width(ui, sub, 11.0)) + 26.0 + 20.0,
        })
        .fold(0.0, f32::max);
    (w + 12.0).clamp(300.0, 420.0)
}

fn format_time(secs: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let ago = now.saturating_sub(secs);
    if ago < 60 {
        "just now".into()
    } else if ago < 3600 {
        format!("{} min ago", ago / 60)
    } else if ago < 86_400 {
        format!("{} h ago", ago / 3600)
    } else {
        format!("{} days ago", ago / 86_400)
    }
}

pub fn timestamp() -> String {
    // Local-time stamp without a date crate: seconds since epoch broken into UTC fields.
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}.{:02}.{:02}", rem / 3600, (rem % 3600) / 60, rem % 60)
}
