//! Release checklist: compile and export the documents of a project.
//!
//! Usage: `cargo run --release --example checklist -p typst-studio-core -- <dir> <file.typ>...`

use std::path::PathBuf;

use typst_studio_core::{Session, SystemFonts};

fn main() {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("project directory"));

    for name in args {
        // Embedded fonts only: output must not depend on the machine.
        let mut session = Session::with_fonts(root.clone(), SystemFonts::Exclude);
        let id = session.open_file(&name).expect("file should open");
        session.world().set_main(id);

        let preview = session.preview();
        let errors: Vec<_> = preview.diagnostics.iter().filter(|d| d.error).collect();
        let pdf = session
            .export_pdf()
            .expect("a compiled document")
            .expect("PDF export");

        let out = root.join(format!("{}.pdf", name.replace('/', "-")));
        std::fs::write(&out, &pdf).expect("write the PDF");

        println!(
            "{name}: pages={} errors={} pdf={} bytes -> {}",
            session.page_count(),
            errors.len(),
            pdf.len(),
            out.display()
        );
        for error in errors {
            println!("   error: {}", error.message);
        }
    }
}
