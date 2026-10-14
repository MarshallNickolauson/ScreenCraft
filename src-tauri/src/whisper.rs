//! On-device caption generation with whisper.cpp (through the `whisper-rs` bindings).
//!
//! Nothing is uploaded. The only network request is the optional, user-started download of a
//! ggml model file; after that the speech recognition runs entirely on this machine.

use crate::state::CmdResult;
use screencraft_core::project::Project;
use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter, Manager};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

/// (id, file, approximate bytes)
const MODELS: &[(&str, &str, u64)] = &[
    ("tiny.en", "ggml-tiny.en.bin", 77_704_715),
    ("base.en", "ggml-base.en.bin", 147_964_211),
    ("small.en", "ggml-small.en.bin", 487_614_201),
    ("base", "ggml-base.bin", 147_951_465),
    ("large-v3-turbo-q5_0", "ggml-large-v3-turbo-q5_0.bin", 574_041_195),
];
const BASE_URL: &str = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/";

#[derive(Serialize, Clone)]
struct Progress {
    stage: &'static str,
    pct: f64,
}

fn model_info(id: &str) -> Result<(&'static str, u64), String> {
    MODELS.iter().find(|m| m.0 == id).map(|m| (m.1, m.2)).ok_or_else(|| format!("unknown model {id}"))
}

fn model_path(app: &AppHandle, file: &str) -> Result<PathBuf, String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?.join("models");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(dir.join(file))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStatus {
    pub id: String,
    pub downloaded: bool,
    pub size_bytes: u64,
}

#[tauri::command]
pub fn whisper_models(app: AppHandle) -> CmdResult<Vec<ModelStatus>> {
    let mut out = Vec::new();
    for (id, file, size) in MODELS {
        let have = model_path(&app, file).map(|p| p.exists()).unwrap_or(false);
        out.push(ModelStatus { id: (*id).into(), downloaded: have, size_bytes: *size });
    }
    Ok(out)
}

fn download(app: &AppHandle, id: &str) -> Result<PathBuf, String> {
    let (file, approx) = model_info(id)?;
    let dest = model_path(app, file)?;
    if dest.exists() {
        return Ok(dest);
    }
    // ClipCraft keeps the same ggml files; reuse them instead of downloading again
    if let Ok(home) = std::env::var("HOME") {
        let sib = PathBuf::from(home).join("Library/Application Support/dev.clipcraft.app/models").join(file);
        if sib.exists() && (std::fs::hard_link(&sib, &dest).is_ok() || std::fs::copy(&sib, &dest).is_ok()) {
            return Ok(dest);
        }
    }
    let part = dest.with_extension("part");
    let resp = ureq::get(&format!("{BASE_URL}{file}")).call().map_err(|e| format!("download failed: {e}"))?;
    let total = resp
        .header("content-length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(approx);
    let mut reader = resp.into_reader();
    let mut out = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let (mut done, mut last_pct) = (0u64, -1i64);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("download interrupted: {e}"))?;
        if n == 0 {
            break;
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        done += n as u64;
        let pct = (done as f64 / total as f64 * 100.0).min(100.0);
        if pct as i64 != last_pct {
            last_pct = pct as i64;
            let _ = app.emit("whisper-progress", Progress { stage: "download", pct });
        }
    }
    out.flush().map_err(|e| e.to_string())?;
    drop(out);
    // a truncated file would crash the model loader, so refuse anything far from the expected size
    if done < approx / 2 {
        let _ = std::fs::remove_file(&part);
        return Err("the model download was incomplete".into());
    }
    std::fs::rename(&part, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

#[tauri::command]
pub async fn whisper_download(app: AppHandle, model: String) -> CmdResult<()> {
    tauri::async_runtime::spawn_blocking(move || download(&app, &model).map(|_| ()))
        .await
        .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Caption {
    pub text: String,
    pub start_ms: f64,
    pub end_ms: f64,
    /// start of each word in `text`, same clock as `start_ms`
    pub word_starts: Vec<f64>,
}

/// Box-average downsample to 16 kHz, the rate whisper expects.
fn to_16k(mono: &[f32], rate: u32) -> Vec<f32> {
    if rate == 16_000 {
        return mono.to_vec();
    }
    let ratio = rate as f64 / 16_000.0;
    let n = (mono.len() as f64 / ratio) as usize;
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let a = (i as f64 * ratio) as usize;
        let b = (((i + 1) as f64 * ratio) as usize).clamp(a + 1, mono.len());
        out.push(mono[a..b].iter().sum::<f32>() / (b - a) as f32);
    }
    out
}

/// Run the model over 16 kHz mono samples. Timestamps are shifted back by `offset_ms`
/// (the microphone clock runs `offset_ms` ahead of the recording's).
fn recognize(
    model_file: &Path,
    pcm: &[f32],
    model: &str,
    language: &str,
    offset_ms: f64,
    on_progress: impl FnMut(i32) + 'static,
) -> Result<Vec<Caption>, String> {
    let ctx = WhisperContext::new_with_params(
        model_file.to_str().ok_or("bad model path")?,
        WhisperContextParameters::default(),
    )
    .map_err(|e| format!("could not load the model: {e}"))?;
    let mut state = ctx.create_state().map_err(|e| e.to_string())?;

    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    let threads = std::thread::available_parallelism().map(|n| n.get() as i32).unwrap_or(4).min(8);
    params.set_n_threads(threads);
    let lang = if model.ends_with(".en") { "en" } else { language };
    params.set_language(Some(lang));
    params.set_translate(false);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    params.set_suppress_blank(true);
    params.set_suppress_nst(true);
    // one segment per word with token timestamps: exact word times let us cut clean caption
    // lines ourselves. No context keeps one wrong segment from derailing the rest.
    params.set_no_context(true);
    params.set_token_timestamps(true);
    params.set_max_len(1);
    params.set_split_on_word(true);
    // the style hint is English, so it stays off for other languages
    if lang == "en" || lang == "auto" {
        params.set_initial_prompt("Hello, and welcome. Today, we're going to talk about something important.");
    }
    params.set_progress_callback_safe(on_progress);

    state.full(params, pcm).map_err(|e| format!("transcription failed: {e}"))?;

    let mut words: Vec<(String, f64, f64)> = Vec::new();
    for seg in state.as_iter() {
        let text = seg.to_str_lossy().map_err(|e| e.to_string())?.trim().to_string();
        // whisper emits markers like [BLANK_AUDIO] / (music) for non-speech
        if text.is_empty()
            || text.starts_with("[_")
            || (text.starts_with('[') && text.ends_with(']'))
            || (text.starts_with('(') && text.ends_with(')'))
            || text.starts_with('♪')
        {
            continue;
        }
        // centiseconds on the microphone clock; the project uses the source clock
        let s = (seg.start_timestamp() as f64 * 10.0 - offset_ms).max(0.0);
        let e = seg.end_timestamp() as f64 * 10.0 - offset_ms;
        if e <= 0.0 {
            continue;
        }
        words.push((text, s, e));
    }
    Ok(chunk(clean(words)))
}

/// Trim, drop empties, and make the times monotone without overlaps.
fn clean(raw: Vec<(String, f64, f64)>) -> Vec<(String, f64, f64)> {
    let mut out: Vec<(String, f64, f64)> = Vec::with_capacity(raw.len());
    for (text, mut t0, mut t1) in raw {
        if let Some(prev) = out.last_mut() {
            if t0 < prev.1 {
                t0 = prev.1;
            }
            if t0 < prev.2 {
                if t0 >= prev.1 + 40.0 {
                    prev.2 = t0;
                } else {
                    t0 = prev.2;
                }
            }
        }
        t1 = t1.max(t0 + 40.0);
        out.push((text, t0, t1));
    }
    out
}

/// Group words into short on-screen lines: at most 3 words / 24 characters, broken at sentence
/// ends, commas and pauses, and held briefly so lines never flicker.
fn chunk(words: Vec<(String, f64, f64)>) -> Vec<Caption> {
    const MAX_WORDS: usize = 3;
    let mut out: Vec<Caption> = Vec::new();
    let mut cur: Vec<(String, f64, f64)> = Vec::new();
    let mut chars = 0usize;
    let flush = |cur: &mut Vec<(String, f64, f64)>, out: &mut Vec<Caption>, chars: &mut usize| {
        if cur.is_empty() {
            return;
        }
        let text = cur.iter().map(|w| w.0.as_str()).collect::<Vec<_>>().join(" ");
        let word_starts = cur.iter().map(|w| w.1).collect();
        out.push(Caption { text, start_ms: cur[0].1, end_ms: cur[cur.len() - 1].2, word_starts });
        cur.clear();
        *chars = 0;
    };
    let mut last_end: Option<f64> = None;
    for (text, t0, t1) in words {
        let gap = last_end.map(|e| t0 - e).unwrap_or(0.0);
        let n = text.chars().count();
        if !cur.is_empty() && (gap > 450.0 || cur.len() >= MAX_WORDS || chars + n > 24) {
            flush(&mut cur, &mut out, &mut chars);
        }
        chars += n + 1;
        last_end = Some(t1);
        let t = text.trim_end_matches(['"', '\'', ')', '”', '’']).to_string();
        cur.push((text, t0, t1));
        let hard = t.ends_with(['.', '?', '!', '؟']);
        let soft = t.ends_with([',', ';', ':', '—', '،', '؛']);
        if hard || (soft && cur.len() >= 2) {
            flush(&mut cur, &mut out, &mut chars);
        }
    }
    flush(&mut cur, &mut out, &mut chars);
    // hold each line until the next starts when the gap is short, else a beat longer
    for i in 0..out.len() {
        let hold = out[i].end_ms + 320.0;
        out[i].end_ms = match out.get(i + 1).map(|c| c.start_ms) {
            Some(n) if n - out[i].end_ms < 320.0 => n,
            _ => hold,
        };
    }
    out
}

fn transcribe(app: &AppHandle, dir: &Path, project: &Project, model: &str, language: &str) -> Result<Vec<Caption>, String> {
    let model_file = download(app, model)?;
    let emit = |stage: &'static str, pct: f64| {
        let _ = app.emit("whisper-progress", Progress { stage, pct });
    };

    // speech comes from the microphone track when there is one, else the recording's own audio
    emit("audio", 0.0);
    let (rel, offset_ms) = match project.media.mic.as_deref() {
        Some(m) => (m.to_string(), project.media.mic_offset_ms),
        None => (project.media.screen.clone(), 0.0),
    };
    let path = dir.join(&rel);
    let decoded = crate::levelmic::decode_mono(&path);
    #[cfg(windows)]
    let decoded = decoded.or_else(|e| crate::mf::decode_audio_mono(&path.to_string_lossy()).map_err(|_| e));
    let (mono, rate) = decoded.map_err(|e| format!("no audio to transcribe ({e})"))?;
    let pcm = to_16k(&mono, rate);
    drop(mono);

    emit("model", 0.0);
    let app2 = app.clone();
    let out = recognize(&model_file, &pcm, model, language, offset_ms, move |p| {
        let _ = app2.emit("whisper-progress", Progress { stage: "transcribe", pct: p as f64 });
    })?;
    emit("done", 100.0);
    Ok(out)
}

#[tauri::command]
pub async fn whisper_transcribe(
    app: AppHandle,
    dir: String,
    project: Project,
    model: String,
    language: String,
) -> CmdResult<Vec<Caption>> {
    tauri::async_runtime::spawn_blocking(move || transcribe(&app, Path::new(&dir), &project, &model, &language))
        .await
        .map_err(|e| e.to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `WHISPER_MODEL=<ggml file> WHISPER_WAV=<audio file> cargo test -p screencraft whisper -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn transcribes_speech() {
        let model = std::env::var("WHISPER_MODEL").unwrap();
        let wav = std::env::var("WHISPER_WAV").unwrap();
        let (mono, rate) = crate::levelmic::decode_mono(Path::new(&wav)).unwrap();
        let caps = recognize(Path::new(&model), &to_16k(&mono, rate), "tiny.en", "en", 0.0, |_| {}).unwrap();
        for c in &caps {
            println!("{:.0}-{:.0} {}", c.start_ms, c.end_ms, c.text);
        }
        assert!(!caps.is_empty());
    }

    #[test]
    fn chunks_break_on_arabic_punctuation() {
        let w = |s: &str, t0: f64, t1: f64| (s.to_string(), t0, t1);
        let caps = chunk(vec![
            w("مرحبا", 0.0, 300.0),
            w("بكم؟", 320.0, 620.0),
            w("أهلاً", 660.0, 900.0),
            w("وسهلاً،", 930.0, 1200.0),
            w("صديقي", 1230.0, 1500.0),
        ]);
        // the question mark closes the first line, the Arabic comma the second
        assert_eq!(caps.len(), 3);
        assert_eq!(caps[0].text, "مرحبا بكم؟");
        assert_eq!(caps[1].text, "أهلاً وسهلاً،");
        assert_eq!(caps[2].text, "صديقي");
    }
}
