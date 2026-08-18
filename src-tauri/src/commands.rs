//! Commands invoked from the frontend.
//!
//! All offsets crossing this boundary are UTF-16 code units, the unit
//! JavaScript strings and CodeMirror use. Conversion to Typst's UTF-8 byte
//! offsets happens here, so neither side has to think about it.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{Manager, State};
use typst_studio_core::{
    CompletionKind, SessionId, Tooltip, Workspace, byte_offset, utf16_offset,
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
    /// Start offset in UTF-16 code units, absent if the diagnostic does not
    /// point into the document being edited.
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

type Response<T> = Result<T, String>;

#[tauri::command]
pub fn create_session(workspace: State<Workspace>, root: String) -> SessionId {
    workspace.create(PathBuf::from(root))
}

#[tauri::command]
pub fn close_session(workspace: State<Workspace>, session: SessionId) -> bool {
    workspace.close(session)
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

    let id = session.world().main_id();
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
    let id = session.world().main_id();
    let text = session.world().source_text(id).unwrap_or_default();

    let diagnostics = preview
        .diagnostics
        .iter()
        .map(|diag| {
            // Only offsets inside the edited document are meaningful to the
            // editor; a diagnostic from an imported file has no place to point.
            let range = diag
                .range
                .clone()
                .filter(|_| diag.file == Some(id))
                .map(|r| (utf16_offset(&text, r.start), utf16_offset(&text, r.end)));

            Diagnostic {
                message: diag.message.clone(),
                error: diag.error,
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
    let Some(text) = main_text(&session) else {
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
    let Some(text) = main_text(&session) else {
        return Ok(None);
    };

    Ok(session
        .tooltip(byte_offset(&text, cursor))
        .map(|tooltip| match tooltip {
            Tooltip::Text(text) => TooltipInfo { text: text.into(), code: false },
            Tooltip::Code(text) => TooltipInfo { text: text.into(), code: true },
        }))
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
fn main_text(session: &typst_studio_core::Session) -> Option<String> {
    session.world_ref().source_text(session.world_ref().main_id())
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

    /// A missing session must be an error, not a panic.
    #[test]
    fn unknown_session_is_reported() {
        let workspace = Workspace::new();
        let err = recompile(&workspace, 42).unwrap_err();
        assert!(err.contains("42"), "error should name the session: {err}");
    }
}
