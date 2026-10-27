//! `cargo run -p screencraft-capture --example cameras`: list the cameras (phones included).
fn main() {
    for (id, name) in screencraft_capture::camera::list() {
        println!("{id}\t{name}");
    }
}
