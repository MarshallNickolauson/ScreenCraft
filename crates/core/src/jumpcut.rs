//! Jump cuts: finding the silent stretches of a talk and turning them into disabled slices.
//!
//! A jump cut is a [`Slice`] with `disabled` set. It stays on the timeline, greyed out, so it can
//! be inspected, trimmed or turned back on; playback skips it and the export leaves it out.

use crate::project::Slice;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct JumpCutSettings {
    /// Shortest silence worth cutting.
    pub min_silence_ms: f64,
    /// Kept on each side of a cut so words are never clipped.
    pub padding_ms: f64,
    /// 0..1: how much counts as silence (higher cuts more, including quiet speech).
    pub sensitivity: f64,
}

impl Default for JumpCutSettings {
    fn default() -> Self {
        Self { min_silence_ms: 450.0, padding_ms: 110.0, sensitivity: 0.5 }
    }
}

/// Silent stretches `[start, end]` (ms) from per-frame levels in dBFS (`frame_ms` apart, frame 0 at
/// time 0). The threshold sits between the room's noise floor and the speaking level, so it
/// adapts to every microphone.
pub fn find_silences(levels_db: &[f32], frame_ms: f64, s: &JumpCutSettings) -> Vec<[f64; 2]> {
    let n = levels_db.len();
    if n < 4 {
        return Vec::new();
    }
    let mut sorted: Vec<f32> = levels_db.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = sorted[n / 10];
    let speech = sorted[(n * 9 / 10).min(n - 1)];
    if speech - floor < 8.0 {
        return Vec::new(); // no clear difference between talking and quiet
    }
    let k = 0.12 + 0.38 * s.sensitivity.clamp(0.0, 1.0);
    let thr = floor + (speech - floor) * k as f32;
    let mut quiet: Vec<bool> = levels_db.iter().map(|&v| v < thr).collect();
    // a click or a breath (a sound shorter than 60 ms) inside a silence does not end it
    let blip = (60.0 / frame_ms).ceil().max(1.0) as usize;
    let mut i = 0;
    while i < n {
        if quiet[i] {
            i += 1;
            continue;
        }
        let a = i;
        while i < n && !quiet[i] {
            i += 1;
        }
        if i - a < blip && a > 0 && i < n {
            for q in &mut quiet[a..i] {
                *q = true;
            }
        }
    }
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if !quiet[i] {
            i += 1;
            continue;
        }
        let a = i;
        while i < n && quiet[i] {
            i += 1;
        }
        let (t0, t1) = (a as f64 * frame_ms, i as f64 * frame_ms);
        if t1 - t0 >= s.min_silence_ms {
            let (c0, c1) = (t0 + s.padding_ms, t1 - s.padding_ms);
            if c1 - c0 >= 120.0 {
                out.push([c0, c1]);
            }
        }
    }
    out
}

/// Split slices so each source range in `ranges` becomes its own disabled slice. Returns how many
/// jump cuts were made. Frozen frames and slices that are already jump cuts are left alone.
pub fn apply_jump_cuts(slices: &mut Vec<Slice>, ranges: &[[f64; 2]], mut new_id: impl FnMut() -> String) -> usize {
    let mut made = 0;
    let mut out: Vec<Slice> = Vec::with_capacity(slices.len() + ranges.len() * 2);
    for s in slices.drain(..) {
        if s.disabled || s.freeze || s.ramp {
            out.push(s);
            continue;
        }
        let mut hits: Vec<[f64; 2]> = ranges
            .iter()
            .map(|r| [r[0].max(s.source_start_ms), r[1].min(s.source_end_ms)])
            .filter(|r| r[1] - r[0] >= 40.0)
            .collect();
        hits.sort_by(|a, b| a[0].partial_cmp(&b[0]).unwrap());
        if hits.is_empty() {
            out.push(s);
            continue;
        }
        let mut cursor = s.source_start_ms;
        let mut first = true;
        let mut push = |out: &mut Vec<Slice>, a: f64, b: f64, disabled: bool, first: &mut bool| {
            if b - a < 1.0 {
                return;
            }
            let mut piece = s.clone();
            piece.source_start_ms = a;
            piece.source_end_ms = b;
            piece.disabled = disabled;
            if *first {
                *first = false;
            } else {
                piece.id = new_id();
                piece.transition = None;
            }
            out.push(piece);
        };
        for h in hits {
            if h[0] < cursor {
                continue;
            }
            push(&mut out, cursor, h[0], false, &mut first);
            push(&mut out, h[0], h[1], true, &mut first);
            made += 1;
            cursor = h[1];
        }
        push(&mut out, cursor, s.source_end_ms, false, &mut first);
    }
    *slices = out;
    made
}

/// Turn every jump cut back on and join neighbouring pieces that were only split to make them.
pub fn clear_jump_cuts(slices: &mut Vec<Slice>) {
    for s in slices.iter_mut() {
        s.disabled = false;
    }
    heal(slices);
}

/// Merge neighbours that continue each other with identical settings.
pub fn heal(slices: &mut Vec<Slice>) {
    let mut out: Vec<Slice> = Vec::with_capacity(slices.len());
    for s in slices.drain(..) {
        if let Some(prev) = out.last_mut() {
            let same = prev.disabled == s.disabled
                && !prev.freeze
                && !s.freeze
                && !prev.ramp
                && !s.ramp
                && s.transition.is_none()
                && (prev.source_end_ms - s.source_start_ms).abs() < 0.5
                && (prev.time_scale - s.time_scale).abs() < 1e-9
                && (prev.volume - s.volume).abs() < 1e-9
                && prev.hide_cursor == s.hide_cursor
                && prev.disable_smooth_mouse == s.disable_smooth_mouse;
            if same {
                prev.source_end_ms = s.source_end_ms;
                continue;
            }
        }
        out.push(s);
    }
    *slices = out;
}

/// Total length of the jump cuts, as played (ms).
pub fn cut_total_ms(slices: &[Slice]) -> f64 {
    slices.iter().filter(|s| s.disabled).map(|s| (s.source_end_ms - s.source_start_ms).max(0.0) / s.time_scale.max(1e-3)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn levels(pattern: &[(usize, f32)]) -> Vec<f32> {
        pattern.iter().flat_map(|&(n, v)| std::iter::repeat(v).take(n)).collect()
    }

    #[test]
    fn finds_a_pause_between_sentences() {
        // 1 s of speech, 1 s of silence, 1 s of speech (20 ms frames)
        let l = levels(&[(50, -20.0), (50, -60.0), (50, -20.0)]);
        let cuts = find_silences(&l, 20.0, &JumpCutSettings::default());
        assert_eq!(cuts.len(), 1, "{cuts:?}");
        assert!(cuts[0][0] >= 1000.0 && cuts[0][1] <= 2000.0, "{cuts:?}");
        // padding is kept on both sides
        assert!(cuts[0][0] - 1000.0 >= 100.0 && 2000.0 - cuts[0][1] >= 100.0);
    }

    #[test]
    fn short_pauses_stay() {
        let l = levels(&[(50, -20.0), (10, -60.0), (50, -20.0)]);
        assert!(find_silences(&l, 20.0, &JumpCutSettings::default()).is_empty());
    }

    #[test]
    fn splits_into_disabled_pieces_and_heals_back() {
        let mut sl = vec![Slice { source_start_ms: 0.0, source_end_ms: 10_000.0, ..Default::default() }];
        let mut n = 0;
        let made = apply_jump_cuts(&mut sl, &[[2000.0, 3000.0], [6000.0, 7000.0]], || {
            n += 1;
            format!("s{n}")
        });
        assert_eq!(made, 2);
        assert_eq!(sl.len(), 5);
        assert_eq!(sl.iter().filter(|s| s.disabled).count(), 2);
        assert_eq!(crate::timeline::effective_duration_ms(&sl), 8000.0);
        assert_eq!(crate::timeline::output_duration_ms(&sl), 10_000.0);
        clear_jump_cuts(&mut sl);
        assert_eq!(sl.len(), 1);
        assert_eq!(sl[0].source_end_ms, 10_000.0);
    }
}
