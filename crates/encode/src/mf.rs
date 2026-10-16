//! H.264 on Windows through Media Foundation (the operating system's licensed encoder and
//! decoder). Synchronous transforms: NV12 in / Annex-B out for encoding, the reverse for decoding.
//!
//! NOTE: written against the `windows` crate API without a Windows machine at hand; it is
//! type-checked and exercised by the Windows CI / first run, not by the macOS test suite.

#![allow(non_snake_case)]

use crate::codec::VideoEncoder;
use crate::h264::{annexb_to_avcc, avcc_to_annexb, build_avcc};
use crate::types::*;
use crate::{Error, Result};
use std::mem::ManuallyDrop;
use std::sync::Once;
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

fn init() -> Result<()> {
    static ONCE: Once = Once::new();
    let mut r = Ok(());
    ONCE.call_once(|| unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        if let Err(e) = MFStartup(MF_VERSION, MFSTARTUP_FULL) {
            r = Err(Error::Codec(format!("Media Foundation failed to start: {e}")));
        }
    });
    r
}

fn err(what: &str, e: windows::core::Error) -> Error {
    Error::Codec(format!("{what}: {e}"))
}

fn pack(hi: u32, lo: u32) -> u64 {
    ((hi as u64) << 32) | lo as u64
}

unsafe fn video_type(subtype: &windows::core::GUID, w: u32, h: u32, fps: (u32, u32)) -> windows::core::Result<IMFMediaType> {
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
    t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
    t.SetUINT64(&MF_MT_FRAME_SIZE, pack(w, h))?;
    t.SetUINT64(&MF_MT_FRAME_RATE, pack(fps.0, fps.1))?;
    t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack(1, 1))?;
    t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
    Ok(t)
}

unsafe fn sample_from(bytes: &[u8], pts_100ns: i64, dur_100ns: i64) -> windows::core::Result<IMFSample> {
    let buf = MFCreateMemoryBuffer(bytes.len() as u32)?;
    let mut p = std::ptr::null_mut();
    buf.Lock(&mut p, None, None)?;
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len());
    buf.Unlock()?;
    buf.SetCurrentLength(bytes.len() as u32)?;
    let s = MFCreateSample()?;
    s.AddBuffer(&buf)?;
    s.SetSampleTime(pts_100ns)?;
    s.SetSampleDuration(dur_100ns)?;
    Ok(s)
}

unsafe fn sample_bytes(s: &IMFSample) -> windows::core::Result<Vec<u8>> {
    let buf = s.ConvertToContiguousBuffer()?;
    let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
    buf.Lock(&mut p, None, Some(&mut len))?;
    let v = std::slice::from_raw_parts(p, len as usize).to_vec();
    buf.Unlock()?;
    Ok(v)
}

/// Pull every finished output sample out of `mft`.
unsafe fn drain(mft: &IMFTransform, provides_samples: bool, out_size: u32) -> windows::core::Result<Vec<IMFSample>> {
    let mut out = Vec::new();
    loop {
        let sample = if provides_samples {
            None
        } else {
            let s = MFCreateSample()?;
            s.AddBuffer(&MFCreateMemoryBuffer(out_size)?)?;
            Some(s)
        };
        let mut buf = [MFT_OUTPUT_DATA_BUFFER { dwStreamID: 0, pSample: ManuallyDrop::new(sample), dwStatus: 0, pEvents: ManuallyDrop::new(None) }];
        let mut status = 0u32;
        match mft.ProcessOutput(0, &mut buf, &mut status) {
            Ok(()) => {
                if let Some(s) = ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pSample, ManuallyDrop::new(None))) {
                    out.push(s);
                }
                let _ = ManuallyDrop::into_inner(std::mem::replace(&mut buf[0].pEvents, ManuallyDrop::new(None)));
            }
            Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => break,
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}

pub struct MfH264Encoder {
    mft: IMFTransform,
    cfg: VideoConfig,
    avcc: Vec<u8>,
    provides: bool,
    out_size: u32,
}

// The transform is only used through &mut self from one thread at a time.
unsafe impl Send for MfH264Encoder {}

impl MfH264Encoder {
    pub fn new(cfg: &VideoConfig) -> Result<Self> {
        init()?;
        unsafe {
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264EncoderMFT, None, CLSCTX_INPROC_SERVER).map_err(|e| err("no H.264 encoder", e))?;
            let fps = (cfg.fps_num, cfg.fps_den.max(1));
            let out_t = video_type(&MFVideoFormat_H264, cfg.width, cfg.height, fps).map_err(|e| err("encoder type", e))?;
            out_t.SetUINT32(&MF_MT_AVG_BITRATE, cfg.bitrate_kbps.saturating_mul(1000)).map_err(|e| err("bitrate", e))?;
            out_t.SetUINT32(&MF_MT_MPEG2_PROFILE, 100).map_err(|e| err("profile", e))?;
            mft.SetOutputType(0, &out_t, 0).map_err(|e| err("encoder output type", e))?;
            let in_t = video_type(&MFVideoFormat_NV12, cfg.width, cfg.height, fps).map_err(|e| err("encoder input type", e))?;
            mft.SetInputType(0, &in_t, 0).map_err(|e| err("encoder input type", e))?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(|e| err("start", e))?;
            mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(|e| err("start", e))?;
            let info = mft.GetOutputStreamInfo(0).map_err(|e| err("stream info", e))?;
            let provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
            Ok(Self { mft, cfg: cfg.clone(), avcc: Vec::new(), provides, out_size: info.cbSize.max(1 << 20) })
        }
    }

    fn collect(&mut self, samples: Vec<IMFSample>) -> Result<Vec<Packet>> {
        let mut out = Vec::new();
        for s in samples {
            let (bytes, pts) = unsafe { (sample_bytes(&s).map_err(|e| err("read output", e))?, s.GetSampleTime().unwrap_or(0) / 10) };
            let (data, ps, key) = annexb_to_avcc(&bytes);
            if let Some((sps, pps)) = ps {
                if self.avcc.is_empty() {
                    self.avcc = build_avcc(&sps, &pps);
                }
            }
            if !data.is_empty() {
                out.push(Packet { data, pts_us: pts, key });
            }
        }
        Ok(out)
    }
}

impl VideoEncoder for MfH264Encoder {
    fn name(&self) -> &'static str {
        "Media Foundation"
    }

    fn extradata(&self) -> Vec<u8> {
        self.avcc.clone()
    }

    fn encode(&mut self, f: &I420, pts_us: i64) -> Result<Vec<Packet>> {
        if f.width != self.cfg.width || f.height != self.cfg.height {
            return Err(Error::Config("frame size changed mid-stream".into()));
        }
        let (w, h) = (f.width as usize, f.height as usize);
        // I420 -> NV12
        let mut nv = Vec::with_capacity(w * h * 3 / 2);
        nv.extend_from_slice(&f.y);
        for i in 0..(w / 2) * (h / 2) {
            nv.push(f.u[i]);
            nv.push(f.v[i]);
        }
        let dur = self.cfg.frame_duration_us() * 10;
        let samples = unsafe {
            let s = sample_from(&nv, pts_us * 10, dur).map_err(|e| err("input sample", e))?;
            self.mft.ProcessInput(0, &s, 0).map_err(|e| err("encode", e))?;
            drain(&self.mft, self.provides, self.out_size).map_err(|e| err("encode output", e))?
        };
        self.collect(samples)
    }

    fn flush(&mut self) -> Result<Vec<Packet>> {
        let samples = unsafe {
            self.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0).map_err(|e| err("flush", e))?;
            self.mft.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0).map_err(|e| err("flush", e))?;
            drain(&self.mft, self.provides, self.out_size).map_err(|e| err("flush output", e))?
        };
        self.collect(samples)
    }
}

pub struct MfH264Decoder {
    mft: IMFTransform,
    params: Vec<Vec<u8>>,
    length_size: usize,
    provides: bool,
    out_size: u32,
    width: u32,
    height: u32,
    stride: u32,
    t: i64,
}

unsafe impl Send for MfH264Decoder {}

impl MfH264Decoder {
    pub fn new(params: &[Vec<u8>], length_size: usize) -> Result<Self> {
        init()?;
        unsafe {
            let mft: IMFTransform = CoCreateInstance(&CLSID_MSH264DecoderMFT, None, CLSCTX_INPROC_SERVER).map_err(|e| err("no H.264 decoder", e))?;
            let in_t = MFCreateMediaType().map_err(|e| err("decoder type", e))?;
            in_t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).map_err(|e| err("decoder type", e))?;
            in_t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).map_err(|e| err("decoder type", e))?;
            mft.SetInputType(0, &in_t, 0).map_err(|e| err("decoder input type", e))?;
            let mut d = Self { mft, params: params.to_vec(), length_size, provides: false, out_size: 0, width: 0, height: 0, stride: 0, t: 0 };
            d.pick_output()?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0).map_err(|e| err("start", e))?;
            d.mft.ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0).map_err(|e| err("start", e))?;
            Ok(d)
        }
    }

    /// Choose NV12 output and read its size.
    unsafe fn pick_output(&mut self) -> Result<()> {
        let mut i = 0;
        loop {
            let t = self.mft.GetOutputAvailableType(0, i).map_err(|e| err("no NV12 output", e))?;
            if t.GetGUID(&MF_MT_SUBTYPE).map(|g| g == MFVideoFormat_NV12).unwrap_or(false) {
                self.mft.SetOutputType(0, &t, 0).map_err(|e| err("decoder output type", e))?;
                let fs = t.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
                self.width = (fs >> 32) as u32;
                self.height = fs as u32;
                self.stride = t.GetUINT32(&MF_MT_DEFAULT_STRIDE).unwrap_or(self.width);
                let info = self.mft.GetOutputStreamInfo(0).map_err(|e| err("stream info", e))?;
                self.provides = info.dwFlags & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32 != 0;
                self.out_size = info.cbSize.max(self.width * self.height * 3 / 2);
                return Ok(());
            }
            i += 1;
        }
    }

    /// Feed one length-prefixed access unit; returns the picture when one is ready.
    pub fn decode(&mut self, sample: &[u8]) -> Result<Option<I420>> {
        let key = sample.len() > self.length_size && {
            // an IDR slice carries the parameter sets in front of it for the decoder
            let mut i = 0;
            let mut found = false;
            while i + self.length_size < sample.len() {
                let mut n = 0usize;
                for k in 0..self.length_size {
                    n = (n << 8) | sample[i + k] as usize;
                }
                i += self.length_size;
                if i < sample.len() && sample[i] & 0x1F == 5 {
                    found = true;
                    break;
                }
                i += n;
            }
            found
        };
        let annexb = avcc_to_annexb(sample, self.length_size, if key { Some(&self.params) } else { None });
        unsafe {
            let s = sample_from(&annexb, self.t, 333_333).map_err(|e| err("input sample", e))?;
            self.t += 333_333;
            match self.mft.ProcessInput(0, &s, 0) {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_NOTACCEPTING => {}
                Err(e) => return Err(err("decode", e)),
            }
            loop {
                match drain(&self.mft, self.provides, self.out_size) {
                    Ok(v) => {
                        let Some(s) = v.into_iter().last() else { return Ok(None) };
                        let nv = sample_bytes(&s).map_err(|e| err("read picture", e))?;
                        return Ok(Some(self.to_i420(&nv)));
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => self.pick_output()?,
                    Err(e) => return Err(err("decode output", e)),
                }
            }
        }
    }

    fn to_i420(&self, nv: &[u8]) -> I420 {
        let (w, h, st) = (self.width as usize, self.height as usize, (self.stride as usize).max(self.width as usize));
        let mut f = I420::new(self.width & !1, self.height & !1);
        let (cw, ch) = (w / 2, h / 2);
        for r in 0..h.min(f.height as usize) {
            f.y[r * w..r * w + w].copy_from_slice(&nv[r * st..r * st + w]);
        }
        let uv = &nv[st * h..];
        for r in 0..ch {
            for x in 0..cw {
                f.u[r * cw + x] = uv[r * st + x * 2];
                f.v[r * cw + x] = uv[r * st + x * 2 + 1];
            }
        }
        f
    }
}
