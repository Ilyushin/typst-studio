# Typst Studio development plan

A desktop client for Typst on Tauri 2, with a core that links the compiler
crates directly. Every step ends with a checkable criterion; the next one does
not start until the previous one works.

## Current state

Done and verified (`cargo test`, 23/23; benchmark in step 3):

- `crates/core/src/world.rs` — `StudioWorld`: a `World` implementation on top of
  `typst-kit`. Open documents live in memory as `Source` objects that shadow the
  on-disk state; edits go through `Source::edit` (incremental reparsing, stable
  span numbers).
- `crates/core/src/lib.rs` — `Session::preview()`: compilation to a
  `PagedDocument`, diagnostics resolved to byte ranges, `page_svg()` for the
  preview, `comemo::evict` after every compilation. A failed compilation keeps
  the previous document instead of blanking the preview.
- `crates/core/src/workspace.rs` — several windows, each with its own session.
- `crates/core/examples/bench.rs` — latency measurements.
- `src-tauri`, `ui` — the running application (step 5), interface in English
  and Russian, completion and hover from the compiler (step 7), and two-way
  navigation between text and preview (step 8).

Environment: Rust 1.97.1 (upstream MSRV is 1.92), pinned to rev `35417aa76`.

## Language policy

- Code, comments, documentation, and commit messages: **English only**.
- Application interface: **English and Russian**, switchable at runtime and
  remembered across restarts (`ui/src/i18n.ts`). Every new user-facing string
  goes through `t()` — never a literal in the markup or the code.
- User documents: both languages must render correctly. The embedded fonts cover
  Cyrillic, so output does not depend on system fonts (verified). The Russian
  starter document sets `#set text(lang: "ru")` for correct hyphenation and
  quotation marks.
- Interface language and document language are independent settings.

---

## Step 3. Recompilation benchmark — done

`crates/core/examples/bench.rs`, run with
`cargo run --release --example bench -p typst-studio-core`. A 49-page document
(headings, prose, formulas, tables), 105 KB. Median of 7 single-character edits.
Machine: Apple Silicon, release build.

| Scenario | Time |
|---|---|
| Cold compilation | 75 ms |
| Recompilation after an edit at the start | 8.1 ms |
| Recompilation after an edit at the end | 7.1 ms |
| SVG for one page | 2.8 ms |
| SVG for all 49 pages | 108 ms |

Conclusions:

1. **Incrementality works** — recompilation is 10x cheaper than a cold run. An
   edit at the start costs no more than one at the end: span numbers are
   renumbered locally and layout is cached per element.
2. **The bottleneck is rendering, not compiling.** Drawing the whole document to
   SVG (108 ms) costs 14x the recompilation that produced it (8 ms). The preview
   must render only visible pages.
3. At 8 ms per cycle a 100 ms debounce is excessive; 30-50 ms feels instant.
4. `Session::preview()` calls `comemo::evict(10)`, mirroring upstream's watch
   mode (`crates/typst-cli/src/watch.rs:82`). Without it the cache of a
   long-running editor grows without bound.

Caveats: one synthetic document on one machine. Documents with heavy graphics,
large tables, or packages may behave differently — re-measure on a real document
once the UI exists.

## Step 4. Keeping compilation state in `Session` — done

`Session` owns the most recent successfully compiled `PagedDocument`.

- Field `document: Option<PagedDocument>` with `document()` and `page_count()`.
- `preview()` returns `Preview { updated, diagnostics }`; the document is no
  longer handed out by value.
- **A failed compilation keeps the previous document.** While typing, a document
  is invalid much of the time; blanking the preview on every intermediate error
  is unacceptable. `updated: false` tells the UI it is showing an older state.
- `page_svg(index)` became a session method rendering from the stored document.

Test: `keeps_last_document_when_compilation_fails`. No performance regression.

## Step 5. Tauri shell (MVP) — done

Run with `npm run dev` from the repository root.

- `crates/core/src/workspace.rs` — `Workspace`: multiple sessions, each behind
  its own lock, so windows compile in parallel. The `comemo` cache is
  process-global (windows on the same project reuse each other's work) and
  eviction scales with the number of windows; otherwise one window's document
  ages out of the cache while the user works in another.
- `src-tauri` — commands `create_session`, `close_session`, `open_document`,
  `apply_edit`, `compile`, `page_svg`, `open_window`. The logic sits in plain
  functions over `&Workspace`, so it is testable without a GUI.
- `ui` — CodeMirror 6 plus the preview pane. 40 ms debounce, edits queued and
  applied back-to-front (otherwise earlier offsets shift), preview renders only
  visible pages via `IntersectionObserver`.
- `utf16_offset` / `byte_offset` in the core, converted in `src-tauri`:
  CodeMirror counts UTF-16 code units, Typst counts UTF-8 bytes.
- `ui/src/i18n.ts` — English and Russian interface, plural forms via
  `Intl.PluralRules`, choice remembered in `localStorage`.

Verified: 12 tests (9 core, 3 commands) including an edit and a diagnostic in
Cyrillic text; clippy clean; `tsc` clean; the app starts and a trace showed the
full chain `create_session -> compile -> page_svg(0)`, with only the visible
page requested.

**Still to check by hand** (not automatable without a UI driver): typing updates
the preview, an error is underlined in the right place, the language switch
relabels the interface, the button opens a second window.

Pitfalls worth remembering: a debug Tauri build loads the frontend from the Vite
dev server rather than from `dist`, so `cargo run` alone shows an empty window.
The Tauri CLI only looks for `src-tauri` in subfolders of the current directory,
hence the root `package.json` and the crate living in `src-tauri/` rather than
`crates/`. Core APIs such as `path.homeDir` need a capability declaration
(`src-tauri/capabilities/default.json`).

## Step 6. Cancelling and queueing compilations — low priority

`comemo` cannot interrupt a compilation midway. Per step 3, a cycle on a 49-page
document takes 8 ms, so the queue does not pile up while typing and cancellation
is not urgent. Revisit if recompilation on real documents exceeds ~50 ms.

Tasks:

- Last-one-wins: a new edit cancels a pending compilation, the running one
  finishes.
- Busy indicator when compilation takes longer than ~500 ms.

Criterion: after ten seconds of continuous typing the preview catches up within
one compilation cycle.

## Step 7. IDE features — done

- `impl IdeWorld for StudioWorld`: `upcast()`, `files()` returns the open
  documents for path completion, `packages()` is empty until step 11 fetches the
  Universe index in the background.
- `Session::complete(cursor, explicit)` and `Session::tooltip(cursor)`, both
  passing the stored document — label completions (`@ref`) exist only once
  something has been compiled, which is what step 4 was for.
- Commands `complete` and `tooltip`, converting cursor and replacement offsets
  between UTF-16 and bytes in both directions.
- `ui/src/ide.ts` wires them into CodeMirror. Typst marks placeholders as
  `${name}`, which is also CodeMirror's snippet syntax, so those completions
  become real snippets with tab stops. Queries wait for the edit queue to drain,
  otherwise a query could outrun the text it asks about.
- `Session::world_ref()` added: the read-only queries cannot take the `&mut`
  that `world()` hands out.

Verified: 17 tests (12 core, 5 commands). `completes_standard_library_functions`
covers the `#` case, `completes_labels_from_the_compiled_document` covers labels,
`completion_offsets_are_utf16` covers a completion after Cyrillic text, and
tooltips are checked on both layers. clippy and `tsc` clean; the app starts.

Note: completion and tooltip text comes from the compiler and is English only.
That is upstream's documentation, not ours to translate; our own labels stay
localized.

**Still to check by hand**: the popup appears while typing `#`, Ctrl-Space
forces it, hovering `heading` shows documentation.

## Step 8. Text and preview navigation — done

- `Session::jump_from_click(page, x, y)` and `Session::jump_from_cursor(cursor)`.
  Coordinates are fractions of the page size, so neither the commands nor the
  frontend deal in typographic units.
- Commands `jump_from_click` and `jump_from_cursor`, converting cursor offsets
  to UTF-16 on the way out. A click into another file returns `otherFile`; there
  is nothing to show until step 9 can open files.
- `ui/src/preview.ts` reports clicks as page fractions and draws a cursor marker;
  `scrollIntoView({ block: "nearest" })` keeps the preview still when the spot is
  already visible. `ui/src/main.ts` debounces cursor tracking at 150 ms — cursor
  moves are far more frequent than edits.

Measured behaviour, worth knowing before designing on top of it:

- **Click resolution is per character.** Clicking along a rendered line yields
  offsets that grow left to right, landing inside the clicked word.
- **Cursor resolution is per line.** `jump_from_cursor` returns the start of the
  rendered text run, so every cursor position within one line maps to the same
  spot. That is what upstream resolves, and it matches forward search in LaTeX
  editors. The marker is therefore line-grained by design, not by oversight.

Verified: 23 tests (16 core, 7 commands). `click_maps_to_the_character_under_it`
scans the rendered line rather than hard-coding coordinates, which depend on font
metrics; `click_returns_utf16_cursor_offsets` covers Cyrillic text. clippy and
`tsc` clean; the app starts.

Not handled yet: `Jump::Url` is resolved but ignored, since opening a browser
needs the opener plugin.

**Still to check by hand**: clicking a word in the preview moves the cursor to
it, and moving the cursor marks the matching line in the preview.

## Step 9. Project files

Tasks:

- Open a folder as a project, show a file tree.
- Save (`Cmd+S`), unsaved-changes indicator.
- `typst_kit::watcher::Watcher` for external changes: reload a file changed on
  disk if it is not open; if it is open with unsaved edits, ask the user.
- Multi-file documents mean diagnostics can point into files other than the one
  on screen; today those are dropped. Show them against the right file.

Criterion: `#include` of another project file works, and editing the included
file updates the preview.

## Step 10. Export

Tasks:

- PDF via `typst_pdf::pdf(&document, &PdfOptions)`. `ident` stays `Smart::Auto`
  until there is a stable project identifier.
- PNG pages via `typst-render`, SVG via `typst-svg` (already available).
- Save dialog, and handling of export errors (PDF export is fallible because of
  tagging).

Criterion: the exported PDF opens and matches the preview, for both English and
Russian documents.

## Step 11. Typst Universe packages

Tasks:

- `UniversePackages` is already wired up; move downloads to a background thread
  — they block, and would freeze the UI.
- Download indicator, and a clear message when the network is unavailable.
- Fill `IdeWorld::packages()` for import completions.

Criterion: `#import "@preview/cetz:0.3.1"` on a clean machine downloads the
package without freezing the interface.

## Step 12. Packaging and distribution

Tasks:

- Builds for macOS (signing and notarization) and Windows.
- Auto-update via the Tauri updater.
- Replace the placeholder icon in `src-tauri/icons`.
- Settle the font policy: embedded fonts give reproducible output (and cover
  Cyrillic), system fonts give familiarity. Both are enabled today; make the
  precedence explicit and document it.

Criterion: the installer works on a clean machine and compiles a document.

---

## Open questions

- **HTML export**: Typst can produce `HtmlDocument`; decide whether the client
  needs it.
- **Collaborative editing**: if it is on the roadmap, the core architecture
  changes substantially — decide early.
- **More interface languages**: `i18n.ts` holds a plain record of strings, which
  is fine for two. A third one is the moment to move to a catalogue format.

## Maintenance rules

- Bump the upstream pin deliberately, in its own commit, with tests run.
  Internal APIs (`Routines`, `World`, the `typst-kit` feature set) change
  between versions.
- Client code never goes to `typst/typst`: upstream does not accept
  AI-implemented contributions, and a desktop client is out of its scope.
- Deterministic output: do not use `f64::sin`, `powf`, `ln` and friends —
  upstream bans them via `clippy.toml` for reproducible rendering. If the core
  starts doing geometry itself, carry that rule into our own `clippy.toml`.
