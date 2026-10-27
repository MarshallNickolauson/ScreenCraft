//! Camera capture through the operating system's camera API (AVFoundation, Media Foundation or
//! Video4Linux), used for the live preview in the recorder and for recording the camera bubble.

use nokhwa::pixel_format::RgbFormat;
use nokhwa::utils::{ApiBackend, CameraIndex, RequestedFormat, RequestedFormatType, Resolution};
use nokhwa::Camera;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Fans one camera stream out to the live preview and (while recording) the recorder.
#[derive(Default)]
pub struct CameraHub {
    subs: std::sync::Mutex<Vec<std::sync::mpsc::SyncSender<Arc<CamFrame>>>>,
    latest: std::sync::Mutex<Option<Arc<CamFrame>>>,
}

impl CameraHub {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn publish(&self, f: CamFrame) {
        let f = Arc::new(f);
        *self.latest.lock().unwrap() = Some(f.clone());
        // a slow subscriber drops frames rather than stalling the camera
        self.subs.lock().unwrap().retain(|s| !matches!(s.try_send(f.clone()), Err(std::sync::mpsc::TrySendError::Disconnected(_))));
    }

    pub fn latest(&self) -> Option<Arc<CamFrame>> {
        self.latest.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        *self.latest.lock().unwrap() = None;
    }

    pub fn subscribe(&self, capacity: usize) -> std::sync::mpsc::Receiver<Arc<CamFrame>> {
        let (tx, rx) = std::sync::mpsc::sync_channel(capacity);
        self.subs.lock().unwrap().push(tx);
        rx
    }
}

pub struct CamFrame {
    pub width: u32,
    pub height: u32,
    /// Packed RGB8.
    pub rgb: Vec<u8>,
    pub at: Instant,
}

/// What the camera is for, which decides how sharp a picture to ask it for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum CamPreset {
    /// The small bubble over a screen recording: 720p is plenty.
    #[default]
    Bubble,
    /// A talking-head video in landscape: as sharp as 1080p.
    TalkingHead,
    /// A vertical (9:16) talking-head video cut from the picture: as sharp as the camera allows
    /// (up to 4K), because a 9:16 column of a 1080p picture is only 608 px wide.
    TalkingHeadVertical,
}

/// Cameras as (id, name). On macOS the id is the device's unique id, so a phone used as a camera
/// (iPhone through Continuity Camera over USB or Wi-Fi, an Android phone in USB webcam mode, or a
/// virtual camera app such as Camo or DroidCam) is listed and opens like any other camera. Elsewhere
/// it is the device index as a string.
pub fn list() -> Vec<(String, String)> {
    #[cfg(target_os = "macos")]
    {
        request_access_blocking();
        let found = mac_devices();
        if !found.is_empty() {
            return found;
        }
    }
    nokhwa::query(ApiBackend::Auto)
        .map(|v| v.into_iter().map(|i| (i.index().as_index().map(|n| n.to_string()).unwrap_or_default(), i.human_name())).collect())
        .unwrap_or_default()
}

/// Every video device AVFoundation knows, including Continuity Camera phones and external (UVC)
/// cameras, which nokhwa's own list leaves out.
#[cfg(target_os = "macos")]
fn mac_devices() -> Vec<(String, String)> {
    use objc2::msg_send;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2_foundation::{NSArray, NSString};
    let Some(cls) = AnyClass::get(c"AVCaptureDeviceDiscoverySession") else { return Vec::new() };
    let types: Vec<Retained<NSString>> = [
        "AVCaptureDeviceTypeBuiltInWideAngleCamera",
        "AVCaptureDeviceTypeContinuityCamera",
        "AVCaptureDeviceTypeExternal",
        "AVCaptureDeviceTypeExternalUnknown",
    ]
    .iter()
    .map(|t| NSString::from_str(t))
    .collect();
    let types = NSArray::from_retained_slice(&types);
    let media = NSString::from_str("vide");
    unsafe {
        let session: Option<Retained<AnyObject>> = msg_send![cls, discoverySessionWithDeviceTypes: &*types, mediaType: &*media, position: 0isize];
        let Some(session) = session else { return Vec::new() };
        let devices: Retained<NSArray<AnyObject>> = msg_send![&*session, devices];
        let mut out: Vec<(String, String)> = Vec::new();
        for d in devices.iter() {
            let uid: Retained<NSString> = msg_send![&*d, uniqueID];
            let name: Retained<NSString> = msg_send![&*d, localizedName];
            let (uid, name) = (uid.to_string(), name.to_string());
            if !out.iter().any(|(i, _)| *i == uid) {
                out.push((uid, name));
            }
        }
        out
    }
}

#[cfg(target_os = "macos")]
fn request_access_blocking() {
    let (tx, rx) = std::sync::mpsc::channel();
    nokhwa::nokhwa_initialize(move |ok| {
        let _ = tx.send(ok);
    });
    let _ = rx.recv_timeout(std::time::Duration::from_secs(30));
}

pub struct CameraStream {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl CameraStream {
    pub fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for CameraStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

/// Open camera `id` (None = the first one) and call `on_frame` for every frame on a worker thread.
/// Returns once the stream is running or the reason it could not.
pub fn start(id: Option<&str>, preset: CamPreset, mut on_frame: impl FnMut(CamFrame) + Send + 'static) -> Result<CameraStream, String> {
    // a number is a device index; anything else is a device's unique id (macOS)
    let index = match id {
        Some(s) => s.parse::<u32>().map(CameraIndex::Index).unwrap_or_else(|_| CameraIndex::String(s.to_string())),
        None => CameraIndex::Index(0),
    };
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let stop2 = stop.clone();
    let join = std::thread::Builder::new()
        .name("screencraft-camera".into())
        .spawn(move || {
            // Not every camera offers the exact format, so try a few ways of asking.
            let sizes: &[(u32, u32)] = match preset {
                CamPreset::Bubble => &[(1280, 720)],
                CamPreset::TalkingHead => &[(1920, 1080), (1280, 720)],
                CamPreset::TalkingHeadVertical => &[(3840, 2160), (2560, 1440), (1920, 1080), (1280, 720)],
            };
            let mut tries: Vec<RequestedFormatType> = sizes.iter().map(|&(w, h)| RequestedFormatType::HighestResolution(Resolution::new(w, h))).collect();
            tries.push(RequestedFormatType::AbsoluteHighestFrameRate);
            tries.push(RequestedFormatType::None);
            let mut opened = None;
            let mut last = String::new();
            for t in tries {
                match Camera::new(index.clone(), RequestedFormat::new::<RgbFormat>(t)) {
                    Ok(c) => {
                        opened = Some(c);
                        break;
                    }
                    Err(e) => last = e.to_string(),
                }
            }
            let mut cam = match opened {
                Some(c) => c,
                None => {
                    let _ = ready_tx.send(Err(format!("could not open the camera: {last}")));
                    return;
                }
            };
            if let Err(e) = cam.open_stream() {
                let _ = ready_tx.send(Err(format!("could not start the camera: {e}")));
                return;
            }
            let _ = ready_tx.send(Ok(()));
            while !stop2.load(Ordering::SeqCst) {
                match cam.frame() {
                    Ok(buf) => {
                        let at = Instant::now();
                        if let Ok(img) = buf.decode_image::<RgbFormat>() {
                            on_frame(CamFrame { width: img.width(), height: img.height(), rgb: img.into_raw(), at });
                        }
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(5)),
                }
            }
            let _ = cam.stop_stream();
        })
        .map_err(|e| e.to_string())?;
    match ready_rx.recv_timeout(std::time::Duration::from_secs(20)) {
        Ok(Ok(())) => Ok(CameraStream { stop, join: Some(join) }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the camera did not start in time".into()),
    }
}
