# Changelog

## Unreleased

- **Jump cuts.** *Auto jump cuts* finds the pauses in your speech and switches those stretches off:
  they stay on the timeline, greyed out, but playback skips them and the export leaves them out.
  Drag a jump cut's edges to adjust it, press X to switch any clip on or off, and *Turn all jump cuts
  off* brings everything back.
- **Talking-head videos.** A new *Camera* capture mode records only the camera (no screen) and the
  whole editor still works. *9:16* records vertically (a landscape camera is cropped around its
  centre, an upright phone feed is used as it is) and also keeps a dragged area 9:16. Camera-only
  recordings open with their jump cuts already made.
- **Phone as the camera.** A phone plugged in by USB (iPhone through Continuity Camera, Android 14+
  in Webcam mode, or Camo / DroidCam) is listed in the camera menu, which looks again every time it
  opens. Cameras are now found through AVFoundation on macOS, so external and Continuity cameras
  appear.

## 0.4.0: native rewrite

ScreenCraft is now a single native Rust application (egui + wgpu), with the same look, layout and
features as 0.3. The Tauri / web UI is gone.

- **Capture engines.** ScreenCaptureKit on macOS, DXGI Desktop Duplication on Windows and the
  xdg-desktop-portal ScreenCast + PipeWire stream on Linux (Wayland and X11), behind one `Capturer`
  trait. Frames are re-timed to a constant frame rate before encoding.
- **Audio capture.** Microphone through cpal everywhere, system audio from ScreenCaptureKit (macOS), WASAPI
  loopback (Windows) and PulseAudio / PipeWire monitor sources (Linux). Every stream is stamped on one
  monotonic clock and aligned to the first video frame (gaps filled, overlap trimmed, drift nudged).
- **Encoding.** New sans-I/O encode stack: H.264 through the operating system's own licensed codec
  (VideoToolbox on macOS, Media Foundation on Windows; no H.264 code is bundled), AV1 (rav1e) and
  VP9 (libvpx, optional feature), Opus audio, MP4 and WebM muxers that describe file
  writes instead of performing them. Exports are byte-identical for the same input regardless of thread
  count; a threaded wrapper overlaps rendering and encoding.
- **Editor, matching 0.3.** The 0.3 colours, layout, nine inspector panels, six-track timeline,
  export dialog, presets, raw-file menu and About / updates window, drawn natively. The wgpu
  compositor is shared by the preview and the exporter: wallpapers (with motion), gradients, images,
  blur, padding, corners, shadow, **device frames**, camera zoom / pan, cursor, click effects,
  highlight, the **camera bubble and layouts**, **annotations** (including blur, pixelate and
  spotlight), shortcut display, **captions** (on-device whisper.cpp, SRT), watermark, and dissolve /
  fade / flash / blur / slide / zoom **transitions**.
- **Audio editing.** Levels, **music** with fades and ducking under speech, click and typing **sound
  effects**, microphone levelling.
- **Recorder.** The floating bar with Display / Window / Area (drag out the area), camera, microphone
  and system-audio inputs with meters, quality menu, Projects menu, countdown, HUD, discard, and the
  permissions window. The camera is recorded alongside the screen.
- **Export.** MP4 (H.264 or AV1), WebM, **GIF**, motion blur and a **compress after export** pass; the
  standalone **compressor** window; the notify-only **update check**.
- **Projects.** One window per project, unsaved-edit prompt, zooms generated the first time a
  recording opens. 0.3 `.screencraft` packages (including their fragmented-MP4 camera files and
  separate microphone files) open as they are.
- **Platform.** The app is dark only, like 0.3, and handles high-DPI displays.
- **Installers.** Named after the version on both platforms: `screencraft_0_4_0.dmg` and
  `screencraft_0_4_0.exe`.

## 0.3.4

- **Updates open without the "could not verify" warning.** Download in About, Privacy & Updates now
  fetches the installer itself (to Downloads) and opens it, so macOS does not quarantine it the
  way a browser download is. Nothing is installed automatically; you still drag it across.

## 0.3.3

- **Update-flow test release.** No functional changes from 0.3.2; published to check that the update
  notification and download flow works end to end.

## 0.3.2

- **Updates are easier to spot.** When a newer version is found at startup, the editor's Updates
  button turns green and names the version.
- **Report a bug** from the editor: a bug icon next to GitHub opens the repo's issues page.
- **Area picking fixes.** Esc while an area is chosen (before recording) opens the selector again so
  you can redraw it, and ScreenCraft always starts on Display instead of a stale Area or Window.
- **whisper.cpp now comes from our fork** (a git submodule), which is what builds the caption engine.
  Clone with `--recurse-submodules`.

## 0.3.1

- **Recording controls.** The record bar is now locked bottom-centre on the main display. While recording, the
  timer and Stop move freely (and sit just outside an area's border); a trash button scraps the
  whole recording after a confirmation. Area recordings get a red, pulsing outline with a REC tag,
  and the menu bar shows a recording indicator with the elapsed time.
- **Window and area picking.** Hovering a window in the list outlines it on screen. Choosing an area
  dims every display so you can start dragging on any of them.
- **Fixes.** Stop always stops (a camera or microphone recorder that had already ended could block it),
  the menu-bar "Stop Recording" is only enabled while recording, and Windows no longer offers
  "Compress after export", which could not work there and made exports fail.
- **Update notifications (never automatic).** At startup ScreenCraft can check, with one small
  request that sends only its version number, whether a newer version exists. If so, About, Privacy
  & Updates tells you and offers the download page; nothing is downloaded or installed by itself.
  Turn it off in that window, or press Check now to look on demand.
- **About, Privacy & Updates** window (menu-bar icon and the ScreenCraft menu) with the privacy
  notice, update status and the on/off switch.
- **Captions that read like ClipCraft's.** Generated captions are timed word by word and cut into
  short lines (up to 3 words, broken at sentence ends, commas and pauses). Each line is held a
  moment so nothing flickers, consecutive lines swap instantly, and only a line after a pause eases
  in and fades out.
- **Spoken-word highlight.** The word being said is coloured as it is spoken, in a colour of your
  choice (on any preset). Imported or edited captions spread the words evenly across the line.
- **More accurate recognition.** The default model is now small (English), and a most-accurate
  multilingual model (large-v3-turbo) is available. Models already downloaded by ClipCraft are reused.
- **TikTok preset reworked:** white text with a black stroke, caps on, 70 px, 60% wide, 80% height,
  7 px stroke, spoken word highlighted. TikTok red and Highlight presets are gone; it is the default.
- **Stroke width** is adjustable from 0 to 30 px, with its own colour.
- **Select several captions** (⌘/Ctrl-click, Shift-click for a range, ⌘/Ctrl + A) to move or delete
  them together.

- **Bluetooth headphones stay in stereo while you record.** Opening a Bluetooth headset's microphone
  makes the headphones drop to call quality (16 kHz mono), which ruined what you heard and what was
  recorded. ScreenCraft now prefers the Mac's own microphone, never opens a headset microphone by
  accident (not even to fill the device list), and while a microphone is in use it pins the system's
  default input to the built-in one, restoring it when you turn the microphone off or quit. Picking
  a Bluetooth microphone on purpose still works and says what it costs.

## 0.3.0

- **Auto captions:** *Captions → Generate captions* transcribes the microphone on your machine with
  [whisper.cpp](https://github.com/ggml-org/whisper.cpp). Pick a model (tiny / base / small English, or
  multilingual base); it is downloaded once on request. Nothing is uploaded.
- **Device frames:** laptop, monitor, tablet, phone and browser window around the screen.
- **Animated wallpapers:** subtle motion (drift, bokeh, waves, dust) on any wallpaper, identical in preview and export.
- Legal and compliance: `THIRD-PARTY-NOTICES.md`, `COMPLIANCE.md`, `SECURITY.md`, license/disclaimer in the README.
- A website (Vite, in `website/`) with docs, downloads and legal pages, deployed on Cloudflare.

## 0.2.0

- Sped-up clips show one compact pill ("2×"), and a speed ramp is one block ("2.3× → 4.2×")
  instead of a row of slivers; zoom regions over a ramp stay one block too.
- Watermark: upload a logo, place it on a 3×3 grid, set size and opacity (also in exports).
- Zoom regions always follow the cursor (the manual option is gone).
- New shortcuts: Q / E jump to the start / end, A / D jump to the previous / next zoom start.
  Duplicate moved to ⌘D.

- The cursor now hides when you start typing and stays hidden until the mouse moves again
  (shortcuts like ⌘C do not hide it).

**Editor**
- Annotations: text, arrows, boxes, circles, highlights, blur, pixelate and spotlight,
  pinned to the recording or fixed on the frame, dragged on the preview.
- Captions: type or import SRT, TikTok-style default (white text, heavy black outline, middle
  of the bottom third) plus TikTok red, highlight, classic and minimal presets; export SRT.
- Shortcut display: pops up recorded shortcuts and special keys (only those are stored).
- Camera layouts track: large bubble, camera only, side by side (either side), camera off.
  Every block can have its own camera settings (size, position, shape, mirror, border, shadow,
  flush-to-edge) and the camera eases smoothly between blocks. Nine anchor points for the
  bubble and a flush-to-edge option.
- Pace: speed up idle stretches, speed ramps, freeze frame and cut transitions (cross-dissolve,
  fade, flash, blur, slide, zoom).
- Music fades and ducking under speech; built-in click and typing sound effects.
- Hold Space for 2x; timeline zoom; multi-select zoom regions (shift-click) that edit and move
  together; draggable timeline and side-panel dividers; Esc closes dialogs; presets keep 2%
  padding and can be renamed.

**Recorder**
- Spinners while inputs load and Record stays disabled until ready; cancel the countdown.
- The preview proxy builds in the background as soon as a recording stops; long recordings get
  a lighter proxy.

**Export**
- Roughly 40-65 fps at 1080p60 with source frames scaled to what each frame needs.
- No more silent hangs: if the hardware video encoder stops answering (the export used to sit
  at "Rendering frame 13" forever), the export now notices, retries once with the software
  encoder, and otherwise shows an error. Cancel works while it waits, and reading a frame
  from the recording times out instead of waiting forever.

**Platform**
- Windows support (thanks @PierrunoYT) and an NSIS installer script.
- New app icon, DMG and installer scripts.

## 0.1.0

First public version: recording, editing, zoom, export and compression on macOS.
