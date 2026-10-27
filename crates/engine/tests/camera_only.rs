//! A talking-head recording: the camera is the picture, there is no screen.

use screencraft_capture::camera::{CamFrame, CameraHub};
use screencraft_capture::Target;
use screencraft_encode::HwPreference;
use screencraft_engine::{CameraSpec, Package, RecordOptions, Recording, VideoSource};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

fn feed(hub: Arc<CameraHub>, stop: Arc<AtomicBool>, w: u32, h: u32) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut i = 0u32;
        while !stop.load(Ordering::SeqCst) {
            let mut rgb = vec![0u8; (w * h * 3) as usize];
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let o = (y * w as usize + x) * 3;
                    rgb[o] = (x * 255 / w as usize) as u8;
                    rgb[o + 1] = (y * 255 / h as usize) as u8;
                    rgb[o + 2] = (i * 8 % 255) as u8;
                }
            }
            hub.publish(CamFrame { width: w, height: h, rgb, at: Instant::now() });
            i += 1;
            std::thread::sleep(Duration::from_millis(33));
        }
    })
}

fn record(vertical: bool, feed_size: (u32, u32)) -> (screencraft_core::project::Project, Package, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("sc-camonly-{}-{}-{}x{}", std::process::id(), vertical, feed_size.0, feed_size.1));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let package = Package::open(dir.join("talk.screencraft"));
    let hub = CameraHub::new();
    let stop = Arc::new(AtomicBool::new(false));
    let t = feed(hub.clone(), stop.clone(), feed_size.0, feed_size.1);
    let rec = Recording::start(RecordOptions {
        target: Target::Camera { vertical },
        fps: 30,
        max_long_side: None,
        system_audio: true, // ignored: nothing plays "on the computer" in a talking-head video
        microphone: false,
        microphone_id: None,
        output: package.clone(),
        camera: Some(CameraSpec { id: None, hub }),
        bitrate_kbps: Some(4000),
        hardware: HwPreference::Software,
    })
    .expect("start");
    std::thread::sleep(Duration::from_millis(1400));
    let done = rec.stop().expect("stop");
    stop.store(true, Ordering::SeqCst);
    t.join().unwrap();
    (done.project, package, dir)
}

#[test]
fn vertical_talking_head_is_nine_by_sixteen() {
    let (p, package, dir) = record(true, (1280, 720));
    assert!(p.media.camera_only);
    assert!(!p.media.has_system_audio);
    assert!(p.media.camera.is_none(), "the camera is the screen, not a bubble");
    assert_eq!((p.media.width, p.media.height), (404, 720), "a 9:16 column of the 16:9 feed");
    assert_eq!(p.config.styles.padding_ratio, 0.0);
    assert!(p.config.cursor.hide);
    assert!(p.media.duration_ms > 800.0, "{}", p.media.duration_ms);
    let mut v = VideoSource::open(&package.screen_path(&p)).expect("open");
    assert_eq!((v.width, v.height), (404, 720));
    let f = v.frame(2); assert!(f.is_ok(), "{:?}", f.err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn landscape_talking_head_keeps_the_whole_picture() {
    let (p, package, dir) = record(false, (1280, 720));
    assert_eq!((p.media.width, p.media.height), (1280, 720));
    let mut v = VideoSource::open(&package.screen_path(&p)).expect("open");
    let f = v.frame(2);
    assert!(f.is_ok(), "{:?}", f.err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn upright_feed_is_used_as_it_is() {
    let (p, _, dir) = record(true, (720, 1280));
    assert_eq!((p.media.width, p.media.height), (720, 1280));
    let _ = std::fs::remove_dir_all(&dir);
}
