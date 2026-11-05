//! Popover menus (Presets, Raw files) and the About, privacy & updates window.

use super::{open_url, EditorState};
use crate::kit;
use crate::theme::*;
use crate::updates::{self, UpdateInfo};
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};
use screencraft_core::project::{CameraConfig, CursorConfig, Project, Styles};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::mpsc::{channel, Receiver};

pub struct Dialogs {
    pub about: bool,
    pub raw: bool,
    pub projects: bool,
    pub projects_anchor: Option<Rect>,
    pub raw_anchor: Option<Rect>,
    pub presets_open: bool,
    pub presets_manage: bool,
    pub presets_anchor: Option<Rect>,
    rename_buf: BTreeMap<String, String>,
    focus_name: Option<String>,
    about_info: Option<UpdateInfo>,
    checking: bool,
    check_rx: Option<Receiver<UpdateInfo>>,
    download_msg: Option<String>,
    download_pct: Option<f32>,
    download_rx: Option<Receiver<Result<std::path::PathBuf, String>>>,
    pct_rx: Option<Receiver<f32>>,
    asked: bool,
}

impl Default for Dialogs {
    fn default() -> Self {
        Self {
            about: false,
            raw: false,
            projects: false,
            projects_anchor: None,
            raw_anchor: None,
            presets_open: false,
            presets_manage: false,
            presets_anchor: None,
            rename_buf: BTreeMap::new(),
            focus_name: None,
            about_info: None,
            checking: false,
            check_rx: None,
            download_msg: None,
            download_pct: None,
            download_rx: None,
            pct_rx: None,
            asked: false,
        }
    }
}

impl Dialogs {
    pub fn update_available(&self) -> bool {
        updates::update_found().is_some()
    }
}

// ---- presets -----------------------------------------------------------------------------

fn presets_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|d| d.join("ScreenCraft").join("presets.json"))
}

fn user_presets() -> Vec<(String, Value)> {
    presets_path()
        .and_then(|p| std::fs::read(p).ok())
        .and_then(|b| serde_json::from_slice::<serde_json::Map<String, Value>>(&b).ok())
        .map(|m| m.into_iter().collect())
        .unwrap_or_default()
}

fn save_user_presets(all: &[(String, Value)]) {
    if let Some(p) = presets_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let m: serde_json::Map<String, Value> = all.iter().cloned().collect();
        let _ = std::fs::write(p, serde_json::to_vec_pretty(&m).unwrap_or_default());
    }
}

fn builtin() -> Vec<(&'static str, Value)> {
    const PAD: f64 = 0.02;
    vec![
        ("Clean", json!({ "styles": { "paddingRatio": PAD, "cornerRadius": 14, "background": { "kind": "wallpaper", "wallpaper": "sunset" } } })),
        ("Minimal", json!({ "styles": { "paddingRatio": PAD, "cornerRadius": 10, "background": { "kind": "color", "color": "#0b0c12" }, "shadow": { "intensity": 0.15 } } })),
        (
            "Vivid",
            json!({ "styles": { "paddingRatio": PAD, "cornerRadius": 24, "background": { "kind": "gradient", "gradientFrom": "#ff6ad5", "gradientTo": "#3b82c4", "gradientAngle": 135 } }, "cursor": { "clickEffect": "ripple", "size": 34 } }),
        ),
        ("Sunset", json!({ "styles": { "paddingRatio": PAD, "cornerRadius": 18, "background": { "kind": "wallpaper", "wallpaper": "sunset" } } })),
    ]
}

fn deep_merge(t: &mut Value, s: &Value) {
    if let (Some(to), Some(so)) = (t.as_object_mut(), s.as_object()) {
        for (k, v) in so {
            if v.is_object() {
                deep_merge(to.entry(k.clone()).or_insert_with(|| json!({})), v);
            } else {
                to.insert(k.clone(), v.clone());
            }
        }
    }
}

fn apply_preset(p: &mut Project, pr: &Value) {
    // go through JSON so partial presets merge field by field, exactly like the web editor
    for key in ["styles", "cursor", "camera"] {
        let Some(src) = pr.get(key) else { continue };
        match key {
            "styles" => {
                let mut v = serde_json::to_value(&p.config.styles).unwrap_or(Value::Null);
                deep_merge(&mut v, src);
                if let Ok(x) = serde_json::from_value::<Styles>(v) {
                    p.config.styles = x;
                }
            }
            "cursor" => {
                let mut v = serde_json::to_value(&p.config.cursor).unwrap_or(Value::Null);
                deep_merge(&mut v, src);
                if let Ok(x) = serde_json::from_value::<CursorConfig>(v) {
                    p.config.cursor = x;
                }
            }
            _ => {
                let mut v = serde_json::to_value(&p.config.camera).unwrap_or(Value::Null);
                deep_merge(&mut v, src);
                if let Ok(x) = serde_json::from_value::<CameraConfig>(v) {
                    p.config.camera = x;
                }
            }
        }
    }
    p.config.styles.padding_ratio = 0.02;
}

// ---- popover chrome -----------------------------------------------------------------------

struct Menu<'a> {
    ui: &'a mut egui::Ui,
}

impl Menu<'_> {
    fn header(&mut self, text: &str) {
        let (r, _) = self.ui.allocate_exact_size(Vec2::new(self.ui.available_width(), 22.0), Sense::hover());
        kit::spaced_text(self.ui, Pos2::new(r.min.x + 10.0, r.min.y + 6.0), &text.to_uppercase(), 10.0, MUTED, 1.0);
    }
    fn hr(&mut self) {
        self.ui.add_space(5.0);
        let (r, _) = self.ui.allocate_exact_size(Vec2::new(self.ui.available_width(), 1.0), Sense::hover());
        self.ui.painter().rect_filled(r, 0.0, LINE_2);
        self.ui.add_space(5.0);
    }
    fn item(&mut self, label: &str) -> bool {
        let (r, resp) = self.ui.allocate_exact_size(Vec2::new(self.ui.available_width(), 30.0), Sense::click());
        if resp.hovered() {
            self.ui.painter().rect_filled(r, CornerRadius::same(6), RAISE);
        }
        self.ui.painter().text(Pos2::new(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, label, kit::font(13.0), TEXT);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }
}

/// Show `content` in a popover whose right edge lines up with `anchor`. Returns true when a click
/// landed outside of it.
fn popover(ctx: &egui::Context, id: &str, anchor: Rect, content: impl FnOnce(&mut Menu)) -> bool {
    let mut outside = false;
    let area = egui::Area::new(egui::Id::new(id)).order(egui::Order::Foreground).fixed_pos(Pos2::new(anchor.right() - 230.0, anchor.bottom() + 6.0)).show(ctx, |ui| {
        egui::Frame::new()
            .fill(PANEL_2)
            .stroke(Stroke::new(1.0, LINE_2))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(egui::Margin::same(6))
            .shadow(egui::Shadow { offset: [0, 14], blur: 44, spread: 0, color: Color32::from_black_alpha(140) })
            .show(ui, |ui| {
                ui.set_min_width(210.0);
                ui.set_max_width(260.0);
                ui.spacing_mut().item_spacing = Vec2::new(0.0, 0.0);
                content(&mut Menu { ui });
            });
    });
    if ctx.input(|i| i.pointer.any_pressed()) {
        if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
            if !area.response.rect.contains(p) && !anchor.contains(p) {
                outside = true;
            }
        }
    }
    outside
}

pub fn show(st: &mut EditorState, ctx: &egui::Context) {
    projects_menu(st, ctx);
    presets_menu(st, ctx);
    raw_menu(st, ctx);
    about(st, ctx);
}

fn projects_menu(st: &mut EditorState, ctx: &egui::Context) {
    if !st.dialogs.projects {
        return;
    }
    let anchor = st.dialogs.projects_anchor.unwrap_or(Rect::from_min_size(Pos2::new(60.0, 40.0), Vec2::new(90.0, 32.0)));
    let dir = st.package.dir.clone();
    let root = crate::prefs::Prefs::load().recordings_dir;
    let list = screencraft_engine::package::list_packages(&root);
    let mut open: Option<std::path::PathBuf> = None;
    let mut reveal_it = false;
    let mut reveal_root = false;
    let outside = {
        // anchored under the button, left-aligned like the original
        let mut o = false;
        let area = egui::Area::new(egui::Id::new("projects-menu")).order(egui::Order::Foreground).fixed_pos(Pos2::new(anchor.left(), anchor.bottom() + 6.0)).show(ctx, |ui| {
            egui::Frame::new()
                .fill(PANEL_2)
                .stroke(Stroke::new(1.0, LINE_2))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(6))
                .shadow(egui::Shadow { offset: [0, 14], blur: 44, spread: 0, color: Color32::from_black_alpha(140) })
                .show(ui, |ui| {
                    ui.set_min_width(260.0);
                    ui.set_max_width(320.0);
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    let mut m = Menu { ui };
                    m.header("Recent Projects");
                    for (pkg, _) in list.iter().take(8) {
                        let name = pkg.load_project().map(|p| p.name).unwrap_or_else(|_| pkg.dir.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default());
                        if m.item(&name) {
                            open = Some(pkg.dir.clone());
                        }
                    }
                    if list.is_empty() {
                        m.item("No projects yet");
                    }
                    m.hr();
                    if m.item("Show this project in the file manager") {
                        reveal_it = true;
                    }
                    if m.item("Open recordings folder") {
                        reveal_root = true;
                    }
                });
        });
        if ctx.input(|i| i.pointer.any_pressed()) {
            if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
                if !area.response.rect.contains(p) && !anchor.contains(p) {
                    o = true;
                }
            }
        }
        o
    };
    if let Some(d) = open {
        st.dialogs.projects = false;
        st.open_request = Some(d);
    } else if reveal_it {
        st.dialogs.projects = false;
        reveal(&dir.join(&st.project.media.screen));
    } else if reveal_root {
        st.dialogs.projects = false;
        let _ = open_url(&root.to_string_lossy());
    } else if outside {
        st.dialogs.projects = false;
    }
}

fn presets_menu(st: &mut EditorState, ctx: &egui::Context) {
    if !st.dialogs.presets_open {
        return;
    }
    let anchor = st.dialogs.presets_anchor.unwrap_or(Rect::from_min_size(Pos2::new(900.0, 40.0), Vec2::new(100.0, 32.0)));
    let manage = st.dialogs.presets_manage;
    let mut apply: Option<Value> = None;
    let mut close = false;
    let mut save_new = false;
    let mut toggle_manage = false;
    let mut delete: Option<String> = None;
    let mut rename: Option<(String, String)> = None;
    let mut user = user_presets();
    let focus = st.dialogs.focus_name.clone();
    let mut bufs = std::mem::take(&mut st.dialogs.rename_buf);
    let outside = popover(ctx, "presets-menu", anchor, |m| {
        if !manage {
            for (n, v) in builtin() {
                if m.item(n) {
                    apply = Some(v);
                    close = true;
                }
            }
            if !user.is_empty() {
                m.hr();
                for (n, v) in &user {
                    if m.item(n) {
                        apply = Some(v.clone());
                        close = true;
                    }
                }
            }
            m.hr();
            // Save Preset + gear
            let (r, _) = m.ui.allocate_exact_size(Vec2::new(m.ui.available_width(), 32.0), Sense::hover());
            let save = Rect::from_min_max(r.min, Pos2::new(r.max.x - 40.0, r.max.y));
            let gear = Rect::from_min_max(Pos2::new(r.max.x - 34.0, r.min.y), r.max);
            let rs = m.ui.interact(save, m.ui.id().with("save"), Sense::click());
            m.ui.painter().rect_filled(save, CornerRadius::same(6), if rs.hovered() { LINE } else { RAISE });
            m.ui.painter().text(save.center(), Align2::CENTER_CENTER, "Save Preset", kit::font(13.0), TEXT);
            if rs.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                save_new = true;
            }
            let rg = m.ui.interact(gear, m.ui.id().with("gear"), Sense::click());
            m.ui.painter().rect_stroke(gear, CornerRadius::same(6), Stroke::new(1.0, if rg.hovered() { ACCENT_2 } else { LINE_2 }), StrokeKind::Inside);
            crate::icons::paint(m.ui, "gear", Rect::from_center_size(gear.center(), Vec2::splat(16.0)), ICON);
            if rg.on_hover_text("Rename or delete your presets").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                toggle_manage = true;
            }
        } else {
            m.header("Your presets");
            if user.is_empty() {
                m.ui.add_space(4.0);
                m.ui.label(egui::RichText::new("No saved presets yet. Use Save Preset first.").size(12.0).color(MUTED));
                m.ui.add_space(4.0);
            }
            for (n, _) in &user {
                let buf = bufs.entry(n.clone()).or_insert_with(|| n.clone());
                m.ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let r = ui.add_sized([150.0, 28.0], egui::TextEdit::singleline(buf).margin(egui::Margin::symmetric(8, 6)).char_limit(40));
                    if focus.as_deref() == Some(n.as_str()) {
                        r.request_focus();
                    }
                    if r.lost_focus() {
                        let next = buf.trim().to_string();
                        if !next.is_empty() && next != *n {
                            rename = Some((n.clone(), next));
                        } else {
                            *buf = n.clone();
                        }
                    }
                    if kit::icon_btn_sized(ui, "trash", "Delete preset", 28.0, 15.0, true).clicked() {
                        delete = Some(n.clone());
                    }
                });
            }
            m.hr();
            let (r, resp) = m.ui.allocate_exact_size(Vec2::new(m.ui.available_width(), 32.0), Sense::click());
            m.ui.painter().rect_filled(r, CornerRadius::same(6), if resp.hovered() { LINE } else { RAISE });
            m.ui.painter().text(r.center(), Align2::CENTER_CENTER, "Done", kit::font(13.0), TEXT);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                toggle_manage = true;
            }
        }
    });
    st.dialogs.rename_buf = bufs;
    st.dialogs.focus_name = None;
    if let Some(v) = apply {
        st.edit(|p| apply_preset(p, &v));
    }
    if save_new {
        let mut n = user.len() + 1;
        while user.iter().any(|(k, _)| k == &format!("My preset {n}")) {
            n += 1;
        }
        let name = format!("My preset {n}");
        let c = &st.project.config;
        user.push((name.clone(), json!({ "styles": c.styles, "cursor": c.cursor, "camera": c.camera })));
        save_user_presets(&user);
        st.dialogs.presets_manage = true;
        st.dialogs.focus_name = Some(name);
    }
    if let Some(n) = delete {
        user.retain(|(k, _)| k != &n);
        save_user_presets(&user);
        st.dialogs.rename_buf.remove(&n);
    }
    if let Some((from, to)) = rename {
        if !user.iter().any(|(k, _)| k == &to) {
            for (k, _) in user.iter_mut() {
                if *k == from {
                    *k = to.clone();
                }
            }
            save_user_presets(&user);
        }
        st.dialogs.rename_buf.remove(&from);
    }
    if toggle_manage {
        st.dialogs.presets_manage = !st.dialogs.presets_manage;
    }
    if close || outside {
        st.dialogs.presets_open = false;
        st.dialogs.presets_manage = false;
        st.dialogs.presets_anchor = None;
    }
}

fn raw_menu(st: &mut EditorState, ctx: &egui::Context) {
    if !st.dialogs.raw {
        return;
    }
    let anchor = st.dialogs.raw_anchor.unwrap_or(Rect::from_min_size(Pos2::new(820.0, 40.0), Vec2::new(60.0, 32.0)));
    let media = st.project.media.clone();
    let dir = st.package.dir.clone();
    let name = st.project.name.clone();
    let mut action: Option<u8> = None;
    let outside = popover(ctx, "raw-menu", anchor, |m| {
        m.header("Raw recording files");
        if m.item("Save raw screen video as…") {
            action = Some(0);
        }
        if media.camera.is_some() && m.item("Save raw camera video as…") {
            action = Some(1);
        }
        m.hr();
        if m.item("Open raw video in compressor…") {
            action = Some(3);
        }
        let fm = if cfg!(target_os = "macos") { "Finder" } else if cfg!(target_os = "windows") { "Explorer" } else { "the file manager" };
        if m.item(&format!("Show raw files in {fm}")) {
            action = Some(2);
        }
    });
    if let Some(a) = action {
        st.dialogs.raw = false;
        match a {
            0 | 1 => {
                let (rel, label) = if a == 0 { (media.screen.clone(), "(raw screen)") } else { (media.camera.clone().unwrap_or_default(), "(raw camera)") };
                let ext = std::path::Path::new(&rel).extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_string();
                let base = name.replace(['/', '\\', ':'], "-");
                if let Some(dst) = rfd::FileDialog::new().set_file_name(format!("{base} {label}.{ext}")).add_filter(ext.to_uppercase(), &[&ext]).save_file() {
                    match std::fs::copy(dir.join(&rel), &dst) {
                        Ok(_) => reveal(&dst),
                        Err(e) => st.error = Some(format!("Could not copy: {e}")),
                    }
                }
            }
            3 => st.compress_request = Some(dir.join(&media.screen)),
            _ => reveal(&dir.join(&media.screen)),
        }
    } else if outside {
        st.dialogs.raw = false;
    }
}

pub fn reveal(path: &std::path::Path) {
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg("-R").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("explorer").arg(format!("/select,{}", path.display())).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path.parent().unwrap_or(path)).spawn();
}

fn card(ui: &mut egui::Ui, tint: bool, add: impl FnOnce(&mut egui::Ui)) {
    let (fill, border) = if tint { (alpha(ACCENT, 0.12), alpha(Color32::from_rgb(106, 174, 232), 0.55)) } else { (PANEL, LINE) };
    egui::Frame::new().fill(fill).stroke(Stroke::new(1.0, border)).corner_radius(CornerRadius::same(10)).inner_margin(egui::Margin::symmetric(16, 14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        add(ui);
    });
}

fn h2(ui: &mut egui::Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 14.0), Sense::hover());
    kit::spaced_text(ui, r.min, &text.to_uppercase(), 10.5, MUTED, 1.26);
    ui.add_space(8.0 - ui.spacing().item_spacing.y);
}

fn para(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(LABEL)).wrap());
}

fn small_btn(ui: &mut egui::Ui, text: &str, enabled: bool) -> bool {
    let tw = kit::text_width(ui, text, 13.0);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(tw + 26.0, 32.0), if enabled { Sense::click() } else { Sense::hover() });
    ui.painter().rect_filled(r, CornerRadius::same(7), if resp.hovered() && enabled { RAISE } else { PANEL_2 });
    ui.painter().rect_stroke(r, CornerRadius::same(7), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
    ui.painter().text(r.center(), Align2::CENTER_CENTER, text, kit::font(13.0), if enabled { Color32::from_rgb(0xe1, 0xe7, 0xf5) } else { alpha(MUTED, 0.7) });
    enabled && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn about(st: &mut EditorState, ctx: &egui::Context) {
    if !st.dialogs.about {
        st.dialogs.asked = false;
        return;
    }
    let d = &mut st.dialogs;
    // opening this window is itself a request to know, so look straight away
    if updates::SUPPORTED && !d.asked {
        d.asked = true;
        d.checking = true;
        let (tx, rx) = channel();
        d.check_rx = Some(rx);
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(updates::fetch());
            c.request_repaint();
        });
    }
    if let Some(rx) = &d.check_rx {
        if let Ok(info) = rx.try_recv() {
            if info.available {
                updates::remember(&info);
            }
            d.about_info = Some(info);
            d.checking = false;
            d.check_rx = None;
        }
    }
    if let Some(rx) = &d.pct_rx {
        while let Ok(p) = rx.try_recv() {
            d.download_pct = Some(p);
        }
    }
    if let Some(rx) = &d.download_rx {
        if let Ok(r) = rx.try_recv() {
            d.download_msg = Some(match r {
                Ok(p) => format!("Saved to {} and opened. Quit ScreenCraft, drag it onto Applications and replace the old copy, then start it again.", p.display()),
                Err(e) => format!("The download did not finish ({e}). You can get it from the website instead."),
            });
            d.download_pct = None;
            d.download_rx = None;
        }
    }
    let mut open = true;
    let screen = ctx.content_rect();
    // dim the editor behind
    egui::Area::new(egui::Id::new("about-dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let r = ui.allocate_rect(screen, Sense::click());
        ui.painter().rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(4, 7, 12, 168));
        if r.clicked() {
            open = false;
        }
    });
    let info = d.about_info.clone();
    let checking = d.checking;
    let (mut recheck, mut do_download, mut later, mut toggle_check) = (false, false, false, false);
    let dl_pct = d.download_pct;
    let dl_msg = d.download_msg.clone();
    egui::Window::new("About ScreenCraft")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .fixed_size(Vec2::new(520.0, 0.0))
        .frame(egui::Frame::new().fill(BG).stroke(Stroke::new(1.0, LINE_2)).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::symmetric(30, 26)))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 16.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(egui::RichText::new("ScreenCraft").size(22.0).color(TEXT));
                    ui.label(egui::RichText::new(format!("Version {}", updates::VERSION)).size(12.0).color(MUTED));
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 10.0;
                        ui.label(egui::RichText::new("Links").size(11.0).color(MUTED));
                        for (label, url) in [
                            ("X", "https://x.com/mdnlabs"),
                            ("GitHub", "https://github.com/MarshallNickolauson/ScreenCraft"),
                            ("Report an issue", "https://github.com/MarshallNickolauson/ScreenCraft/issues"),
                        ] {
                            ui.add(egui::Hyperlink::from_label_and_url(egui::RichText::new(label).size(12.5).color(ACCENT_2), url));
                        }
                    });
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                    if kit::icon_btn(ui, "close", "Close").clicked() {
                        open = false;
                    }
                });
            });
            if updates::SUPPORTED {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 6.0;
                    h2(ui, "Updates");
                    if let Some(i) = info.as_ref().filter(|i| i.available) {
                        card(ui, true, |ui| {
                            ui.label(egui::RichText::new(format!("ScreenCraft {} is available (you have {})", i.latest, i.current)).strong().size(13.0).color(TEXT));
                            for n in &i.notes {
                                para(ui, &format!("•  {n}"));
                            }
                            ui.add_space(4.0);
                            let help = dl_msg.clone().unwrap_or_else(|| "Download saves the new version to your Downloads folder and opens it. ScreenCraft never installs anything by itself: you drag it across when you are ready.".into());
                            ui.add(egui::Label::new(egui::RichText::new(help).size(13.0).color(MUTED)).wrap());
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                let label = match dl_pct {
                                    Some(p) => format!("Downloading {}%", p.round()),
                                    None if dl_msg.is_some() && dl_msg.as_deref().map(|m| m.starts_with("Saved")).unwrap_or(false) => "Downloaded".into(),
                                    None => "Download".to_string(),
                                };
                                if kit::primary(ui, None, &label).clicked() && dl_pct.is_none() {
                                    do_download = true;
                                }
                                if small_btn(ui, "Later", true) {
                                    later = true;
                                }
                            });
                        });
                    }
                    card(ui, false, |ui| {
                        ui.horizontal(|ui| {
                            let text = match &info {
                                _ if checking => "Checking…".to_string(),
                                None => "Not checked yet.".into(),
                                Some(i) if i.error.is_some() => "Could not check for updates. You are offline or the server is unreachable.".into(),
                                Some(i) if i.available => format!("Version {} is available.", i.latest),
                                Some(i) => format!("You have the latest version ({}).", i.current),
                            };
                            let col = if info.as_ref().map(|i| i.error.is_some()).unwrap_or(false) { Color32::from_rgb(0xff, 0x9a, 0xa6) } else { LABEL };
                            ui.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(col)).wrap());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if small_btn(ui, "Check now", !checking) {
                                    recheck = true;
                                }
                            });
                        });
                        ui.add_space(8.0);
                        let mut on = updates::enabled();
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                // leave room for the switch on the right
                                ui.set_max_width((ui.available_width() - 56.0).max(120.0));
                                ui.spacing_mut().item_spacing.y = 2.0;
                                para(ui, "Check for updates when ScreenCraft starts");
                                ui.label(egui::RichText::new("Off means ScreenCraft never contacts the update server unless you press Check now.").size(12.0).color(MUTED));
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                let (r, resp) = ui.allocate_exact_size(Vec2::new(38.0, 22.0), Sense::click());
                                ui.painter().rect_filled(r, CornerRadius::same(11), if on { ACCENT } else { LINE_2 });
                                let kx = r.min.x + 3.0 + if on { 16.0 } else { 0.0 };
                                ui.painter().circle_filled(Pos2::new(kx + 8.0, r.center().y), 8.0, Color32::WHITE);
                                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    on = !on;
                                    toggle_check = true;
                                }
                            });
                        });
                        if toggle_check {
                            updates::set_enabled(on);
                        }
                    });
                });
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                h2(ui, "Privacy");
                card(ui, false, |ui| {
                    ui.spacing_mut().item_spacing.y = 8.0;
                    ui.add(egui::Label::new(egui::RichText::new("ScreenCraft works on your computer. Your recordings, screen, camera, microphone, cursor data and captions are never uploaded. There are no accounts, no analytics and no telemetry.").size(13.0).color(LABEL)).wrap());
                    para(ui, if updates::SUPPORTED { "Two things use the network, and only these:" } else { "One thing uses the network, and only when you ask:" });
                    if updates::SUPPORTED {
                        para(ui, "•  Update check. At startup (you can turn it off above) ScreenCraft requests one small file from screencraft.mdnlabs.dev to see whether a newer version exists. The request contains the app version and nothing else about you. The site is served by Cloudflare, which, like any web host, sees the IP address of a request. If you press Download, ScreenCraft fetches the installer from the same site and saves it to your Downloads folder.");
                    }
                    para(ui, "•  Caption models. Only if you ask to generate captions, a speech model file is downloaded once from Hugging Face. Your audio is never sent anywhere; recognition runs on your machine.");
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 14.0;
                        for (t, u) in [
                            ("Privacy policy", "https://screencraft.mdnlabs.dev/privacy"),
                            ("What is stored and where", "https://screencraft.mdnlabs.dev/docs/privacy-and-data"),
                            ("Third-party notices", "https://screencraft.mdnlabs.dev/third-party-notices"),
                        ] {
                            if ui.add(egui::Label::new(egui::RichText::new(t).size(12.5).color(ACCENT_2)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                let _ = open_url(u);
                            }
                        }
                    });
                });
            });
        });
    let d = &mut st.dialogs;
    if recheck {
        d.checking = true;
        let (tx, rx) = channel();
        d.check_rx = Some(rx);
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(updates::fetch());
            c.request_repaint();
        });
    }
    if later {
        if let Some(i) = &d.about_info {
            updates::dismiss(&i.latest);
        }
        if let Some(i) = d.about_info.as_mut() {
            i.available = false;
        }
    }
    if do_download {
        if let Some(i) = d.about_info.clone() {
            let (tx, rx) = channel();
            let (ptx, prx) = channel();
            d.download_rx = Some(rx);
            d.pct_rx = Some(prx);
            d.download_pct = Some(0.0);
            let c = ctx.clone();
            std::thread::spawn(move || {
                let c2 = c.clone();
                let r = updates::download(&i.download, &i.latest, &|p| {
                    let _ = ptx.send(p);
                    c2.request_repaint();
                });
                if r.is_err() {
                    let _ = open_url(&i.page);
                }
                let _ = tx.send(r);
                c.request_repaint();
            });
        }
    }
    if checking || d.download_rx.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        st.dialogs.about = false;
    }
}

/// "Save changes to ...?" over the editor, shown before the window closes or another project opens.
pub fn save_prompt(st: &mut EditorState, ctx: &egui::Context) {
    if st.close_prompt.is_none() {
        return;
    }
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("save-dim")).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        ui.allocate_rect(screen, Sense::click());
        ui.painter().rect_filled(screen, 0.0, Color32::from_rgba_unmultiplied(4, 7, 12, 158));
    });
    let mut answer = None;
    egui::Window::new("save-ask")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .fixed_size(Vec2::new(350.0, 0.0))
        .frame(egui::Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE_2)).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(20)).shadow(egui::Shadow { offset: [0, 20], blur: 60, spread: 0, color: Color32::from_black_alpha(140) }))
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing = Vec2::new(8.0, 6.0);
            ui.label(egui::RichText::new(format!("Save changes to “{}”?", st.project.name)).strong().size(15.0).color(TEXT));
            ui.label(egui::RichText::new("Your edits will be lost if you don't save them.").size(12.5).color(MUTED));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if kit::pill(ui, None, "Don't Save", false, false, false, None).clicked() {
                    answer = Some(super::Answer::Discard);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if kit::primary(ui, None, "Save").clicked() {
                        answer = Some(super::Answer::Save);
                    }
                    if kit::pill(ui, None, "Cancel", false, false, false, None).clicked() {
                        answer = Some(super::Answer::Cancel);
                    }
                });
            });
        });
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        answer = Some(super::Answer::Cancel);
    }
    if let Some(a) = answer {
        st.close_answer = Some(a);
    }
}
