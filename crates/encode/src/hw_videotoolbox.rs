//! Hardware H.264 on macOS through VideoToolbox (raw C API, no Objective-C needed).
//! One synchronous session; frames come back in decode order with `AllowFrameReordering` off.

#![allow(non_snake_case, non_upper_case_globals, non_camel_case_types)]

use crate::codec::VideoEncoder;
use crate::h264::build_avcc;
use crate::types::*;
use crate::{Error, Result};
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

type OSStatus = i32;
type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CVPixelBufferRef = *mut c_void;
type CMSampleBufferRef = *const c_void;
type VTCompressionSessionRef = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTime {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

impl CMTime {
    fn us(v: i64) -> Self {
        Self { value: v, timescale: 1_000_000, flags: 1, epoch: 0 }
    }
    fn invalid() -> Self {
        Self { value: 0, timescale: 0, flags: 0, epoch: 0 }
    }
}

type OutputCallback = extern "C" fn(*mut c_void, *mut c_void, OSStatus, u32, CMSampleBufferRef);

#[link(name = "VideoToolbox", kind = "framework")]
extern "C" {
    static kVTCompressionPropertyKey_RealTime: CFStringRef;
    static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    static kVTCompressionPropertyKey_AllowFrameReordering: CFStringRef;
    static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    static kVTProfileLevel_H264_High_AutoLevel: CFStringRef;
    static kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder: CFStringRef;

    fn VTCompressionSessionCreate(
        allocator: CFAllocatorRef,
        width: i32,
        height: i32,
        codec: u32,
        encoder_spec: CFTypeRef,
        source_attrs: CFTypeRef,
        compressed_allocator: CFAllocatorRef,
        callback: OutputCallback,
        refcon: *mut c_void,
        out: *mut VTCompressionSessionRef,
    ) -> OSStatus;
    fn VTSessionSetProperty(session: VTCompressionSessionRef, key: CFStringRef, value: CFTypeRef) -> OSStatus;
    fn VTCompressionSessionPrepareToEncodeFrames(session: VTCompressionSessionRef) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(
        session: VTCompressionSessionRef,
        image: CVPixelBufferRef,
        pts: CMTime,
        duration: CMTime,
        frame_props: CFTypeRef,
        source_refcon: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    fn VTCompressionSessionCompleteFrames(session: VTCompressionSessionRef, until: CMTime) -> OSStatus;
    fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);
}

#[link(name = "CoreMedia", kind = "framework")]
extern "C" {
    static kCMSampleAttachmentKey_NotSync: CFStringRef;
    fn CMSampleBufferGetDataBuffer(sbuf: CMSampleBufferRef) -> *const c_void;
    fn CMSampleBufferGetFormatDescription(sbuf: CMSampleBufferRef) -> *const c_void;
    fn CMSampleBufferGetPresentationTimeStamp(sbuf: CMSampleBufferRef) -> CMTime;
    fn CMSampleBufferGetSampleAttachmentsArray(sbuf: CMSampleBufferRef, create: u8) -> *const c_void;
    fn CMBlockBufferGetDataPointer(
        buf: *const c_void,
        offset: usize,
        length_at_offset: *mut usize,
        total_length: *mut usize,
        data: *mut *mut u8,
    ) -> OSStatus;
    fn CMBlockBufferGetDataLength(block: *const c_void) -> usize;
    fn CMBlockBufferCopyDataBytes(block: *const c_void, offset: usize, len: usize, dest: *mut c_void) -> OSStatus;
    fn CMVideoFormatDescriptionGetH264ParameterSetAtIndex(
        desc: *const c_void,
        index: usize,
        ptr_out: *mut *const u8,
        size_out: *mut usize,
        count_out: *mut usize,
        nal_header_len_out: *mut i32,
    ) -> OSStatus;
}

#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferCreate(
        allocator: CFAllocatorRef,
        width: usize,
        height: usize,
        format: u32,
        attrs: CFTypeRef,
        out: *mut CVPixelBufferRef,
    ) -> i32;
    fn CVPixelBufferLockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferUnlockBaseAddress(pb: CVPixelBufferRef, flags: u64) -> i32;
    fn CVPixelBufferGetBaseAddressOfPlane(pb: CVPixelBufferRef, plane: usize) -> *mut u8;
    fn CVPixelBufferGetBytesPerRowOfPlane(pb: CVPixelBufferRef, plane: usize) -> usize;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    static kCFBooleanFalse: CFTypeRef;
    static kCFAllocatorDefault: CFAllocatorRef;
    fn CFNumberCreate(alloc: CFAllocatorRef, ty: i32, value: *const c_void) -> CFTypeRef;
    fn CFRelease(obj: CFTypeRef);
    fn CFArrayGetCount(a: *const c_void) -> isize;
    fn CFArrayGetValueAtIndex(a: *const c_void, i: isize) -> *const c_void;
    fn CFDictionaryContainsKey(d: *const c_void, key: *const c_void) -> u8;
    fn CFDictionaryCreate(
        alloc: CFAllocatorRef,
        keys: *const *const c_void,
        values: *const *const c_void,
        n: isize,
        key_cbs: *const c_void,
        val_cbs: *const c_void,
    ) -> CFTypeRef;
    static kCFTypeDictionaryKeyCallBacks: c_void;
    static kCFTypeDictionaryValueCallBacks: c_void;
}

const kCFNumberSInt32Type: i32 = 3;
const kCFNumberFloat64Type: i32 = 6;
const kCMVideoCodecType_H264: u32 = 0x6176_6331; // 'avc1'
const kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange: u32 = 0x3432_3076; // '420v'

#[derive(Default)]
struct Shared {
    packets: Vec<Packet>,
    avcc: Vec<u8>,
    error: Option<String>,
}

extern "C" fn on_output(refcon: *mut c_void, _src: *mut c_void, status: OSStatus, _flags: u32, sbuf: CMSampleBufferRef) {
    // SAFETY: refcon is the Arc<Mutex<Shared>> pointer we leaked in `new`, alive until Drop.
    let shared = unsafe { &*(refcon as *const Mutex<Shared>) };
    let mut sh = shared.lock().unwrap();
    if status != 0 || sbuf.is_null() {
        sh.error = Some(format!("VideoToolbox frame failed ({status})"));
        return;
    }
    unsafe {
        let block = CMSampleBufferGetDataBuffer(sbuf);
        if block.is_null() {
            return;
        }
        // copy the whole sample: the buffer may be made of several pieces, so a single pointer
        // can cover only the first of them
        let total = CMBlockBufferGetDataLength(block);
        let mut data = vec![0u8; total];
        if total == 0 || CMBlockBufferCopyDataBytes(block, 0, total, data.as_mut_ptr() as *mut c_void) != 0 {
            sh.error = Some("VideoToolbox returned an unreadable buffer".into());
            return;
        }

        let mut key = true;
        let att = CMSampleBufferGetSampleAttachmentsArray(sbuf, 0);
        if !att.is_null() && CFArrayGetCount(att) > 0 {
            let d = CFArrayGetValueAtIndex(att, 0);
            if CFDictionaryContainsKey(d, kCMSampleAttachmentKey_NotSync) != 0 {
                key = false;
            }
        }
        if key && sh.avcc.is_empty() {
            let desc = CMSampleBufferGetFormatDescription(sbuf);
            let (mut sps, mut pps) = (std::ptr::null::<u8>(), std::ptr::null::<u8>());
            let (mut sl, mut pl, mut cnt, mut nl) = (0usize, 0usize, 0usize, 0i32);
            let a = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, 0, &mut sps, &mut sl, &mut cnt, &mut nl);
            let b = CMVideoFormatDescriptionGetH264ParameterSetAtIndex(desc, 1, &mut pps, &mut pl, &mut cnt, &mut nl);
            if a == 0 && b == 0 && sl >= 4 {
                let s = std::slice::from_raw_parts(sps, sl);
                let p = std::slice::from_raw_parts(pps, pl);
                let mut rec = build_avcc(s, p);
                rec[4] = 0xFC | ((nl.clamp(1, 4) - 1) as u8);
                sh.avcc = rec;
            }
        }
        let t = CMSampleBufferGetPresentationTimeStamp(sbuf);
        let pts_us = if t.timescale > 0 { t.value * 1_000_000 / t.timescale as i64 } else { 0 };
        sh.packets.push(Packet { data, pts_us, key });
    }
}

pub struct VtH264Encoder {
    session: VTCompressionSessionRef,
    shared: *const Mutex<Shared>,
    width: usize,
    height: usize,
    frame_dur: i64,
}

// The session and refcon are only touched through &mut self, and VideoToolbox is thread-safe.
unsafe impl Send for VtH264Encoder {}

fn cf_i32(v: i32) -> CFTypeRef {
    unsafe { CFNumberCreate(kCFAllocatorDefault, kCFNumberSInt32Type, &v as *const i32 as *const c_void) }
}

impl VtH264Encoder {
    pub fn new(cfg: &VideoConfig) -> Result<Self> {
        let shared = Arc::into_raw(Arc::new(Mutex::new(Shared::default())));
        unsafe {
            // Require the hardware block so we fail fast (and fall back) on machines without it.
            let mut spec: CFTypeRef = std::ptr::null();
            if cfg.hardware == HwPreference::Hardware {
                let keys = [kVTVideoEncoderSpecification_RequireHardwareAcceleratedVideoEncoder as *const c_void];
                let vals = [kCFBooleanTrue];
                spec = CFDictionaryCreate(
                    kCFAllocatorDefault,
                    keys.as_ptr(),
                    vals.as_ptr(),
                    1,
                    &kCFTypeDictionaryKeyCallBacks as *const c_void,
                    &kCFTypeDictionaryValueCallBacks as *const c_void,
                );
            }
            let mut session: VTCompressionSessionRef = std::ptr::null_mut();
            let st = VTCompressionSessionCreate(
                kCFAllocatorDefault,
                cfg.width as i32,
                cfg.height as i32,
                kCMVideoCodecType_H264,
                spec,
                std::ptr::null(),
                std::ptr::null(),
                on_output,
                shared as *mut c_void,
                &mut session,
            );
            if !spec.is_null() {
                CFRelease(spec);
            }
            if st != 0 || session.is_null() {
                drop(Arc::from_raw(shared));
                return Err(Error::Codec(format!("VideoToolbox session failed ({st})")));
            }
            let set = |key: CFStringRef, val: CFTypeRef, own: bool| {
                VTSessionSetProperty(session, key, val);
                if own {
                    CFRelease(val);
                }
            };
            set(kVTCompressionPropertyKey_RealTime, kCFBooleanFalse, false);
            set(kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse, false);
            set(kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_High_AutoLevel, false);
            set(kVTCompressionPropertyKey_AverageBitRate, cf_i32((cfg.bitrate_kbps as i64 * 1000).min(i32::MAX as i64) as i32), true);
            set(kVTCompressionPropertyKey_MaxKeyFrameInterval, cf_i32(cfg.effective_keyint() as i32), true);
            let fps = cfg.fps();
            let n = CFNumberCreate(kCFAllocatorDefault, kCFNumberFloat64Type, &fps as *const f64 as *const c_void);
            set(kVTCompressionPropertyKey_ExpectedFrameRate, n, true);
            VTCompressionSessionPrepareToEncodeFrames(session);
            Ok(Self {
                session,
                shared,
                width: cfg.width as usize,
                height: cfg.height as usize,
                frame_dur: cfg.frame_duration_us(),
            })
        }
    }

    fn shared(&self) -> &Mutex<Shared> {
        unsafe { &*self.shared }
    }

    fn take(&mut self) -> Result<Vec<Packet>> {
        let mut sh = self.shared().lock().unwrap();
        if let Some(e) = sh.error.take() {
            return Err(Error::Codec(e));
        }
        Ok(std::mem::take(&mut sh.packets))
    }
}

impl VideoEncoder for VtH264Encoder {
    fn name(&self) -> &'static str {
        "VideoToolbox"
    }

    fn extradata(&self) -> Vec<u8> {
        self.shared().lock().unwrap().avcc.clone()
    }

    fn encode(&mut self, f: &I420, pts_us: i64) -> Result<Vec<Packet>> {
        if f.width as usize != self.width || f.height as usize != self.height {
            return Err(Error::Config("frame size changed mid-stream".into()));
        }
        unsafe {
            let mut pb: CVPixelBufferRef = std::ptr::null_mut();
            let st = CVPixelBufferCreate(
                kCFAllocatorDefault,
                self.width,
                self.height,
                kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                std::ptr::null(),
                &mut pb,
            );
            if st != 0 || pb.is_null() {
                return Err(Error::Codec(format!("CVPixelBufferCreate failed ({st})")));
            }
            CVPixelBufferLockBaseAddress(pb, 0);
            let (w, h) = (self.width, self.height);
            let yp = CVPixelBufferGetBaseAddressOfPlane(pb, 0);
            let ys = CVPixelBufferGetBytesPerRowOfPlane(pb, 0);
            for r in 0..h {
                std::ptr::copy_nonoverlapping(f.y.as_ptr().add(r * w), yp.add(r * ys), w);
            }
            let cp = CVPixelBufferGetBaseAddressOfPlane(pb, 1);
            let cs = CVPixelBufferGetBytesPerRowOfPlane(pb, 1);
            for r in 0..h / 2 {
                let row = cp.add(r * cs);
                for x in 0..w / 2 {
                    *row.add(x * 2) = f.u[r * (w / 2) + x];
                    *row.add(x * 2 + 1) = f.v[r * (w / 2) + x];
                }
            }
            CVPixelBufferUnlockBaseAddress(pb, 0);
            let st = VTCompressionSessionEncodeFrame(
                self.session,
                pb,
                CMTime::us(pts_us),
                CMTime::us(self.frame_dur),
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            );
            CFRelease(pb as CFTypeRef);
            if st != 0 {
                return Err(Error::Codec(format!("VideoToolbox encode failed ({st})")));
            }
        }
        self.take()
    }

    fn flush(&mut self) -> Result<Vec<Packet>> {
        unsafe {
            VTCompressionSessionCompleteFrames(self.session, CMTime::invalid());
        }
        self.take()
    }
}

impl Drop for VtH264Encoder {
    fn drop(&mut self) {
        unsafe {
            VTCompressionSessionCompleteFrames(self.session, CMTime::invalid());
            VTCompressionSessionInvalidate(self.session);
            CFRelease(self.session as CFTypeRef);
            drop(Arc::from_raw(self.shared));
        }
    }
}
