//! Area selection and the on-screen outline of what will be recorded: a full-screen dim with a
//! crosshair for dragging out the area, and a click-through frame around the chosen area / window.

use crate::kit;
use crate::theme::*;
use eframe::egui::{self, Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, Vec2, ViewportBuilder, ViewportId};
use screencraft_capture::DisplayInfo;
use screencraft_core::project::Rect as PRect;

pub enum Outcome {
    Pending,
    Picked(PRect),
    Cancelled,
}

pub struct Picker {
    display: Option<DisplayInfo>,
    drag: Option<(Pos2, Pos2)>,
    /// Width / height the drawn area must keep (9:16 for a vertical video).
    ratio: Option<f32>,
}

impl Picker {
    pub fn new(display: Option<DisplayInfo>, ratio: Option<f32>) -> Self {
        Self { display, drag: None, ratio }
    }

    pub fn display_id(&self) -> Option<u32> {
        self.display.as_ref().map(|d| d.id)
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Outcome {
        let (x, y, w, h) = self.display.as_ref().map(|d| (d.x as f32, d.y as f32, d.width as f32, d.height as f32)).unwrap_or((0.0, 0.0, 1440.0, 900.0));
        let builder = ViewportBuilder::default()
            .with_title("Select area")
            .with_position(Pos2::new(x, y))
            .with_inner_size([w, h])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_has_shadow(false)
            .with_taskbar(false)
            .with_always_on_top();
        let mut out = Outcome::Pending;
        let origin = (x, y);
        let drag = &mut self.drag;
        let ratio = self.ratio;
        ctx.show_viewport_immediate(ViewportId::from_hash_of("area-picker"), builder, |ui, _| {
            let ctx = ui.ctx().clone();
            ctx.set_cursor_icon(egui::CursorIcon::Crosshair);
            let full = ui.max_rect();
            egui::CentralPanel::default().frame(egui::Frame::NONE).show_inside(ui, |ui| {
                let resp = ui.allocate_rect(full, egui::Sense::click_and_drag());
                let painter = ui.painter();
                if resp.drag_started() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        *drag = Some((p, p));
                    }
                }
                if resp.dragged() {
                    if let (Some(d), Some(p)) = (drag.as_mut(), resp.interact_pointer_pos()) {
                        d.1 = match ratio {
                            Some(r) => {
                                // keep the shape: take the larger of the two sizes the pointer implies
                                let (dx, dy) = (p.x - d.0.x, p.y - d.0.y);
                                let (w, h) = (dx.abs(), dy.abs());
                                let (w2, h2) = if h > 0.0 && w / h.max(1.0) > r { (h * r, h) } else { (w, w / r) };
                                Pos2::new(d.0.x + w2 * dx.signum(), d.0.y + h2 * dy.signum())
                            }
                            None => p,
                        };
                    }
                }
                let dim = Color32::from_black_alpha(89);
                match *drag {
                    Some((a, b)) => {
                        let sel = Rect::from_two_pos(a, b);
                        // dim everything around the selection
                        for r in [
                            Rect::from_min_max(full.min, Pos2::new(full.max.x, sel.min.y)),
                            Rect::from_min_max(Pos2::new(full.min.x, sel.max.y), full.max),
                            Rect::from_min_max(Pos2::new(full.min.x, sel.min.y), Pos2::new(sel.min.x, sel.max.y)),
                            Rect::from_min_max(Pos2::new(sel.max.x, sel.min.y), Pos2::new(full.max.x, sel.max.y)),
                        ] {
                            painter.rect_filled(r, 0.0, Color32::from_black_alpha(115));
                        }
                        let corners = [sel.left_top(), sel.right_top(), sel.right_bottom(), sel.left_bottom(), sel.left_top()];
                        for w in corners.windows(2) {
                            painter.extend(egui::Shape::dashed_line(&[w[0], w[1]], Stroke::new(2.0, ACCENT_2), 6.0, 4.0));
                        }
                        let label = format!("{} × {}", sel.width().round(), sel.height().round());
                        let tw = kit::text_width(ui, &label, 12.0) + 16.0;
                        let lr = Rect::from_min_size(Pos2::new(sel.min.x, (sel.min.y - 26.0).max(2.0)), Vec2::new(tw, 22.0));
                        painter.rect_filled(lr, CornerRadius::same(4), ACCENT);
                        painter.text(lr.center(), Align2::CENTER_CENTER, label, kit::font(12.0), Color32::WHITE);
                    }
                    None => {
                        painter.rect_filled(full, 0.0, dim);
                    }
                }
                // hint
                let msg = if ratio.is_some() { "Drag to select a vertical (9:16) area · Esc to cancel" } else { "Drag to select the area to record · Esc to cancel" };
                let tw = kit::text_width(ui, msg, 12.0) + 28.0;
                let hr = Rect::from_min_size(Pos2::new(full.center().x - tw / 2.0, full.min.y + 24.0), Vec2::new(tw, 34.0));
                painter.rect_filled(hr, CornerRadius::same(8), PANEL);
                painter.rect_stroke(hr, CornerRadius::same(8), Stroke::new(1.0, LINE_2), StrokeKind::Inside);
                painter.rect_filled(Rect::from_min_size(hr.min, Vec2::new(3.0, hr.height())), CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 }, ACCENT);
                painter.text(hr.center(), Align2::CENTER_CENTER, msg, kit::font(12.0), TEXT);
                if resp.drag_stopped() {
                    if let Some((a, b)) = drag.take() {
                        let sel = Rect::from_two_pos(a, b);
                        if sel.width() >= 24.0 && sel.height() >= 24.0 {
                            out = Outcome::Picked(PRect { x: (origin.0 + sel.min.x) as f64, y: (origin.1 + sel.min.y) as f64, width: sel.width() as f64, height: sel.height() as f64 });
                        }
                    }
                }
            });
            if ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.viewport().close_requested()) {
                out = Outcome::Cancelled;
            }
        });
        out
    }
}

/// The click-through outline around `rect` (global logical points): `sel` the chosen area, `hl` a
/// window being hovered in the menu, `rec` while recording.
pub fn draw_frame(ctx: &egui::Context, rect: PRect, style: &'static str, displays: &[DisplayInfo]) {
    let d = displays
        .iter()
        .find(|d| rect.x + rect.width / 2.0 >= d.x && rect.x + rect.width / 2.0 < d.x + d.width && rect.y + rect.height / 2.0 >= d.y && rect.y + rect.height / 2.0 < d.y + d.height)
        .or(displays.first());
    let (dx, dy, dw, dh) = d.map(|d| (d.x as f32, d.y as f32, d.width as f32, d.height as f32)).unwrap_or((0.0, 0.0, 1440.0, 900.0));
    let builder = ViewportBuilder::default()
        .with_title("frame")
        .with_position(Pos2::new(dx, dy))
        .with_inner_size([dw, dh])
        .with_decorations(false)
        .with_transparent(true)
        .with_resizable(false)
        .with_has_shadow(false)
        .with_taskbar(false)
        .with_mouse_passthrough(true)
        .with_always_on_top();
    ctx.show_viewport_immediate(ViewportId::from_hash_of(("frame", style)), builder, |ui, _| {
        let full = ui.max_rect();
        let hole = Rect::from_min_size(Pos2::new(rect.x as f32 - dx, rect.y as f32 - dy), Vec2::new(rect.width as f32, rect.height as f32));
        let p = ui.painter();
        let dim = match style {
            "hl" => Color32::TRANSPARENT,
            "rec" => Color32::from_black_alpha(46),
            _ => Color32::from_black_alpha(77),
        };
        for r in [
            Rect::from_min_max(full.min, Pos2::new(full.max.x, hole.min.y)),
            Rect::from_min_max(Pos2::new(full.min.x, hole.max.y), full.max),
            Rect::from_min_max(Pos2::new(full.min.x, hole.min.y), Pos2::new(hole.min.x, hole.max.y)),
            Rect::from_min_max(Pos2::new(hole.max.x, hole.min.y), Pos2::new(full.max.x, hole.max.y)),
        ] {
            p.rect_filled(r, 0.0, dim);
        }
        match style {
            "hl" => {
                p.rect_filled(hole, CornerRadius::same(6), Color32::from_rgba_unmultiplied(106, 174, 232, 36));
                p.rect_stroke(hole, CornerRadius::same(6), Stroke::new(3.0, ACCENT_2), StrokeKind::Inside);
            }
            "rec" => {
                let t = ui.input(|i| i.time);
                let a = 0.55 + 0.45 * (0.5 + 0.5 * (t * std::f64::consts::TAU / 1.6).cos()) as f32;
                p.rect_stroke(hole, CornerRadius::same(3), Stroke::new(2.0, alpha(DANGER, a)), StrokeKind::Inside);
                let tag = Rect::from_min_size(Pos2::new(hole.min.x, (hole.min.y - 28.0).max(4.0)), Vec2::new(52.0, 22.0));
                p.rect_filled(tag, CornerRadius::same(6), DANGER);
                p.circle_filled(Pos2::new(tag.min.x + 12.0, tag.center().y), 3.5, Color32::WHITE);
                p.text(Pos2::new(tag.min.x + 22.0, tag.center().y), Align2::LEFT_CENTER, "REC", kit::font(11.0), Color32::WHITE);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
            }
            _ => {
                let corners = [hole.left_top(), hole.right_top(), hole.right_bottom(), hole.left_bottom(), hole.left_top()];
                for w in corners.windows(2) {
                    p.extend(egui::Shape::dashed_line(&[w[0], w[1]], Stroke::new(2.0, ACCENT_2), 6.0, 4.0));
                }
            }
        }
    });
}
