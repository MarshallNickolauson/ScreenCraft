//! Record synthetically, then export through the real compositor and encoders.

use screencraft_capture::Target;
use screencraft_encode::mp4read::{Mp4File, TrackKind};
use screencraft_encode::{Container, Effort, HwPreference, VideoCodec};
use screencraft_engine::{export_project, ExportOptions, Package, RecordOptions, Recording};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::Synthetic;

fn record(dir: &std::path::Path) -> (Package, screencraft_core::project::Project) {
    let package = Package::open(dir.join("src.screencraft"));
    let rec = Recording::start_with(
        RecordOptions {
            target: Target::Display(None),
            fps: 30,
            max_long_side: None,
            system_audio: true,
            microphone: false,
            microphone_id: None,
            output: package.clone(),
            camera: None,
            bitrate_kbps: Some(4000),
            hardware: HwPreference::Software,
        },
        Box::new(Synthetic { stop: Arc::new(AtomicBool::new(false)), thread: None }),
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(1400));
    let done = rec.stop().unwrap();
    (package, done.project)
}

fn opts(container: Container, codec: VideoCodec) -> ExportOptions {
    ExportOptions {
        container,
        codec,
        resolution: 360,
        fps: 30,
        bitrate_kbps: Some(1500),
        effort: Effort::Fast,
        hardware: HwPreference::Software,
        audio: true,
        threads: 2,
    }
}

#[test]
fn exports_mp4_and_webm_with_edits_applied() {
    if screencraft_render::Compositor::headless().is_err() {
        eprintln!("skipping: no GPU");
        return;
    }
    let dir = std::env::temp_dir().join(format!("sc-export-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (package, mut project) = record(&dir);
    let full_ms = project.media.duration_ms;

    // Trim: drop the first 400 ms and the last 300 ms, and speed the rest up 2x.
    project.scene.slices[0].source_start_ms = 400.0;
    project.scene.slices[0].source_end_ms = full_ms - 300.0;
    project.scene.slices[0].time_scale = 2.0;
    let expected_ms = (full_ms - 700.0) / 2.0;

    let mp4 = dir.join("out.mp4");
    let r = export_project(&package, &project, &opts(Container::Mp4, VideoCodec::H264), &mp4, |_| true).unwrap();
    assert_eq!((r.width, r.height), (640, 360));
    let f = Mp4File::open(&mp4).unwrap();
    let v = f.video().unwrap();
    assert_eq!(&v.codec, b"avc1");
    assert_eq!(v.samples.len(), r.frames);
    let got_ms = v.samples.len() as f64 * 1000.0 / 30.0;
    assert!((got_ms - expected_ms).abs() < 70.0, "duration {got_ms} vs {expected_ms}");
    assert!(f.tracks.iter().any(|t| t.kind == TrackKind::Audio));

    let webm = dir.join("out.webm");
    export_project(&package, &project, &opts(Container::WebM, VideoCodec::Av1), &webm, |_| true).unwrap();
    let bytes = std::fs::read(&webm).unwrap();
    assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3]);

    // Same project, same options -> identical bytes (software AV1 encoder, same GPU). H.264 goes
    // through the OS encoder, which makes no byte-for-byte promise.
    let (a, b) = (dir.join("a.mp4"), dir.join("b.mp4"));
    export_project(&package, &project, &opts(Container::Mp4, VideoCodec::Av1), &a, |_| true).unwrap();
    export_project(&package, &project, &opts(Container::Mp4, VideoCodec::Av1), &b, |_| true).unwrap();
    assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap());

    // Cancelling removes the partial file.
    let cancelled = dir.join("cancel.mp4");
    let err = export_project(&package, &project, &opts(Container::Mp4, VideoCodec::H264), &cancelled, |_| false);
    assert!(err.is_err());
    assert!(!cancelled.exists());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn jump_cuts_are_left_out_of_the_export() {
    if screencraft_render::Compositor::headless().is_err() {
        eprintln!("skipping: no GPU");
        return;
    }
    let dir = std::env::temp_dir().join(format!("sc-jumpcut-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let (package, mut project) = record(&dir);
    let full_ms = project.media.duration_ms;

    // switch off the middle third; it stays on the timeline but must not be exported
    let (a, b) = (full_ms / 3.0, full_ms * 2.0 / 3.0);
    let mut n = 0;
    screencraft_core::jumpcut::apply_jump_cuts(&mut project.scene.slices, &[[a, b]], || {
        n += 1;
        format!("jc-{n}")
    });
    assert_eq!(project.scene.slices.iter().filter(|s| s.disabled).count(), 1);
    let expected_ms = full_ms - (b - a);

    let mp4 = dir.join("out.mp4");
    export_project(&package, &project, &opts(Container::Mp4, VideoCodec::H264), &mp4, |_| true).unwrap();
    let f = Mp4File::open(&mp4).unwrap();
    let got_ms = f.video().unwrap().samples.len() as f64 * 1000.0 / 30.0;
    assert!((got_ms - expected_ms).abs() < 70.0, "duration {got_ms} vs {expected_ms}");
    let _ = std::fs::remove_dir_all(&dir);
}
