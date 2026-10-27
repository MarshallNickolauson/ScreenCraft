//! ScreenCraft engine: everything between the platform APIs and the editor UI that is not
//! pixels-on-screen. A `Recording` drives capture, audio, input logging and encoding into a
//! `.screencraft` package; `Package` reads and writes that folder; `MediaSource` decodes it
//! back for the editor and the exporter.

pub mod audio_mix;
pub mod camera_rec;
pub mod compress;
pub mod export;
pub mod gif;
pub mod jumpcuts;
pub mod package;
pub mod recorder;
pub mod sfx;
pub mod source;

pub use gif::{export_gif, GifReport};
pub use export::{build_soundtrack, decode_music, mic_speech, export_project, ExportOptions, ExportReport};
pub use package::Package;
pub use recorder::{CameraSpec, Finished, LiveStats, RecordOptions, Recording};
pub use source::{AudioSource, FrameTable, Frames, MultiSource, VideoSource};
