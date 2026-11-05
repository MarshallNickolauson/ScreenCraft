//! The permissions window: what ScreenCraft needs from the operating system, with a button for
//! each (a port of `permissions.html`).

use crate::kit;
use crate::recorder::Recorder;
use crate::theme::*;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2,
    ViewportBuilder, ViewportId,
};

fn open_pane(url: &str) {
    let _ = crate::editor::open_url(url);
}

/// Draw the window; returns false once the user is done with it.
pub fn show(ctx: &egui::Context, rec: &mut Recorder) -> bool {
    let mut keep = true;
    let p = rec.permissions;
    let builder = ViewportBuilder::default()
        .with_title("ScreenCraft permissions")
        .with_inner_size([680.0, 820.0])
        .with_min_inner_size([560.0, 620.0])
        .with_resizable(true);
    ctx.show_viewport_immediate(ViewportId::from_hash_of("permissions"), builder, |ui, _| {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin { left: 28, right: 28, top: 24, bottom: 20 }))
            .show_inside(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::new(8.0, 8.0);
                egui::ScrollArea::vertical()
                    .max_height((ui.available_height() - 48.0).max(0.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                    ui.label(egui::RichText::new("Set up ScreenCraft").size(22.0).strong().color(TEXT));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new("Screen Recording is required to record. Camera, microphone, and typing focus are optional.")
                                .size(13.0)
                                .color(MUTED),
                        )
                        .wrap(),
                    );
                    ui.add_space(8.0);

                    section(ui, "Required to record");
                    if permission_row(
                        ui,
                        "monitor",
                        "Screen Recording",
                        "Captures your display, selected windows, or an area of the screen.",
                        Some(p.screen),
                        if p.screen { "Open Settings" } else { "Grant" },
                    ) {
                        if !p.screen {
                            screencraft_capture::request_screen_access();
                        }
                        open_pane("x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture");
                    }

                    section(ui, "Optional recording inputs");
                    if permission_row(
                        ui,
                        "camera",
                        "Camera",
                        "Adds a camera bubble to recordings.",
                        None,
                        "Open Settings",
                    ) {
                        open_pane("x-apple.systempreferences:com.apple.preference.security?Privacy_Camera");
                    }
                    if permission_row(
                        ui,
                        "mic",
                        "Microphone",
                        "Records narration alongside system audio.",
                        None,
                        "Open Settings",
                    ) {
                        open_pane("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
                    }

                    section(ui, "Optional typing focus");
                    if permission_row(
                        ui,
                        "annotate",
                        "Accessibility",
                        "Lets the camera follow the text cursor while you type.",
                        Some(p.accessibility),
                        "Open Settings",
                    ) {
                        open_pane("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility");
                    }
                    if permission_row(
                        ui,
                        "captions",
                        "Input Monitoring",
                        "Detects typing so ScreenCraft can focus the camera on the text cursor.",
                        Some(p.input_monitoring),
                        "Open Settings",
                    ) {
                        if !p.input_monitoring {
                            screencraft_capture::request_input_monitoring();
                        }
                        open_pane("x-apple.systempreferences:com.apple.preference.security?Privacy_ListenEvent");
                    }
                });

                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let refresh = kit::pill(ui, None, "Check status", false, false, false, None);
                    let refresh = mark_button(ui, refresh, "Check permission status", true);
                    if activate(ui, &refresh) {
                        rec.permissions = screencraft_capture::permissions();
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let typing_ready = p.accessibility && p.input_monitoring;
                        let label = if p.screen && !typing_ready { "Continue without typing focus" } else { "Continue" };
                        let response = ui.add_enabled_ui(p.screen, |ui| kit::primary(ui, None, label)).inner;
                        let response = mark_button(ui, response, label, p.screen);
                        if activate(ui, &response) {
                            keep = false;
                        }
                    });
                });
            });
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            keep = false;
        }
    });
    keep
}

fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(title).size(12.0).strong().color(MUTED));
}

fn permission_row(
    ui: &mut egui::Ui,
    icon: &str,
    name: &str,
    description: &str,
    state: Option<bool>,
    action: &str,
) -> bool {
    let width = ui.available_width();
    let (row, _) = ui.allocate_exact_size(Vec2::new(width, 78.0), Sense::hover());
    ui.painter().rect_filled(row, CornerRadius::same(9), PANEL);
    ui.painter().rect_stroke(
        row,
        CornerRadius::same(9),
        Stroke::new(1.0, LINE),
        StrokeKind::Inside,
    );

    let action_width = kit::text_width(ui, action, 12.5) + 22.0;
    let action_rect = Rect::from_min_size(
        Pos2::new(row.max.x - 12.0 - action_width, row.center().y - 19.0),
        Vec2::new(action_width, 38.0),
    );
    let state_text = match state {
        Some(true) => "Granted",
        Some(false) => "Not granted",
        None => "Not checked",
    };
    let state_width = kit::text_width(ui, state_text, 12.0) + 14.0;
    let state_x = action_rect.min.x - state_width - 14.0;
    let text_left = row.min.x + 64.0;
    let text_right = state_x - 12.0;

    let icon_rect = Rect::from_min_size(
        Pos2::new(row.min.x + 14.0, row.center().y - 18.0),
        Vec2::splat(36.0),
    );
    ui.painter()
        .rect_filled(icon_rect, CornerRadius::same(8), PANEL_2);
    crate::icons::paint(
        ui,
        icon,
        Rect::from_center_size(icon_rect.center(), Vec2::splat(20.0)),
        Color32::from_rgb(0xb9, 0xc4, 0xd9),
    );
    ui.painter().text(
        Pos2::new(text_left, row.min.y + 22.0),
        Align2::LEFT_CENTER,
        name,
        kit::font(13.5),
        TEXT,
    );
    ui.put(
        Rect::from_min_max(
            Pos2::new(text_left, row.min.y + 37.0),
            Pos2::new(text_right, row.max.y - 7.0),
        ),
        egui::Label::new(egui::RichText::new(description).size(11.5).color(MUTED)).wrap(),
    );
    let (status_color, status_fill) = match state {
        Some(true) => (OK, alpha(OK, 0.12)),
        Some(false) => (DANGER, alpha(DANGER, 0.12)),
        None => (MUTED, Color32::from_rgb(0x18, 0x20, 0x2d)),
    };
    ui.painter().rect_filled(
        Rect::from_min_size(
            Pos2::new(state_x, row.center().y - 12.0),
            Vec2::new(state_width, 24.0),
        ),
        CornerRadius::same(6),
        status_fill,
    );
    ui.painter().text(
        Pos2::new(state_x + 7.0, row.center().y),
        Align2::LEFT_CENTER,
        state_text,
        kit::font(11.5),
        status_color,
    );

    let response = ui.interact(
        action_rect,
        ui.id().with(("permission", name)),
        Sense::click(),
    );
    ui.painter().rect_filled(
        action_rect,
        CornerRadius::same(7),
        if response.hovered() { RAISE } else { PANEL_2 },
    );
    ui.painter().rect_stroke(
        action_rect,
        CornerRadius::same(7),
        Stroke::new(1.0, if response.has_focus() { TEXT } else { LINE_2 }),
        StrokeKind::Inside,
    );
    ui.painter().text(
        action_rect.center(),
        Align2::CENTER_CENTER,
        action,
        kit::font(12.5),
        TEXT,
    );
    let accessible_label = format!("{action} for {name}. {state_text}. {description}");
    let response = mark_button(ui, response, &accessible_label, true);
    activate(ui, &response)
}

fn mark_button(
    ui: &egui::Ui,
    response: egui::Response,
    label: &str,
    enabled: bool,
) -> egui::Response {
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(2.0),
            CornerRadius::same(9),
            Stroke::new(1.0, TEXT),
            StrokeKind::Outside,
        );
    }
    response
}

fn activate(ui: &egui::Ui, response: &egui::Response) -> bool {
    response.clicked()
        || (response.has_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Space)))
}
