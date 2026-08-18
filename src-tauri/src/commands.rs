//! Commands invoked from the frontend.
//!
//! All offsets crossing this boundary are UTF-16 code units, the unit
//! JavaScript strings and CodeMirror use. Conversion to Typst's UTF-8 byte
//! offsets happens here, so neither side has to think about it.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{Manager, State};
use typst_studio_core::{
    CompletionKind, DocPosition, Jump, SessionId, Tooltip, Workspace, byte_offset,
    utf16_offset,
};

/// The result of one compilation, as the frontend sees it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    /// Whether the document was replaced. When false, the previously compiled
    /// document is still on display.
    pub updated: bool,
    /// The page count of the currently displayed document.
    pub pages: usize,
    pub diagnostics: Vec<Diagnostic>,
}

/// A diagnostic with offsets the editor can use directly.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub message: String,
    pub error: bool,
    /// Path relative to the project root, absent for an unsaved document or a
    /// diagnostic that points nowhere.
    pub file: Option<String>,
    /// Start offset in UTF-16 code units, present only when the diagnostic
    /// points into the document currently being edited.
    pub from: Option<usize>,
    pub to: Option<usize>,
}

/// Completions for one cursor position.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Completions {
    /// Offset the completion replaces from, in UTF-16 code units.
    pub from: usize,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    /// Replacement text, possibly with `${placeholder}` snippet markers.
    pub apply: Option<String>,
    pub detail: Option<String>,
    pub kind: &'static str,
}

/// A hover tooltip.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TooltipInfo {
    pub text: String,
    /// Whether the text is Typst code rather than prose.
    pub code: bool,
}

/// A spot in the rendered document, in fractions of the page size.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Spot {
    pub page: usize,
    pub x: f64,
    pub y: f64,
}

/// Where a click in the preview leads.
#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Destination {
    /// A cursor offset in the edited document, in UTF-16 code units.
    Cursor { offset: usize },
    /// A spot in another file, which cannot be shown until step 9 opens files.
    OtherFile,
    Url { url: String },
    Spot(Spot),
}

type Response<T> = Result<T, String>;

#[tauri::command]
pub fn create_session(workspace: State<Workspace>, root: String) -> SessionId {
    workspace.create(PathBuf::from(root))
}

#[tauri::command]
pub fn close_session(workspace: State<Workspace>, session: SessionId) -> bool {
    workspace.close(session)
}

/// Opens a project directory, discarding the session's previous state.
#[tauri::command]
pub fn open_project(
    app: tauri::AppHandle,
    workspace: State<Workspace>,
    session: SessionId,
    root: String,
) -> Response<Vec<String>> {
    let files = project(workspace.inner(), session, root)?;
    crate::watch::watch(app, session);
    Ok(files)
}

pub(crate) fn project(
    workspace: &Workspace,
    session: SessionId,
    root: String,
) -> Response<Vec<String>> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let mut session = handle.lock().map_err(lock_poisoned)?;
    session.open_project(PathBuf::from(root));
    Ok(session.world_ref().project_files())
}

/// The Typst files in the project.
#[tauri::command]
pub fn project_files(
    workspace: State<Workspace>,
    session: SessionId,
) -> Response<Vec<String>> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let session = handle.lock().map_err(lock_poisoned)?;
    Ok(session.world_ref().project_files())
}

/// The document shown in the editor.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenFile {
    pub path: String,
    pub text: String,
    /// Whether this file is the one being compiled.
    pub compiled: bool,
    pub dirty: bool,
}

/// Opens a project file for editing, leaving the compiled document alone.
#[tauri::command]
pub fn open_file(
    workspace: State<Workspace>,
    session: SessionId,
    path: String,
) -> Response<OpenFile> {
    file(workspace.inner(), session, path)
}

pub(crate) fn file(
    workspace: &Workspace,
    session: SessionId,
    path: String,
) -> Response<OpenFile> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let mut session = handle.lock().map_err(lock_poisoned)?;

    let id = session.open_file(&path).map_err(|err| err.to_string())?;
    let text = session.world_ref().source_text(id).unwrap_or_default();

    Ok(OpenFile {
        path,
        text,
        compiled: session.world_ref().main_id() == id,
        dirty: session.world_ref().is_dirty(id),
    })
}

/// Makes the file in the editor the one that gets compiled.
#[tauri::command]
pub fn set_compiled(workspace: State<Workspace>, session: SessionId) -> Response<()> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let mut session = handle.lock().map_err(lock_poisoned)?;
    let active = session.active_id();
    session.world().set_main(active);
    Ok(())
}

/// Writes the document in the editor back to disk.
#[tauri::command]
pub fn save(workspace: State<Workspace>, session: SessionId) -> Response<()> {
    store(workspace.inner(), session)
}

pub(crate) fn store(workspace: &Workspace, session: SessionId) -> Response<()> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let mut session = handle.lock().map_err(lock_poisoned)?;
    let active = session.active_id();
    session.save(active).map_err(|err| err.to_string())
}

/// Re-reads the document in the editor from disk after an external change.
///
/// Returns `false` when it has unsaved changes, leaving them untouched for the
/// user to resolve.
#[tauri::command]
pub fn reload(workspace: State<Workspace>, session: SessionId) -> Response<Option<OpenFile>> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let mut session = handle.lock().map_err(lock_poisoned)?;

    if !session.reload_active().map_err(|err| err.to_string())? {
        return Ok(None);
    }

    let id = session.active_id();
    let Some(path) = session.world_ref().relative_path(id) else {
        return Ok(None);
    };

    Ok(Some(OpenFile {
        path,
        text: session.world_ref().source_text(id).unwrap_or_default(),
        compiled: session.world_ref().main_id() == id,
        dirty: false,
    }))
}

/// Whether the document in the editor has unsaved changes.
#[tauri::command]
pub fn is_dirty(workspace: State<Workspace>, session: SessionId) -> Response<bool> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let session = handle.lock().map_err(lock_poisoned)?;
    Ok(session.world_ref().is_dirty(session.active_id()))
}

/// Opens a document in the session, replacing whatever was open.
#[tauri::command]
pub fn open_document(
    workspace: State<Workspace>,
    session: SessionId,
    path: Option<String>,
    text: String,
) -> Response<()> {
    open(workspace.inner(), session, path, text)
}

pub(crate) fn open(
    workspace: &Workspace,
    session: SessionId,
    path: Option<String>,
    text: String,
) -> Response<()> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let mut session = handle.lock().map_err(lock_poisoned)?;
    let path = path.map(PathBuf::from);
    session
        .world()
        .open(path.as_deref(), text)
        .map_err(|err| err.to_string())?;
    Ok(())
}

/// Applies one edit to the open document.
///
/// This is the hot path, invoked on every keystroke.
#[tauri::command]
pub fn apply_edit(
    workspace: State<Workspace>,
    session: SessionId,
    from: usize,
    to: usize,
    text: String,
) -> Response<()> {
    edit(workspace.inner(), session, from, to, text)
}

pub(crate) fn edit(
    workspace: &Workspace,
    session: SessionId,
    from: usize,
    to: usize,
    text: String,
) -> Response<()> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let mut session = handle.lock().map_err(lock_poisoned)?;

    let id = session.active_id();
    let current = session
        .world()
        .source_text(id)
        .ok_or("no document is open")?;

    let range = byte_offset(&current, from)..byte_offset(&current, to);
    session
        .world()
        .edit(id, range, &text)
        .ok_or("no document is open")?;
    Ok(())
}

/// Recompiles the open document.
#[tauri::command]
pub fn compile(workspace: State<Workspace>, session: SessionId) -> Response<CompileResult> {
    recompile(workspace.inner(), session)
}

pub(crate) fn recompile(
    workspace: &Workspace,
    session: SessionId,
) -> Response<CompileResult> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let mut session = handle.lock().map_err(lock_poisoned)?;

    let preview = session.preview();
    let active = session.active_id();
    let text = session.world().source_text(active).unwrap_or_default();

    let diagnostics = preview
        .diagnostics
        .iter()
        .map(|diag| {
            // Offsets are only meaningful for the document on screen; for other
            // files the path is all the editor can act on.
            let range = diag
                .range
                .clone()
                .filter(|_| diag.file == Some(active))
                .map(|r| (utf16_offset(&text, r.start), utf16_offset(&text, r.end)));

            Diagnostic {
                message: diag.message.clone(),
                error: diag.error,
                file: diag.file.and_then(|id| session.world_ref().relative_path(id)),
                from: range.map(|(from, _)| from),
                to: range.map(|(_, to)| to),
            }
        })
        .collect();

    Ok(CompileResult {
        updated: preview.updated,
        pages: session.page_count(),
        diagnostics,
    })
}

/// Completions for the cursor position.
#[tauri::command]
pub fn complete(
    workspace: State<Workspace>,
    session: SessionId,
    cursor: usize,
    explicit: bool,
) -> Response<Option<Completions>> {
    completions(workspace.inner(), session, cursor, explicit)
}

pub(crate) fn completions(
    workspace: &Workspace,
    session: SessionId,
    cursor: usize,
    explicit: bool,
) -> Response<Option<Completions>> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let session = handle.lock().map_err(lock_poisoned)?;
    let Some(text) = active_text(&session) else {
        return Ok(None);
    };

    let Some((from, items)) = session.complete(byte_offset(&text, cursor), explicit)
    else {
        return Ok(None);
    };

    Ok(Some(Completions {
        from: utf16_offset(&text, from),
        items: items
            .into_iter()
            .map(|item| CompletionItem {
                label: item.label.into(),
                apply: item.apply.map(Into::into),
                detail: item.detail.map(Into::into),
                kind: kind_name(&item.kind),
            })
            .collect(),
    }))
}

/// The tooltip for the cursor position.
#[tauri::command]
pub fn tooltip(
    workspace: State<Workspace>,
    session: SessionId,
    cursor: usize,
) -> Response<Option<TooltipInfo>> {
    hover(workspace.inner(), session, cursor)
}

pub(crate) fn hover(
    workspace: &Workspace,
    session: SessionId,
    cursor: usize,
) -> Response<Option<TooltipInfo>> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let session = handle.lock().map_err(lock_poisoned)?;
    let Some(text) = active_text(&session) else {
        return Ok(None);
    };

    Ok(session
        .tooltip(byte_offset(&text, cursor))
        .map(|tooltip| match tooltip {
            Tooltip::Text(text) => TooltipInfo { text: text.into(), code: false },
            Tooltip::Code(text) => TooltipInfo { text: text.into(), code: true },
        }))
}

/// Resolves a click in the preview.
///
/// `x` and `y` are fractions of the page size, so the frontend does not have to
/// know about typographic units.
#[tauri::command]
pub fn jump_from_click(
    workspace: State<Workspace>,
    session: SessionId,
    page: usize,
    x: f64,
    y: f64,
) -> Response<Option<Destination>> {
    click(workspace.inner(), session, page, x, y)
}

pub(crate) fn click(
    workspace: &Workspace,
    session: SessionId,
    page: usize,
    x: f64,
    y: f64,
) -> Response<Option<Destination>> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let session = handle.lock().map_err(lock_poisoned)?;

    Ok(session.jump_from_click(page, x, y).map(|jump| match jump {
        Jump::Source { file, offset } if file == session.active_id() => {
            let text = active_text(&session).unwrap_or_default();
            Destination::Cursor { offset: utf16_offset(&text, offset) }
        }
        Jump::Source { .. } => Destination::OtherFile,
        Jump::Url(url) => Destination::Url { url },
        Jump::Position(position) => Destination::Spot(spot(position)),
    }))
}

/// Where the cursor's text sits in the rendered document.
#[tauri::command]
pub fn jump_from_cursor(
    workspace: State<Workspace>,
    session: SessionId,
    cursor: usize,
) -> Response<Vec<Spot>> {
    cursor_spots(workspace.inner(), session, cursor)
}

pub(crate) fn cursor_spots(
    workspace: &Workspace,
    session: SessionId,
    cursor: usize,
) -> Response<Vec<Spot>> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let session = handle.lock().map_err(lock_poisoned)?;
    let Some(text) = active_text(&session) else {
        return Ok(Vec::new());
    };

    Ok(session
        .jump_from_cursor(byte_offset(&text, cursor))
        .into_iter()
        .map(spot)
        .collect())
}

/// Renders one page to SVG.
///
/// The frontend asks only for pages it is about to show: rendering the whole
/// document costs more than the recompilation that produced it.
#[tauri::command]
pub fn page_svg(
    workspace: State<Workspace>,
    session: SessionId,
    index: usize,
) -> Response<Option<String>> {
    let handle = workspace.get(session).ok_or_else(|| no_session(session))?;
    let session = handle.lock().map_err(lock_poisoned)?;
    Ok(session.page_svg(index))
}

/// Opens another editor window, which will create its own session.
#[tauri::command]
pub fn open_window(app: tauri::AppHandle) -> Response<String> {
    let label = format!("window-{}", app.webview_windows().len() + 1);
    tauri::WebviewWindowBuilder::new(&app, &label, tauri::WebviewUrl::default())
        .title("Typst Studio")
        .inner_size(1200.0, 800.0)
        .build()
        .map_err(|err| err.to_string())?;
    Ok(label)
}

/// The text of the document being edited, if one is open.
fn active_text(session: &typst_studio_core::Session) -> Option<String> {
    session.world_ref().source_text(session.active_id())
}

fn spot(position: DocPosition) -> Spot {
    Spot { page: position.page, x: position.x, y: position.y }
}

/// A stable name for the completion kind, for the frontend to style by.
fn kind_name(kind: &CompletionKind) -> &'static str {
    match kind {
        CompletionKind::Syntax => "syntax",
        CompletionKind::Func => "func",
        CompletionKind::Type => "type",
        CompletionKind::Param => "param",
        CompletionKind::Constant => "constant",
        CompletionKind::Path => "path",
        CompletionKind::Package => "package",
        CompletionKind::Label => "label",
        CompletionKind::Font => "font",
        CompletionKind::Symbol(_) => "symbol",
    }
}

fn no_session(session: SessionId) -> String {
    format!("session {session} does not exist")
}

fn lock_poisoned<T>(_: T) -> String {
    "session is unusable after a panic".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Replays what the frontend does: open a document, type into it using
    /// UTF-16 offsets, recompile. Cyrillic text is the case that breaks if the
    /// offset units are confused anywhere along the way.
    #[test]
    fn edits_land_on_the_right_bytes_in_cyrillic_text() {
        let workspace = Workspace::new();
        let session = workspace.create(std::env::temp_dir());

        open(&workspace, session, None, "= Привет мир".into()).unwrap();
        assert!(recompile(&workspace, session).unwrap().updated);

        // "= Привет мир": in UTF-16 the word "мир" starts at offset 9.
        edit(&workspace, session, 9, 12, "друг".into()).unwrap();

        let handle = workspace.get(session).unwrap();
        let mut guard = handle.lock().unwrap();
        let id = guard.world().main_id();
        assert_eq!(guard.world().source_text(id).unwrap(), "= Привет друг");
    }

    /// Diagnostics must come back with UTF-16 offsets that point at the actual
    /// error, not at a byte offset reinterpreted as a code unit.
    #[test]
    fn diagnostics_use_utf16_offsets() {
        let workspace = Workspace::new();
        let session = workspace.create(std::env::temp_dir());

        // The error sits after Cyrillic text, so byte and UTF-16 offsets differ.
        let text = "Привет\n\n#(1 + \"a\")";
        open(&workspace, session, None, text.into()).unwrap();

        let result = recompile(&workspace, session).unwrap();
        let diag = result
            .diagnostics
            .iter()
            .find(|d| d.error)
            .expect("expected an error");

        let from = diag.from.expect("error should point into the document");
        let utf16: Vec<u16> = text.encode_utf16().collect();
        let head = String::from_utf16(&utf16[..from]).unwrap();
        // The span covers the expression inside the parentheses. Had the byte
        // offset been passed through unconverted, six Cyrillic characters would
        // have shifted this by six.
        assert_eq!(head, "Привет\n\n#(", "offset must land at the expression");
    }

    /// Completion offsets must be UTF-16 on both ends, or the editor replaces
    /// the wrong span in a document with Cyrillic text.
    #[test]
    fn completion_offsets_are_utf16() {
        let workspace = Workspace::new();
        let session = workspace.create(std::env::temp_dir());

        let text = "Привет\n\n#";
        open(&workspace, session, None, text.into()).unwrap();
        recompile(&workspace, session).unwrap();

        let cursor = text.encode_utf16().count();
        let result = completions(&workspace, session, cursor, false)
            .unwrap()
            .expect("expected completions");

        assert_eq!(result.from, cursor, "completion starts at the cursor");
        assert!(
            result.items.iter().any(|item| item.label == "heading"),
            "expected `heading` among {} items",
            result.items.len()
        );
    }

    /// Hovering a function name returns its documentation.
    #[test]
    fn tooltip_is_returned_for_a_function() {
        let workspace = Workspace::new();
        let session = workspace.create(std::env::temp_dir());

        open(&workspace, session, None, "#heading[Title]".into()).unwrap();
        recompile(&workspace, session).unwrap();

        let tooltip = hover(&workspace, session, 4)
            .unwrap()
            .expect("expected a tooltip");
        assert!(!tooltip.text.is_empty());
    }

    /// A click must come back as a UTF-16 cursor offset, not a byte offset.
    #[test]
    fn click_returns_utf16_cursor_offsets() {
        let workspace = Workspace::new();
        let session = workspace.create(std::env::temp_dir());

        // Cyrillic ahead of the target word makes byte and UTF-16 offsets differ.
        let text = "Привет замечательный мир";
        open(&workspace, session, None, text.into()).unwrap();
        recompile(&workspace, session).unwrap();

        let utf16_len = text.encode_utf16().count();
        let mut offsets = Vec::new();
        for yi in 0..40 {
            let y = 0.08 + f64::from(yi) * 0.001;
            for xi in 0..40 {
                let x = 0.10 + f64::from(xi) * 0.01;
                if let Some(Destination::Cursor { offset }) =
                    click(&workspace, session, 0, x, y).unwrap()
                {
                    offsets.push(offset);
                }
            }
        }

        assert!(!offsets.is_empty(), "the rendered line should be clickable");
        assert!(
            offsets.iter().all(|&offset| offset <= utf16_len),
            "offsets must be UTF-16 and stay within the document ({utf16_len} units): {:?}",
            offsets.iter().max()
        );
    }

    /// The cursor maps to a spot on the page, in page fractions.
    #[test]
    fn cursor_maps_to_a_spot_on_the_page() {
        let workspace = Workspace::new();
        let session = workspace.create(std::env::temp_dir());

        open(&workspace, session, None, "Привет мир".into()).unwrap();
        recompile(&workspace, session).unwrap();

        let spots = cursor_spots(&workspace, session, 8).unwrap();
        let spot = spots.first().expect("cursor should map into the document");
        assert_eq!(spot.page, 0);
        assert!((0.0..=1.0).contains(&spot.x) && (0.0..=1.0).contains(&spot.y));
    }

    fn project_dir(files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "typst-studio-cmd-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for (name, content) in files {
            std::fs::write(root.join(name), content).unwrap();
        }
        root
    }

    /// Editing an included file keeps the preview on the compiled document and
    /// reflects the edit — the acceptance criterion for project files.
    #[test]
    fn editing_an_include_updates_the_compiled_document() {
        let root = project_dir(&[
            ("main.typ", "= Main\n\n#include \"chapter.typ\"\n"),
            ("chapter.typ", "= Chapter\n"),
        ]);
        let workspace = Workspace::new();
        let session = workspace.create(root.clone());

        let files = project(&workspace, session, root.display().to_string()).unwrap();
        assert_eq!(files, vec!["chapter.typ", "main.typ"]);

        file(&workspace, session, "main.typ".into()).unwrap();
        let handle = workspace.get(session).unwrap();
        {
            let mut guard = handle.lock().unwrap();
            let main = guard.active_id();
            guard.world().set_main(main);
        }
        assert_eq!(recompile(&workspace, session).unwrap().pages, 1);

        // Switch the editor to the chapter; the main document stays compiled.
        let opened = file(&workspace, session, "chapter.typ".into()).unwrap();
        assert!(!opened.compiled, "the chapter is edited, not compiled");

        let end = opened.text.encode_utf16().count();
        edit(&workspace, session, end, end, "\n#pagebreak()\n".into()).unwrap();

        let result = recompile(&workspace, session).unwrap();
        assert_eq!(result.pages, 2, "the edit must reach the compiled document");

        std::fs::remove_dir_all(&root).ok();
    }

    /// Diagnostics from another file carry its path, so the editor can point at
    /// it instead of dropping the message.
    #[test]
    fn diagnostics_name_the_file_they_come_from() {
        let root = project_dir(&[
            ("main.typ", "#include \"broken.typ\"\n"),
            ("broken.typ", "#(1 + \"a\")\n"),
        ]);
        let workspace = Workspace::new();
        let session = workspace.create(root.clone());
        project(&workspace, session, root.display().to_string()).unwrap();

        file(&workspace, session, "main.typ".into()).unwrap();
        let handle = workspace.get(session).unwrap();
        {
            let mut guard = handle.lock().unwrap();
            let main = guard.active_id();
            guard.world().set_main(main);
        }

        let result = recompile(&workspace, session).unwrap();
        let diagnostic = result
            .diagnostics
            .iter()
            .find(|d| d.error)
            .expect("expected an error from the included file");

        assert_eq!(diagnostic.file.as_deref(), Some("broken.typ"));
        assert!(
            diagnostic.from.is_none(),
            "offsets belong to the edited document only"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Saving writes to disk; an external change is only pulled in when the
    /// editor has nothing to lose.
    #[test]
    fn save_and_reload_respect_unsaved_edits() {
        let root = project_dir(&[("doc.typ", "= One\n")]);
        let workspace = Workspace::new();
        let session = workspace.create(root.clone());
        project(&workspace, session, root.display().to_string()).unwrap();
        file(&workspace, session, "doc.typ".into()).unwrap();

        edit(&workspace, session, 2, 5, "Two".into()).unwrap();
        store(&workspace, session).unwrap();
        assert_eq!(std::fs::read_to_string(root.join("doc.typ")).unwrap(), "= Two\n");

        // An external change with no pending edits is picked up.
        std::fs::write(root.join("doc.typ"), "= Three\n").unwrap();
        let handle = workspace.get(session).unwrap();
        {
            let mut guard = handle.lock().unwrap();
            assert!(guard.reload_active().unwrap());
            let id = guard.active_id();
            assert_eq!(guard.world_ref().source_text(id).unwrap(), "= Three\n");
        }

        // With pending edits it must refuse rather than discard them.
        edit(&workspace, session, 2, 7, "Local".into()).unwrap();
        std::fs::write(root.join("doc.typ"), "= Outside\n").unwrap();
        {
            let mut guard = handle.lock().unwrap();
            assert!(!guard.reload_active().unwrap(), "unsaved edits must survive");
            let id = guard.active_id();
            assert_eq!(guard.world_ref().source_text(id).unwrap(), "= Local\n");
        }

        std::fs::remove_dir_all(&root).ok();
    }

    /// A missing session must be an error, not a panic.
    #[test]
    fn unknown_session_is_reported() {
        let workspace = Workspace::new();
        let err = recompile(&workspace, 42).unwrap_err();
        assert!(err.contains("42"), "error should name the session: {err}");
    }
}
