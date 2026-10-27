//! GIF export: 15 fps, at most 540 px on the short side, a 256-colour palette per frame.

use crate::export::{render_frame, spawn_decoder, upload_frames};
use crate::Package;
use color_quant::NeuQuant;
use screencraft_core::project::Project;
use screencraft_core::track::compute_track;
use screencraft_render::{output_size, Compositor};
use std::path::Path;

pub struct GifReport {
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
}

pub fn export_gif(package: &Package, project: &Project, resolution: u32, out: &Path, mut progress: impl FnMut(f32) -> bool) -> Result<GifReport, String> {
    const FPS: u32 = 15;
    let effective = project.effective();
    let project = &effective;
    let res = resolution.clamp(144, 540);
    let (w, h) = output_size(project, res);
    let (w, h) = (w & !1, h & !1);
    let log = package.load_input(project);
    let track = compute_track(project, &log, FPS as f64);
    if track.count == 0 {
        return Err("nothing to export: the timeline is empty".into());
    }
    let (frx, decoder) = spawn_decoder(package, project, &track)?;
    let mut gpu = Compositor::headless()?;
    let file = std::fs::File::create(out).map_err(|e| format!("cannot create {}: {e}", out.display()))?;
    let mut enc = gif::Encoder::new(std::io::BufWriter::new(file), w as u16, h as u16, &[]).map_err(|e| e.to_string())?;
    enc.set_repeat(gif::Repeat::Infinite).map_err(|e| e.to_string())?;
    let delay = (100.0 / FPS as f64).round() as u16; // centiseconds
    let mut result: Result<(), String> = Ok(());
    let mut cancelled = false;
    for i in 0..track.count {
        let frames = match frx.recv() {
            Ok(Ok(f)) => f,
            Ok(Err(e)) => {
                result = Err(e);
                break;
            }
            Err(_) => {
                result = Err("decoder stopped".into());
                break;
            }
        };
        upload_frames(&mut gpu, &frames);
        let rgba = match render_frame(&mut gpu, project, &track, i, (w, h), 0.0) {
            Ok(r) => r,
            Err(e) => {
                result = Err(e);
                break;
            }
        };
        // a palette per frame keeps colours faithful as the picture changes
        let nq = NeuQuant::new(10, 256, &rgba);
        let palette: Vec<u8> = nq.color_map_rgb();
        let indices: Vec<u8> = rgba.chunks_exact(4).map(|p| nq.index_of(p) as u8).collect();
        let mut frame = gif::Frame::from_indexed_pixels(w as u16, h as u16, indices, None);
        frame.palette = Some(palette);
        frame.delay = delay;
        if let Err(e) = enc.write_frame(&frame) {
            result = Err(e.to_string());
            break;
        }
        if i % 2 == 0 && !progress(i as f32 / track.count as f32) {
            cancelled = true;
            break;
        }
    }
    drop(frx);
    let _ = decoder.join();
    drop(enc);
    if cancelled || result.is_err() {
        let _ = std::fs::remove_file(out);
        return Err(if cancelled { "export cancelled".into() } else { result.err().unwrap_or_default() });
    }
    progress(1.0);
    Ok(GifReport { bytes: std::fs::metadata(out).map(|m| m.len()).unwrap_or(0), width: w, height: h })
}
