//! Renders the application icon from a Typst source file.
//!
//! Usage: `cargo run --release --example icon -p typst-studio-core -- <dir> <out.png>`
//! The directory must contain `icon.typ`.

use std::path::PathBuf;

use typst_studio_core::Session;

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("project directory"));
    let out = PathBuf::from(args.next().expect("output PNG path"));

    let mut session = Session::new(root);
    let id = session.open_file("icon.typ").expect("icon.typ should exist");
    session.world().set_main(id);

    let preview = session.preview();
    for diagnostic in &preview.diagnostics {
        println!("{}: {}", if diagnostic.error { "error" } else { "warning" }, diagnostic.message);
    }
    assert!(preview.updated, "the icon should compile");

    // 4 pixels per point turns the 256pt page into a 1024px icon.
    let png = session.export_png(0, 4.0).expect("page 0");
    std::fs::write(&out, png).expect("write the icon");
    println!("wrote {}", out.display());
}
