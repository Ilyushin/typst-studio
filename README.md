# Typst Studio

Desktop client for [Typst](https://github.com/typst/typst), built on the
compiler crates directly rather than on the `typst` CLI.

## Layout

- `crates/core` — headless compilation core. Owns the `World` implementation,
  open-document state, compilation, and preview rendering. No UI dependencies,
  testable on its own.
- `src-tauri` — Tauri 2 shell. Thin command layer over the core; each window
  gets its own session from the shared `Workspace`.
- `ui` — CodeMirror 6 frontend (TypeScript, Vite).
- `planning` — development plan (`PLAN.md`) and release process
  (`RELEASING.md`).

## Design

Documents open in the editor live in memory as `Source` objects that shadow the
on-disk state. Keystrokes go through `Source::edit`, which reparses
incrementally and keeps span numbers stable, so `comemo` reuses most of the
previous compilation instead of recompiling from scratch.

The preview is rendered to SVG, letting the UI zoom without a compiler
round-trip. Only visible pages are rendered: drawing a whole 49-page document
costs 108 ms against 8 ms for the recompilation that produced it.

Offsets crossing the Rust/JS boundary are UTF-16 code units, converted in
`src-tauri`. Typst counts UTF-8 bytes, and mixing the two silently misplaces
every highlight in non-ASCII text.

Each window owns a session; sessions share the process-global `comemo` cache,
and eviction scales with their number so one window's work is not aged out by
another's.

Compilation runs off the main thread. A compilation can download a package, and
a synchronous Tauri command would block the window while it does.

**Fonts.** The embedded set is registered first and system fonts extend it, so a
document naming an embedded font renders identically everywhere, even where a
font of the same name is installed. `SystemFonts::Exclude` drops system fonts
entirely for reproducible output. The embedded fonts cover Cyrillic on their
own.

## Languages

Code, comments, and documentation are English only.

The interface ships in English and Russian (`ui/src/i18n.ts`), switchable from
the header and remembered across restarts; it defaults to the system locale.
User-facing strings go through `t()` rather than being written inline, and
plural forms come from `Intl.PluralRules` — Russian needs three of them.

Documents are expected in both languages. The embedded fonts cover Cyrillic, so
output does not depend on what is installed on the machine. The interface
language and the document language are separate: the Russian starter document
sets `#set text(lang: "ru")` for hyphenation and quotation marks, but any
document can be written in any interface language.

## Development

```sh
npm install              # once, in the repo root and in ui/
npm run dev              # runs the app (starts Vite, builds Rust, opens a window)
cargo test               # Rust tests
npm --prefix ui run check   # frontend typecheck
cargo run --release --example bench -p typst-studio-core   # compile latency
```

Run `npm run dev` from the repository root — the Tauri CLI locates the project
by the `src-tauri` subfolder and will not find it from elsewhere. A debug build
loads the frontend from the Vite dev server, so `cargo run` on its own shows an
empty window.

## Packaging

```sh
npm run build     # installers in target/release/bundle/
```

Local builds are unsigned. Signing, notarization, and the release workflow are
described in `planning/RELEASING.md`. The app icon is drawn in Typst
(`src-tauri/icons/icon.typ`) and regenerated with
`cargo run --release --example icon -p typst-studio-core -- src-tauri/icons /tmp/icon.png`
followed by `npx tauri icon /tmp/icon.png -o src-tauri/icons`.

The upstream dependency is pinned to a specific git revision in the workspace
`Cargo.toml`. Typst's internal APIs change between versions; bump the pin
deliberately. Requires Rust 1.92+.
