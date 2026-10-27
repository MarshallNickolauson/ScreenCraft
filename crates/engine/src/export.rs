//! Render a project to a video file: decode -> compose on the GPU -> encode.

use crate::audio_mix::{Lane, Soundtrack};
use crate::package::Package;
use crate::source::AudioSource;
use screencraft_core::project::Project;
use screencraft_core::track::compute_track;
use screencraft_encode::color::{default_threads, to_i420, PixelOrder};
use screencraft_encode::mp4read::Mp4File;
use screencraft_encode::{
    AudioConfig, Container, Effort, ExportSettings, ExportSummary, FileSink, HwPreference, ThreadedExporter, VideoCodec,
    VideoConfig, I420,
};
use screencraft_render::{build_scene, build_scene_with, output_size, CamOverride, Compositor, Slot};
use std::path::Path;
use std::sync::mpsc::sync_channel;
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub container: Container,
    pub codec: VideoCodec,
    /// Length of the shorter output side in pixels.
    pub resolution: u32,
    pub fps: u32,
    pub bitrate_kbps: Option<u32>,
    pub effort: Effort,
    pub hardware: HwPreference,
    pub audio: bool,
    /// Encoder threads; 0 = automatic. The output bytes do not depend on this.
    pub threads: usize,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            container: Container::Mp4,
            codec: VideoCodec::H264,
            resolution: 1080,
            fps: 60,
            bitrate_kbps: None,
            effort: Effort::Balanced,
            hardware: HwPreference::Auto,
            audio: true,
            threads: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ExportReport {
    pub summary: ExportSummary,
    pub width: u32,
    pub height: u32,
    pub frames: usize,
}

struct MicInfo {
    gain: f32,
    /// Speech stretches in microphone time (ms).
    speech: Vec<[f64; 2]>,
}

fn mic_cache() -> &'static std::sync::Mutex<std::collections::HashMap<(std::path::PathBuf, u32, u64), std::sync::Arc<MicInfo>>> {
    static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<(std::path::PathBuf, u32, u64), std::sync::Arc<MicInfo>>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// Level and speech analysis of a microphone track, streamed so long recordings stay cheap:
/// the loudness gain the mix applies (quiet microphones are lifted, never turned down) and the
/// stretches where someone is talking (for ducking and idle detection).
fn analyse_mic(file: &Arc<Mp4File>, track: usize, key: (std::path::PathBuf, u32, u64)) -> Option<Arc<MicInfo>> {
    if let Some(v) = mic_cache().lock().unwrap().get(&key) {
        return Some(v.clone());
    }
    let mut src = AudioSource::open(file.clone(), track).ok()?;
    let total_ms = src.duration_ms;
    const WIN_MS: f64 = 50.0;
    let win = (RATE_HZ * WIN_MS / 1000.0) as usize;
    let mut rms: Vec<f32> = Vec::new();
    let mut lvl0: Vec<f32> = Vec::new();
    let mut peak = 0f32;
    let mut t = 0.0;
    while t < total_ms {
        let chunk_ms = 1000.0;
        let pcm = src.read(t, (RATE_HZ * chunk_ms / 1000.0) as usize).ok()?;
        for w in pcm.chunks_exact(win * 2) {
            let (mut sum_mono, mut sum_l) = (0f32, 0f32);
            for f in w.chunks_exact(2) {
                let m = (f[0] + f[1]) * 0.5;
                sum_mono += m * m;
                sum_l += f[0] * f[0];
                peak = peak.max(m.abs());
            }
            rms.push((sum_mono / win as f32).sqrt());
            lvl0.push((sum_l / win as f32).sqrt());
        }
        t += chunk_ms;
    }
    if rms.is_empty() {
        return None;
    }
    let mut sorted = rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let top = &sorted[(sorted.len() as f32 * 0.8) as usize..];
    let speech_level = top.iter().sum::<f32>() / top.len().max(1) as f32;
    let gain = if peak < 1e-5 { 1.0 } else { (0.1 / speech_level.max(1e-5)).min(0.9 / peak).clamp(1.0, 60.0) };
    // speech segments against the room's own noise floor (20th percentile)
    let lv: Vec<f32> = lvl0.iter().map(|v| v * gain).collect();
    let mut s2 = lv.clone();
    s2.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = s2[s2.len() / 5];
    let thr = (floor * 3.0).max(0.02);
    let (hold, bridge) = (6usize, 7usize);
    let mut segs: Vec<[f64; 2]> = Vec::new();
    let (mut start, mut last): (Option<usize>, usize) = (None, 0);
    for (i, v) in lv.iter().enumerate() {
        if *v <= thr {
            continue;
        }
        match start {
            None => start = Some(i),
            Some(s) if i - last > bridge => {
                segs.push([s as f64 * WIN_MS, (last + hold) as f64 * WIN_MS]);
                start = Some(i);
            }
            _ => {}
        }
        last = i;
    }
    if let Some(s) = start {
        segs.push([s as f64 * WIN_MS, (last + hold) as f64 * WIN_MS]);
    }
    segs.retain(|r| r[1] - r[0] >= 150.0);
    let info = Arc::new(MicInfo { gain, speech: segs });
    mic_cache().lock().unwrap().insert(key, info.clone());
    Some(info)
}

const RATE_HZ: f64 = 48_000.0;

/// A decoded audio file (0.3 recordings keep the microphone in its own `mic.m4a`) as a lane.
struct PcmFile {
    pcm: Arc<Vec<f32>>,
    /// The file's clock runs `offset_ms` ahead of the recording's.
    offset_ms: f64,
}

impl crate::audio_mix::PcmSource for PcmFile {
    fn read(&mut self, start_ms: f64, frames: usize) -> Vec<f32> {
        let first = ((start_ms + self.offset_ms) * RATE_HZ / 1000.0).round() as i64;
        let mut out = vec![0f32; frames * 2];
        for k in 0..frames {
            let i = first + k as i64;
            if i >= 0 && ((i as usize) * 2 + 1) < self.pcm.len() {
                let m = (self.pcm[i as usize * 2] + self.pcm[i as usize * 2 + 1]) * 0.5;
                out[k * 2] = m;
                out[k * 2 + 1] = m;
            }
        }
        out
    }
}

/// Mic playback is always centred: both channels are averaged and sent to both ears (many mics
/// deliver two channels with one of them empty).
struct MonoAvg(AudioSource);

impl crate::audio_mix::PcmSource for MonoAvg {
    fn read(&mut self, start_ms: f64, frames: usize) -> Vec<f32> {
        let mut pcm = self.0.read(start_ms, frames).unwrap_or_else(|_| vec![0.0; frames * 2]);
        for f in pcm.chunks_exact_mut(2) {
            let m = (f[0] + f[1]) * 0.5;
            f[0] = m;
            f[1] = m;
        }
        pcm
    }
}

fn music_cache() -> &'static std::sync::Mutex<std::collections::HashMap<(std::path::PathBuf, u64), Arc<Vec<f32>>>> {
    static C: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<(std::path::PathBuf, u64), Arc<Vec<f32>>>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// A music file decoded to 48 kHz stereo, cached by path and modification time.
pub fn decode_music(path: &Path) -> Option<Arc<Vec<f32>>> {
    let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0);
    let key = (path.to_path_buf(), mtime);
    if let Some(v) = music_cache().lock().unwrap().get(&key) {
        return Some(v.clone());
    }
    let pcm = Arc::new(screencraft_audio::decode_file(path).ok()?);
    music_cache().lock().unwrap().insert(key, pcm.clone());
    Some(pcm)
}

/// Speech stretches of the microphone as output-timeline ranges, through the clip list.
fn speech_output_ranges(speech: &[[f64; 2]], offset_ms: f64, slices: &[screencraft_core::project::Slice]) -> Vec<[f64; 2]> {
    let mut out: Vec<[f64; 2]> = Vec::new();
    let mut acc = 0.0;
    for s in slices {
        let scale = s.time_scale.max(1e-3);
        let len = (s.source_end_ms - s.source_start_ms).max(0.0) / scale;
        if !s.freeze {
            for r in speech {
                let lo = (r[0] - offset_ms).max(s.source_start_ms);
                let hi = (r[1] - offset_ms).min(s.source_end_ms);
                if hi > lo {
                    out.push([acc + (lo - s.source_start_ms) / scale, acc + (hi - s.source_start_ms) / scale]);
                }
            }
        }
        acc += len;
    }
    out.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
    let mut merged: Vec<[f64; 2]> = Vec::new();
    for r in out {
        match merged.last_mut() {
            Some(l) if r[0] - l[1] < 120.0 => l[1] = l[1].max(r[1]),
            _ => merged.push(r),
        }
    }
    merged
}

/// Where the microphone track speaks, in source milliseconds (for idle detection).
pub fn mic_speech(package: &Package, project: &Project) -> Option<Vec<[f64; 2]>> {
    let t = project.media.mic_track?;
    let path = package.screen_path(project);
    let file = Arc::new(Mp4File::open(&path).ok()?);
    let key = (path, t, std::fs::metadata(package.screen_path(project)).map(|m| m.len()).unwrap_or(0));
    let info = analyse_mic(&file, t as usize, key)?;
    Some(info.speech.iter().map(|r| [r[0] - project.media.mic_offset_ms, r[1] - project.media.mic_offset_ms]).collect())
}

/// The edited soundtrack of a project: every audio track of `screen.mp4` through the cuts,
/// speeds and volumes, plus music (fades, ducking under speech) and the click / typing effects.
/// `None` when nothing audible is in the project.
pub fn build_soundtrack(package: &Package, project: &Project, track: &screencraft_core::track::RenderTrack) -> Result<Option<Soundtrack>, String> {
    let a = &project.config.audio;
    let master = (a.volume.max(0.0) * 2.0) as f32;
    let recorded = project.media.has_system_audio || project.media.has_microphone || project.media.mic.is_some();
    let sc = &project.scene;
    let want_fx = a.click_sounds || a.key_sounds;
    if !recorded && sc.music_clips.is_empty() && !want_fx {
        return Ok(None);
    }
    let mut lanes = Vec::new();
    let mut mic: Option<Arc<MicInfo>> = None;
    if recorded {
        let screen_path = package.screen_path(project);
        let file = Arc::new(Mp4File::open(&screen_path).map_err(|e| e.to_string())?);
        let size = std::fs::metadata(&screen_path).map(|m| m.len()).unwrap_or(0);
        if let (Some(t), false) = (project.media.system_track, a.mute_system_audio) {
            if let Ok(src) = AudioSource::open(file.clone(), t as usize) {
                lanes.push(Lane { source: Box::new(src), gain: master * a.device_volume as f32 });
            }
        }
        if let (Some(t), false) = (project.media.mic_track, a.mute_microphone) {
            if let Ok(src) = AudioSource::open(file.clone(), t as usize) {
                mic = analyse_mic(&file, t as usize, (screen_path.clone(), t, size));
                let level = mic.as_ref().map(|m| m.gain).unwrap_or(1.0);
                lanes.push(Lane { source: Box::new(MonoAvg(src)), gain: master * a.mic_volume as f32 * level });
            }
        }
    }
    // a separate microphone file from a 0.3 recording
    if let (None, Some(rel), false) = (project.media.mic_track, project.media.mic.as_deref(), a.mute_microphone) {
        if let Some(pcm) = decode_music(&package.dir.join(rel)) {
            lanes.push(Lane { source: Box::new(PcmFile { pcm, offset_ms: project.media.mic_offset_ms }), gain: master * a.mic_volume as f32 * 4.0 });
        }
    }
    let mut extras = crate::audio_mix::Extras { music_gain: master * a.music_volume as f32, duck_level: a.duck_level as f32, ..Default::default() };
    for c in &sc.music_clips {
        if let Some(pcm) = decode_music(&package.dir.join(&c.file)) {
            extras.music.push(crate::audio_mix::MusicPlay { pcm, offset_ms: c.offset_ms, in_ms: c.in_ms, dur_ms: c.dur_ms, fade_in_ms: c.fade_in_ms, fade_out_ms: c.fade_out_ms });
        }
    }
    if a.duck_music {
        if let Some(m) = &mic {
            extras.duck_ranges = speech_output_ranges(&m.speech, project.media.mic_offset_ms, &sc.slices);
        }
    }
    let mut hits = Vec::new();
    if a.click_sounds {
        let set = crate::sfx::load_set("click", if a.click_style.is_empty() { "soft" } else { &a.click_style });
        let marks: Vec<(f64, u8)> = track.clicks.iter().map(|c| (c.t_out_ms, 0)).collect();
        let g = master * a.click_volume as f32;
        hits.extend(crate::sfx::place(&set, &marks).into_iter().map(|mut h| {
            h.gain *= g;
            h
        }));
    }
    if a.key_sounds {
        let set = crate::sfx::load_set("key", if a.key_style.is_empty() { "mechanical" } else { &a.key_style });
        let marks: Vec<(f64, u8)> = track.keys.iter().map(|k| (k.t_out_ms, k.kind)).collect();
        let g = master * a.key_volume as f32;
        hits.extend(crate::sfx::place(&set, &marks).into_iter().map(|mut h| {
            h.gain *= g;
            h
        }));
    }
    hits.sort_by_key(|h| h.start_frame);
    extras.hits = hits;
    if lanes.is_empty() && extras.music.is_empty() && extras.hits.is_empty() {
        return Ok(None);
    }
    Ok(Some(Soundtrack::new(project, lanes).with_extras(extras)))
}

type FrameRx = std::sync::mpsc::Receiver<Result<crate::source::Frames, String>>;

/// Decode the source pictures each output frame needs on their own thread (the decoders are not
/// Send, so they are created over there).
pub(crate) fn spawn_decoder(package: &Package, project: &Project, track: &screencraft_core::track::RenderTrack) -> Result<(FrameRx, std::thread::JoinHandle<()>), String> {
    let src_idx: Vec<(f64, Option<f64>)> = (0..track.count)
        .map(|i| (track.t_src[i], track.other_alpha.get(i).filter(|a| **a > 0.003).map(|_| track.other_t_src[i])))
        .collect();
    let (ftx, frx) = sync_channel::<Result<crate::source::Frames, String>>(4);
    let (dp, dproj) = (package.clone(), project.clone());
    let decoder = std::thread::Builder::new()
        .name("screencraft-export-decode".into())
        .spawn(move || {
            let mut m = match crate::source::MultiSource::open(&dp, &dproj) {
                Ok(m) => m,
                Err(e) => {
                    let _ = ftx.send(Err(e));
                    return;
                }
            };
            for (t, other) in src_idx {
                if ftx.send(m.fetch(t, other)).is_err() {
                    return;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok((frx, decoder))
}

pub(crate) fn upload_frames(gpu: &mut Compositor, frames: &crate::source::Frames) {
    gpu.upload(&frames.screen);
    for (slot, f) in [(Slot::Screen2, &frames.screen2), (Slot::Camera, &frames.camera), (Slot::Camera2, &frames.camera2)] {
        match f {
            Some(f) => gpu.upload_slot(slot, f),
            None => gpu.clear_slot(slot),
        }
    }
}

/// Render frame `i`, blending sub-frames across the camera's motion when `blur` > 0 (a port of the
/// 0.3 exporter's motion blur).
pub(crate) fn render_frame(gpu: &mut Compositor, project: &Project, track: &screencraft_core::track::RenderTrack, i: usize, out: (u32, u32), blur: f64) -> Result<Vec<u8>, String> {
    let cam = CamOverride::at(track, i);
    let prev = CamOverride::at(track, i.saturating_sub(1));
    let next = CamOverride::at(track, (i + 1).min(track.count.saturating_sub(1)));
    let motion = (next.zoom - prev.zoom).abs() * 4.0 + (next.cx - prev.cx).abs() * 12.0 + (next.cy - prev.cy).abs() * 12.0;
    if blur > 0.0 && motion > 0.01 {
        let n = (2 + (motion * 40.0).round() as usize).min(6);
        let mut acc: Vec<u8> = Vec::new();
        for k in 0..n {
            let f = if n == 1 { 0.0 } else { (k as f64 / (n - 1) as f64 - 0.5) * blur };
            let c = if f < 0.0 { CamOverride::lerp(cam, prev, -f) } else { CamOverride::lerp(cam, next, f) };
            let scene = build_scene_with(project, track, i, out, Some(c));
            gpu.render(&scene);
            let px = gpu.read_rgba()?;
            if k == 0 {
                acc = px;
            } else {
                // running average: each sub-frame blends in with weight 1 / (k + 1)
                for (a, b) in acc.iter_mut().zip(px.iter()) {
                    *a = ((*a as u32 * k as u32 + *b as u32 + (k as u32 + 1) / 2) / (k as u32 + 1)) as u8;
                }
            }
        }
        return Ok(acc);
    }
    let scene = build_scene(project, track, i, out);
    gpu.render(&scene);
    gpu.read_rgba()
}

fn even_down(n: u32) -> u32 {
    n & !1
}

/// Render `project` (stored in `package`) to `out`. `progress` gets 0..1 and returns `false`
/// to cancel. A cancelled export removes the partial file and returns an error.
pub fn export_project(
    package: &Package,
    project: &Project,
    opts: &ExportOptions,
    out: &Path,
    mut progress: impl FnMut(f32) -> bool,
) -> Result<ExportReport, String> {
    // jump cuts (disabled stretches) are left out of the file
    let effective = project.effective();
    let project = &effective;
    if !opts.container.supports(opts.codec) {
        return Err(format!("{} can't be stored in .{}", opts.codec.name(), opts.container.extension()));
    }
    let fps = opts.fps.clamp(1, 240);
    let log = package.load_input(project);
    let track = compute_track(project, &log, fps as f64);
    if track.count == 0 {
        return Err("nothing to export: the timeline is empty".into());
    }
    let (w, h) = output_size(project, opts.resolution.max(144));
    let (w, h) = (even_down(w), even_down(h));

    let (frx, decoder) = spawn_decoder(package, project, &track)?;

    let mut soundtrack = if opts.audio { build_soundtrack(package, project, &track)? } else { None };

    let mut video = VideoConfig::new(opts.codec, w, h, fps);
    if let Some(b) = opts.bitrate_kbps {
        video.bitrate_kbps = b;
    }
    video.effort = opts.effort;
    video.hardware = opts.hardware;
    video.threads = opts.threads;
    let settings = ExportSettings {
        container: opts.container,
        video,
        audio: if soundtrack.is_some() { vec![AudioConfig::default()] } else { vec![] },
    };
    let sink = FileSink::create(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let exporter = ThreadedExporter::spawn(settings, Box::new(sink)).map_err(|e| e.to_string())?;

    let mut gpu = Compositor::headless()?;
    let threads = default_threads();
    let mut cancelled = false;
    let mut failure: Option<String> = None;
    // GPU read-back and colour conversion overlap: this thread renders and reads frames, a
    // second one converts them to I420 and feeds the encoder, in order.
    std::thread::scope(|scope| {
        let (rtx, rrx) = sync_channel::<(usize, Vec<u8>)>(3);
        let exporter = &exporter;
        let mut soundtrack = soundtrack.take();
        let conv = scope.spawn(move || -> Result<(), String> {
            let mut audio_done = 0u64;
            for (i, rgba) in rrx {
                let yuv = to_i420(&rgba, w, h, w as usize * 4, PixelOrder::Rgba, threads);
                let pts_us = (i as u64 * 1_000_000 / fps as u64) as i64;
                exporter.push_video(Arc::new(yuv), pts_us).map_err(|e| e.to_string())?;
                if let Some(st) = soundtrack.as_mut() {
                    let end = (i as u64 + 1) * 48_000 / fps as u64;
                    let n = (end - audio_done) as usize;
                    let pcm = st.render(audio_done, n);
                    audio_done = end;
                    exporter.push_audio(0, pcm).map_err(|e| e.to_string())?;
                }
            }
            Ok(())
        });
        let mut produce = || -> Result<(), String> {
            for i in 0..track.count {
                let frames = frx.recv().map_err(|_| "decoder stopped".to_string())??;
                upload_frames(&mut gpu, &frames);
                let rgba = render_frame(&mut gpu, project, &track, i, (w, h), project.config.animations.motion_blur.min(1.0))?;
                if rtx.send((i, rgba)).is_err() {
                    break; // the converter stopped; its error is collected below
                }
                if i % 4 == 0 && !progress(i as f32 / track.count as f32) {
                    cancelled = true;
                    break;
                }
            }
            Ok(())
        };
        let produced = produce();
        drop(rtx);
        let converted = conv.join().unwrap_or_else(|_| Err("encoder thread panicked".into()));
        failure = produced.err().or(converted.err());
    });
    if let Some(e) = failure {
        drop(frx);
        let _ = decoder.join();
        drop(exporter);
        let _ = std::fs::remove_file(out);
        return Err(e);
    }
    drop(frx);
    let _ = decoder.join();
    if cancelled {
        drop(exporter);
        let _ = std::fs::remove_file(out);
        return Err("export cancelled".into());
    }
    let summary = exporter.finish().map_err(|e| e.to_string())?;
    progress(1.0);
    Ok(ExportReport { summary, width: w, height: h, frames: track.count })
}
