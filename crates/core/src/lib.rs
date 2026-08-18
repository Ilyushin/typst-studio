//! Headless compilation core for the Typst Studio desktop client.
//!
//! The UI layer never talks to the Typst compiler directly: it opens documents,
//! feeds edits into [`Session::edit`], and asks for a [`Preview`].

mod workspace;
mod world;

pub use self::workspace::{SessionId, Workspace};
pub use self::world::StudioWorld;
pub use typst_ide::{Completion, CompletionKind, Tooltip};

use std::ops::Range;
use std::path::PathBuf;

use typst::World;
use typst::diag::{Severity, SourceDiagnostic, Warned};
use typst::syntax::{DiagSpanKind, FileId, Side, Source};
use typst_layout::PagedDocument;
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
        Self { world: StudioWorld::new(root), document: None, peers: 1 }
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
        let source = self.main_source()?;
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
        let source = self.main_source()?;
        typst_ide::tooltip(
            &self.world,
            self.document.as_ref(),
            &source,
            cursor,
            Side::Before,
        )
    }

    fn main_source(&self) -> Option<Source> {
        self.world.source(self.world.main_id()).ok()
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
        session.world().open(None, text.into()).unwrap();
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
