# Typst Studio development plan

A desktop client for Typst on Tauri 2, with a core that links the compiler
crates directly. Every step ends with a checkable criterion; the next one does
not start until the previous one works.

## Current state

Done and verified (`cargo test`, 40/40; benchmark in step 3):

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
  navigation between text and preview (step 8), and multi-file projects with
  file watching (step 9), export to PDF, PNG, and SVG (step 10), and Universe
  packages with off-thread compilation (step 11), and packaging (step 12).

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

## Step 9. Project files — done

The central change is that the **edited** document and the **compiled** document
are now separate. Editing a chapter that another file includes has to keep the
preview on the main document; without the split, clicking a file in the tree
would silently redirect the preview to it.

- `StudioWorld` holds several open documents, tracks which have unsaved changes,
  writes them back with `save`, re-reads them with `reload`, lists project
  sources with `project_files`, and reports `dependencies` for the watcher.
- `Session` owns `active` (in the editor) beside the world's `main` (compiled).
  All IDE queries and edits follow `active`; compilation follows `main`.
- `Session::reload_active` refuses while the file has unsaved changes: dropping
  the user's edits because something else touched the file is not an option.
- Commands: `open_project`, `project_files`, `open_file`, `set_compiled`, `save`,
  `is_dirty`, `reload`. Diagnostics now carry the path they come from; offsets
  are still only produced for the file on screen, since that is the only one the
  editor can underline.
- `src-tauri/src/watch.rs` runs `typst_kit::watcher::Watcher` on a thread per
  session and emits `files-changed`. It watches exactly what the last
  compilation read, so an included file is watched and an unrelated one is not.
- The UI gained a file list, an open-folder dialog (`tauri-plugin-dialog`),
  `Cmd+S`, a modified indicator, a "preview this file" button, and a count of
  diagnostics that point into other files.

Verified: 29 tests (19 core, 10 commands). The acceptance criterion is covered
twice — `edits_to_an_included_file_reach_the_preview` in the core and
`editing_an_include_updates_the_compiled_document` through the commands: editing
an included chapter adds a page to the main document's preview.
`save_and_reload_respect_unsaved_edits` covers the disk round trip in both
directions. clippy and `tsc` clean; the app starts.

Known rough edges: the watcher thread notices a closed session only after the
next file system event, so it can linger briefly; it holds only an app handle.
A file changed on disk while the editor has unsaved edits produces a warning in
the status line — the user resolves it by saving, and there is no merge view.

**Still to check by hand**: opening a folder lists its files, clicking one edits
it while the preview stays on the main document, `Cmd+S` clears the modified
mark, and an external edit refreshes the preview.

## Step 10. Export — done

- `Session::export_pdf` (`Smart::Auto` identifier, tagging left on),
  `export_png(index, scale)`, and `export_svg` for the whole document. PDF is
  the one fallible target, so it returns a message the UI can show.
- Command `export(path, page)` picks the format from the file extension and
  reports an unsupported one instead of guessing. PNG holds a single page, so it
  exports the page at the top of the preview.
- The UI has an Export button with a save dialog offering PDF, PNG, and SVG;
  the suggested name comes from the compiled file.

Verified: 36 tests (24 core, 12 commands). `exports_pdf_for_both_languages`
covers English and Russian documents, and `exported_pdf_has_the_pages_of_the_preview`
checks that the file carries the pages the preview shows.

The acceptance criterion was also checked outside the test suite, with poppler
on an exported Russian document:

```
$ pdfinfo out.pdf     -> Pages: 2, Tagged: yes, PDF version 1.7, A4
$ pdftotext out.pdf - -> Привет / Мир
```

So the file opens in a third-party reader, carries the page count of the
preview, stays tagged for accessibility, and its Cyrillic text extracts
correctly — the embedded fonts make it into the file.

**Still to check by hand**: the save dialog offers the three formats and the
exported file opens in a viewer of choice.

## Step 11. Typst Universe packages — done

The blocking part is not the download itself but where it happens: a package is
fetched inside a compilation, and a synchronous Tauri command runs on the main
thread. So the fix was to move the heavy commands off it.

- `compile`, `export`, `complete`, and `tooltip` are now async and run their work
  through `spawn_blocking`. Their logic moved into functions taking a session
  handle, which keeps them testable without a GUI.
- `crates/core/src/packages.rs` fetches and parses the Universe index, keeping
  the newest version of each package. Unparsable entries are skipped rather than
  failing the whole index, so a registry change cannot break completion.
- The index is cached in the user's cache directory for a day. It is two
  megabytes; re-downloading it on every start would be rude on a metered
  connection. Measured: 994 ms cold, 13 ms from cache, 1532 packages.
- `src-tauri/src/index.rs` fetches it once per process on a background thread and
  hands it to every window, then emits `packages-ready`.
- The UI shows "compiling…" once a compilation passes 300 ms, which is what a
  package download looks like from the outside.
- The editor now auto-closes brackets and quotes. This is not cosmetic:
  completion inside `#import "@preview/` only fires when the string is closed, so
  without it the feature is invisible to anyone typing normally.

Verified: 39 tests (27 core, 12 commands), including
`completes_package_imports_from_the_index` with a fixture instead of the network.
Checked against the real registry outside the test suite:

```
#import "@preview/cetz:0.3.1"              -> compiles in 1011 ms, 1 page
#import "@preview/definitely-not-real:9.9.9" -> package not found
                                              (searched for @preview/definitely-not-real:9.9.9)
```

So a real package downloads and compiles, and a missing one produces a message
that names what was searched for.

**Still to check by hand**: typing `#import "@preview/` offers package names, and
the first compilation that pulls a package shows "compiling…" rather than
freezing the window.

## Step 12. Packaging and distribution — mostly done

- **Icon.** Drawn in Typst (`src-tauri/icons/icon.typ`) and rendered through our
  own core with `cargo run --release --example icon`, then expanded into every
  platform format with `npx tauri icon`. The mark is built from rectangles
  rather than a glyph, so it does not depend on which font is available.
- **Bundle.** `tauri.conf.json` now carries the metadata a package needs:
  targets, icons, category, descriptions, copyright, publisher, macOS minimum
  version, and the NSIS install mode. `LICENSE` (Apache-2.0) was added, matching
  what the manifests already declared.
- **Font policy, settled.** Embedded fonts are registered first and system fonts
  extend them, so a document naming an embedded font renders identically
  everywhere even where a font of that name is installed. `SystemFonts::Exclude`
  drops system fonts for reproducible output. Covered by
  `embedded_fonts_alone_render_cyrillic`.
- **CI.** `.github/workflows/ci.yml` runs clippy, tests, and the frontend
  typecheck. `.github/workflows/release.yml` builds installers for Apple
  Silicon, Intel macOS, and Windows on a `v*` tag and opens a draft release.
  Signing secrets are passed through; without them the build produces unsigned
  artifacts rather than failing.
- **Release process** documented in `planning/RELEASING.md`: signing,
  notarization, version bumping, and a pre-release checklist.

Verified locally: `npm run build` produces `Typst Studio.app` (66 MB) and
`Typst Studio_0.1.0_aarch64.dmg` (25 MB). The bundled app launches both from
Finder and directly, and its frontend really runs from the embedded assets
rather than a dev server — proven by clearing the package-index cache, starting
the bundle, and watching the 2 MB cache reappear, which only happens after the
frontend creates a session.

Checked on the built DMG:

| Check | Result |
|---|---|
| `hdiutil verify` | checksum valid |
| Mounts without a dialog | after removing `licenseFile` (see below) |
| Image contents | app plus an `/Applications` symlink |
| `Info.plist` | 0.1.0, `app.typst.studio`, minimum macOS 11.0, icon set |
| Installed copy on a fresh profile | starts, frontend runs, index cached |
| Both languages compile and export | 0 errors, tagged PDFs, text extracts |
| Included file reaches the export | yes |
| Fonts in the PDF | Libertinus Serif and NewCMMath, all embedded |

Two defects the manual pass found, both fixed:

- The disk image demanded agreement to the license before mounting, because the
  bundle set `licenseFile`. That is a dialog in front of every install and it
  blocks scripted mounts, so the setting is gone; the license still ships in the
  repository and inside the app.
- The root `package.json` had no `version`, so the three versions the checklist
  compares could not agree and `npm version` would not work.

Not done, and deliberately so:

- **Signing and notarization** need an Apple Developer account, and Windows
  signing needs a certificate from a CA. The workflow and the documentation are
  ready for both; the secrets are yours to add.
- **Automatic updates** need a key pair and a place to publish the manifest.
  Both are deployment decisions, and enabling the updater halfway would produce
  a build that cannot verify its own updates. `RELEASING.md` lists the five
  steps to turn it on.
- The **acceptance criterion** — installs on a clean machine and compiles a
  document — can only be finished by installing the DMG on a machine that has
  never seen the project.

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
