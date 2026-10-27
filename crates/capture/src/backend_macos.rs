//! ScreenCaptureKit backend: raw BGRA frames and system audio straight from the stream.

use crate::{even, CaptureRequest, Capturer, DisplayInfo, Event, Frame, Permissions, Sources, StartInfo, Target, WindowInfo};
use screencapturekit::cg::CGRect;
use screencapturekit::cm::{CMSampleBufferExt, CMSampleBufferSCExt, SCFrameStatus};
use screencapturekit::cv::CVPixelBufferLockFlags;
use screencapturekit::prelude::*;
use screencraft_audio::{AudioBlock, Clock, Source};
use screencraft_core::project::Rect;
use std::sync::mpsc::SyncSender;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGPreflightScreenCaptureAccess() -> bool;
    fn CGRequestScreenCaptureAccess() -> bool;
    fn CGPreflightListenEventAccess() -> bool;
    fn CGRequestListenEventAccess() -> bool;
}

pub fn permissions() -> Permissions {
    Permissions {
        screen: unsafe { CGPreflightScreenCaptureAccess() },
        input_monitoring: unsafe { CGPreflightListenEventAccess() },
        accessibility: crate::input::accessibility_trusted(),
    }
}

pub fn request_screen_access() -> bool {
    unsafe { CGRequestScreenCaptureAccess() }
}

pub fn request_input_monitoring() -> bool {
    unsafe { CGRequestListenEventAccess() }
}

pub fn list_sources() -> Result<Sources, String> {
    let content = SCShareableContent::get().map_err(|e| format!("{e} (Screen Recording permission needed)"))?;
    let me = std::process::id() as i32;
    let displays = content
        .displays()
        .iter()
        .enumerate()
        .map(|(i, d)| {
            let f = d.frame();
            let is_main = f.origin.x == 0.0 && f.origin.y == 0.0;
            DisplayInfo {
                id: d.display_id(),
                name: if is_main { "Main Display".into() } else { format!("Display {}", i + 1) },
                x: f.origin.x,
                y: f.origin.y,
                width: f.size.width,
                height: f.size.height,
                is_main,
            }
        })
        .collect();
    let windows = content
        .windows()
        .iter()
        .filter(|w| w.is_on_screen() && w.window_layer() == 0)
        .filter_map(|w| {
            let app = w.owning_application()?;
            if app.process_id() == me {
                return None;
            }
            let f = w.frame();
            if f.size.width < 120.0 || f.size.height < 80.0 {
                return None;
            }
            Some(WindowInfo {
                id: w.window_id(),
                title: w.title().unwrap_or_default(),
                app: app.application_name(),
                x: f.origin.x,
                y: f.origin.y,
                width: f.size.width,
                height: f.size.height,
            })
        })
        .collect();
    Ok(Sources { displays, windows })
}

#[derive(Default)]
pub struct PlatformCapturer {
    stream: Option<SCStream>,
}

struct VideoHandler {
    clock: Clock,
    tx: SyncSender<Event>,
}

impl SCStreamOutputTrait for VideoHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, _of: SCStreamOutputType) {
        // Idle / blank frames carry no new picture; the CFR stage repeats the last one.
        match sample.frame_status() {
            Some(SCFrameStatus::Complete) | Some(SCFrameStatus::Started) | None => {}
            _ => return,
        }
        let at = self.clock.now_ns();
        let Some(pb) = sample.pixel_buffer() else { return };
        let Ok(guard) = pb.lock(CVPixelBufferLockFlags::READ_ONLY) else { return };
        let (w, h, stride) = (guard.width(), guard.height(), guard.bytes_per_row());
        let Some(src) = (unsafe { guard.as_slice() }) else { return };
        if src.len() < stride * h || w < 2 || h < 2 {
            return;
        }
        let (ew, eh) = (w & !1, h & !1);
        let mut bgra = vec![0u8; ew * eh * 4];
        for y in 0..eh {
            bgra[y * ew * 4..(y + 1) * ew * 4].copy_from_slice(&src[y * stride..y * stride + ew * 4]);
        }
        // try_send: if the encoder is behind, dropping a frame beats stalling ScreenCaptureKit.
        let _ = self.tx.try_send(Event::Frame(Frame { width: ew as u32, height: eh as u32, bgra, pts_ns: at }));
    }
}

struct AudioHandler {
    clock: Clock,
    tx: SyncSender<Event>,
}

impl SCStreamOutputTrait for AudioHandler {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, _of: SCStreamOutputType) {
        let end = self.clock.now_ns();
        let Ok(list) = sample.audio_buffer_list() else { return };
        let n = list.num_buffers();
        if n == 0 {
            return;
        }
        let planes: Vec<&[u8]> = list.iter().map(|b| b.data()).collect();
        let (channels, data): (u16, Vec<f32>) = if n == 1 {
            // Interleaved.
            let ch = list.get(0).map(|b| b.number_channels as u16).unwrap_or(2).max(1);
            let d = planes[0].chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
            (ch, d)
        } else {
            // One buffer per channel (planar): interleave.
            let frames = planes.iter().map(|p| p.len() / 4).min().unwrap_or(0);
            let mut d = Vec::with_capacity(frames * n);
            for i in 0..frames {
                for p in &planes {
                    let o = i * 4;
                    d.push(f32::from_le_bytes([p[o], p[o + 1], p[o + 2], p[o + 3]]));
                }
            }
            (n as u16, d)
        };
        if data.is_empty() {
            return;
        }
        let rate = 48_000u32;
        let dur = (data.len() / channels as usize) as u64 * 1_000_000_000 / rate as u64;
        let _ = self.tx.send(Event::SystemAudio(AudioBlock {
            source: Source::System,
            pts_ns: end.saturating_sub(dur),
            sample_rate: rate,
            channels,
            data,
        }));
    }
}

impl Capturer for PlatformCapturer {
    fn start(&mut self, req: &CaptureRequest, clock: Clock, tx: SyncSender<Event>) -> Result<StartInfo, String> {
        if self.stream.is_some() {
            return Err("already capturing".into());
        }
        let content = SCShareableContent::get().map_err(|e| format!("{e} (is Screen Recording permission granted?)"))?;
        let displays = content.displays();
        let windows = content.windows();
        let me = std::process::id() as i32;
        let own: Vec<&SCWindow> = windows
            .iter()
            .filter(|w| w.owning_application().map(|a| a.process_id() == me).unwrap_or(false))
            .collect();

        let (filter, bounds, source_rect, display_id): (SCContentFilter, Rect, Option<CGRect>, Option<u32>) = match &req.target {
            Target::Camera { .. } => return Err("the camera is recorded by the camera capturer".into()),
            Target::Window(id) => {
                let w = windows.iter().find(|w| w.window_id() == *id).ok_or("window not found")?;
                let f = w.frame();
                let filter = SCContentFilter::create().with_window(w).build().map_err(|e| e.to_string())?;
                (filter, Rect { x: f.origin.x, y: f.origin.y, width: f.size.width, height: f.size.height }, None, None)
            }
            Target::Display(id) | Target::Area { display: id, .. } => {
                let display = match id {
                    Some(id) => displays.iter().find(|d| d.display_id() == *id),
                    None => displays.first(),
                }
                .ok_or("display not found")?;
                let df = display.frame();
                let filter = SCContentFilter::create()
                    .with_display(display)
                    .with_excluding_windows(&own)
                    .build()
                    .map_err(|e| e.to_string())?;
                match &req.target {
                    Target::Camera { .. } => unreachable!(),
                    Target::Area { rect, .. } => {
                        let local = CGRect::new(rect.x - df.origin.x, rect.y - df.origin.y, rect.width, rect.height);
                        (filter, *rect, Some(local), Some(display.display_id()))
                    }
                    _ => (
                        filter,
                        Rect { x: df.origin.x, y: df.origin.y, width: df.size.width, height: df.size.height },
                        None,
                        Some(display.display_id()),
                    ),
                }
            }
        };

        let native = filter.point_pixel_scale().max(1.0) as f64;
        let long_pts = bounds.width.max(bounds.height).max(1.0);
        let scale = match req.max_long_side {
            Some(m) if m > 0 => native.min(m as f64 / long_pts),
            _ => native,
        };
        let (width, height) = (even(bounds.width * scale), even(bounds.height * scale));

        let mut config = SCStreamConfiguration::new()
            .with_width(width)
            .with_height(height)
            .with_fps(req.fps.clamp(10, 120))
            .with_pixel_format(PixelFormat::BGRA)
            .with_shows_cursor(false)
            .with_queue_depth(8);
        if let Some(r) = source_rect {
            config = config.with_source_rect(r);
        }
        if req.system_audio {
            config = config
                .with_captures_audio(true)
                .with_excludes_current_process_audio(true)
                .with_sample_rate(48_000)
                .with_channel_count(2);
        }

        let mut stream = SCStream::new(&filter, &config).map_err(|e| e.to_string())?;
        stream
            .add_output_handler(VideoHandler { clock: clock.clone(), tx: tx.clone() }, SCStreamOutputType::Screen)
            .map_err(|e| e.to_string())?;
        if req.system_audio {
            stream
                .add_output_handler(AudioHandler { clock, tx }, SCStreamOutputType::Audio)
                .map_err(|e| e.to_string())?;
        }
        stream.start_capture().map_err(|e| format!("{e} (check Screen Recording permission)"))?;
        self.stream = Some(stream);
        Ok(StartInfo { width, height, bounds, display_id, provides_system_audio: req.system_audio })
    }

    fn stop(&mut self) {
        if let Some(s) = self.stream.take() {
            let _ = s.stop_capture();
        }
    }
}

impl Drop for PlatformCapturer {
    fn drop(&mut self) {
        self.stop();
    }
}
