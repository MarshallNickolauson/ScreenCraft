//! ScreenCraft application: egui / wgpu UI over the capture, render and encode engines.

pub mod app;
pub mod area;
pub mod compressor;
pub mod editor;
pub mod headless;
pub mod icons;
pub mod imgdata;
pub mod kit;
pub mod live;
pub mod music;
pub mod permissions;
pub mod prefs;
pub mod recorder;
pub mod srt;
pub mod whisper;
pub mod theme;
pub mod updates;

#[cfg(target_os = "macos")]
mod macos_menu;

#[cfg(target_os = "macos")]
pub use macos_menu::configure_event_loop as configure_macos_event_loop;
