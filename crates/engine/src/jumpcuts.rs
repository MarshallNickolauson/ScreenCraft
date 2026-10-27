//! Audio levels of a recording for jump-cut detection.

use crate::export::decode_music;
use crate::package::Package;
use crate::source::AudioSource;
use screencraft_core::project::Project;
use screencraft_encode::mp4read::Mp4File;
use std::sync::Arc;

/// Length of one level frame.
pub const FRAME_MS: f64 = 20.0;
const RATE: f64 = 48_000.0;

/// Loudness of the speech track in dBFS, one value per [`FRAME_MS`] from source time 0. Uses the
/// microphone (a track of `screen.mp4`, or a 0.3 recording's own file); a recording without a
/// microphone falls back to the system audio.
pub fn audio_levels(package: &Package, project: &Project) -> Result<Vec<f32>, String> {
    let win = (RATE * FRAME_MS / 1000.0) as usize;
    let to_db = |sum: f32| 20.0 * (sum / win as f32).sqrt().max(1e-6).log10();
    let m = &project.media;
    if let Some(t) = m.mic_track.or(m.system_track) {
        let path = package.screen_path(project);
        let file = Arc::new(Mp4File::open(&path).map_err(|e| e.to_string())?);
        let mut src = AudioSource::open(file, t as usize)?;
        let total = src.duration_ms.min(m.duration_ms.max(1.0) + 1000.0);
        let mut out: Vec<f32> = Vec::new();
        let mut t = 0.0;
        while t < total {
            let pcm = src.read(t, RATE as usize)?;
            for w in pcm.chunks_exact(win * 2) {
                out.push(to_db(w.chunks_exact(2).map(|f| ((f[0] + f[1]) * 0.5).powi(2)).sum()));
            }
            t += 1000.0;
        }
        return Ok(out);
    }
    if let Some(rel) = m.mic.as_deref() {
        let pcm = decode_music(&package.dir.join(rel)).ok_or("could not read the microphone file")?;
        let skip = ((m.mic_offset_ms.max(0.0)) * RATE / 1000.0) as usize * 2;
        let body = pcm.get(skip.min(pcm.len())..).unwrap_or(&[]);
        return Ok(body.chunks_exact(win * 2).map(|w| to_db(w.chunks_exact(2).map(|f| ((f[0] + f[1]) * 0.5).powi(2)).sum())).collect());
    }
    Err("this recording has no audio to find jump cuts in".into())
}
