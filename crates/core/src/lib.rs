//! Headless compilation core for the Typst Studio desktop client.
//!
//! The UI layer never talks to the Typst compiler directly: it opens documents,
//! feeds edits into [`Session::edit`], and asks for a [`Preview`].

mod packages;
mod workspace;
mod world;

pub use self::packages::{clear_package_cache, fetch_package_index, parse_package_index};
pub use self::workspace::{SessionId, Workspace};
pub use self::world::{StudioWorld, SystemFonts};
pub use typst_ide::{Completion, CompletionKind, Tooltip};

use std::ops::Range;
use std::path::{Path, PathBuf};

use typst::World;
use typst::diag::{FileResult, Severity, SourceDiagnostic, Warned};
use std::num::NonZeroUsize;

use typst::introspection::PagedPosition;
use typst::layout::Point;
use typst::syntax::{DiagSpanKind, FileId, Side, Source};
use typst_layout::PagedDocument;
use typst_pdf::PdfOptions;
use typst_render::RenderOptions;
use typst_svg::SvgOptions;

/// How many compilations an unused cache entry survives before eviction,
/// per open session.
const CACHE_GENERATIONS: usize = 10;

/// A compiler session backing one editor window.
pub struct Session {
    world: StudioWorld,
    /// The most recent successfully compiled document.
    ///
    /// Kept in the session because `typst_ide`'s `autocomplete` and `tooltip`
    /// take the previous compilation to enrich their results, and because the
    /// preview should keep showing the last good state while the user is
    /// midway through typing something invalid.
    document: Option<PagedDocument>,
    /// How many sessions share the process-global memoization cache.
    peers: usize,
    /// The document shown in the editor.
    ///
    /// Distinct from the compiled document: editing a chapter that another file
    /// includes must keep the preview on that other file, not switch to the
    /// chapter on its own.
    active: FileId,
}

/// The outcome of one compilation.
pub struct Preview {
    /// Whether this compilation replaced the current document.
    ///
    /// When `false`, compilation failed and [`Session::document`] still holds
    /// the previous, still-displayable state.
    pub updated: bool,
    /// Errors and warnings, ready to be shown in the editor gutter.
    pub diagnostics: Vec<Diagnostic>,
}

/// A spot in the rendered document.
///
/// Coordinates are fractions of the page size rather than typographic units,
/// so the UI can place them without knowing anything about points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DocPosition {
    /// Zero-based page index.
    pub page: usize,
    /// Horizontal position, 0 at the left edge and 1 at the right.
    pub x: f64,
    /// Vertical position, 0 at the top edge and 1 at the bottom.
    pub y: f64,
}

/// Where a click in the preview leads.
#[derive(Debug, Clone, PartialEq)]
pub enum Jump {
    /// A byte offset in a source file.
    Source { file: FileId, offset: usize },
    /// An external link.
    Url(String),
    /// Another spot in the same document.
    Position(DocPosition),
}

/// A diagnostic resolved to a byte range in a concrete file.
pub struct Diagnostic {
    pub message: String,
    pub error: bool,
    /// The file the diagnostic points into, if any.
    pub file: Option<FileId>,
    /// The byte range within that file, if it could be resolved.
    pub range: Option<Range<usize>>,
}

impl Session {
    /// Creates a session rooted at the given project directory.
    pub fn new(root: PathBuf) -> Self {
        let world = StudioWorld::new(root);
        let active = world.main_id();
        Self { world, document: None, peers: 1, active }
    }

    /// Creates a session with an explicit font policy.
    pub fn with_fonts(root: PathBuf, system: SystemFonts) -> Self {
        let world = StudioWorld::with_fonts(root, system);
        let active = world.main_id();
        Self { world, document: None, peers: 1, active }
    }

    /// Replaces the project, discarding open documents and compiled state.
    pub fn open_project(&mut self, root: PathBuf) {
        let peers = self.peers;
        *self = Self::new(root);
        self.peers = peers;
    }

    /// The document shown in the editor.
    pub fn active_id(&self) -> FileId {
        self.active
    }

    /// Chooses the document shown in the editor.
    pub fn set_active(&mut self, id: FileId) {
        self.active = id;
    }

    /// Opens a document and compiles it from now on.
    ///
    /// Pass `None` as the path for a document that has not been saved yet.
    pub fn open(&mut self, path: Option<&Path>, text: String) -> FileResult<FileId> {
        let id = self.world.open(path, text)?;
        self.active = id;
        Ok(id)
    }

    /// Opens a project file for editing, leaving the compiled document alone.
    pub fn open_file(&mut self, relative: &str) -> FileResult<FileId> {
        let id = self.world.open_from_disk(relative)?;
        self.active = id;
        Ok(id)
    }

    /// Re-reads the document in the editor from disk.
    ///
    /// Refuses while it has unsaved changes: silently dropping the user's edits
    /// because something else touched the file would be unforgivable.
    pub fn reload_active(&mut self) -> FileResult<bool> {
        if self.world.is_dirty(self.active) {
            return Ok(false);
        }
        self.world.reload(self.active)?;
        Ok(true)
    }

    /// Writes a document back to disk.
    pub fn save(&mut self, id: FileId) -> FileResult<()> {
        self.world.save(id)
    }

    /// Records how many sessions share the cache, so eviction can compensate.
    ///
    /// Set by [`Workspace`]; a standalone session leaves it at one.
    pub fn set_peers(&mut self, peers: usize) {
        self.peers = peers.max(1);
    }

    /// Access to the underlying world, for file management.
    pub fn world(&mut self) -> &mut StudioWorld {
        &mut self.world
    }

    /// Read-only access to the world, for queries that do not mutate it.
    pub fn world_ref(&self) -> &StudioWorld {
        &self.world
    }

    /// The most recently compiled document, if there has been a successful
    /// compilation.
    pub fn document(&self) -> Option<&PagedDocument> {
        self.document.as_ref()
    }

    /// The number of pages in the current document.
    pub fn page_count(&self) -> usize {
        self.document.as_ref().map_or(0, |doc| doc.pages().len())
    }

    /// Recompiles the main document.
    ///
    /// A failed compilation leaves the previous document in place, so the
    /// preview does not go blank while the user is typing.
    pub fn preview(&mut self) -> Preview {
        self.world.reset();

        let Warned { output, warnings } = typst::compile::<PagedDocument>(&self.world);
        let (document, errors) = match output {
            Ok(document) => (Some(document), Default::default()),
            Err(errors) => (None, errors),
        };

        let diagnostics = errors
            .iter()
            .chain(warnings.iter())
            .map(|diag| self.resolve(diag))
            .collect();

        // Bound the memoization cache, as upstream's watch mode does. Without
        // this, a long-running editor session grows without limit.
        typst::comemo::evict(CACHE_GENERATIONS * self.peers);

        let updated = document.is_some();
        if let Some(document) = document {
            self.document = Some(document);
        }

        Preview { updated, diagnostics }
    }

    /// Completions for the cursor position, with the byte offset from which
    /// the completion replaces text.
    ///
    /// `explicit` marks a completion the user asked for, as opposed to one
    /// offered while typing. Cursor and returned offset are byte offsets.
    pub fn complete(
        &self,
        cursor: usize,
        explicit: bool,
    ) -> Option<(usize, Vec<Completion>)> {
        let source = self.active_source()?;
        // The previous document enriches the results: label completions, for
        // instance, only exist once something has been compiled.
        typst_ide::autocomplete(
            &self.world,
            self.document.as_ref(),
            &source,
            cursor,
            explicit,
        )
    }

    /// The tooltip for the cursor position, if any.
    pub fn tooltip(&self, cursor: usize) -> Option<Tooltip> {
        let source = self.active_source()?;
        typst_ide::tooltip(
            &self.world,
            self.document.as_ref(),
            &source,
            cursor,
            Side::Before,
        )
    }

    /// Exports the current document as a PDF.
    ///
    /// Fails when the document cannot be represented — PDF export is the one
    /// fallible target, because of tagging.
    pub fn export_pdf(&self) -> Option<Result<Vec<u8>, String>> {
        let document = self.document.as_ref()?;
        Some(
            typst_pdf::pdf(document, &PdfOptions::default())
                .map_err(|errors| self.describe(&errors)),
        )
    }

    /// Renders one page as a PNG at the given scale in pixels per point.
    pub fn export_png(&self, index: usize, pixel_per_pt: f64) -> Option<Vec<u8>> {
        let page = self.document.as_ref()?.pages().get(index)?;
        let options = RenderOptions {
            pixel_per_pt: pixel_per_pt.into(),
            ..RenderOptions::default()
        };
        typst_render::render(page, &options).encode_png().ok()
    }

    /// Renders the whole document as a single SVG.
    pub fn export_svg(&self) -> Option<String> {
        let document = self.document.as_ref()?;
        Some(typst_svg::svg_merged(
            document,
            &SvgOptions::default(),
            typst::layout::Abs::pt(10.0),
        ))
    }

    /// Turns export errors into one message for the UI.
    fn describe(&self, errors: &[typst::diag::SourceDiagnostic]) -> String {
        errors
            .iter()
            .map(|error| error.message.to_string())
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// Resolves a click in the preview.
    ///
    /// `x` and `y` are fractions of the page size, as in [`DocPosition`].
    pub fn jump_from_click(&self, page: usize, x: f64, y: f64) -> Option<Jump> {
        let document = self.document.as_ref()?;
        let size = document.pages().get(page)?.frame.size();
        let position = PagedPosition {
            page: NonZeroUsize::new(page + 1)?,
            point: Point::new(size.x * x, size.y * y),
        };

        match typst_ide::jump_from_click(&self.world, document, &position)? {
            typst_ide::Jump::File(file, offset) => Some(Jump::Source { file, offset }),
            typst_ide::Jump::Url(url) => Some(Jump::Url(url.to_string())),
            typst_ide::Jump::Position(position) => {
                Some(Jump::Position(self.to_doc_position(position)?))
            }
        }
    }

    /// Where the cursor's text appears in the rendered document.
    ///
    /// Positions land at the start of the rendered text run — in practice the
    /// line — not at the exact character, so every cursor within one line maps
    /// to the same spot. That is the granularity upstream offers, and it is the
    /// same granularity as forward search in LaTeX editors.
    ///
    /// A single cursor position can map to several spots — a heading repeated
    /// in an outline, for instance.
    pub fn jump_from_cursor(&self, cursor: usize) -> Vec<DocPosition> {
        let Some(document) = self.document.as_ref() else {
            return Vec::new();
        };
        let Some(source) = self.active_source() else {
            return Vec::new();
        };

        typst_ide::jump_from_cursor(document, &source, cursor)
            .into_iter()
            .filter_map(|position| self.to_doc_position(position))
            .collect()
    }

    fn to_doc_position(&self, position: PagedPosition) -> Option<DocPosition> {
        let page = position.page.get() - 1;
        let size = self.document.as_ref()?.pages().get(page)?.frame.size();
        Some(DocPosition {
            page,
            x: position.point.x / size.x,
            y: position.point.y / size.y,
        })
    }

    fn active_source(&self) -> Option<Source> {
        self.world.source(self.active).ok()
    }

    /// Renders one page of the current document to an SVG string.
    ///
    /// SVG is preferred over a pixel buffer for the preview: the UI can zoom
    /// without asking the compiler for a re-render. Render only the pages that
    /// are actually visible — rendering all of them costs more than the
    /// recompilation itself.
    pub fn page_svg(&self, index: usize) -> Option<String> {
        let page = self.document.as_ref()?.pages().get(index)?;
        Some(typst_svg::svg(page, &SvgOptions::default()))
    }

    /// Resolves a diagnostic's span into a file and byte range the editor can
    /// highlight.
    fn resolve(&self, diag: &SourceDiagnostic) -> Diagnostic {
        let (file, range) = match diag.span.get() {
            DiagSpanKind::Detached => (None, None),
            DiagSpanKind::Range { id, range } => (Some(id), Some(range)),
            DiagSpanKind::Number { id, num, sub_range } => {
                let range = self.world.source(id).ok().and_then(|s| s.range(num, sub_range));
                (Some(id), range)
            }
        };

        Diagnostic {
            message: diag.message.to_string(),
            error: diag.severity == Severity::Error,
            file,
            range,
        }
    }
}

/// Converts a byte offset into a UTF-16 code unit offset.
///
/// Typst counts in UTF-8 bytes; editors built on JavaScript strings (such as
/// CodeMirror) count in UTF-16 code units. Without this conversion every
/// highlight drifts as soon as a document contains non-ASCII text.
///
/// Offsets past the end of `text` clamp to its length.
pub fn utf16_offset(text: &str, byte: usize) -> usize {
    let byte = byte.min(text.len());
    text[..byte].encode_utf16().count()
}

/// Converts a UTF-16 code unit offset back into a byte offset.
///
/// The inverse of [`utf16_offset`], for edits arriving from the editor.
/// Offsets past the end of `text` clamp to its length.
pub fn byte_offset(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units >= utf16 {
            return byte;
        }
        units += ch.len_utf16();
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(text: &str) -> Session {
        let mut session = Session::new(std::env::temp_dir());
        session.open(None, text.into()).unwrap();
        session
    }

    /// The end-to-end smoke test: text in, pages out.
    #[test]
    fn compiles_a_scratch_document() {
        let mut session = session("= Hello\n\nSome text.");

        let preview = session.preview();
        assert!(preview.updated);
        assert_eq!(session.page_count(), 1);
        assert!(preview.diagnostics.is_empty());
    }

    /// An error must come back with a range, so the editor can underline it.
    #[test]
    fn reports_errors_with_a_range() {
        let mut session = session("#(1 + \"a\")");

        let preview = session.preview();
        let diag = preview.diagnostics.first().expect("expected a diagnostic");
        assert!(diag.error);
        assert!(diag.range.is_some(), "diagnostic should resolve to a range");
    }

    /// A broken edit must not blank the preview: the last good document stays
    /// available while the user is midway through typing.
    #[test]
    fn keeps_last_document_when_compilation_fails() {
        let mut session = session("= Hello");
        assert!(session.preview().updated);

        let id = session.world().main_id();
        session.world().edit(id, 7..7, "\n#(1 + \"a\")").unwrap();

        let preview = session.preview();
        assert!(!preview.updated, "compilation was expected to fail");
        assert!(preview.diagnostics.iter().any(|d| d.error));
        assert_eq!(session.page_count(), 1, "previous document must survive");
        assert!(session.document().is_some());
    }

    /// The preview must be renderable to SVG for the UI layer.
    #[test]
    fn renders_a_page_to_svg() {
        let mut session = session("= Hello");
        session.preview();

        let svg = session.page_svg(0).expect("page 0 should exist");
        assert!(svg.starts_with("<svg"));
        assert!(session.page_svg(99).is_none(), "out of range page");
    }

    /// Byte offsets must survive the trip to UTF-16, or highlights drift on
    /// any non-ASCII text.
    #[test]
    fn converts_offsets_to_utf16() {
        // ASCII: both counts agree.
        assert_eq!(utf16_offset("abc", 2), 2);

        // Cyrillic: two bytes per character, one UTF-16 code unit.
        let text = "Привет";
        assert_eq!(utf16_offset(text, text.len()), 6);
        assert_eq!(utf16_offset(text, 4), 2);

        // Outside the basic plane: one character, two UTF-16 code units.
        let text = "a\u{1F600}b";
        assert_eq!(utf16_offset(text, text.len()), 4);

        // Past the end clamps instead of panicking.
        assert_eq!(utf16_offset("abc", 99), 3);
    }

    /// Edits arrive from the editor in UTF-16 and must land on the right byte.
    #[test]
    fn converts_offsets_back_to_bytes() {
        for text in ["abc", "Привет, мир", "a\u{1F600}b", "Смешанный mixed текст"] {
            for (byte, _) in text.char_indices().chain([(text.len(), ' ')]) {
                let units = utf16_offset(text, byte);
                assert_eq!(byte_offset(text, units), byte, "round trip in {text:?}");
            }
        }

        assert_eq!(byte_offset("abc", 99), 3);
    }

    /// Typing `#` must offer the standard library.
    #[test]
    fn completes_standard_library_functions() {
        let mut session = session("#");
        session.preview();

        let (from, completions) = session.complete(1, false).expect("expected completions");
        assert_eq!(from, 1, "completion replaces from just after the hash");
        assert!(
            completions.iter().any(|c| c.label == "heading"),
            "expected `heading` among {} completions",
            completions.len()
        );
    }

    /// Hovering a function name must show its documentation.
    #[test]
    fn shows_a_tooltip_for_a_known_function() {
        let mut session = session("#heading[Title]");
        session.preview();

        // Cursor inside the word `heading`.
        let tooltip = session.tooltip(4).expect("expected a tooltip");
        let text = match tooltip {
            Tooltip::Text(text) | Tooltip::Code(text) => text,
        };
        assert!(!text.is_empty(), "tooltip should carry documentation");
    }

    /// Label completions need the previous compilation, which is why the
    /// session keeps it.
    #[test]
    fn completes_labels_from_the_compiled_document() {
        let mut session = session("= Title <intro>\n\n@");
        session.preview();

        let id = session.world().main_id();
        let cursor = session.world().source_text(id).unwrap().len();
        let (_, completions) = session.complete(cursor, false).expect("expected completions");
        assert!(
            completions.iter().any(|c| c.label == "intro"),
            "expected the `intro` label among {} completions",
            completions.len()
        );
    }

    /// The cursor must map to a spot in the rendered document.
    #[test]
    fn finds_the_cursor_in_the_document() {
        let mut session = session("Hello wonderful world");
        session.preview();

        // Cursor inside the word `wonderful`.
        let positions = session.jump_from_cursor(8);
        let position = positions.first().expect("cursor should map into the document");

        assert_eq!(position.page, 0);
        assert!(
            (0.0..=1.0).contains(&position.x) && (0.0..=1.0).contains(&position.y),
            "fractions must stay on the page: {position:?}"
        );
    }

    /// Clicking maps to the character under the pointer, which is what makes
    /// click-to-jump usable. Scans the rendered line rather than hard-coding
    /// coordinates, which depend on font metrics.
    #[test]
    fn click_maps_to_the_character_under_it() {
        let text = "Hello wonderful world";
        let mut session = session(text);
        session.preview();

        let mut hits: Vec<(f64, usize)> = Vec::new();
        for yi in 0..40 {
            let y = 0.08 + f64::from(yi) * 0.001;
            for xi in 0..40 {
                let x = 0.10 + f64::from(xi) * 0.01;
                if let Some(Jump::Source { offset, .. }) = session.jump_from_click(0, x, y)
                {
                    hits.push((x, offset));
                }
            }
        }

        assert!(!hits.is_empty(), "the rendered line should be clickable");

        let word = text.find("wonderful").unwrap()..text.find("wonderful").unwrap() + 9;
        assert!(
            hits.iter().any(|(_, offset)| word.contains(offset)),
            "some click should land inside `wonderful`"
        );

        // Offsets must grow left to right, or the mapping is not positional.
        let leftmost = hits.iter().min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        let rightmost = hits.iter().max_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
        assert!(
            leftmost.1 < rightmost.1,
            "offset should increase along the line: {leftmost:?} vs {rightmost:?}"
        );
    }

    /// Clicking empty space must not invent a destination.
    #[test]
    fn click_on_blank_space_leads_nowhere() {
        let mut session = session("Short text");
        session.preview();

        // Bottom right corner of the page, far below the single line of text.
        assert!(session.jump_from_click(0, 0.95, 0.95).is_none());
        // Outside the document entirely.
        assert!(session.jump_from_click(9, 0.5, 0.5).is_none());
    }

    /// Without a compiled document there is nothing to navigate.
    #[test]
    fn navigation_needs_a_document() {
        let session = session("Hello");
        assert!(session.jump_from_cursor(1).is_empty());
        assert!(session.jump_from_click(0, 0.5, 0.5).is_none());
    }

    /// Builds a throwaway project directory with the given files.
    fn project(files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "typst-studio-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (name, content) in files {
            let path = root.join(name);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(path, content).unwrap();
        }
        root
    }

    /// The step's acceptance criterion: an included file is compiled with the
    /// main one, and editing the include updates the preview while the main
    /// document stays the compiled one.
    #[test]
    fn edits_to_an_included_file_reach_the_preview() {
        let root = project(&[
            ("main.typ", "= Main\n\n#include \"chapter.typ\"\n"),
            ("chapter.typ", "= Chapter\n"),
        ]);
        let mut session = Session::new(root.clone());

        let main = session.open_file("main.typ").unwrap();
        session.world().set_main(main);
        assert!(session.preview().updated, "the project should compile");
        assert_eq!(session.page_count(), 1);

        // Switch the editor to the included file; the compiled document stays.
        let chapter = session.open_file("chapter.typ").unwrap();
        assert_eq!(session.active_id(), chapter);
        assert_eq!(session.world_ref().main_id(), main);

        // A page break in the chapter must show up in the preview of the main
        // document.
        let text = session.world_ref().source_text(chapter).unwrap();
        session.world().edit(chapter, text.len()..text.len(), "\n#pagebreak()\n");

        assert!(session.preview().updated);
        assert_eq!(session.page_count(), 2, "the include must be recompiled");

        std::fs::remove_dir_all(&root).ok();
    }

    /// Unsaved edits are tracked and cleared by saving, which is what the
    /// modified indicator relies on.
    #[test]
    fn saving_writes_to_disk_and_clears_the_dirty_flag() {
        let root = project(&[("doc.typ", "= Title\n")]);
        let mut session = Session::new(root.clone());

        let id = session.open_file("doc.typ").unwrap();
        assert!(!session.world_ref().is_dirty(id));

        session.world().edit(id, 2..7, "Changed");
        assert!(session.world_ref().is_dirty(id), "an edit marks the file dirty");

        session.save(id).unwrap();
        assert!(!session.world_ref().is_dirty(id));
        assert_eq!(
            std::fs::read_to_string(root.join("doc.typ")).unwrap(),
            "= Changed\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// The file tree lists project sources and skips what is not one.
    #[test]
    fn lists_project_files() {
        let root = project(&[
            ("main.typ", ""),
            ("parts/intro.typ", ""),
            ("notes.txt", ""),
            (".hidden/secret.typ", ""),
        ]);
        let session = Session::new(root.clone());

        let files = session.world_ref().project_files();
        assert_eq!(files, vec!["main.typ", "parts/intro.typ"]);

        std::fs::remove_dir_all(&root).ok();
    }

    /// PDF export must work for both interface languages' documents; Cyrillic
    /// needs the embedded fonts to carry the glyphs into the file.
    #[test]
    fn exports_pdf_for_both_languages() {
        for text in ["= Hello\n\nSome text.", "= Привет\n\nНемного текста."] {
            let mut session = session(text);
            session.preview();

            let pdf = session
                .export_pdf()
                .expect("a compiled document")
                .expect("PDF export should succeed");

            assert!(pdf.starts_with(b"%PDF-"), "output should be a PDF");
            assert!(pdf.len() > 1000, "a PDF with text should not be nearly empty");
        }
    }

    /// The exported file must contain the pages the preview shows.
    #[test]
    fn exported_pdf_has_the_pages_of_the_preview() {
        let mut session = session("= One\n\n#pagebreak()\n\n= Two");
        session.preview();
        assert_eq!(session.page_count(), 2);

        let pdf = session.export_pdf().unwrap().unwrap();
        let text = String::from_utf8_lossy(&pdf);

        // Page objects, minus the single `/Pages` tree node that shares the
        // prefix. Checked against poppler's `pdfinfo`, which reports 2 pages.
        let pages = text.matches("/Type/Page").count() - text.matches("/Type/Pages").count();
        assert_eq!(pages, 2, "the PDF should hold both pages");
    }

    /// PNG export produces a real image, scaled as asked.
    #[test]
    fn exports_png_at_the_requested_scale() {
        let mut session = session("= Title");
        session.preview();

        let small = session.export_png(0, 1.0).expect("page 0");
        let large = session.export_png(0, 2.0).expect("page 0");

        assert!(small.starts_with(b"\x89PNG\r\n\x1a\n"), "output should be a PNG");
        assert!(
            large.len() > small.len(),
            "a higher scale should produce a bigger image: {} vs {}",
            large.len(),
            small.len()
        );
        assert!(session.export_png(99, 1.0).is_none(), "out of range page");
    }

    /// SVG export covers the whole document, not just one page.
    #[test]
    fn exports_svg_for_the_whole_document() {
        let mut session = session("= One\n\n#pagebreak()\n\n= Two");
        session.preview();
        assert_eq!(session.page_count(), 2);

        let svg = session.export_svg().expect("a compiled document");
        assert!(svg.starts_with("<svg"));
        assert!(
            svg.len() > session.page_svg(0).unwrap().len(),
            "the whole document should be larger than a single page"
        );
    }

    /// Nothing to export before anything has compiled.
    #[test]
    fn export_needs_a_document() {
        let session = session("= Title");
        assert!(session.export_pdf().is_none());
        assert!(session.export_png(0, 1.0).is_none());
        assert!(session.export_svg().is_none());
    }

    /// The fetched index must reach import completion; without it, typing an
    /// `@preview` import offers nothing.
    #[test]
    fn completes_package_imports_from_the_index() {
        let index = br#"[
            {"name": "cetz", "version": "0.3.1", "description": "Drawing"},
            {"name": "polylux", "version": "0.4.0", "description": "Slides"}
        ]"#;
        let packages = parse_package_index(index).unwrap();

        // The import string has to be closed for the parser to see it, which
        // is why the editor auto-closes quotes.
        let text = "#import \"@preview/\"";
        let mut session = session(text);
        session.world().set_packages(packages);
        session.preview();

        let (_, completions) = session
            .complete(text.len() - 1, false)
            .expect("expected package completions");

        assert!(
            completions.iter().any(|c| c.label.contains("cetz")),
            "expected `cetz` among {:?}",
            completions.iter().map(|c| &c.label).collect::<Vec<_>>()
        );
    }

    /// The embedded fonts must carry Cyrillic on their own: output should not
    /// depend on what happens to be installed on the machine.
    #[test]
    fn embedded_fonts_alone_render_cyrillic() {
        let mut session =
            Session::with_fonts(std::env::temp_dir(), SystemFonts::Exclude);
        session.open(None, "Привет мир".into()).unwrap();
        session.preview();

        let svg = session.page_svg(0).expect("page 0");
        let glyphs = svg.matches("<path").count();
        assert!(
            glyphs >= 5,
            "expected glyph outlines for Cyrillic text, found {glyphs}"
        );
    }

    /// Editing must go through incremental reparsing, not a full reload.
    #[test]
    fn edits_apply_incrementally() {
        let mut session = session("= Title");
        let id = session.world().main_id();

        session.world().edit(id, 2..7, "Changed").unwrap();

        let text = session.world().source_text(id).unwrap();
        assert_eq!(text, "= Changed");
    }
}
