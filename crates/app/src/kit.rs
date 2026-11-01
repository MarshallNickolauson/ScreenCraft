//! The 0.3 inspector controls (sliders, toggles, segmented buttons, tiles ...) painted to match
//! the web UI's CSS pixel for pixel.

use crate::icons;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, Vec2};

pub fn font(size: f32) -> FontId {
    FontId::proportional(size)
}

pub fn text_width(ui: &Ui, text: &str, size: f32) -> f32 {
    ui.painter().layout_no_wrap(text.to_string(), font(size), Color32::WHITE).size().x
}

/// `text` shortened with an ellipsis so it fits `max_w` (CSS `text-overflow: ellipsis`).
pub fn fit_text(ui: &Ui, text: &str, size: f32, max_w: f32) -> String {
    if text_width(ui, text, size) <= max_w {
        return text.to_string();
    }
    let mut s: String = text.to_string();
    while !s.is_empty() && text_width(ui, &format!("{s}…"), size) > max_w {
        s.pop();
    }
    format!("{}…", s.trim_end())
}

/// Text with CSS-style letter spacing; returns its width.
pub fn spaced_text(ui: &Ui, pos: Pos2, text: &str, size: f32, color: Color32, spacing: f32) -> f32 {
    let mut x = pos.x;
    for ch in text.chars() {
        let g = ui.painter().layout_no_wrap(ch.to_string(), font(size), color);
        let w = g.size().x;
        ui.painter().galley(Pos2::new(x, pos.y), g, color);
        x += w + spacing;
    }
    x - pos.x
}

/// Section heading inside a panel (`#panel h3`).
pub fn h3(ui: &mut Ui, text: &str, first: bool) {
    ui.add_space(if first { 2.0 } else { 18.0 });
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 13.0), Sense::hover());
    spaced_text(ui, rect.min, &text.to_uppercase(), 10.5, MUTED, 1.26);
    ui.add_space(10.0 - ui.spacing().item_spacing.y);
}

/// Wrapped muted text (`.hint` / `.muted`).
pub fn hint(ui: &mut Ui, text: &str) {
    ui.add_space(2.0);
    ui.add(egui::Label::new(egui::RichText::new(text).size(12.0).color(MUTED)).wrap());
    ui.add_space(2.0);
}

pub fn muted(ui: &mut Ui, text: &str) {
    ui.add(egui::Label::new(egui::RichText::new(text).size(13.0).color(MUTED)).wrap());
}

/// `.field > label`: a label on the left and an accent value on the right.
fn field_label(ui: &mut Ui, label: &str, value: Option<&str>) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 16.0), Sense::hover());
    ui.painter().text(rect.left_center(), Align2::LEFT_CENTER, label, font(13.0), LABEL);
    if let Some(v) = value {
        ui.painter().text(rect.right_center(), Align2::RIGHT_CENTER, v, font(13.0), ACCENT_2);
    }
}

/// `<input type=range>` with the 0.3 track and square thumb. Returns true when the value changed.
pub fn slider(ui: &mut Ui, label: &str, v: &mut f64, min: f64, max: f64, step: f64, fmt: impl Fn(f64) -> String) -> bool {
    ui.add_space(6.0);
    field_label(ui, label, Some(&fmt(*v)));
    ui.add_space(1.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::click_and_drag());
    let (tw, half) = (14.0, 7.0);
    let track = Rect::from_min_max(Pos2::new(rect.min.x, rect.center().y - 2.0), Pos2::new(rect.max.x, rect.center().y + 2.0));
    let mut changed = false;
    if resp.dragged() || resp.clicked() || resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            let t = ((p.x - rect.min.x - half) / (rect.width() - tw)).clamp(0.0, 1.0) as f64;
            let mut nv = min + (max - min) * t;
            if step > 0.0 {
                nv = (nv / step).round() * step;
            }
            nv = nv.clamp(min, max);
            if (nv - *v).abs() > f64::EPSILON {
                *v = nv;
                changed = true;
            }
        }
    }
    let t = if max > min { ((*v - min) / (max - min)).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let cx = rect.min.x + half + t * (rect.width() - tw);
    ui.painter().rect_filled(track, CornerRadius::same(2), LINE_2);
    let thumb = Rect::from_center_size(Pos2::new(cx, rect.center().y), Vec2::splat(tw));
    let col = if resp.hovered() || resp.dragged() { Color32::WHITE } else { ACCENT_2 };
    ui.painter().rect_filled(thumb.expand(1.0), CornerRadius::same(5), ACCENT_2);
    ui.painter().rect_filled(thumb, CornerRadius::same(4), BG);
    ui.painter().rect_filled(thumb.shrink(2.0), CornerRadius::same(3), col);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.add_space(6.0);
    changed
}

/// `.row-t` with a 36x20 toggle.
pub fn toggle_row(ui: &mut Ui, label: &str, hint_text: Option<&str>, v: &mut bool) -> bool {
    ui.add_space(6.0);
    let w = ui.available_width();
    let label_w = w - 36.0 - 12.0;
    let galley = ui.painter().layout(label.to_string(), font(13.0), TEXT, label_w);
    let hint_g = hint_text.map(|h| ui.painter().layout(h.to_string(), font(12.0), MUTED, label_w));
    let h = galley.size().y + hint_g.as_ref().map(|g| g.size().y + 2.0).unwrap_or(0.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h.max(20.0)), Sense::click());
    let top = rect.min;
    ui.painter().galley(top, galley.clone(), TEXT);
    if let Some(g) = hint_g {
        ui.painter().galley(Pos2::new(top.x, top.y + galley.size().y + 2.0), g, MUTED);
    }
    let tg = Rect::from_min_size(Pos2::new(rect.max.x - 36.0, rect.center().y - 10.0), Vec2::new(36.0, 20.0));
    ui.painter().rect_filled(tg, CornerRadius::same(6), if *v { ACCENT } else { LINE_2 });
    let kx = tg.min.x + 3.0 + if *v { 16.0 } else { 0.0 };
    ui.painter().rect_filled(Rect::from_min_size(Pos2::new(kx, tg.min.y + 3.0), Vec2::splat(14.0)), CornerRadius::same(4), Color32::WHITE);
    ui.add_space(6.0);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        *v = !*v;
        return true;
    }
    false
}

struct Cell<'a, T> {
    value: T,
    label: &'a str,
    w: f32,
}

/// `.seg`: joined buttons in a rounded 1px box, the current one filled with the accent.
pub fn segmented<T: PartialEq + Clone>(ui: &mut Ui, label: Option<&str>, options: &[(T, &str)], cur: &mut T) -> bool {
    if let Some(l) = label {
        ui.add_space(6.0);
        field_label(ui, l, None);
        ui.add_space(1.0);
    }
    let avail = ui.available_width();
    let cells: Vec<Cell<T>> =
        options.iter().map(|(v, l)| Cell { value: v.clone(), label: l, w: text_width(ui, l, 13.0) + 24.0 + 1.0 }).collect();
    // wrap into rows
    let mut rows: Vec<Vec<usize>> = vec![vec![]];
    let mut x = 0.0;
    for (i, c) in cells.iter().enumerate() {
        if x + c.w > avail && !rows.last().unwrap().is_empty() {
            rows.push(vec![]);
            x = 0.0;
        }
        rows.last_mut().unwrap().push(i);
        x += c.w;
    }
    let row_h = 29.0;
    let used_w = rows.iter().map(|r| r.iter().map(|&i| cells[i].w).sum::<f32>()).fold(0.0, f32::max).min(avail);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(used_w, row_h * rows.len() as f32), Sense::hover());
    let mut changed = false;
    let nrows = rows.len();
    let mut hit: Vec<(Rect, usize, bool, bool, usize)> = Vec::new();
    for (ri, row) in rows.iter().enumerate() {
        let mut x = rect.min.x;
        for (k, &i) in row.iter().enumerate() {
            let r = Rect::from_min_size(Pos2::new(x, rect.min.y + ri as f32 * row_h), Vec2::new(cells[i].w, row_h));
            hit.push((r, i, k == 0, k == row.len() - 1, ri));
            x += cells[i].w;
        }
    }
    for (r, i, first, last, ri) in hit {
        let resp = ui.interact(r, ui.id().with(("seg", i, label, r.min.x.round() as i32, r.min.y.round() as i32)), Sense::click());
        let on = cells[i].value == *cur;
        let cr = CornerRadius {
            nw: if first && ri == 0 { 7 } else { 0 },
            ne: if last && ri == 0 { 7 } else { 0 },
            sw: if first && ri == nrows - 1 { 7 } else { 0 },
            se: if last && ri == nrows - 1 { 7 } else { 0 },
        };
        let inner = r.shrink(1.0);
        if on {
            ui.painter().rect_filled(inner, cr, ACCENT);
        } else if resp.hovered() {
            ui.painter().rect_filled(inner, cr, PANEL_2);
        }
        ui.painter().text(r.center() - Vec2::new(0.5, 0.0), Align2::CENTER_CENTER, cells[i].label, font(13.0), if on { Color32::WHITE } else { LABEL });
        if !last {
            ui.painter().line_segment([Pos2::new(r.max.x - 0.5, r.min.y + 1.0), Pos2::new(r.max.x - 0.5, r.max.y - 1.0)], Stroke::new(1.0, LINE_2));
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
            *cur = cells[i].value.clone();
            changed = true;
        }
    }
    ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
    ui.add_space(6.0);
    changed
}

/// Colour row: label left, 44x30 swatch right that opens the picker.
pub fn color_row(ui: &mut Ui, label: &str, hex: &mut String) -> bool {
    ui.add_space(6.0);
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).size(13.0).color(TEXT));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut c = parse_hex(hex);
            ui.spacing_mut().interact_size = Vec2::new(44.0, 30.0);
            if egui::color_picker::color_edit_button_srgba(ui, &mut c, egui::color_picker::Alpha::Opaque).changed() {
                *hex = to_hex(c);
                changed = true;
            }
        });
    });
    ui.add_space(6.0);
    changed
}

fn dashed_rect(ui: &Ui, r: Rect, stroke: Stroke) {
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in corners.windows(2) {
        ui.painter().extend(Shape::dashed_line(&[w[0], w[1]], stroke, 4.0, 3.0));
    }
}

/// `.btn-line`: a full-width dashed button.
pub fn btn_line(ui: &mut Ui, text: &str, danger: bool) -> Response {
    btn_line_w(ui, text, danger, ui.available_width())
}

pub fn btn_line_w(ui: &mut Ui, text: &str, danger: bool, w: f32) -> Response {
    ui.add_space(2.0);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 33.0), Sense::click());
    let hov = resp.hovered();
    ui.painter().rect_filled(rect, CornerRadius::same(7), if hov { RAISE } else { PANEL_2 });
    if hov {
        ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
    } else {
        dashed_rect(ui, rect.shrink(0.5), Stroke::new(1.0, LINE_2));
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, text, font(13.0), if danger { rgb_danger() } else { rgb_btn() });
    ui.add_space(2.0);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn rgb_danger() -> Color32 {
    Color32::from_rgb(0xff, 0x8a, 0x97)
}
fn rgb_btn() -> Color32 {
    Color32::from_rgb(0xcd, 0xd5, 0xe5)
}

/// `.pill`: a bordered button, optionally with a leading icon and a trailing chevron.
pub fn pill(ui: &mut Ui, icon: Option<&str>, text: &str, on: bool, small: bool, chevron: bool, color: Option<Color32>) -> Response {
    let size = if small { 12.0 } else { 13.0 };
    let (px, py) = if small { (9.0, 5.0) } else { (11.0, 7.0) };
    let tw = text_width(ui, text, size);
    let mut w = px * 2.0 + tw;
    if icon.is_some() {
        w += 18.0 + 6.0;
    }
    if chevron {
        w += 13.0 + 6.0;
    }
    let h = size * 1.25 + py * 2.0 + 2.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h.max(if small { 26.0 } else { 33.0 })), Sense::click());
    let hov = resp.hovered();
    let (bg, border) = if on { (alpha(ACCENT, 0.25), ACCENT) } else if hov { (RAISE, LINE_2) } else { (PANEL_2, LINE) };
    ui.painter().rect_filled(rect, CornerRadius::same(7), bg);
    ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, border), StrokeKind::Inside);
    let fg = color.unwrap_or(Color32::from_rgb(0xd3, 0xdb, 0xea));
    let mut x = rect.min.x + px;
    if let Some(i) = icon {
        icons::paint(ui, i, Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)), fg);
        x += 24.0;
    }
    ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, text, font(size), fg);
    if chevron {
        icons::paint(ui, "chev", Rect::from_center_size(Pos2::new(rect.max.x - px - 6.5, rect.center().y), Vec2::splat(13.0)), fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `.primary`: the accent call to action.
pub fn primary(ui: &mut Ui, icon: Option<&str>, text: &str) -> Response {
    let tw = text_width(ui, text, 13.0);
    let w = 30.0 + tw + if icon.is_some() { 25.0 } else { 0.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, 34.0), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(7), if resp.hovered() { ACCENT_2 } else { ACCENT });
    ui.painter().rect_stroke(rect, CornerRadius::same(7), Stroke::new(1.0, Color32::from_white_alpha(41)), StrokeKind::Inside);
    let mut x = rect.min.x + 15.0;
    if let Some(i) = icon {
        icons::paint(ui, i, Rect::from_center_size(Pos2::new(x + 9.0, rect.center().y), Vec2::splat(18.0)), Color32::WHITE);
        x += 25.0;
    }
    ui.painter().text(Pos2::new(x, rect.center().y), Align2::LEFT_CENTER, text, font(13.0), Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `.icon-btn`: 32x32 ghost button.
pub fn icon_btn(ui: &mut Ui, name: &str, tip: &str) -> Response {
    icon_btn_sized(ui, name, tip, 32.0, 18.0, true)
}

pub fn icon_btn_sized(ui: &mut Ui, name: &str, tip: &str, box_px: f32, icon_px: f32, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(box_px), Sense::click());
    let hov = resp.hovered() && enabled;
    if hov {
        ui.painter().rect_filled(rect, CornerRadius::same(7), RAISE);
    }
    let col = if !enabled { alpha(ICON, 0.4) } else if hov { Color32::WHITE } else { ICON };
    icons::paint(ui, name, Rect::from_center_size(rect.center(), Vec2::splat(icon_px)), col);
    let r = if tip.is_empty() { resp } else { resp.on_hover_text(tip) };
    if enabled {
        r.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        r
    }
}

/// `.add-tile`: icon over a label, in a bordered tile.
pub fn add_tile(ui: &mut Ui, icon: Option<&str>, label: &str, w: f32) -> Response {
    let h = if icon.is_some() { 62.0 } else { 34.0 };
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
    let hov = resp.hovered();
    ui.painter().rect_filled(rect, CornerRadius::same(8), if hov { RAISE } else { PANEL_2 });
    ui.painter().rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, if hov { ACCENT_2 } else { LINE_2 }), StrokeKind::Inside);
    let fg = if hov { Color32::WHITE } else { Color32::from_rgb(0xcd, 0xd6, 0xe8) };
    if let Some(i) = icon {
        icons::paint(ui, i, Rect::from_center_size(Pos2::new(rect.center().x, rect.min.y + 20.0), Vec2::splat(20.0)), fg);
        ui.painter().text(Pos2::new(rect.center().x, rect.max.y - 14.0), Align2::CENTER_CENTER, label, font(11.0), fg);
    } else {
        ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font(11.0), fg);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A grid of `n` equal columns, `gap` apart, calling `item(ui, index, width)` per cell.
pub fn grid(ui: &mut Ui, n: usize, count: usize, gap: f32, row_h: f32, mut item: impl FnMut(&mut Ui, usize, Rect)) {
    let w = ui.available_width();
    let cw = (w - gap * (n as f32 - 1.0)) / n as f32;
    let rows = count.div_ceil(n);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, rows as f32 * row_h + gap * (rows.max(1) as f32 - 1.0)), Sense::hover());
    for i in 0..count {
        let (r, c) = (i / n, i % n);
        let cell = Rect::from_min_size(Pos2::new(rect.min.x + c as f32 * (cw + gap), rect.min.y + r as f32 * (row_h + gap)), Vec2::new(cw, row_h));
        item(ui, i, cell);
    }
}

pub fn cell_response(ui: &mut Ui, rect: Rect, id: impl std::hash::Hash + std::fmt::Debug) -> Response {
    ui.interact(rect, ui.id().with(id), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The 3x3 anchor grid (`.pos-grid`). `cur` is the current (x, y) in 0..1.
pub fn pos_grid(ui: &mut Ui, label: &str, cur: (f64, f64)) -> Option<(f64, f64)> {
    ui.add_space(6.0);
    field_label(ui, label, None);
    ui.add_space(6.0);
    let spots: [(f64, f64, &str); 9] = [
        (0.0, 0.0, "↖"),
        (0.5, 0.0, "↑"),
        (1.0, 0.0, "↗"),
        (0.0, 0.5, "←"),
        (0.5, 0.5, "•"),
        (1.0, 0.5, "→"),
        (0.0, 1.0, "↙"),
        (0.5, 1.0, "↓"),
        (1.0, 1.0, "↘"),
    ];
    let mut out = None;
    let w = 168.0_f32.min(ui.available_width());
    ui.allocate_ui(Vec2::new(w, 0.0), |ui| {
        grid(ui, 3, 9, 4.0, 29.0, |ui, i, cell| {
            let (x, y, name) = spots[i];
            let on = (cur.0 - x).abs() < 0.01 && (cur.1 - y).abs() < 0.01;
            let resp = cell_response(ui, cell, ("pos", label, i));
            ui.painter().rect_filled(cell, CornerRadius::same(6), if on { ACCENT } else if resp.hovered() { RAISE } else { PANEL_2 });
            ui.painter().rect_stroke(cell, CornerRadius::same(6), Stroke::new(1.0, if on { ACCENT } else { LINE_2 }), StrokeKind::Inside);
            ui.painter().text(cell.center(), Align2::CENTER_CENTER, name, font(13.0), if on { Color32::WHITE } else { Color32::from_rgb(0xcd, 0xd6, 0xe8) });
            if resp.clicked() {
                out = Some((x, y));
            }
        });
    });
    ui.add_space(6.0);
    out
}

/// A `<select>`: returns the newly chosen index.
pub fn select(ui: &mut Ui, id: &str, current: &str, items: &[&str], min_w: f32) -> Option<usize> {
    let mut out = None;
    egui::ComboBox::from_id_salt(id).width(min_w).selected_text(egui::RichText::new(current).size(13.0)).show_ui(ui, |ui| {
        for (i, it) in items.iter().enumerate() {
            if ui.selectable_label(*it == current, *it).clicked() {
                out = Some(i);
            }
        }
    });
    out
}
