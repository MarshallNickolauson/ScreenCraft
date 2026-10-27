//! ScreenCraft core: everything about a recording that is not pixels.

pub mod input;
pub mod jumpcut;
pub mod pace;
pub mod project;
pub mod spring;
pub mod timeline;
pub mod track;
pub mod zoom;

pub use input::{CursorKind, InputLog};
pub use project::{Media, Project};
pub use track::{compute_track, RenderTrack};
