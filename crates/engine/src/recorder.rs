//! One recording session.
//!
//! ```text
//!  capture backend ──frames──┐                    ┌─ video: BGRA → I420 → CFR → encoder
//!  system audio (SCK) ───────┼──► worker thread ──┤
//!  microphone / loopback ────┘   (one timeline)   └─ audio: align → 48 kHz → Opus
//!  input tracker (cursor, clicks, keys) ───────────────► input.json
//! ```
//!
//! Everything is stamped on one `Clock`; time zero is the first video frame.

use crate::package::Package;
use crossbeam_channel::{select, unbounded, Receiver, Sender};
use screencraft_audio::{AudioBlock, AudioCapture, CaptureConfig, Clock, Source, StreamAligner};
use screencraft_capture::cfr::Cfr;
use screencraft_capture::input::InputTracker;
use screencraft_capture::pixels::crop_scale;
use screencraft_capture::{create_capturer, CameraCapturer, CaptureRequest, Capturer, Event, StartInfo, Target};
use screencraft_core::project::{Media, Project, Rect};
use screencraft_encode::color::{default_threads, to_i420, PixelOrder};
use screencraft_encode::{
    AudioConfig, Container, Effort, ExportSettings, FileSink, HwPreference, ThreadedExporter, VideoCodec, VideoConfig, I420,
};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug)]
pub struct RecordOptions {
    pub target: Target,
    pub fps: u32,
    pub max_long_side: Option<u32>,
    pub system_audio: bool,
    pub microphone: bool,
    pub microphone_id: Option<String>,
    pub output: Package,
    pub camera: Option<CameraSpec>,
    /// `None` picks a bitrate from the resolution. Recordings are an editing master, so this
    /// is intentionally high.
    pub bitrate_kbps: Option<u32>,
    pub hardware: HwPreference,
}

impl RecordOptions {
    /// A talking-head video: the camera is the picture and there is no screen.
    pub fn camera_only(&self) -> bool {
        matches!(self.target, Target::Camera { .. })
    }
}

/// The camera bubble: the hub that fans the live camera out, shared with the recorder's preview.
#[derive(Clone)]
pub struct CameraSpec {
    pub id: Option<String>,
    pub hub: Arc<screencraft_capture::camera::CameraHub>,
}

impl std::fmt::Debug for CameraSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "CameraSpec({:?})", self.id)
    }
}

/// Counters the UI can poll while recording.
#[derive(Default)]
pub struct LiveStats {
    pub frames: AtomicU64,
    pub repeated_frames: AtomicU64,
    mic_peak: AtomicU32,
    system_peak: AtomicU32,
}

impl LiveStats {
    pub fn mic_peak(&self) -> f32 {
        f32::from_bits(self.mic_peak.load(Ordering::Relaxed))
    }
    pub fn system_peak(&self) -> f32 {
        f32::from_bits(self.system_peak.load(Ordering::Relaxed))
    }
    fn set_peak(slot: &AtomicU32, v: f32) {
        // Fast attack, slow release, so meters are readable at UI frame rates.
        let old = f32::from_bits(slot.load(Ordering::Relaxed));
        let new = if v > old { v } else { old * 0.92 + v * 0.08 };
        slot.store(new.to_bits(), Ordering::Relaxed);
    }
}

pub struct Finished {
    pub package: Package,
    pub project: Project,
    pub duration_ms: f64,
    pub encoder: String,
    pub frames: u64,
}

enum Ctl {
    Stop { end_ns: u64 },
}

pub struct Recording {
    camera: Option<crate::camera_rec::CameraRecorder>,
    capturer: Box<dyn Capturer>,
    audio: Option<AudioCapture>,
    tracker: Option<InputTracker>,
    ctl: Sender<Ctl>,
    worker: Option<JoinHandle<Result<WorkerResult, String>>>,
    clock: Clock,
    info: StartInfo,
    pub stats: Arc<LiveStats>,
    started_unix_ms: f64,
    opts: RecordOptions,
    first_frame: Arc<Mutex<Option<u64>>>,
    encoder_name: String,
}

struct WorkerResult {
    frames: u64,
    duration_ms: f64,
    origin_ns: u64,
}

struct Tracks {
    system: Option<usize>,
    mic: Option<usize>,
}

fn unix_ms() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
}

impl Recording {
    /// Start capturing. Returns once frames are flowing (or an error explains why not).
    pub fn start(opts: RecordOptions) -> Result<Self, String> {
        let capturer: Box<dyn Capturer> = match &opts.target {
            Target::Camera { .. } => {
                let hub = opts.camera.as_ref().map(|c| c.hub.clone()).ok_or("turn the camera on to record a talking-head video")?;
                Box::new(CameraCapturer::new(hub))
            }
            _ => create_capturer(),
        };
        Self::start_with(opts, capturer)
    }

    /// Like `start`, with a caller-supplied capture backend (tests, synthetic sources).
    pub fn start_with(opts: RecordOptions, capturer: Box<dyn Capturer>) -> Result<Self, String> {
        let dir = opts.output.dir.clone();
        let r = Self::start_inner(opts, capturer);
        if r.is_err() {
            // Don't leave an empty package folder behind after a failed start.
            let _ = std::fs::remove_file(dir.join("screen.mp4"));
            let _ = std::fs::remove_dir(&dir);
        }
        r
    }

    fn start_inner(mut opts: RecordOptions, mut capturer: Box<dyn Capturer>) -> Result<Self, String> {
        let camera_only = matches!(opts.target, Target::Camera { .. });
        if camera_only {
            // nothing plays "on the computer" in a talking-head video
            opts.system_audio = false;
        }
        std::fs::create_dir_all(&opts.output.dir).map_err(|e| e.to_string())?;
        let clock = Clock::start();
        let (cap_tx, cap_rx) = crossbeam_channel::bounded::<Event>(24);
        // The capture API wants a std sync sender; bridge it to crossbeam.
        let (std_tx, std_rx) = std::sync::mpsc::sync_channel::<Event>(24);
        let bridge = std::thread::Builder::new()
            .name("screencraft-bridge".into())
            .spawn(move || {
                while let Ok(ev) = std_rx.recv() {
                    if cap_tx.send(ev).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let _ = bridge;

        let req = CaptureRequest {
            target: opts.target.clone(),
            fps: opts.fps,
            max_long_side: opts.max_long_side,
            system_audio: opts.system_audio,
        };
        // no cursor, clicks or keys to log when the camera is the picture
        let tracker = (!camera_only).then(|| {
            InputTracker::start(match &opts.target {
                Target::Display(d) => *d,
                Target::Area { display, .. } => *display,
                Target::Window(_) | Target::Camera { .. } => None,
            })
        });
        let info = match capturer.start(&req, clock.clone(), std_tx) {
            Ok(i) => i,
            Err(e) => {
                // Drop the tracker's threads cleanly.
                if let Some(t) = tracker {
                    let _ = t.finish(clock.epoch(), Rect::default());
                }
                return Err(e);
            }
        };

        // Audio devices: the microphone everywhere; loopback where the capture API can't.
        let (audio_tx, audio_rx) = unbounded::<AudioBlock>();
        let (a_std_tx, a_std_rx) = std::sync::mpsc::channel::<AudioBlock>();
        let _ = std::thread::Builder::new().name("screencraft-audio-bridge".into()).spawn(move || {
            while let Ok(b) = a_std_rx.recv() {
                if audio_tx.send(b).is_err() {
                    break;
                }
            }
        });
        let want_loopback = opts.system_audio && !info.provides_system_audio;
        let audio = if opts.microphone || want_loopback {
            let cfg = CaptureConfig {
                microphone: opts.microphone_id.clone(),
                use_microphone: opts.microphone,
                system: None,
                use_system: want_loopback,
            };
            match AudioCapture::start(&cfg, &clock, a_std_tx) {
                Ok(a) => Some(a),
                Err(e) => {
                    capturer.stop();
                    if let Some(t) = tracker {
                        let _ = t.finish(clock.epoch(), Rect::default());
                    }
                    return Err(e);
                }
            }
        } else {
            None
        };

        // Encoder: H.264 master with short GOPs so the editor can seek quickly.
        let (w, h) = (info.width, info.height);
        let mut video = VideoConfig::new(VideoCodec::H264, w, h, opts.fps);
        video.bitrate_kbps = opts.bitrate_kbps.unwrap_or_else(|| {
            // Roughly 1.5x the delivery bitrate: an editing master.
            (screencraft_encode::default_bitrate_kbps(w, h, opts.fps) * 3 / 2).min(100_000)
        });
        video.keyint = (opts.fps / 2).max(1);
        video.hardware = opts.hardware;
        video.effort = Effort::Fast;
        let wants_system = opts.system_audio;
        let wants_mic = opts.microphone;
        let mut audio_cfg = Vec::new();
        let mut tracks = Tracks { system: None, mic: None };
        if wants_system {
            tracks.system = Some(audio_cfg.len());
            audio_cfg.push(AudioConfig::default());
        }
        if wants_mic {
            tracks.mic = Some(audio_cfg.len());
            audio_cfg.push(AudioConfig::default());
        }
        let screen_path = opts.output.dir.join("screen.mp4");
        let sink = FileSink::create(&screen_path).map_err(|e| e.to_string())?;
        let exporter = ThreadedExporter::spawn(
            ExportSettings { container: Container::Mp4, video: video.clone(), audio: audio_cfg },
            Box::new(sink),
        )
        .map_err(|e| e.to_string())?;
        let encoder_name = screencraft_encode::create_video_encoder(&video)
            .map(|e| e.name().to_string())
            .unwrap_or_else(|_| "H.264".into());

        let stats = Arc::new(LiveStats::default());
        let first_frame = Arc::new(Mutex::new(None));
        let (ctl_tx, ctl_rx) = unbounded::<Ctl>();
        let worker = {
            let (stats, first) = (stats.clone(), first_frame.clone());
            let fps = opts.fps;
            let (cw, ch) = (info.width, info.height);
            std::thread::Builder::new()
                .name("screencraft-record".into())
                .spawn(move || worker_loop(cap_rx, audio_rx, ctl_rx, exporter, tracks, fps, (cw, ch), stats, first))
                .map_err(|e| e.to_string())?
        };

        let camera = opts.camera.as_ref().filter(|_| !camera_only).and_then(|c| {
            match crate::camera_rec::CameraRecorder::start(c.hub.clone(), opts.output.dir.join("camera.mp4"), clock.clone(), opts.hardware) {
                Ok(r) => Some(r),
                Err(e) => {
                    eprintln!("[screencraft] camera not recorded: {e}");
                    None
                }
            }
        });
        let mut rec = Self {
            camera,
            capturer,
            audio,
            tracker,
            ctl: ctl_tx,
            worker: Some(worker),
            clock,
            info,
            stats,
            started_unix_ms: unix_ms(),
            opts,
            first_frame,
            encoder_name,
        };

        // Wait for the first frame so the caller knows capture is really running.
        let deadline = std::time::Instant::now() + Duration::from_secs(6);
        while rec.first_frame.lock().unwrap().is_none() {
            if std::time::Instant::now() > deadline {
                let _ = rec.abort();
                return Err("screen capture produced no frames (check the Screen Recording permission)".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(rec)
    }

    pub fn info(&self) -> &StartInfo {
        &self.info
    }

    pub fn encoder(&self) -> &str {
        &self.encoder_name
    }

    pub fn elapsed_ms(&self) -> f64 {
        match *self.first_frame.lock().unwrap() {
            Some(t0) => self.clock.now_ns().saturating_sub(t0) as f64 / 1e6,
            None => 0.0,
        }
    }

    fn abort(&mut self) -> Result<(), String> {
        if let Some(c) = self.camera.take() {
            let _ = c.finish(0);
            let _ = std::fs::remove_file(self.opts.output.dir.join("camera.mp4"));
        }
        self.capturer.stop();
        if let Some(a) = self.audio.take() {
            a.stop();
        }
        let end_ns = self.clock.now_ns();
        let _ = self.ctl.send(Ctl::Stop { end_ns });
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        if let Some(t) = self.tracker.take() {
            let _ = t.finish(self.clock.epoch(), Rect::default());
        }
        Ok(())
    }

    /// Stop, flush everything to disk and write the project.
    pub fn stop(mut self) -> Result<Finished, String> {
        self.capturer.stop();
        if let Some(a) = self.audio.take() {
            a.stop();
        }
        let end_ns = self.clock.now_ns();
        self.ctl.send(Ctl::Stop { end_ns }).map_err(|_| "recorder stopped unexpectedly".to_string())?;
        let result = self
            .worker
            .take()
            .ok_or("already stopped")?
            .join()
            .map_err(|_| "recorder thread panicked".to_string())??;

        let camera_done = self.camera.take().and_then(|c| c.finish(result.origin_ns));
        let zero = self.clock.epoch() + Duration::from_nanos(result.origin_ns);
        let log = self.tracker.take().map(|t| t.finish(zero, self.info.bounds)).unwrap_or_default();
        let json = serde_json::to_vec(&log).map_err(|e| e.to_string())?;
        std::fs::write(self.opts.output.dir.join("input.json"), json).map_err(|e| e.to_string())?;

        let (mut sys_track, mut mic_track) = (None, None);
        let mut next = 0u32;
        if self.opts.system_audio {
            sys_track = Some(next);
            next += 1;
        }
        if self.opts.microphone {
            mic_track = Some(next);
        }
        let media = Media {
            duration_ms: result.duration_ms,
            width: self.info.width,
            height: self.info.height,
            bounds: self.info.bounds,
            has_system_audio: self.opts.system_audio,
            has_microphone: self.opts.microphone,
            system_track: sys_track,
            mic_track,
            camera: camera_done.as_ref().map(|_| "camera.mp4".to_string()),
            camera_offset_ms: camera_done.unwrap_or(0.0),
            ..Media::default()
        };
        let stem = self
            .opts
            .output
            .dir
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Recording".into());
        let mut project = if self.opts.camera_only() { Project::talking_head(&stem, media) } else { Project::from_recording(&stem, media) };
        project.id = format!("{:x}", self.started_unix_ms as u64);
        project.created_at = format!("{}", self.started_unix_ms as u64);
        self.opts.output.save_project(&project)?;
        Ok(Finished {
            package: self.opts.output.clone(),
            project,
            duration_ms: result.duration_ms,
            encoder: self.encoder_name.clone(),
            frames: result.frames,
        })
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.abort();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn worker_loop(
    cap_rx: Receiver<Event>,
    audio_rx: Receiver<AudioBlock>,
    ctl_rx: Receiver<Ctl>,
    exporter: ThreadedExporter,
    tracks: Tracks,
    fps: u32,
    size: (u32, u32),
    stats: Arc<LiveStats>,
    first_frame: Arc<Mutex<Option<u64>>>,
) -> Result<WorkerResult, String> {
    let threads = default_threads();
    let mut cfr: Cfr<Arc<I420>> = Cfr::new(fps);
    let mut origin: Option<u64> = None;
    let mut aligners: Vec<(Source, StreamAligner)> = Vec::new();
    let mut held_audio: Vec<AudioBlock> = Vec::new();
    let mut frames_sent = 0u64;
    let mut end_ns: Option<u64> = None;

    let track_of = |s: Source| match s {
        Source::System => tracks.system,
        Source::Microphone => tracks.mic,
    };

    // Feed one audio block through its aligner into the right encoder track.
    let feed = |aligners: &mut Vec<(Source, StreamAligner)>, b: AudioBlock, origin_ns: u64| -> Result<(), String> {
        let Some(track) = track_of(b.source) else { return Ok(()) };
        let peak = screencraft_audio::mix::peak(&b.data);
        match b.source {
            Source::Microphone => LiveStats::set_peak(&stats.mic_peak, peak),
            Source::System => LiveStats::set_peak(&stats.system_peak, peak),
        }
        let idx = match aligners.iter().position(|(s, _)| *s == b.source) {
            Some(i) => i,
            None => {
                aligners.push((b.source, StreamAligner::new(origin_ns, b.sample_rate, b.channels)));
                aligners.len() - 1
            }
        };
        let out = aligners[idx].1.push(b.pts_ns, &b.data);
        if !out.is_empty() {
            exporter.push_audio(track, out).map_err(|e| e.to_string())?;
        }
        Ok(())
    };

    let mut handle_frame = |cfr: &mut Cfr<Arc<I420>>,
                            origin: &mut Option<u64>,
                            f: screencraft_capture::Frame|
     -> Result<(), String> {
        let (w, h) = (f.width, f.height);
        let yuv = if (w, h) == size {
            to_i420(&f.bgra, w, h, w as usize * 4, PixelOrder::Bgra, threads)
        } else {
            // The source changed size mid-recording (window resized): fit it to the master size.
            let scaled = crop_scale(&f.bgra, w as usize * 4, (0, 0, w, h), size);
            to_i420(&scaled, size.0, size.1, size.0 as usize * 4, PixelOrder::Bgra, threads)
        };
        if origin.is_none() {
            *origin = Some(f.pts_ns);
            *first_frame.lock().unwrap() = Some(f.pts_ns);
        }
        let ready = cfr.push(f.pts_ns, Arc::new(yuv));
        for (slot, frame) in ready {
            let pts = cfr.slot_pts_us(slot);
            exporter.push_video(frame, pts).map_err(|e| e.to_string())?;
            frames_sent += 1;
        }
        stats.frames.store(frames_sent, Ordering::Relaxed);
        Ok(())
    };

    loop {
        select! {
            recv(cap_rx) -> ev => match ev {
                Ok(Event::Frame(f)) => handle_frame(&mut cfr, &mut origin, f)?,
                Ok(Event::SystemAudio(b)) => match origin {
                    Some(o) => feed(&mut aligners, b, o)?,
                    None => held_audio.push(b),
                },
                Ok(Event::Ended(_)) | Err(_) => { if end_ns.is_none() { end_ns = Some(u64::MAX); } break; }
            },
            recv(audio_rx) -> b => if let Ok(b) = b {
                match origin {
                    Some(o) => feed(&mut aligners, b, o)?,
                    None => held_audio.push(b),
                }
            },
            recv(ctl_rx) -> c => match c {
                Ok(Ctl::Stop { end_ns: e }) => { end_ns = Some(e); break; }
                Err(_) => break,
            },
        }
        // Audio that arrived before the first frame can be released now.
        if let Some(o) = origin {
            if !held_audio.is_empty() {
                for b in std::mem::take(&mut held_audio) {
                    feed(&mut aligners, b, o)?;
                }
            }
        }
    }

    // Drain whatever the producers queued before they were stopped.
    while let Ok(ev) = cap_rx.try_recv() {
        match ev {
            Event::Frame(f) => handle_frame(&mut cfr, &mut origin, f)?,
            Event::SystemAudio(b) => {
                if let Some(o) = origin {
                    feed(&mut aligners, b, o)?;
                }
            }
            Event::Ended(_) => {}
        }
    }
    while let Ok(b) = audio_rx.try_recv() {
        if let Some(o) = origin {
            feed(&mut aligners, b, o)?;
        }
    }

    let Some(origin_ns) = origin else {
        return Err("no frames were captured".into());
    };
    let end = end_ns.filter(|&e| e != u64::MAX).unwrap_or(origin_ns + (frames_sent * 1_000_000_000 / fps as u64));
    for (slot, frame) in cfr.finish(end) {
        let pts = cfr.slot_pts_us(slot);
        exporter.push_video(frame, pts).map_err(|e| e.to_string())?;
        frames_sent += 1;
    }
    // Pad every audio stream to the length of the video.
    let video_end_ns = origin_ns + cfr.slots() * 1_000_000_000 / fps as u64;
    for (src, al) in aligners.iter_mut() {
        let tail = al.finish(video_end_ns);
        if let (Some(t), false) = (track_of(*src), tail.is_empty()) {
            exporter.push_audio(t, tail).map_err(|e| e.to_string())?;
        }
    }
    let duration_ms = cfr.slots() as f64 * 1000.0 / fps as f64;
    exporter.finish().map_err(|e| e.to_string())?;
    Ok(WorkerResult { frames: frames_sent, duration_ms, origin_ns })
}
