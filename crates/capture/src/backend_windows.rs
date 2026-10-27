//! Windows backend: DXGI Desktop Duplication. Frames are copied into a CPU staging texture,
//! cropped to the requested area and optionally scaled down, then delivered as BGRA.
//! System audio comes from WASAPI loopback (see `screencraft_audio`), not from here.

use crate::monitors::{self, Display};
use crate::pixels::crop_scale;
use crate::{even, CaptureRequest, Capturer, DisplayInfo, Event, Frame, Permissions, Sources, StartInfo, Target, WindowInfo};
use screencraft_audio::Clock;
use screencraft_core::project::Rect;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, SyncSender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::Interface;
use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::*;

pub fn permissions() -> Permissions {
    Permissions { screen: true, input_monitoring: true, accessibility: true }
}

pub fn request_screen_access() -> bool {
    true
}

pub fn request_input_monitoring() -> bool {
    true
}

pub fn list_sources() -> Result<Sources, String> {
    let displays = monitors::displays()
        .into_iter()
        .enumerate()
        .map(|(i, d)| {
            let (x, y, width, height) = d.logical();
            DisplayInfo {
                id: d.id,
                name: if d.is_primary { "Main Display".into() } else { format!("Display {}", i + 1) },
                x,
                y,
                width,
                height,
                is_main: d.is_primary,
            }
        })
        .collect();
    let windows = monitors::windows()
        .into_iter()
        .filter_map(|w| {
            let d = monitors::display_of_window(w.hwnd)?;
            let (x, y) = d.to_logical(w.phys.0 as f64, w.phys.1 as f64);
            let (width, height) = (w.phys.2 as f64 / d.scale, w.phys.3 as f64 / d.scale);
            if width < 120.0 || height < 80.0 {
                return None;
            }
            Some(WindowInfo { id: w.hwnd.0 as usize as u32, title: w.title, app: w.app, x, y, width, height })
        })
        .collect();
    Ok(Sources { displays, windows })
}

#[derive(Default)]
pub struct PlatformCapturer {
    stop: Option<Arc<AtomicBool>>,
    thread: Option<JoinHandle<()>>,
}

struct Duplication {
    device: ID3D11Device,
    ctx: ID3D11DeviceContext,
    dup: IDXGIOutputDuplication,
    staging: Option<(ID3D11Texture2D, u32, u32)>,
}

fn find_output(display: &Display) -> Result<(IDXGIAdapter1, IDXGIOutput), String> {
    unsafe {
        let factory: IDXGIFactory1 = CreateDXGIFactory1().map_err(|e| format!("DXGI: {e}"))?;
        let mut a = 0;
        while let Ok(adapter) = factory.EnumAdapters1(a) {
            let mut o = 0;
            while let Ok(output) = adapter.EnumOutputs(o) {
                if let Ok(desc) = output.GetDesc() {
                    if desc.Monitor == display.handle {
                        return Ok((adapter, output));
                    }
                }
                o += 1;
            }
            a += 1;
        }
    }
    Err("the display is not reachable through DXGI (is it on another GPU or a remote session?)".into())
}

impl Duplication {
    fn new(display: &Display) -> Result<Self, String> {
        let (adapter, output) = find_output(display)?;
        unsafe {
            let mut device: Option<ID3D11Device> = None;
            let mut ctx: Option<ID3D11DeviceContext> = None;
            let base: IDXGIAdapter = adapter.cast().map_err(|e| e.to_string())?;
            D3D11CreateDevice(
                &base,
                D3D_DRIVER_TYPE_UNKNOWN,
                HMODULE::default(),
                D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                Some(&[D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0]),
                D3D11_SDK_VERSION,
                Some(&mut device),
                None,
                Some(&mut ctx),
            )
            .map_err(|e| format!("Direct3D: {e}"))?;
            let device = device.ok_or("Direct3D device")?;
            let ctx = ctx.ok_or("Direct3D context")?;
            let out1: IDXGIOutput1 = output.cast().map_err(|e| e.to_string())?;
            let dup = out1.DuplicateOutput(&device).map_err(|e| format!("Desktop Duplication: {e}"))?;
            Ok(Self { device, ctx, dup, staging: None })
        }
    }

    /// Grab the next picture as a full-output BGRA image: (width, height, stride, pixels).
    /// `Ok(None)` = nothing new within the timeout.
    fn next(&mut self, timeout_ms: u32) -> Result<Option<(u32, u32, usize, Vec<u8>)>, windows::core::Error> {
        unsafe {
            let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut res: Option<IDXGIResource> = None;
            match self.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
                Err(e) => return Err(e),
            }
            let result = (|| -> Result<Option<(u32, u32, usize, Vec<u8>)>, windows::core::Error> {
                // Only the pointer moved: no new desktop picture.
                if info.LastPresentTime == 0 {
                    return Ok(None);
                }
                let tex: ID3D11Texture2D = res.as_ref().ok_or_else(windows::core::Error::empty)?.cast()?;
                let mut desc = D3D11_TEXTURE2D_DESC::default();
                tex.GetDesc(&mut desc);
                let need_new = !matches!(&self.staging, Some((_, w, h)) if *w == desc.Width && *h == desc.Height);
                if need_new {
                    let mut sd = desc;
                    sd.Usage = D3D11_USAGE_STAGING;
                    sd.BindFlags = 0;
                    sd.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
                    sd.MiscFlags = 0;
                    sd.MipLevels = 1;
                    sd.ArraySize = 1;
                    let mut st: Option<ID3D11Texture2D> = None;
                    self.device.CreateTexture2D(&sd, None, Some(&mut st))?;
                    self.staging = Some((st.ok_or_else(windows::core::Error::empty)?, desc.Width, desc.Height));
                }
                let staging = &self.staging.as_ref().unwrap().0;
                self.ctx.CopyResource(staging, &tex);
                let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
                self.ctx.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
                let stride = mapped.RowPitch as usize;
                let len = stride * desc.Height as usize;
                let data = std::slice::from_raw_parts(mapped.pData as *const u8, len).to_vec();
                self.ctx.Unmap(staging, 0);
                Ok(Some((desc.Width, desc.Height, stride, data)))
            })();
            let _ = self.dup.ReleaseFrame();
            result
        }
    }
}

impl Capturer for PlatformCapturer {
    fn start(&mut self, req: &CaptureRequest, clock: Clock, tx: SyncSender<Event>) -> Result<StartInfo, String> {
        if self.thread.is_some() {
            return Err("already capturing".into());
        }
        // Resolve the display and the capture rectangle in that display's physical pixels.
        let (display, phys_crop): (Display, (i32, i32, i32, i32)) = match &req.target {
            Target::Camera { .. } => return Err("the camera is recorded by the camera capturer".into()),
            Target::Display(id) => {
                let d = monitors::display(*id).ok_or("display not found")?;
                let p = d.phys;
                (d, (0, 0, p.2, p.3))
            }
            Target::Area { display, rect } => {
                let d = monitors::display(*display).ok_or("display not found")?;
                let (px, py) = d.to_physical(rect.x, rect.y);
                let (pw, ph) = (rect.width * d.scale, rect.height * d.scale);
                (d, (px as i32 - d.phys.0, py as i32 - d.phys.1, pw as i32, ph as i32))
            }
            Target::Window(id) => {
                let hwnd = windows::Win32::Foundation::HWND(*id as usize as *mut core::ffi::c_void);
                let d = monitors::display_of_window(hwnd).ok_or("window not found")?;
                let f = monitors::window_frame(hwnd).ok_or("window not found")?;
                (d, (f.0 - d.phys.0, f.1 - d.phys.1, f.2, f.3))
            }
        };
        // Clamp to the output.
        let (ox, oy) = (phys_crop.0.max(0), phys_crop.1.max(0));
        let cw = (phys_crop.2.min(display.phys.2 - ox)).max(2);
        let ch = (phys_crop.3.min(display.phys.3 - oy)).max(2);
        let (cw, ch) = ((cw & !1) as u32, (ch & !1) as u32);
        let scale = match req.max_long_side {
            Some(m) if m > 0 => (m as f64 / cw.max(ch) as f64).min(1.0),
            _ => 1.0,
        };
        let (out_w, out_h) = (even(cw as f64 * scale), even(ch as f64 * scale));
        let (lx, ly) = display.to_logical((display.phys.0 + ox) as f64, (display.phys.1 + oy) as f64);
        let bounds = Rect { x: lx, y: ly, width: cw as f64 / display.scale, height: ch as f64 / display.scale };

        let stop = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = channel::<Result<(), String>>();
        let (stop2, fps) = (stop.clone(), req.fps.clamp(10, 120));
        let crop = (ox as u32, oy as u32, cw, ch);
        let thread = std::thread::Builder::new()
            .name("screencraft-dxgi".into())
            .spawn(move || {
                let mut dup = match Duplication::new(&display) {
                    Ok(d) => {
                        let _ = ready_tx.send(Ok(()));
                        d
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let min_gap = Duration::from_secs_f64(0.8 / fps as f64);
                let mut last_sent: Option<Instant> = None;
                let mut last_frame: Option<Vec<u8>> = None;
                while !stop2.load(Ordering::SeqCst) {
                    match dup.next(100) {
                        Ok(Some((w, h, stride, data))) => {
                            if crop.0 + crop.2 > w || crop.1 + crop.3 > h {
                                continue;
                            }
                            if let Some(t) = last_sent {
                                if t.elapsed() < min_gap {
                                    continue;
                                }
                            }
                            let bgra = crop_scale(&data, stride, crop, (out_w, out_h));
                            last_sent = Some(Instant::now());
                            last_frame = Some(bgra.clone());
                            let _ = tx.try_send(Event::Frame(Frame { width: out_w, height: out_h, bgra, pts_ns: clock.now_ns() }));
                        }
                        Ok(None) => {}
                        Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                            // Resolution change, secure desktop, fullscreen app: re-acquire.
                            std::thread::sleep(Duration::from_millis(100));
                            match Duplication::new(&display) {
                                Ok(d) => dup = d,
                                Err(_) => std::thread::sleep(Duration::from_millis(400)),
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Event::Ended(Some(format!("screen capture failed: {e}"))));
                            break;
                        }
                    }
                }
                let _ = last_frame;
            })
            .map_err(|e| e.to_string())?;
        ready_rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "capture start timed out".to_string())??;
        self.stop = Some(stop);
        self.thread = Some(thread);
        Ok(StartInfo { width: out_w, height: out_h, bounds, display_id: Some(display.id), provides_system_audio: false })
    }

    fn stop(&mut self) {
        if let Some(s) = self.stop.take() {
            s.store(true, Ordering::SeqCst);
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for PlatformCapturer {
    fn drop(&mut self) {
        self.stop();
    }
}
