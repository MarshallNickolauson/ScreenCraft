use crate::clock::Clock;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use std::sync::mpsc::Sender;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Microphone,
    System,
}

/// A block as the device delivered it.
#[derive(Debug)]
pub struct AudioBlock {
    pub source: Source,
    /// Capture time of the first frame, on the shared `Clock`.
    pub pts_ns: u64,
    pub sample_rate: u32,
    pub channels: u16,
    pub data: Vec<f32>,
}

#[derive(Clone, Debug, Default)]
pub struct CaptureConfig {
    /// `DeviceInfo::id` of a microphone; `None` for the default input.
    pub microphone: Option<String>,
    pub use_microphone: bool,
    /// Loopback device id, or `None` for the default output (Windows) / nothing elsewhere.
    pub system: Option<String>,
    pub use_system: bool,
}

pub struct AudioCapture {
    streams: Vec<cpal::Stream>,
    pub microphone_format: Option<(u32, u16)>,
    pub system_format: Option<(u32, u16)>,
}

fn build(
    device: &cpal::Device,
    config: cpal::SupportedStreamConfig,
    source: Source,
    clock: Clock,
    tx: Sender<AudioBlock>,
) -> Result<cpal::Stream, String> {
    let rate = config.sample_rate();
    let channels = config.channels();
    let sc: StreamConfig = config.config();
    macro_rules! stream {
        ($t:ty, $conv:expr) => {{
            let clock = clock.clone();
            let tx = tx.clone();
            device.build_input_stream(
                &sc,
                move |data: &[$t], _| {
                    let frames = data.len() / channels.max(1) as usize;
                    let dur = frames as u64 * 1_000_000_000 / rate.max(1) as u64;
                    let end = clock.now_ns();
                    let conv: fn($t) -> f32 = $conv;
                    let _ = tx.send(AudioBlock {
                        source,
                        pts_ns: end.saturating_sub(dur),
                        sample_rate: rate,
                        channels,
                        data: data.iter().map(|&s| conv(s)).collect(),
                    });
                },
                move |e| eprintln!("audio stream error ({source:?}): {e}"),
                None,
            )
        }};
    }
    let s = match config.sample_format() {
        SampleFormat::F32 => stream!(f32, |s| s),
        SampleFormat::I16 => stream!(i16, |s| s as f32 / 32768.0),
        SampleFormat::U16 => stream!(u16, |s| (s as f32 - 32768.0) / 32768.0),
        SampleFormat::I32 => stream!(i32, |s| s as f32 / 2_147_483_648.0),
        other => return Err(format!("unsupported sample format {other:?}")),
    }
    .map_err(|e| e.to_string())?;
    Ok(s)
}

impl AudioCapture {
    /// Open and start the requested devices. Blocks are sent on `tx` from the audio threads.
    pub fn start(cfg: &CaptureConfig, clock: &Clock, tx: Sender<AudioBlock>) -> Result<Self, String> {
        let host = cpal::default_host();
        let mut streams = Vec::new();
        let mut microphone_format = None;
        let mut system_format = None;

        if cfg.use_microphone {
            // The chosen microphone may be unplugged, switched off or renumbered since it was
            // saved; rather than failing, fall back to one that works (the system default first).
            let chosen = cfg.microphone.as_ref().and_then(|id| id.parse::<cpal::DeviceId>().ok()).and_then(|i| host.device_by_id(&i));
            let device = chosen
                .or_else(|| host.default_input_device())
                .or_else(|| host.input_devices().ok().and_then(|mut it| it.find(|d| d.default_input_config().is_ok())))
                .ok_or_else(|| "no microphone found".to_string())?;
            let conf = device.default_input_config().map_err(|e| format!("microphone: {e}"))?;
            microphone_format = Some((conf.sample_rate(), conf.channels()));
            streams.push(build(&device, conf, Source::Microphone, clock.clone(), tx.clone())?);
        }

        if cfg.use_system {
            // Windows: an output device opened as input is WASAPI loopback. Linux: a monitor
            // source is a normal input. macOS has no cpal loopback; ScreenCaptureKit supplies it.
            let by_id = cfg.system.as_ref().and_then(|id| id.parse::<cpal::DeviceId>().ok()).and_then(|i| host.device_by_id(&i));
            #[cfg(windows)]
            let found = by_id.or_else(|| host.default_output_device()).map(|d| {
                let c = d.default_output_config();
                (d, c)
            });
            #[cfg(not(windows))]
            let found = by_id.or_else(|| {
                host.input_devices().ok().and_then(|mut it| {
                    it.find(|d| d.description().map(|x| x.name().to_lowercase().contains("monitor")).unwrap_or(false))
                })
            }).map(|d| {
                let c = d.default_input_config();
                (d, c)
            });
            match found {
                Some((device, Ok(conf))) => {
                    system_format = Some((conf.sample_rate(), conf.channels()));
                    streams.push(build(&device, conf, Source::System, clock.clone(), tx.clone())?);
                }
                Some((_, Err(e))) => return Err(format!("system audio: {e}")),
                None => return Err("no system-audio loopback device on this platform".into()),
            }
        }

        for s in &streams {
            s.play().map_err(|e| e.to_string())?;
        }
        Ok(Self { streams, microphone_format, system_format })
    }

    pub fn stop(self) {
        drop(self.streams);
    }
}
