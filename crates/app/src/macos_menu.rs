use eframe::egui::Context;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{AboutMetadata, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

pub fn configure_event_loop(builder: &mut eframe::EventLoopBuilder<eframe::UserEvent>) {
    use winit::platform::macos::EventLoopBuilderExtMacOS;

    builder.with_default_menu(false);
}

#[derive(Clone, Copy)]
pub enum Command {
    NewRecording,
    OpenProject,
    Save,
    Export,
    Undo,
    Redo,
    Split,
    ToggleJumpCut,
    Delete,
    SelectAll,
    PlayPause,
    AddZoom,
    AddLayout(&'static str),
    AddAnnotation(&'static str),
    Help,
    ReportProblem,
}

pub struct NativeMenu {
    _menu: Menu,
    events: Receiver<MenuId>,
    commands: HashMap<MenuId, Command>,
    record: MenuItem,
    save: MenuItem,
    export: MenuItem,
    undo: MenuItem,
    redo: MenuItem,
    split: MenuItem,
    toggle_jump_cut: MenuItem,
    delete: MenuItem,
    select_all: MenuItem,
    play_pause: MenuItem,
    add_zoom: MenuItem,
    add_layout: Submenu,
    add_note: Submenu,
}

impl NativeMenu {
    pub fn install(ctx: Context) -> Self {
        let mut commands = HashMap::new();

        let about = PredefinedMenuItem::about(
            Some("About ScreenCraft"),
            Some(AboutMetadata {
                name: Some("ScreenCraft".into()),
                version: Some(env!("CARGO_PKG_VERSION").into()),
                ..Default::default()
            }),
        );
        let app_separator = PredefinedMenuItem::separator();
        let services = PredefinedMenuItem::services(None);
        let services_separator = PredefinedMenuItem::separator();
        let hide = PredefinedMenuItem::hide(Some("Hide ScreenCraft"));
        let hide_others = PredefinedMenuItem::hide_others(Some("Hide Others"));
        let show_all = PredefinedMenuItem::show_all(Some("Show All"));
        let quit = PredefinedMenuItem::quit(Some("Quit ScreenCraft"));
        let quit_separator = PredefinedMenuItem::separator();
        let app = Submenu::with_items(
            "ScreenCraft",
            true,
            &[
                &about,
                &app_separator,
                &services,
                &services_separator,
                &hide,
                &hide_others,
                &show_all,
                &quit_separator,
                &quit,
            ],
        )
        .expect("build ScreenCraft application menu");

        let record = command_item(
            "record",
            "Start Recording (⌘⇧R)",
            None,
            Command::NewRecording,
            &mut commands,
        );
        let open = command_item(
            "open",
            "Open Project…",
            Some(command_key(Code::KeyO)),
            Command::OpenProject,
            &mut commands,
        );
        let file_separator = PredefinedMenuItem::separator();
        let save = command_item(
            "save",
            "Save",
            Some(command_key(Code::KeyS)),
            Command::Save,
            &mut commands,
        );
        let export = command_item(
            "export",
            "Export… (⌘E)",
            None,
            Command::Export,
            &mut commands,
        );
        let file_separator_2 = PredefinedMenuItem::separator();
        let close = PredefinedMenuItem::close_window(Some("Close Window"));
        let file = Submenu::with_items(
            "File",
            true,
            &[
                &record,
                &open,
                &file_separator,
                &save,
                &export,
                &file_separator_2,
                &close,
            ],
        )
        .expect("build File menu");

        let undo = command_item("undo", "Undo (⌘Z)", None, Command::Undo, &mut commands);
        let redo = command_item("redo", "Redo (⇧⌘Z)", None, Command::Redo, &mut commands);
        let edit_separator = PredefinedMenuItem::separator();
        let split = command_item(
            "split",
            "Split at Playhead (S)",
            None,
            Command::Split,
            &mut commands,
        );
        let toggle_jump_cut = command_item(
            "jump-cut",
            "Toggle Jump Cut (X)",
            None,
            Command::ToggleJumpCut,
            &mut commands,
        );
        let delete = command_item(
            "delete",
            "Delete Selection (⌫)",
            None,
            Command::Delete,
            &mut commands,
        );
        let edit_separator_2 = PredefinedMenuItem::separator();
        let select_all = command_item(
            "select-all",
            "Select All (⌘A)",
            None,
            Command::SelectAll,
            &mut commands,
        );
        let edit = Submenu::with_items(
            "Edit",
            true,
            &[
                &undo,
                &redo,
                &edit_separator,
                &split,
                &toggle_jump_cut,
                &delete,
                &edit_separator_2,
                &select_all,
            ],
        )
        .expect("build Edit menu");

        let play_pause = command_item(
            "play-pause",
            "Play / Pause (Space)",
            None,
            Command::PlayPause,
            &mut commands,
        );
        let view_separator = PredefinedMenuItem::separator();
        let add_zoom = command_item(
            "add-zoom",
            "Add Zoom at Playhead (Z)",
            None,
            Command::AddZoom,
            &mut commands,
        );

        let bubble = command_item(
            "layout-bubble",
            "Bubble",
            None,
            Command::AddLayout("bubble"),
            &mut commands,
        );
        let camera_off = command_item(
            "layout-hidden",
            "Camera Off",
            None,
            Command::AddLayout("hidden"),
            &mut commands,
        );
        let large_bubble = command_item(
            "layout-large",
            "Large Bubble",
            None,
            Command::AddLayout("large"),
            &mut commands,
        );
        let camera_only = command_item(
            "layout-camera",
            "Camera Only",
            None,
            Command::AddLayout("camera"),
            &mut commands,
        );
        let side_by_side = command_item(
            "layout-side-right",
            "Side by Side",
            None,
            Command::AddLayout("side-right"),
            &mut commands,
        );
        let layout = Submenu::with_items(
            "Add Camera Layout",
            true,
            &[
                &bubble,
                &camera_off,
                &large_bubble,
                &camera_only,
                &side_by_side,
            ],
        )
        .expect("build camera layout menu");

        let text = command_item(
            "note-text",
            "Text",
            None,
            Command::AddAnnotation("text"),
            &mut commands,
        );
        let arrow = command_item(
            "note-arrow",
            "Arrow",
            None,
            Command::AddAnnotation("arrow"),
            &mut commands,
        );
        let box_note = command_item(
            "note-box",
            "Box",
            None,
            Command::AddAnnotation("box"),
            &mut commands,
        );
        let ellipse = command_item(
            "note-ellipse",
            "Ellipse",
            None,
            Command::AddAnnotation("ellipse"),
            &mut commands,
        );
        let highlight = command_item(
            "note-highlight",
            "Highlight",
            None,
            Command::AddAnnotation("highlight"),
            &mut commands,
        );
        let blur = command_item(
            "note-blur",
            "Blur",
            None,
            Command::AddAnnotation("blur"),
            &mut commands,
        );
        let pixelate = command_item(
            "note-pixelate",
            "Pixelate",
            None,
            Command::AddAnnotation("pixelate"),
            &mut commands,
        );
        let spotlight = command_item(
            "note-spotlight",
            "Spotlight",
            None,
            Command::AddAnnotation("spotlight"),
            &mut commands,
        );
        let note = Submenu::with_items(
            "Add Note",
            true,
            &[
                &text, &arrow, &box_note, &ellipse, &highlight, &blur, &pixelate, &spotlight,
            ],
        )
        .expect("build annotation menu");
        let view = Submenu::with_items(
            "View",
            true,
            &[&play_pause, &view_separator, &add_zoom, &layout, &note],
        )
        .expect("build View menu");

        let minimize = PredefinedMenuItem::minimize(Some("Minimize"));
        let zoom = PredefinedMenuItem::zoom(Some("Zoom"));
        let window_separator = PredefinedMenuItem::separator();
        let bring_all = PredefinedMenuItem::bring_all_to_front(Some("Bring All to Front"));
        let window = Submenu::with_items(
            "Window",
            true,
            &[&minimize, &zoom, &window_separator, &bring_all],
        )
        .expect("build Window menu");

        let help_item = command_item(
            "help",
            "ScreenCraft Help",
            None,
            Command::Help,
            &mut commands,
        );
        let help_separator = PredefinedMenuItem::separator();
        let report = command_item(
            "report-problem",
            "Report a Problem…",
            None,
            Command::ReportProblem,
            &mut commands,
        );
        let help = Submenu::with_items("Help", true, &[&help_item, &help_separator, &report])
            .expect("build Help menu");
        help.set_as_help_menu_for_nsapp();

        let menu = Menu::with_items(&[&app, &file, &edit, &view, &window, &help])
            .expect("build macOS menu bar");
        menu.init_for_nsapp();

        let (tx, events) = mpsc::channel::<MenuId>();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            let _ = tx.send(event.id);
            ctx.request_repaint();
        }));

        Self {
            _menu: menu,
            events,
            commands,
            record,
            save,
            export,
            undo,
            redo,
            split,
            toggle_jump_cut,
            delete,
            select_all,
            play_pause,
            add_zoom,
            add_layout: layout,
            add_note: note,
        }
    }

    pub fn poll(&self) -> Vec<Command> {
        let mut out = Vec::new();
        while let Ok(id) = self.events.try_recv() {
            if let Some(command) = self.commands.get(&id) {
                out.push(*command);
            }
        }
        out
    }

    pub fn update(&self, recording: bool, can_save: bool, can_undo: bool, can_redo: bool) {
        self.record.set_text(if recording {
            "Stop Recording"
        } else {
            "Start Recording"
        });
        self.save.set_enabled(can_save);
        self.export.set_enabled(can_save);
        self.undo.set_enabled(can_undo);
        self.redo.set_enabled(can_redo);
        self.split.set_enabled(can_save);
        self.toggle_jump_cut.set_enabled(can_save);
        self.delete.set_enabled(can_save);
        self.select_all.set_enabled(can_save);
        self.play_pause.set_enabled(can_save);
        self.add_zoom.set_enabled(can_save);
        self.add_layout.set_enabled(can_save);
        self.add_note.set_enabled(can_save);
    }
}

fn command_item(
    id: &'static str,
    title: &str,
    accelerator: Option<Accelerator>,
    command: Command,
    commands: &mut HashMap<MenuId, Command>,
) -> MenuItem {
    let item = MenuItem::with_id(id, title, true, accelerator);
    commands.insert(item.id().clone(), command);
    item
}

fn command_key(code: Code) -> Accelerator {
    Accelerator::new(Modifiers::META, code)
}
