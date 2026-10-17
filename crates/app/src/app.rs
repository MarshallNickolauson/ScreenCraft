//! Application shell: the recorder bar is the root window (bottom-centre, always on top); every
//! open project gets its own editor window, like the 0.3 app.

use crate::editor::EditorState;
use crate::prefs::Prefs;
use crate::recorder::{Action, Evt, Mode, Phase, Recorder, BAR_H, BAR_W};
use eframe::egui::{self, Pos2, Vec2, ViewportBuilder, ViewportCommand, ViewportId};
use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use screencraft_core::project::Rect as PRect;
use screencraft_engine::Package;
use std::time::{Duration, Instant};

struct EditorWin {
    id: ViewportId,
    ed: Box<EditorState>,
    title: String,
}

pub struct App {
    prefs: Prefs,
    rec: Recorder,
    editors: Vec<EditorWin>,
    next_editor: u64,
    render_state: eframe::egui_wgpu::RenderState,
    _hotkeys: Option<GlobalHotKeyManager>,
    hotkey_id: Option<u32>,
    /// Developer hooks: open a package, seek and save a screenshot, then quit.
    shot: Option<Shot>,
    /// `SCREENCRAFT_AUTORECORD=<seconds>`: record the main display, stop, report, quit.
    autorecord: Option<f64>,
    autorecord_started: bool,
    /// Without a window (screenshots and tests): the editor draws inline.
    headless: bool,
    /// The window size last asked for, so the bar is only moved when something changed.
    applied: Option<(Vec2, Phase, bool)>,
    area_picker: Option<crate::area::Picker>,
    show_permissions: bool,
    /// Highlight outline on the screen while hovering a window in the menu.
    hover_window: Option<screencraft_capture::WindowInfo>,
    compressor: Option<crate::compressor::Compressor>,
    pending_open: Vec<std::path::PathBuf>,
    /// Recordings just finished: their zooms are generated the first time they open.
    fresh: Vec<std::path::PathBuf>,
    quit: bool,
    discard_after_stop: bool,
    /// Native window handle (Windows), to keep the recording bar out of the capture.
    native_window: Option<isize>,
}

struct Shot {
    path: std::path::PathBuf,
    after_frames: u32,
    frame: u32,
    requested: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        crate::theme::install(&cc.egui_ctx);
        let render_state = cc.wgpu_render_state.clone().expect("the wgpu renderer is required");
        let mut app = Self::build(&cc.egui_ctx, render_state, true);
        crate::updates::startup_check(cc.egui_ctx.clone());
        app.show_permissions = cfg!(target_os = "macos") && !app.rec.permissions.screen;
        app
    }

    /// Without a window or global shortcuts: for the screenshot tool and tests.
    pub fn new_headless(ctx: &egui::Context, render_state: eframe::egui_wgpu::RenderState) -> Self {
        let mut a = Self::build(ctx, render_state, false);
        a.headless = true;
        a
    }

    pub fn open_package(&mut self, ctx: &egui::Context, dir: std::path::PathBuf) {
        // already open: nothing to do
        if self.editors.iter().any(|e| e.ed.package.dir == dir) {
            return;
        }
        match EditorState::open(Package::open(dir), ctx, self.render_state.clone()) {
            Ok(mut ed) => {
                if let Some(i) = self.fresh.iter().position(|d| *d == ed.package.dir) {
                    self.fresh.remove(i);
                    if ed.project.scene.zooms.is_empty() {
                        ed.auto_zoom();
                        ed.save_now();
                    }
                }
                self.next_editor += 1;
                let title = ed.project.name.clone();
                self.editors.push(EditorWin { id: ViewportId::from_hash_of(("editor", self.next_editor)), ed: Box::new(ed), title });
            }
            Err(e) => self.rec.show_error(e),
        }
    }

    /// Show the compact recording bar as if a recording were running (screenshots only).
    pub fn dev_bar(&mut self) {
        self.rec.phase = Phase::Idle;
    }

    pub fn dev_hud(&mut self) {
        self.rec.phase = Phase::Recording;
        self.rec.started_at = Some(Instant::now() - Duration::from_secs(83));
    }

    pub fn dev_menu(&mut self, which: &str) {
        let items = match which {
            "quality" => self.rec.menu_for_dev(crate::recorder::MenuKind::Quality, &self.prefs),
            "sys" => self.rec.menu_for_dev(crate::recorder::MenuKind::Sys, &self.prefs),
            "folder" => self.rec.menu_for_dev(crate::recorder::MenuKind::Folder, &self.prefs),
            _ => return,
        };
        self.rec.menu = Some(items);
    }

    pub fn dev_seek(&mut self, ms: f64) {
        if let Some(e) = self.editors.first_mut() {
            e.ed.seek(ms);
        }
    }

    pub fn dev_select(&mut self, what: Option<&str>) {
        if let Some(e) = self.editors.first_mut() {
            e.ed.apply_dev_selection(what);
        }
    }

    fn build(ctx: &egui::Context, render_state: eframe::egui_wgpu::RenderState, hotkeys: bool) -> Self {
        let prefs = Prefs::load();
        let (mgr, id) = match if hotkeys { GlobalHotKeyManager::new() } else { Err(global_hotkey::Error::AlreadyRegistered(HotKey::new(None, Code::KeyA))) } {
            Ok(m) => {
                let mods = if cfg!(target_os = "macos") { Modifiers::SUPER | Modifiers::SHIFT } else { Modifiers::CONTROL | Modifiers::SHIFT };
                let hk = HotKey::new(Some(mods), Code::KeyR);
                let id = hk.id();
                let ok = m.register(hk).is_ok();
                (Some(m), ok.then_some(id))
            }
            Err(_) => (None, None),
        };
        let mut rec = Recorder::new(ctx, &prefs);
        rec.permissions = screencraft_capture::permissions();
        let mut app = Self {
            rec,
            prefs,
            editors: Vec::new(),
            next_editor: 0,
            render_state,
            _hotkeys: mgr,
            hotkey_id: id,
            autorecord: std::env::var("SCREENCRAFT_AUTORECORD").ok().and_then(|v| v.parse().ok()),
            autorecord_started: false,
            headless: false,
            applied: None,
            area_picker: None,
            show_permissions: false,
            hover_window: None,
            compressor: None,
            pending_open: Vec::new(),
            fresh: Vec::new(),
            quit: false,
            discard_after_stop: false,
            native_window: None,
            shot: std::env::var_os("SCREENCRAFT_SHOT").map(|p| Shot {
                path: p.into(),
                after_frames: std::env::var("SCREENCRAFT_SHOT_FRAMES").ok().and_then(|v| v.parse().ok()).unwrap_or(40),
                frame: 0,
                requested: false,
            }),
        };
        if let Some(dir) = std::env::var_os("SCREENCRAFT_OPEN") {
            app.open_package(ctx, dir.into());
            if let Ok(ms) = std::env::var("SCREENCRAFT_SEEK").unwrap_or_default().parse::<f64>() {
                app.dev_seek(ms);
            }
            app.dev_select(std::env::var("SCREENCRAFT_SELECT").ok().as_deref());
        }
        app
    }

    /// Save a screenshot of the window to a PNG once things have settled (developer hook).
    fn dev_screenshot(&mut self, ctx: &egui::Context) {
        let Some(shot) = self.shot.as_mut() else { return };
        shot.frame += 1;
        ctx.request_repaint();
        if !shot.requested && shot.frame >= shot.after_frames {
            shot.requested = true;
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::UserData::default()));
        }
        let image = ctx.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = image {
            let (w, h) = (img.width() as u32, img.height() as u32);
            let mut raw = Vec::with_capacity((w * h * 4) as usize);
            for c in &img.pixels {
                raw.extend_from_slice(&c.to_array());
            }
            if let Ok(f) = std::fs::File::create(&shot.path) {
                let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
                enc.set_color(png::ColorType::Rgba);
                enc.set_depth(png::BitDepth::Eight);
                if let Ok(mut wr) = enc.write_header() {
                    let _ = wr.write_image_data(&raw);
                }
            }
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }

    // ---- recording flow ---------------------------------------------------------------------

    /// Press Record: checks, then the 3-2-1 countdown, then capture starts.
    fn begin_recording(&mut self, ctx: &egui::Context) {
        if self.rec.phase != Phase::Idle {
            return;
        }
        if !self.rec.permissions.screen {
            self.show_permissions = true;
            return;
        }
        if self.rec.mode == Mode::Window && self.rec.window.is_none() {
            self.rec.show_error("Choose a window to record (click Window).");
            return;
        }
        if self.rec.mode == Mode::Area && self.rec.area.is_none() {
            self.pick_area(ctx);
            return;
        }
        self.rec.close_menu();
        self.rec.phase = Phase::Countdown;
        self.rec.countdown_at = Some(Instant::now());
        ctx.request_repaint();
    }

    fn start_capture(&mut self, ctx: &egui::Context) {
        match self.rec.build_options(&self.prefs) {
            Ok(opts) => {
                self.prefs.save();
                self.rec.phase = Phase::Starting;
                self.rec.controller.start(opts);
            }
            Err(e) => {
                self.rec.phase = Phase::Idle;
                self.rec.show_error(e);
            }
        }
        ctx.request_repaint();
    }

    fn finish_recording(&mut self, ctx: &egui::Context, discard: bool) {
        match self.rec.phase {
            Phase::Countdown => {
                self.rec.phase = Phase::Idle;
            }
            Phase::Recording | Phase::Confirm => {
                self.discard_after_stop = discard;
                self.rec.phase = Phase::Finishing;
                self.rec.controller.stop();
            }
            _ => {}
        }
        ctx.request_repaint();
    }

    fn pick_area(&mut self, ctx: &egui::Context) {
        let displays = self.rec.displays().to_vec();
        let d = displays.iter().find(|d| Some(d.id) == self.rec.display).or(displays.first()).cloned();
        self.area_picker = Some(crate::area::Picker::new(d));
        ctx.request_repaint();
    }

    fn pump_events(&mut self, ctx: &egui::Context) {
        while let Ok(evt) = self.rec.controller.rx.try_recv() {
            match evt {
                Evt::Started { stats, width, height, encoder } => {
                    self.rec.stats = Some(stats);
                    self.rec.started_at = Some(Instant::now());
                    self.rec.size = (width, height);
                    self.rec.encoder = encoder;
                    self.rec.phase = Phase::Recording;
                }
                Evt::StartFailed(e) => {
                    self.rec.phase = Phase::Idle;
                    self.rec.show_error(e);
                }
                Evt::Stopped(r) => {
                    self.rec.phase = Phase::Idle;
                    self.rec.stats = None;
                    self.rec.started_at = None;
                    let discard = std::mem::take(&mut self.discard_after_stop);
                    if self.autorecord.is_some() {
                        match &r {
                            Ok(f) => eprintln!("[autorecord] finished: {} frames, {:.2}s, encoder {}, package {}", f.frames, f.duration_ms / 1000.0, f.encoder, f.package.dir.display()),
                            Err(e) => eprintln!("[autorecord] failed: {e}"),
                        }
                        ctx.send_viewport_cmd(ViewportCommand::Close);
                    }
                    match r {
                        Ok(f) if discard => {
                            let _ = std::fs::remove_dir_all(&f.package.dir);
                        }
                        Ok(f) => {
                            self.fresh.push(f.package.dir.clone());
                            self.pending_open.push(f.package.dir.clone());
                        }
                        Err(e) => self.rec.show_error(e),
                    }
                }
            }
        }
        // Global shortcut.
        if let Some(id) = self.hotkey_id {
            while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                if ev.id == id && ev.state == HotKeyState::Pressed {
                    match self.rec.phase {
                        Phase::Idle => self.begin_recording(ctx),
                        Phase::Recording => self.finish_recording(ctx, false),
                        Phase::Countdown => self.finish_recording(ctx, false),
                        _ => {}
                    }
                }
            }
        }
        // countdown done
        if self.rec.phase == Phase::Countdown {
            if self.rec.countdown_at.map(|t| t.elapsed() >= Duration::from_secs(3)).unwrap_or(false) {
                self.start_capture(ctx);
            }
        }
    }

    fn handle_action(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::None => {}
            Action::Record => self.begin_recording(ctx),
            Action::Stop => self.finish_recording(ctx, false),
            Action::Cancel => self.finish_recording(ctx, false),
            Action::AskDiscard => self.rec.phase = Phase::Confirm,
            Action::KeepRecording => self.rec.phase = Phase::Recording,
            Action::Discard => self.finish_recording(ctx, true),
            Action::Quit => {
                if !matches!(self.rec.phase, Phase::Recording | Phase::Countdown | Phase::Confirm) {
                    self.quit = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
            Action::OpenProject(dir) => self.pending_open.push(dir),
            Action::PickArea => self.pick_area(ctx),
            Action::HoverWindow(w) => self.hover_window = w,
            Action::ShowPermissions => self.show_permissions = true,
        }
    }

    /// Keep the bar's window the size the current phase wants, anchored at its bottom centre.
    fn place_bar(&mut self, ctx: &egui::Context) {
        let screen_w = self.rec.displays().iter().find(|d| d.is_main).map(|d| d.width as f32).unwrap_or(1440.0);
        let camera = self.prefs.camera;
        // While an editor is open the bar steps aside (a 1px window parked off screen keeps the
        // app's event loop running); it comes back when the last editor closes.
        if !self.editors.is_empty() && self.rec.phase == Phase::Idle {
            let key = (Vec2::splat(1.0), Phase::Idle, true);
            if self.applied != Some(key) {
                ctx.send_viewport_cmd(ViewportCommand::InnerSize(Vec2::splat(1.0)));
                ctx.send_viewport_cmd(ViewportCommand::OuterPosition(Pos2::new(-20000.0, -20000.0)));
                self.applied = Some(key);
            }
            return;
        }
        if matches!(self.applied, Some((v, _, true)) if v == Vec2::splat(1.0)) {
            self.applied = None;
        }
        let want = self.rec.window_size(screen_w, camera);
        let key = (want, self.rec.phase, self.rec.menu.is_some());
        if self.applied == Some(key) {
            return;
        }
        let (outer, first) = ctx.input(|i| (i.viewport().outer_rect, self.applied.is_none()));
        let docked = matches!(self.rec.phase, Phase::Idle | Phase::Starting);
        let pos = match (outer, first, docked) {
            (Some(o), false, _) => {
                // keep the bottom centre where it is
                let cx = o.center().x;
                let bottom = o.max.y;
                Pos2::new(cx - want.x / 2.0, bottom - want.y)
            }
            _ => {
                let d = self.rec.displays().iter().find(|d| d.is_main).cloned();
                let (dx, dy, dw, dh) = d.map(|d| (d.x as f32, d.y as f32, d.width as f32, d.height as f32)).unwrap_or((0.0, 0.0, 1440.0, 900.0));
                Pos2::new(dx + (dw - want.x) / 2.0, dy + dh - want.y - 90.0)
            }
        };
        // after a recording the bar returns to its dock
        let pos = if docked && matches!(self.applied, Some((_, p, _)) if !matches!(p, Phase::Idle | Phase::Starting)) {
            let d = self.rec.displays().iter().find(|d| d.is_main).cloned();
            let (dx, dy, dw, dh) = d.map(|d| (d.x as f32, d.y as f32, d.width as f32, d.height as f32)).unwrap_or((0.0, 0.0, 1440.0, 900.0));
            Pos2::new(dx + (dw - want.x) / 2.0, dy + dh - want.y - 90.0)
        } else {
            pos
        };
        ctx.send_viewport_cmd(ViewportCommand::InnerSize(want));
        ctx.send_viewport_cmd(ViewportCommand::OuterPosition(pos));
        self.applied = Some(key);
    }
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        #[cfg(windows)]
        if self.native_window.is_none() {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            if let Ok(h) = frame.window_handle() {
                if let RawWindowHandle::Win32(w) = h.as_raw() {
                    self.native_window = Some(w.hwnd.get());
                    screencraft_capture::set_excluded_from_capture(w.hwnd.get(), true);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = frame;
        self.pump_events(ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.draw(ui);
    }

    fn on_exit(&mut self) {
        for e in &mut self.editors {
            e.ed.pause();
            e.ed.save_now();
        }
        self.prefs.save();
    }
}

impl App {
    /// One frame of the whole application UI.
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.dev_screenshot(&ctx);
        self.rec.parked = !self.editors.is_empty();
        self.rec.refresh(&self.prefs);
        if let Some(secs) = self.autorecord {
            match self.rec.phase {
                Phase::Idle if !self.autorecord_started => {
                    self.autorecord_started = true;
                    self.start_capture(&ctx);
                }
                Phase::Recording if self.rec.elapsed_ms() >= secs * 1000.0 => self.finish_recording(&ctx, false),
                _ => {}
            }
            if self.rec.error.is_some() && self.rec.phase == Phase::Idle && self.autorecord_started {
                eprintln!("[autorecord] error: {}", self.rec.error.as_ref().map(|e| e.0.clone()).unwrap_or_default());
                ctx.send_viewport_cmd(ViewportCommand::Close);
            }
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if self.rec.phase == Phase::Idle && ctx.input(|i| i.key_pressed(egui::Key::Escape)) && self.rec.menu.is_some() {
            self.rec.close_menu();
        }
        if matches!(self.rec.phase, Phase::Countdown) && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.finish_recording(&ctx, false);
        }

        if self.headless {
            // screenshots and tests: the first editor fills the window, else the bar is drawn on a backdrop
            if let Some(e) = self.editors.first_mut() {
                let _ = e.ed.ui(ui);
                return;
            }
            let rect = ui.max_rect();
            ui.painter().rect_filled(rect, 0.0, crate::theme::BG);
            let bar_area = egui::Rect::from_min_size(Pos2::new(rect.center().x - BAR_W.min(rect.width() - 24.0) / 2.0, rect.max.y - 90.0 - BAR_H - if self.rec.menu.is_some() { 360.0 } else { 0.0 }), Vec2::new(BAR_W.min(rect.width() - 24.0), BAR_H + if self.rec.menu.is_some() { 360.0 } else { 0.0 }));
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(bar_area));
            let a = self.rec.show(&mut child, &mut self.prefs);
            self.handle_action(&ctx, a);
            return;
        }

        self.place_bar(&ctx);
        let a = egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| self.rec.show(ui, &mut self.prefs)).inner;
        self.handle_action(&ctx, a);

        // editors, one window each
        let mut open_requests: Vec<std::path::PathBuf> = Vec::new();
        let mut compress_requests: Vec<std::path::PathBuf> = Vec::new();
        let mut i = 0;
        while i < self.editors.len() {
            let (id, title) = (self.editors[i].id, self.editors[i].title.clone());
            let builder = ViewportBuilder::default().with_title(title).with_inner_size([1440.0, 900.0]).with_min_inner_size([980.0, 620.0]);
            let mut close = false;
            {
                let ed = &mut self.editors[i].ed;
                ctx.show_viewport_immediate(id, builder, |ui, _class| {
                    ed.ui(ui);
                    if ui.ctx().input(|i| i.viewport().close_requested()) {
                        if ed.dirty {
                            ui.ctx().send_viewport_cmd(ViewportCommand::CancelClose);
                            ed.close_prompt = Some(crate::editor::CloseIntent::Window);
                        } else {
                            close = true;
                        }
                    }
                    if let Some(req) = ed.open_request.take() {
                        open_requests.push(req);
                    }
                    if let Some(req) = ed.compress_request.take() {
                        compress_requests.push(req);
                    }
                });
            }
            let title = format!("{}{}", self.editors[i].ed.project.name, if self.editors[i].ed.dirty { " *" } else { "" });
            if title != self.editors[i].title {
                self.editors[i].title = title;
            }
            // the answer to a save prompt
            if let Some(ans) = self.editors[i].ed.close_answer.take() {
                use crate::editor::{Answer, CloseIntent};
                let intent = self.editors[i].ed.close_prompt.take();
                match ans {
                    Answer::Cancel => {}
                    Answer::Save | Answer::Discard => {
                        if ans == Answer::Save {
                            self.editors[i].ed.save_now();
                        } else {
                            self.editors[i].ed.dirty = false;
                        }
                        match intent {
                            Some(CloseIntent::Window) => close = true,
                            Some(CloseIntent::Open(p)) => {
                                self.editors[i].ed.close_after_open = true;
                                open_requests.push(p);
                            }
                            None => {}
                        }
                    }
                }
            }
            if close {
                let mut w = self.editors.remove(i);
                w.ed.pause();
                w.ed.save_now();
            } else {
                i += 1;
            }
        }
        if let Some(p) = compress_requests.pop() {
            self.compressor = Some(crate::compressor::Compressor::new(Some(p)));
        }
        if let Some(mut c) = self.compressor.take() {
            if c.show(&ctx) {
                self.compressor = Some(c);
            }
        }
        open_requests.extend(std::mem::take(&mut self.pending_open));
        for dir in open_requests {
            // opening another project replaces the ones on screen, asking about unsaved edits first
            if self.editors.iter().any(|e| e.ed.package.dir == dir) {
                continue;
            }
            if let Some(e) = self.editors.iter_mut().find(|e| e.ed.dirty && !e.ed.close_after_open) {
                e.ed.close_prompt = Some(crate::editor::CloseIntent::Open(dir));
                continue;
            }
            for mut w in self.editors.drain(..) {
                w.ed.pause();
            }
            self.open_package(&ctx, dir);
        }

        // area picker + frame outline
        if let Some(mut p) = self.area_picker.take() {
            match p.show(&ctx) {
                crate::area::Outcome::Pending => self.area_picker = Some(p),
                crate::area::Outcome::Picked(rect) => {
                    self.rec.area = Some(rect);
                    self.rec.mode = Mode::Area;
                    self.rec.display = p.display_id().or(self.rec.display);
                }
                crate::area::Outcome::Cancelled => {
                    if self.rec.area.is_none() {
                        self.rec.mode = Mode::Display;
                    }
                }
            }
        }
        let outline: Option<(PRect, &'static str)> = match (self.rec.phase, self.rec.mode, self.rec.area, &self.hover_window) {
            (Phase::Recording | Phase::Countdown | Phase::Starting | Phase::Confirm, Mode::Area, Some(a), _) => Some((a, "rec")),
            // a window or a whole display: outline it too, so it is clear what is being recorded
            // (the app's own windows are excluded from the capture, so this never appears in it)
            (Phase::Recording | Phase::Countdown | Phase::Starting | Phase::Confirm, Mode::Window, _, _) => self
                .rec
                .window
                .and_then(|id| self.rec.windows().iter().find(|w| w.id == id))
                .map(|w| (PRect { x: w.x, y: w.y, width: w.width, height: w.height }, "rec")),
            (Phase::Recording | Phase::Countdown | Phase::Starting | Phase::Confirm, Mode::Display, _, _) => {
                let ds = self.rec.displays();
                self.rec
                    .display
                    .and_then(|id| ds.iter().find(|d| d.id == id))
                    .or_else(|| ds.iter().find(|d| d.is_main))
                    .or(ds.first())
                    .map(|d| (PRect { x: d.x, y: d.y, width: d.width, height: d.height }, "rec"))
            }
            (Phase::Idle, _, _, Some(w)) => Some((PRect { x: w.x, y: w.y, width: w.width, height: w.height }, "hl")),
            (Phase::Idle, Mode::Area, Some(a), _) if self.area_picker.is_none() => Some((a, "sel")),
            _ => None,
        };
        if let Some((rect, style)) = outline {
            crate::area::draw_frame(&ctx, rect, style, self.rec.displays());
        }
        if self.show_permissions {
            self.show_permissions = crate::permissions::show(&ctx, &mut self.rec);
        }
        if self.quit {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
    }
}
