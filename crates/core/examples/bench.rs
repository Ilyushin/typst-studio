//! Measures compilation latency, which decides the editor's debounce and
//! whether compilations need to be cancellable.
//!
//! Run with `cargo run --release --example bench`. Debug builds are far too
//! slow to say anything useful about the real thing.

use std::time::{Duration, Instant};

use typst_studio_core::Session;

/// How many times each edit scenario is repeated.
const ROUNDS: usize = 7;

fn main() {
    let text = document(130);
    println!("document: {} bytes", text.len());

    let mut session = Session::new(std::env::temp_dir());
    session.world().open(None, text.clone()).unwrap();

    // Cold: nothing is memoized yet.
    let start = Instant::now();
    let preview = session.preview();
    let cold = start.elapsed();

    assert!(preview.updated, "document should compile");
    assert!(preview.diagnostics.is_empty(), "test document must be clean");

    let pages = session.page_count();
    println!("pages: {pages}");

    // One page to SVG, the per-frame cost of the preview pane.
    let start = Instant::now();
    session.page_svg(0).unwrap();
    let svg = start.elapsed();

    // The whole document, i.e. what a naive preview pane would do per keystroke.
    let start = Instant::now();
    for i in 0..pages {
        session.page_svg(i).unwrap();
    }
    let svg_all = start.elapsed();

    // Warm: a single character typed into the first heading. Everything after
    // the edit shifts, which is the worst realistic case.
    let head = text.find("= Section 1").expect("generated heading") + 2;
    let at_start = measure(&mut session, move |_| head..head);

    // Warm: a single character typed at the very end of the document.
    let at_end = measure(&mut session, |text| text.len()..text.len());

    println!();
    println!("cold compile:      {}", fmt(cold));
    println!("edit at start:     {}", fmt(at_start));
    println!("edit at end:       {}", fmt(at_end));
    println!("svg (1 page):      {}", fmt(svg));
    println!("svg (all pages):   {}", fmt(svg_all));
}

/// Applies `ROUNDS` single-character edits at the position chosen by `where_at`
/// and returns the median recompilation time.
fn measure(
    session: &mut Session,
    where_at: impl Fn(&str) -> std::ops::Range<usize>,
) -> Duration {
    let id = session.world().main_id();
    let mut timings = Vec::with_capacity(ROUNDS);

    for i in 0..ROUNDS {
        // A different character each round, so no round can be served straight
        // from the cache as an identical document.
        let ch = char::from(b'a' + (i % 26) as u8).to_string();
        let range = {
            let source = session.world().source_text(id).unwrap();
            where_at(&source)
        };
        session.world().edit(id, range, &ch).unwrap();

        let start = Instant::now();
        let preview = session.preview();
        timings.push(start.elapsed());

        if let Some(error) = preview.diagnostics.iter().find(|d| d.error) {
            panic!("edit broke the document: {}", error.message);
        }
        assert!(preview.updated, "edit must keep the document valid");
    }

    timings.sort();
    timings[timings.len() / 2]
}

/// Builds a multi-page document with the ingredients of a real one: headings,
/// prose, a formula, and a table.
fn document(sections: usize) -> String {
    let mut out = String::from("#set page(width: 21cm, height: 29.7cm)\n");

    for i in 1..=sections {
        out.push_str(&format!("\n= Section {i}\n\n"));
        for _ in 0..3 {
            out.push_str(
                "Lorem ipsum dolor sit amet, consectetur adipiscing elit, sed do \
                 eiusmod tempor incididunt ut labore et dolore magna aliqua. Ut enim \
                 ad minim veniam, quis nostrud exercitation ullamco laboris nisi ut \
                 aliquip ex ea commodo consequat.\n\n",
            );
        }
        out.push_str("$ f(x) = sum_(k=1)^n x^k / k! $\n\n");
        out.push_str(
            "#table(\n  columns: 3,\n  [A], [B], [C],\n  [1], [2], [3],\n)\n\n",
        );
    }

    out
}

fn fmt(duration: Duration) -> String {
    format!("{:>8.1} ms", duration.as_secs_f64() * 1000.0)
}
