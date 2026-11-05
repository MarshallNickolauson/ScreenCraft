//! End to end without a display: a synthetic capture backend feeds the real recorder, encoder,
//! muxer and decoder.

use screencraft_capture::Target;
use screencraft_encode::mp4read::{Mp4File, TrackKind};
use screencraft_engine::{AudioSource, Package, RecordOptions, Recording, VideoSource};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

mod common;
use common::Synthetic;

#[test]
fn saves_a_recording_when_capture_ends_before_stop() {
    let dir = std::env::temp_dir().join(format!("sc-engine-ended-{}", std::process::id()));
    let package = Package::open(dir.join("t.screencraft"));
    let stopped = Arc::new(AtomicBool::new(false));
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
            hardware: screencraft_encode::HwPreference::Software,
        },
        Box::new(Synthetic { stop: stopped.clone(), thread: None }),
    )
    .expect("start");
    std::thread::sleep(Duration::from_millis(200));
    stopped.store(true, std::sync::atomic::Ordering::SeqCst);
    // Let the capture channel close and the worker finalize before asking to stop.
    std::thread::sleep(Duration::from_secs(1));
    let done = rec.stop().expect("save after capture ended");
    assert!(done.frames > 0);
    let project = package.load_project().expect("saved project");
    let video = VideoSource::open(&package.screen_path(&project)).expect("playable video");
    assert_eq!(video.frame_count as u64, done.frames);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn records_a_synthetic_screen_into_a_package() {
    let dir = std::env::temp_dir().join(format!("sc-engine-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let package = Package::open(dir.join("t.screencraft"));
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
            hardware: screencraft_encode::HwPreference::Software,
        },
        Box::new(Synthetic { stop: Arc::new(AtomicBool::new(false)), thread: None }),
    )
    .expect("start");
    std::thread::sleep(Duration::from_millis(1500));
    let done = rec.stop().expect("stop");
    assert!(done.frames >= 40, "frames {}", done.frames);
    assert!((done.duration_ms - 1500.0).abs() < 300.0, "duration {}", done.duration_ms);

    let project = package.load_project().unwrap();
    assert_eq!((project.media.width, project.media.height), (320, 180));
    assert_eq!(project.media.system_track, Some(0));

    let path = package.screen_path(&project);
    let mut v = VideoSource::open(&path).unwrap();
    assert_eq!(v.frame_count as u64, done.frames);
    // Constant frame rate: ~33.3 ms apart even though the source skipped frames.
    assert!((v.pts_ms(10) - v.pts_ms(9) - 33.3).abs() < 1.0);
    // The bar moves right, so frames further in time have it further right.
    let bar_x = |f: &screencraft_encode::I420| (0..320usize).find(|&x| f.y[90 * 320 + x] > 150).unwrap();
    let a = bar_x(&v.frame(2).unwrap());
    let b = bar_x(&v.frame(8).unwrap());
    assert!(b > a, "bar did not move: {a} -> {b}");
    // Seeking backwards works.
    assert_eq!(bar_x(&v.frame(2).unwrap()), a);

    let file = Arc::new(Mp4File::open(&path).unwrap());
    assert_eq!(file.tracks.iter().filter(|t| t.kind == TrackKind::Audio).count(), 1);
    let mut au = AudioSource::open(file, 0).unwrap();
    let pcm = au.read(500.0, 4800).unwrap();
    let peak = pcm.iter().fold(0f32, |m, s| m.max(s.abs()));
    assert!(peak > 0.3 && peak < 0.7, "tone peak {peak}");
    let _ = std::fs::remove_dir_all(&dir);
}
