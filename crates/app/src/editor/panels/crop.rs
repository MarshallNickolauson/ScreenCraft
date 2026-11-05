use super::*;

pub fn show(st: &mut EditorState, ui: &mut Ui) {
    kit::h3(ui, "Crop", true);
    let c = st.project.config.crop;
    let vals = [
        ("Left", "l", c.x * 100.0),
        ("Top", "t", c.y * 100.0),
        ("Right", "r", (1.0 - c.x - c.width) * 100.0),
        ("Bottom", "b", (1.0 - c.y - c.height) * 100.0),
    ];
    let mut v = [vals[0].2, vals[1].2, vals[2].2, vals[3].2];
    let mut changed = false;
    for (i, (name, _, _)) in vals.iter().enumerate() {
        changed |= kit::slider(ui, name, &mut v[i], 0.0, 60.0, 0.5, |n| format!("{n:.1}%"));
    }
    if changed {
        st.edit_keyed(Some("crop"), |p| {
            let k = |n: f64| n.clamp(0.0, 80.0) / 100.0;
            let (l, t, r, b) = (k(v[0]), k(v[1]), k(v[2]), k(v[3]));
            p.config.crop.x = l;
            p.config.crop.y = t;
            p.config.crop.width = (1.0 - l - r).max(0.1);
            p.config.crop.height = (1.0 - t - b).max(0.1);
        });
    }
    if kit::btn_line(ui, "Reset crop", false).clicked() {
        st.edit(|p| p.config.crop = screencraft_core::project::Rect::FULL);
    }

    kit::h3(ui, "Screen layout", false);
    kit::hint(ui, "Drag the screen in the preview to move it. Drag its lower-right corner to resize it.");
    let mut scale = st.project.config.styles.screen_scale;
    if kit::slider(ui, "Screen size", &mut scale, 0.25, 3.0, 0.05, |v| format!("{}%", (v * 100.0).round())) {
        st.edit_keyed(Some("screen-scale"), |p| p.config.styles.screen_scale = scale);
    }
    if kit::btn_line(ui, "Reset screen layout", false).clicked() {
        st.edit(|p| {
            p.config.styles.screen_scale = 1.0;
            p.config.styles.screen_offset = screencraft_core::project::Point::default();
        });
    }
}
