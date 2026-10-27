//! Developer tool: render a screen of the app to a PNG without opening a window.
//!
//! `screencraft-shot <out.png> [--open <package>] [--seek <ms>] [--select zoom:0|slice:1|cursor|style|...]
//!                   [--size 1440x900] [--scale 1.0] [--dark|--light] [--wait-ms 1500]`
use eframe::egui;
use screencraft::{app::App, headless::Headless};

fn main() {
    // a screenshot must never grab the camera or microphone the real app may be using
    std::env::set_var("SCREENCRAFT_NO_LIVE", "1");
    let args: Vec<String> = std::env::args().collect();
    let out = args.get(1).cloned().unwrap_or_else(|| "shot.png".into());
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned());
    let (w, h) = flag("--size")
        .and_then(|s| s.split_once('x').map(|(a, b)| (a.parse().unwrap_or(1440), b.parse().unwrap_or(900))))
        .unwrap_or((1440u32, 900u32));
    let scale: f32 = flag("--scale").and_then(|s| s.parse().ok()).unwrap_or(1.0);
    let wait_ms: u64 = flag("--wait-ms").and_then(|s| s.parse().ok()).unwrap_or(1500);
    let px = [(w as f32 * scale) as u32, (h as f32 * scale) as u32];

    let mut head = Headless::new(px, scale).expect("GPU");
    if args.iter().any(|a| a == "--light") {
        head.ctx.set_theme(egui::ThemePreference::Light);
    } else if args.iter().any(|a| a == "--dark") {
        head.ctx.set_theme(egui::ThemePreference::Dark);
    }
    let mut app = App::new_headless(&head.ctx, head.state.clone());
    if args.iter().any(|a| a == "--bar") {
        app.dev_bar();
    }
    if args.iter().any(|a| a == "--hud") {
        app.dev_hud();
    }
    if let Some(m) = flag("--menu") {
        app.dev_menu(&m);
    }
    if let Some(p) = flag("--open") {
        app.open_package(&head.ctx, std::path::PathBuf::from(p));
        if let Some(ms) = flag("--seek").and_then(|s| s.parse::<f64>().ok()) {
            app.dev_seek(ms);
        }
        for what in args.iter().enumerate().filter(|(_, a)| *a == "--select").filter_map(|(i, _)| args.get(i + 1)) {
            app.dev_select(Some(what));
        }
    }

    // Let background decoding and probing finish, drawing frames as we go.
    let start = std::time::Instant::now();
    let mut pixels = Vec::new();
    while start.elapsed().as_millis() < wait_ms as u128 || pixels.is_empty() {
        pixels = head.frame(|ui| app.draw(ui));
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    let f = std::fs::File::create(&out).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), px[0], px[1]);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&pixels).unwrap();
    println!("wrote {out} ({}x{})", px[0], px[1]);
}
