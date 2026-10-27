//! Screen capture for ScreenCraft.
//!
//! One `Capturer` per platform delivers raw BGRA frames (and, where the OS offers it, system
//! audio) stamped on a shared `Clock`:
//!
//! * macOS: ScreenCaptureKit
//! * Windows: DXGI Desktop Duplication
//! * Linux: xdg-desktop-portal ScreenCast + PipeWire (Wayland and X11)
//!
//! Cursor, click and key logging lives in `input`; `cfr` turns the irregular frame arrival
//! into a constant frame rate.

pub mod camera;
pub mod camera_capture;
pub mod cfr;
pub mod input;
pub mod pixels;

#[cfg(target_os = "macos")]
mod backend_macos;
#[cfg(target_os = "macos")]
use backend_macos as platform;

#[cfg(windows)]
pub mod monitors;
#[cfg(windows)]
mod backend_windows;
#[cfg(windows)]
use backend_windows as platform;

#[cfg(target_os = "linux")]
mod backend_linux;
#[cfg(target_os = "linux")]
use backend_linux as platform;

use screencraft_audio::{AudioBlock, Clock};
use screencraft_core::project::Rect;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::SyncSender;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    /// Position and size in the platform's logical points.
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub is_main: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub app: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Sources {
    pub displays: Vec<DisplayInfo>,
    pub windows: Vec<WindowInfo>,
}

#[derive(Clone, Debug)]
pub enum Target {
    /// `None` = the main display.
    Display(Option<u32>),
    Window(u32),
    /// A rectangle in global logical points.
    Area { display: Option<u32>, rect: Rect },
    /// No screen at all: the camera is the picture (a talking-head video). `vertical` crops or
    /// keeps the picture in 9:16. Served by [`camera_capture::CameraCapturer`], not the OS backends.
    Camera { vertical: bool },
}

#[derive(Clone, Debug)]
pub struct CaptureRequest {
    pub target: Target,
    pub fps: u32,
    /// Cap on the long side in pixels; `None` keeps the native resolution.
    pub max_long_side: Option<u32>,
    /// Capture what the computer plays. Delivered as `Event::SystemAudio` where the capture
    /// API supplies it (macOS); on other platforms `screencraft_audio` opens a loopback device.
    pub system_audio: bool,
}

/// A tightly packed BGRA frame.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    /// Arrival time on the shared clock.
    pub pts_ns: u64,
}

pub enum Event {
    Frame(Frame),
    SystemAudio(AudioBlock),
    /// The stream ended (display unplugged, user stopped sharing ...). `None` = ended normally.
    Ended(Option<String>),
}

#[derive(Clone, Debug)]
pub struct StartInfo {
    pub width: u32,
    pub height: u32,
    /// Captured area in global logical points (what the input log is normalised against).
    pub bounds: Rect,
    /// Display the capture sits on, for input coordinate mapping.
    pub display_id: Option<u32>,
    /// True when `Event::SystemAudio` will be produced by this capturer.
    pub provides_system_audio: bool,
}

pub trait Capturer: Send {
    fn start(&mut self, req: &CaptureRequest, clock: Clock, tx: SyncSender<Event>) -> Result<StartInfo, String>;
    fn stop(&mut self);
}

pub use camera_capture::CameraCapturer;

pub fn create_capturer() -> Box<dyn Capturer> {
    Box::new(platform::PlatformCapturer::default())
}

pub fn list_sources() -> Result<Sources, String> {
    platform::list_sources()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    pub screen: bool,
    pub input_monitoring: bool,
    pub accessibility: bool,
}

pub fn permissions() -> Permissions {
    platform::permissions()
}

/// Ask the OS for screen-recording access (macOS shows its prompt once).
pub fn request_screen_access() -> bool {
    platform::request_screen_access()
}

pub fn request_input_monitoring() -> bool {
    platform::request_input_monitoring()
}

/// Round to an even pixel count (video encoders need it).
pub fn even(v: f64) -> u32 {
    let n = v.round().max(2.0) as u32;
    n - (n % 2)
}

/// Keep one of our own windows (the recording bar) out of the captured picture. Needed where
/// the capture API can't exclude windows itself (Windows desktop duplication). `hwnd` is the
/// native window handle; returns whether anything was changed.
#[cfg(windows)]
pub fn set_excluded_from_capture(hwnd: isize, excluded: bool) -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE, WDA_NONE};
    unsafe {
        SetWindowDisplayAffinity(HWND(hwnd as *mut core::ffi::c_void), if excluded { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE })
            .is_ok()
    }
}

#[cfg(not(windows))]
pub fn set_excluded_from_capture(_hwnd: isize, _excluded: bool) -> bool {
    false
}
