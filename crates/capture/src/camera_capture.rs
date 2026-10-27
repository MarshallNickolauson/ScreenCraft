//! The camera as the picture: a "capturer" that turns the live camera feed into video frames, so a
//! talking-head video (no screen) goes through the same recorder, encoder and editor as a screen
//! recording. In vertical mode a landscape feed is cropped to 9:16 around its centre; a feed that
//! is already upright (a phone held in portrait) is used as it is.

use crate::camera::{CamFrame, CameraHub};
use crate::{CaptureRequest, Capturer, Event, Frame, StartInfo, Target};
use screencraft_audio::Clock;
use screencraft_core::project::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Duration;

pub struct CameraCapturer {
    hub: Arc<CameraHub>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl CameraCapturer {
    pub fn new(hub: Arc<CameraHub>) -> Self {
        Self { hub, stop: Arc::new(AtomicBool::new(false)), join: None }
    }
}

fn even(v: f64) -> u32 {
    let n = v.round().max(2.0) as u32;
    n - (n % 2)
}

/// The part of a `w`x`h` picture to keep: all of it, or a centred 9:16 column of a landscape one.
fn crop_for(w: u32, h: u32, vertical: bool) -> (u32, u32, u32, u32) {
    if vertical && (w as u64) * 16 > (h as u64) * 9 {
        let cw = (even(h as f64 * 9.0 / 16.0)).min(w);
        ((w - cw) / 2, 0, cw, h)
    } else {
        (0, 0, w, h)
    }
}

/// Crop `crop` out of packed RGB and scale it to `(ow, oh)` as BGRA (bilinear).
fn rgb_to_bgra(f: &CamFrame, crop: (u32, u32, u32, u32), (ow, oh): (u32, u32)) -> Vec<u8> {
    let (cx, cy, cw, ch) = crop;
    let stride = f.width as usize * 3;
    let mut out = vec![255u8; ow as usize * oh as usize * 4];
    let (sx, sy) = (cw as f32 / ow as f32, ch as f32 / oh as f32);
    for y in 0..oh as usize {
        let fy = ((y as f32 + 0.5) * sy - 0.5).clamp(0.0, ch as f32 - 1.0);
        let y0 = fy.floor() as usize;
        let y1 = (y0 + 1).min(ch as usize - 1);
        let ty = fy - y0 as f32;
        for x in 0..ow as usize {
            let fx = ((x as f32 + 0.5) * sx - 0.5).clamp(0.0, cw as f32 - 1.0);
            let x0 = fx.floor() as usize;
            let x1 = (x0 + 1).min(cw as usize - 1);
            let tx = fx - x0 as f32;
            let at = |xx: usize, yy: usize, c: usize| f.rgb[(cy as usize + yy) * stride + (cx as usize + xx) * 3 + c] as f32;
            let o = (y * ow as usize + x) * 4;
            for c in 0..3 {
                let top = at(x0, y0, c) * (1.0 - tx) + at(x1, y0, c) * tx;
                let bot = at(x0, y1, c) * (1.0 - tx) + at(x1, y1, c) * tx;
                // BGRA: channel 0 of the output is blue
                out[o + 2 - c] = (top * (1.0 - ty) + bot * ty + 0.5) as u8;
            }
        }
    }
    out
}

impl Capturer for CameraCapturer {
    fn start(&mut self, req: &CaptureRequest, clock: Clock, tx: SyncSender<Event>) -> Result<StartInfo, String> {
        let Target::Camera { vertical } = req.target else {
            return Err("not a camera target".into());
        };
        if self.join.is_some() {
            return Err("already capturing".into());
        }
        let rx = self.hub.subscribe(3);
        let first = match self.hub.latest() {
            Some(f) => f,
            None => rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the camera is not delivering pictures (is it on?)".to_string())?,
        };
        let crop = crop_for(first.width, first.height, vertical);
        // output size: the cropped picture, capped on its long side
        let (mut ow, mut oh) = (crop.2 as f64, crop.3 as f64);
        if let Some(m) = req.max_long_side.filter(|m| *m > 0) {
            let long = ow.max(oh);
            if long > m as f64 {
                let k = m as f64 / long;
                ow *= k;
                oh *= k;
            }
        }
        let (ow, oh) = (even(ow), even(oh));
        let stop = self.stop.clone();
        stop.store(false, Ordering::SeqCst);
        let first_for_thread = first.clone();
        let join = std::thread::Builder::new()
            .name("screencraft-camera-capture".into())
            .spawn(move || {
                let send = |f: &CamFrame| {
                    let crop = crop_for(f.width, f.height, vertical);
                    let bgra = rgb_to_bgra(f, crop, (ow, oh));
                    let age = f.at.elapsed().as_nanos() as u64;
                    let pts_ns = clock.now_ns().saturating_sub(age);
                    let _ = tx.try_send(Event::Frame(Frame { width: ow, height: oh, bgra, pts_ns }));
                };
                send(&first_for_thread);
                while !stop.load(Ordering::SeqCst) {
                    match rx.recv_timeout(Duration::from_millis(100)) {
                        Ok(f) => send(&f),
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(_) => break,
                    }
                }
                let _ = tx.try_send(Event::Ended(None));
            })
            .map_err(|e| e.to_string())?;
        self.join = Some(join);
        Ok(StartInfo {
            width: ow,
            height: oh,
            bounds: Rect { x: 0.0, y: 0.0, width: ow as f64, height: oh as f64 },
            display_id: None,
            provides_system_audio: false,
        })
    }

    fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for CameraCapturer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_a_landscape_feed_to_nine_sixteen() {
        assert_eq!(crop_for(1920, 1080, true), (656, 0, 608, 1080));
        assert_eq!(crop_for(1920, 1080, false), (0, 0, 1920, 1080));
        // an upright feed is already vertical
        assert_eq!(crop_for(1080, 1920, true), (0, 0, 1080, 1920));
    }

    #[test]
    fn rgb_becomes_bgra() {
        let f = CamFrame { width: 2, height: 2, rgb: vec![10, 20, 30, 10, 20, 30, 10, 20, 30, 10, 20, 30], at: std::time::Instant::now() };
        let px = rgb_to_bgra(&f, (0, 0, 2, 2), (2, 2));
        assert_eq!(&px[..4], &[30, 20, 10, 255]);
    }
}
