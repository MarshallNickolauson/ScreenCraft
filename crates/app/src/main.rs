/// The app icon (the same artwork as the bundle's), so the Dock never shows a default one.
fn icon() -> eframe::egui::IconData {
    let dec = png::Decoder::new(std::io::Cursor::new(include_bytes!("../icon.png").as_slice()));
    let mut r = dec.read_info().expect("icon");
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).expect("icon frame");
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buf[..info.buffer_size()].chunks_exact(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        _ => vec![0; (info.width * info.height * 4) as usize],
    };
    eframe::egui::IconData { rgba, width: info.width, height: info.height }
}

fn main() -> eframe::Result {
    let mut options = eframe::NativeOptions {
        // The recorder bar: borderless, transparent, always on top, bottom-centre of the display.
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("ScreenCraft")
            .with_inner_size([screencraft::recorder::BAR_W, screencraft::recorder::BAR_H])
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false)
            .with_has_shadow(false)
            .with_always_on_top()
            .with_icon(icon()),
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    #[cfg(target_os = "macos")]
    {
        options.event_loop_builder = Some(Box::new(screencraft::configure_macos_event_loop));
    }
    eframe::run_native("ScreenCraft", options, Box::new(|cc| Ok(Box::new(screencraft::app::App::new(cc)))))
}
