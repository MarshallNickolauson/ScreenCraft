//! Live inputs of the recorder: the camera picture shown in the bar (and fed to the recorder)
//! and the microphone level meter, started and stopped to match the current selections.

use crate::prefs::Prefs;
use crate::recorder::Phase;
use eframe::egui;
use screencraft_audio::{AudioBlock, AudioCapture, CaptureConfig, Clock};
use screencraft_capture::camera::{CamFrame, CameraHub, CameraStream};
use screencraft_capture::{create_capturer, CaptureRequest, Capturer, Event, Target};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Camera,
    Mic,
    Sys,
}

struct Started<T> {
    key: Option<String>,
    handle: T,
}

pub struct Live {
    ctx: egui::Context,
    hub: Arc<CameraHub>,
    camera: Option<Started<CameraStream>>,
    camera_starting: Arc<AtomicBool>,
    camera_slot: Arc<Mutex<Option<Result<Started<CameraStream>, String>>>>,
    tex: Option<egui::TextureHandle>,
    last_frame: usize,
    mic: Option<Started<AudioCapture>>,
    mic_starting: Arc<AtomicBool>,
    mic_slot: Arc<Mutex<Option<Result<Started<AudioCapture>, String>>>>,
    mic_level: Arc<AtomicU32>,
    /// Level meter for the computer's own audio: a tiny capture that only listens.
    sys: Option<Box<dyn Capturer>>,
    sys_starting: Arc<AtomicBool>,
    sys_slot: Arc<Mutex<Option<Result<Box<dyn Capturer>, String>>>>,
    sys_level: Arc<AtomicU32>,
    /// The meter could not start; do not retry until the input is switched off and on again.
    sys_failed: bool,
    error: Arc<Mutex<Option<String>>>,
}

fn set_peak(slot: &AtomicU32, v: f32) {
    let old = f32::from_bits(slot.load(Ordering::Relaxed));
    let new = if v > old { v } else { old * 0.92 + v * 0.08 };
    slot.store(new.to_bits(), Ordering::Relaxed);
}

impl Live {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            ctx: ctx.clone(),
            hub: CameraHub::new(),
            camera: None,
            camera_starting: Arc::new(AtomicBool::new(false)),
            camera_slot: Arc::new(Mutex::new(None)),
            tex: None,
            last_frame: 0,
            mic: None,
            mic_starting: Arc::new(AtomicBool::new(false)),
            mic_slot: Arc::new(Mutex::new(None)),
            mic_level: Arc::new(AtomicU32::new(0)),
            sys: None,
            sys_starting: Arc::new(AtomicBool::new(false)),
            sys_slot: Arc::new(Mutex::new(None)),
            sys_level: Arc::new(AtomicU32::new(0)),
            sys_failed: false,
            error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn camera_source(&self) -> Arc<CameraHub> {
        self.hub.clone()
    }

    pub fn take_error(&mut self) -> Option<String> {
        self.error.lock().unwrap().take()
    }

    pub fn loading(&self, which: Input, prefs: &Prefs) -> bool {
        match which {
            Input::Camera => prefs.camera && self.camera_starting.load(Ordering::SeqCst),
            Input::Mic => prefs.microphone && self.mic_starting.load(Ordering::SeqCst),
            Input::Sys => false,
        }
    }

    pub fn any_loading(&self, prefs: &Prefs) -> bool {
        self.loading(Input::Camera, prefs) || self.loading(Input::Mic, prefs)
    }

    pub fn levels(&self) -> (f32, f32) {
        (f32::from_bits(self.mic_level.load(Ordering::Relaxed)), f32::from_bits(self.sys_level.load(Ordering::Relaxed)))
    }

    /// Start or stop the camera and microphone meter so they match `prefs` and the phase.
    pub fn sync(&mut self, prefs: &Prefs, phase: Phase, parked: bool) {
        // finished background starts
        if let Some(r) = self.camera_slot.lock().unwrap().take() {
            match r {
                Ok(s) => self.camera = Some(s),
                Err(e) => *self.error.lock().unwrap() = Some(e),
            }
        }
        if let Some(r) = self.mic_slot.lock().unwrap().take() {
            match r {
                Ok(s) => self.mic = Some(s),
                Err(e) => *self.error.lock().unwrap() = Some(e),
            }
        }
        if let Some(r) = self.sys_slot.lock().unwrap().take() {
            // a meter that cannot start (no permission yet, say) just stays at zero
            match r {
                Ok(c) => self.sys = Some(c),
                Err(_) => self.sys_failed = true,
            }
        }
        let in_use = matches!(phase, Phase::Idle | Phase::Countdown | Phase::Recording | Phase::Confirm | Phase::Starting);
        // camera
        let want_cam = prefs.camera && in_use;
        let cam_key = prefs.camera_id.clone();
        if let Some(c) = &self.camera {
            if !want_cam || c.key != cam_key {
                if let Some(c) = self.camera.take() {
                    c.handle.stop();
                }
                self.hub.clear();
                self.tex = None;
            }
        }
        if want_cam && self.camera.is_none() && !self.camera_starting.swap(true, Ordering::SeqCst) {
            let (slot, flag, hub, ctx, id) = (self.camera_slot.clone(), self.camera_starting.clone(), self.hub.clone(), self.ctx.clone(), cam_key.clone());
            std::thread::spawn(move || {
                let h2 = hub.clone();
                let ctx2 = ctx.clone();
                let r = screencraft_capture::camera::start(id.as_deref(), move |f: CamFrame| {
                    h2.publish(f);
                    ctx2.request_repaint();
                })
                .map(|handle| Started { key: id, handle });
                *slot.lock().unwrap() = Some(r);
                flag.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        // microphone meter (the recording reads the microphone itself while it runs)
        let want_mic = prefs.microphone && phase == Phase::Idle;
        let mic_key = prefs.microphone_id.clone();
        if let Some(m) = &self.mic {
            if !want_mic || m.key != mic_key {
                if let Some(m) = self.mic.take() {
                    m.handle.stop();
                }
                self.mic_level.store(0, Ordering::Relaxed);
            }
        }
        if want_mic && self.mic.is_none() && !self.mic_starting.swap(true, Ordering::SeqCst) {
            let (slot, flag, level, ctx, id) = (self.mic_slot.clone(), self.mic_starting.clone(), self.mic_level.clone(), self.ctx.clone(), mic_key);
            std::thread::spawn(move || {
                let (tx, rx) = std::sync::mpsc::channel::<AudioBlock>();
                let clock = Clock::start();
                let cfg = CaptureConfig { microphone: id.clone(), use_microphone: true, system: None, use_system: false };
                let r = AudioCapture::start(&cfg, &clock, tx).map(|handle| Started { key: id, handle });
                if r.is_ok() {
                    let ctx2 = ctx.clone();
                    std::thread::spawn(move || {
                        while let Ok(b) = rx.recv() {
                            set_peak(&level, screencraft_audio::mix::peak(&b.data));
                            ctx2.request_repaint_after(std::time::Duration::from_millis(40));
                        }
                    });
                }
                *slot.lock().unwrap() = Some(r);
                flag.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
        // system-audio meter: only while the bar is showing and the input is on
        let want_sys = prefs.system_audio && phase == Phase::Idle && !parked;
        if !want_sys {
            self.sys_failed = false;
            if let Some(mut c) = self.sys.take() {
                c.stop();
                self.sys_level.store(0, Ordering::Relaxed);
            }
        }
        if want_sys && self.sys.is_none() && !self.sys_failed && !self.sys_starting.swap(true, Ordering::SeqCst) {
            let (slot, flag, level, ctx) = (self.sys_slot.clone(), self.sys_starting.clone(), self.sys_level.clone(), self.ctx.clone());
            std::thread::spawn(move || {
                let (tx, rx) = std::sync::mpsc::sync_channel::<Event>(4);
                let mut cap = create_capturer();
                let req = CaptureRequest { target: Target::Display(None), fps: 5, max_long_side: Some(64), system_audio: true };
                let r = match cap.start(&req, Clock::start(), tx) {
                    Ok(info) if info.provides_system_audio => {
                        let ctx2 = ctx.clone();
                        std::thread::spawn(move || {
                            while let Ok(ev) = rx.recv() {
                                match ev {
                                    Event::SystemAudio(b) => {
                                        set_peak(&level, screencraft_audio::mix::peak(&b.data));
                                        ctx2.request_repaint_after(std::time::Duration::from_millis(40));
                                    }
                                    Event::Frame(_) => {}
                                    Event::Ended(_) => break,
                                }
                            }
                        });
                        Ok(cap)
                    }
                    Ok(_) => {
                        cap.stop();
                        Err("no system-audio meter on this platform".to_string())
                    }
                    Err(e) => Err(e),
                };
                *slot.lock().unwrap() = Some(r);
                flag.store(false, Ordering::SeqCst);
                ctx.request_repaint();
            });
        }
    }

    /// The current camera picture as a small square texture for the bar.
    pub fn camera_texture(&mut self) -> Option<egui::TextureId> {
        let f = self.hub.latest()?;
        let ptr = Arc::as_ptr(&f) as usize;
        if ptr != self.last_frame || self.tex.is_none() {
            self.last_frame = ptr;
            const S: usize = 96;
            // centre crop to a square, then sample down
            let side = f.width.min(f.height) as usize;
            let (ox, oy) = ((f.width as usize - side) / 2, (f.height as usize - side) / 2);
            let mut px = Vec::with_capacity(S * S);
            for y in 0..S {
                let sy = oy + y * side / S;
                for x in 0..S {
                    let sx = ox + x * side / S;
                    let o = (sy * f.width as usize + sx) * 3;
                    px.push(egui::Color32::from_rgb(f.rgb[o], f.rgb[o + 1], f.rgb[o + 2]));
                }
            }
            let img = egui::ColorImage::new([S, S], px);
            match &mut self.tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.tex = Some(self.ctx.load_texture("camera-preview", img, egui::TextureOptions::LINEAR)),
            }
        }
        self.tex.as_ref().map(|t| t.id())
    }
}
