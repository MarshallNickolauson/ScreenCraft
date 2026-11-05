use super::annotate::note_row;
use super::*;
use crate::editor::{ops, Sel};
use crate::srt;
use crate::theme::fmt_time;
use crate::whisper::{CapEvent, Caption};
use std::sync::mpsc::Receiver;

/// Style presets; position and width are left as the user set them, except TikTok's own.
pub fn preset(name: &str, s: &mut screencraft_core::project::SubtitleStyle) {
    match name {
        "tiktok" => {
            s.style = "tiktok".into();
            s.size = 70.0;
            s.color = "#ffffff".into();
            s.outline_color = "#000000".into();
            s.outline_width = 7.0;
            s.background = String::new();
            s.uppercase = true;
            s.pop = true;
            s.highlight = "#ffe14a".into();
            s.y = 0.8;
            s.width = 0.6;
        }
        "classic" => {
            s.style = "classic".into();
            s.size = 46.0;
            s.color = "#ffffff".into();
            s.outline_color = "#000000".into();
            s.outline_width = 0.0;
            s.background = "#000000b3".into();
            s.uppercase = false;
            s.pop = false;
            s.highlight = String::new();
        }
        _ => {
            s.style = "minimal".into();
            s.size = 42.0;
            s.color = "#ffffff".into();
            s.outline_color = "#000000".into();
            s.outline_width = 3.0;
            s.background = String::new();
            s.uppercase = false;
            s.pop = false;
            s.highlight = String::new();
        }
    }
}

const MODELS: [(&str, &str); 5] = [
    ("small.en", "Accurate · English · 488 MB"),
    ("large-v3-turbo-q5_0", "Most accurate · multilingual · 574 MB"),
    ("base.en", "Balanced · English · 148 MB"),
    ("tiny.en", "Fastest · English · 78 MB"),
    ("base", "Multilingual · 148 MB"),
];

#[derive(Clone, Copy)]
enum CaptionsSection {
    AutoCaptions,
    Content,
    Style,
}

pub struct CapState {
    pub model: String,
    pub language: String,
    pub busy: bool,
    pub msg: String,
    pub rx: Option<Receiver<CapEvent>>,
    pub done: Option<Result<Vec<Caption>, String>>,
}

impl Default for CapState {
    fn default() -> Self {
        Self { model: "small.en".into(), language: "auto".into(), busy: false, msg: String::new(), rx: None, done: None }
    }
}

impl CapState {
    pub fn poll(&mut self, ctx: &egui::Context) {
        let mut finished = false;
        if let Some(rx) = &self.rx {
            while let Ok(ev) = rx.try_recv() {
                match ev {
                    CapEvent::Progress(m) => self.msg = m,
                    CapEvent::Done(r) => {
                        self.done = Some(r);
                        finished = true;
                    }
                }
            }
            if self.busy {
                ctx.request_repaint_after(std::time::Duration::from_millis(150));
            }
        }
        if finished {
            self.busy = false;
            self.rx = None;
            self.msg.clear();
        }
    }
}

pub fn show(st: &mut EditorState, ui: &mut Ui) {
    let sty = st.project.config.subtitles.clone();
    if !matches!(sty.style.as_str(), "tiktok" | "classic" | "minimal") {
        st.edit(|p| preset("tiktok", &mut p.config.subtitles));
    }
    let mut list = st.project.scene.subtitles.clone();
    list.sort_by(|a, b| a.source_start_ms.partial_cmp(&b.source_start_ms).unwrap());
    let cap = if let Sel::Caption(id) = &st.sel { list.iter().find(|c| &c.id == id).cloned() } else { None };
    let multi = st.selected_caption_ids();
    kit::h3(ui, "Captions", true);
    note(ui, "Type captions or import an SRT file. They sit in the middle of the bottom third of the video, like TikTok.");
    let mut jump_to: Option<CaptionsSection> = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(egui::RichText::new("Jump to").size(13.0).color(crate::theme::MUTED));
        if kit::pill(ui, None, "Auto captions", false, false, false, None).clicked() {
            jump_to = Some(CaptionsSection::AutoCaptions);
        }
        let content_label = if cap.is_some() || multi.len() > 1 {
            Some("Selected caption")
        } else if !list.is_empty() {
            Some("Caption list")
        } else {
            None
        };
        if let Some(label) = content_label {
            if kit::pill(ui, None, label, false, false, false, None).clicked() {
                jump_to = Some(CaptionsSection::Content);
            }
        }
        if kit::pill(ui, None, "Style", false, false, false, None).clicked() {
            jump_to = Some(CaptionsSection::Style);
        }
    });
    let auto_section = section_marker(ui);
    // on-device speech recognition (whisper.cpp); runs locally, nothing is uploaded
    kit::h3(ui, "Auto captions", false);
    note(ui, "Transcribes the microphone on this computer with whisper.cpp. The first run downloads the model you pick (once); the audio never leaves your machine.");
    ui.add_space(6.0);
    kit::muted(ui, "Model");
    let cur_label = MODELS.iter().find(|m| m.0 == st.panel.caps.model).map(|m| m.1).unwrap_or("");
    let labels: Vec<&str> = MODELS.iter().map(|m| m.1).collect();
    if let Some(i) = kit::select(ui, "whisper-model", cur_label, &labels, ui.available_width() - 28.0) {
        st.panel.caps.model = MODELS[i].0.to_string();
    }
    if !st.panel.caps.model.ends_with(".en") {
        let langs: Vec<(String, &str)> = vec![("auto".into(), "Auto"), ("en".into(), "English"), ("es".into(), "Spanish"), ("fr".into(), "French"), ("de".into(), "German")];
        let mut l = st.panel.caps.language.clone();
        if kit::segmented(ui, Some("Language"), &langs, &mut l) {
            st.panel.caps.language = l;
        }
    }
    let busy = st.panel.caps.busy;
    if ui.add_enabled_ui(!busy, |ui| kit::btn_line(ui, if busy { "Working…" } else { "Generate captions" }, false)).inner.clicked() && !busy {
        st.start_captions();
    }
    if busy {
        kit::hint(ui, &st.panel.caps.msg.clone());
    }
    kit::hint(ui, "Speech recognition by whisper.cpp (github.com/ggml-org/whisper.cpp, MIT) with OpenAI Whisper models (MIT).");
    let w = (ui.available_width() - 12.0) / 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        if kit::btn_line_w(ui, "Add caption", false, w).clicked() {
            st.add_caption();
        }
        if kit::btn_line_w(ui, "Import SRT…", false, w).clicked() {
            st.import_srt();
        }
        if kit::btn_line_w(ui, "Export SRT…", false, w).clicked() {
            st.export_srt();
        }
    });

    let content_section = section_marker(ui);
    if cap.is_some() && multi.len() > 1 {
        kit::h3(ui, &format!("{} captions selected", multi.len()), false);
        kit::hint(ui, "Drag any selected caption to move them all together. ⌘ / Ctrl-click adds or removes one, Shift-click selects a range, ⌘ / Ctrl + A selects all.");
        if kit::btn_line(ui, &format!("Delete {} captions", multi.len()), true).clicked() {
            st.delete_selected();
        }
        if kit::btn_line(ui, "Deselect", false).clicked() {
            st.select(Sel::None);
        }
    } else if let Some(c) = cap {
        kit::h3(ui, "Selected caption", false);
        ui.add_space(6.0);
        kit::muted(ui, "Text");
        let mut text = c.text.clone();
        if ui.add(egui::TextEdit::multiline(&mut text).desired_rows(3).desired_width(f32::INFINITY).margin(egui::Margin::symmetric(10, 8))).changed() {
            let id = c.id.clone();
            st.edit_keyed(Some(&format!("cap-{id}")), |p| {
                if let Some(x) = p.scene.subtitles.iter_mut().find(|x| x.id == id) {
                    x.text = text;
                }
            });
        }
        kit::hint(ui, &format!("{:.1} s. Drag the block on the Captions track to move it, its ends to trim.", (c.source_end_ms - c.source_start_ms) / 1000.0));
        if kit::btn_line(ui, "Delete caption", true).clicked() {
            st.delete_selected();
        }
    } else if !list.is_empty() {
        kit::h3(ui, &format!("{} caption{}", list.len(), if list.len() == 1 { "" } else { "s" }), false);
        for c in list.iter().take(60) {
            let t0 = ops::src_to_out(&st.project.scene.slices, c.source_start_ms);
            let txt: String = c.text.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(34).collect();
            let label = format!("{}  {}", t0.map(fmt_time).unwrap_or_else(|| "—".into()), if txt.is_empty() { "(empty)".to_string() } else { txt });
            if note_row(ui, &label).clicked() {
                st.select(Sel::Caption(c.id.clone()));
                st.seek(t0.unwrap_or(0.0));
            }
        }
    }

    let style_section = section_marker(ui);
    kit::h3(ui, "Style", false);
    let presets: Vec<(String, &str)> = vec![("tiktok".into(), "TikTok"), ("classic".into(), "Classic"), ("minimal".into(), "Minimal")];
    seg(st, ui, Some("Preset"), &presets, sty.style.clone(), |p, v| preset(&v, &mut p.config.subtitles));
    let sty = st.project.config.subtitles.clone();
    slider(st, ui, "sub-size", "Size", sty.size, (24.0, 120.0, 1.0), px, |p, v| p.config.subtitles.size = v);
    slider(st, ui, "sub-y", "Height", sty.y, (0.15, 0.95, 0.01), pct, |p, v| p.config.subtitles.y = v);
    slider(st, ui, "sub-w", "Width", sty.width, (0.2, 0.95, 0.01), pct, |p, v| p.config.subtitles.width = v);
    color(st, ui, "sub-col", "Text color", &sty.color, |p, v| p.config.subtitles.color = v);
    toggle(st, ui, "Highlight spoken word", Some("Colours each word as it is said."), !sty.highlight.is_empty(), |p, v| p.config.subtitles.highlight = if v { "#ffe14a".into() } else { String::new() });
    if !sty.highlight.is_empty() {
        color(st, ui, "sub-hl", "Highlight color", &sty.highlight, |p, v| p.config.subtitles.highlight = v);
    }
    slider(st, ui, "sub-ow", "Stroke width", sty.outline_width, (0.0, 30.0, 1.0), px, |p, v| p.config.subtitles.outline_width = v);
    if sty.outline_width > 0.0 {
        color(st, ui, "sub-oc", "Stroke color", &sty.outline_color, |p, v| p.config.subtitles.outline_color = v);
    }
    toggle(st, ui, "Capital letters", None, sty.uppercase, |p, v| p.config.subtitles.uppercase = v);
    toggle(st, ui, "Pop in", Some("A quick bounce when each caption appears."), sty.pop, |p, v| p.config.subtitles.pop = v);
    toggle(st, ui, "Box behind text", None, !sty.background.is_empty(), |p, v| p.config.subtitles.background = if v { "#000000b3".into() } else { String::new() });
    if let Some(target) = jump_to {
        let rect = match target {
            CaptionsSection::AutoCaptions => auto_section,
            CaptionsSection::Content => content_section,
            CaptionsSection::Style => style_section,
        };
        ui.scroll_to_rect(rect, Some(egui::Align::Min));
    }
    let _ = srt::parse;
}

fn section_marker(ui: &Ui) -> egui::Rect {
    let cursor = ui.cursor();
    egui::Rect::from_min_size(cursor.min, egui::vec2(cursor.width(), 1.0))
}
