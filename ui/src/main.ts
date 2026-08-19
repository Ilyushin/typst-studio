/** Wires the editor, the compiler backend, the file list, and the preview. */

import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import {
  lintGutter,
  setDiagnostics,
  type Diagnostic as LintDiagnostic,
} from "@codemirror/lint";
import { listen } from "@tauri-apps/api/event";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";

import { Backend, openWindow, type Diagnostic } from "./backend";
import { FileList } from "./files";
import { ideExtensions } from "./ide";
import { Preview } from "./preview";
import { LANGUAGES, language, setLanguage, t, type Language } from "./i18n";
import "./styles.css";

/** Compilation takes ~8 ms on a 49-page document, so the wait can be short. */
const DEBOUNCE_MS = 40;

/** Cursor moves are frequent; locating them in the preview can lag behind. */
const CURSOR_DEBOUNCE_MS = 150;

/** What the status line currently shows, so it can be redrawn on a language switch. */
type Status =
  | { kind: "starting" }
  | { kind: "compiling" }
  | { kind: "compiled"; pages: number; ms: number; elsewhere: number }
  | { kind: "stale"; errors: number }
  | { kind: "warning"; text: string }
  | { kind: "failed"; error: string };

async function main(): Promise<void> {
  const status = new StatusLine(document.querySelector<HTMLElement>("#status")!);
  const backend = await Backend.create(await homeDirectory());

  /** Path of the file being edited, absent for the unsaved starter document. */
  let activePath: string | undefined;
  /** Path of the file being compiled. */
  let compiledPath: string | undefined;
  let dirty = false;

  const files = new FileList(
    document.querySelector<HTMLElement>("#files")!,
    (path) => void openFile(path),
  );

  const preview = new Preview(
    document.querySelector<HTMLElement>("#preview")!,
    backend,
    (page, x, y) => void jumpToSource(page, x, y),
  );

  const initial = t().sampleDocument;
  await backend.openDocument(initial);

  // Edits are shipped to Rust one by one, in order. The queue keeps them from
  // interleaving with each other or with a compilation.
  let queue: Promise<unknown> = Promise.resolve();
  let timer: number | undefined;
  let cursorTimer: number | undefined;
  /** Set while the editor content is replaced wholesale, which is not an edit. */
  let switching = false;

  const view = new EditorView({
    parent: document.querySelector<HTMLElement>("#editor")!,
    state: EditorState.create({
      doc: initial,
      extensions: [
        lineNumbers(),
        highlightActiveLine(),
        history(),
        lintGutter(),
        keymap.of([
          {
            key: "Mod-s",
            run: () => {
              void saveActive();
              return true;
            },
          },
          ...defaultKeymap,
          ...historyKeymap,
        ]),
        EditorView.lineWrapping,
        ...ideExtensions(backend, () => queue.then(() => undefined)),
        EditorView.updateListener.of((update) => {
          if (update.selectionSet || update.docChanged) {
            window.clearTimeout(cursorTimer);
            cursorTimer = window.setTimeout(locateCursor, CURSOR_DEBOUNCE_MS);
          }
          if (!update.docChanged || switching) return;

          // Apply from the end backwards, so earlier offsets stay valid.
          const edits: Array<[number, number, string]> = [];
          update.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
            edits.push([fromA, toA, inserted.toString()]);
          });
          for (const [from, to, text] of edits.reverse()) {
            queue = queue.then(() => backend.applyEdit(from, to, text));
          }

          dirty = true;
          labels.apply();

          window.clearTimeout(timer);
          timer = window.setTimeout(recompile, DEBOUNCE_MS);
        }),
      ],
    }),
  });

  async function recompile(): Promise<void> {
    queue = queue.then(async () => {
      // Downloading a package makes a compilation take seconds; saying so beats
      // a preview that silently sits still.
      const slow = window.setTimeout(() => status.set({ kind: "compiling" }), 300);
      const started = performance.now();
      const result = await backend.compile().finally(() => window.clearTimeout(slow));
      const ms = Math.round(performance.now() - started);

      // Diagnostics without offsets belong to other files; they are counted,
      // not underlined, since there is nothing on screen to underline.
      const here = result.diagnostics.filter((d) => d.from !== null);
      const elsewhere = result.diagnostics.length - here.length;
      view.dispatch(setDiagnostics(view.state, toLint(here, view)));
      preview.update(result.pages);

      status.set(
        result.updated
          ? { kind: "compiled", pages: result.pages, ms, elsewhere }
          : { kind: "stale", errors: result.diagnostics.filter((d) => d.error).length },
      );
    });
    await queue;
  }

  /** Replaces the editor content without reporting it as a user edit. */
  function setDocument(text: string): void {
    switching = true;
    view.dispatch({
      changes: { from: 0, to: view.state.doc.length, insert: text },
      selection: { anchor: 0 },
    });
    switching = false;
  }

  async function chooseProject(): Promise<void> {
    const chosen = await openDialog({ directory: true });
    if (typeof chosen !== "string") return;

    const paths = await backend.openProject(chosen);
    files.update(paths);
    activePath = undefined;
    compiledPath = undefined;

    // A project usually has an obvious entry point; otherwise take the first.
    const entry = paths.find((path) => path === "main.typ") ?? paths[0];
    if (entry === undefined) {
      labels.apply();
      return;
    }

    await openFile(entry);
    await previewActive();
  }

  async function openFile(path: string): Promise<void> {
    await queue;
    const file = await backend.openFile(path);

    setDocument(file.text);
    activePath = file.path;
    dirty = file.dirty;
    if (file.compiled) compiledPath = file.path;

    files.mark(activePath, compiledPath);
    labels.apply();
    await recompile();
  }

  /** Makes the file in the editor the one shown in the preview. */
  async function previewActive(): Promise<void> {
    await queue;
    await backend.setCompiled();
    compiledPath = activePath;
    files.mark(activePath, compiledPath);
    await recompile();
  }

  /** Writes the compiled document to a file the user picks. */
  async function exportDocument(): Promise<void> {
    const suggested = (compiledPath ?? "document.typ").replace(/\.typ$/, ".pdf");
    const target = await saveDialog({
      defaultPath: suggested,
      filters: [
        { name: "PDF", extensions: ["pdf"] },
        { name: "PNG", extensions: ["png"] },
        { name: "SVG", extensions: ["svg"] },
      ],
    });
    if (typeof target !== "string") return;

    await queue;
    try {
      // PNG holds a single page, so export the one the user is looking at.
      await backend.export(target, preview.topPage());
      status.set({ kind: "warning", text: t().exported(target) });
    } catch (error: unknown) {
      status.set({ kind: "warning", text: t().exportFailed(String(error)) });
    }
  }

  async function saveActive(): Promise<void> {
    await queue;
    await backend.save();
    dirty = false;
    labels.apply();
  }

  /** Shows where the cursor's text sits in the preview. */
  async function locateCursor(): Promise<void> {
    await queue;
    const spots = await backend.jumpFromCursor(view.state.selection.main.head);
    preview.highlight(spots[0]);
  }

  /** Moves the cursor to the text behind a click in the preview. */
  async function jumpToSource(page: number, x: number, y: number): Promise<void> {
    await queue;
    const destination = await backend.jumpFromClick(page, x, y);
    if (destination?.kind !== "cursor") return;

    const offset = Math.min(destination.offset, view.state.doc.length);
    view.dispatch({ selection: { anchor: offset }, scrollIntoView: true });
    view.focus();
  }

  // The package index arrived, so import completion just got better.
  await listen<number>("packages-ready", (event) => {
    status.set({ kind: "warning", text: t().packagesReady(event.payload) });
  });

  // Something changed on disk: pull it in when the editor has nothing to lose,
  // and say so when it does.
  await listen("files-changed", () => {
    void (async () => {
      await queue;
      const reloaded = await backend.reload();
      if (reloaded) {
        setDocument(reloaded.text);
        dirty = false;
      } else if (activePath !== undefined && dirty) {
        status.set({ kind: "warning", text: t().changedOnDisk(activePath) });
      }

      files.update(await backend.projectFiles());
      files.mark(activePath, compiledPath);
      labels.apply();
      await recompile();
    })();
  });

  const labels = new Labels(status, files, {
    path: () => activePath,
    dirty: () => dirty,
    onOpenProject: () => void chooseProject(),
    onSave: () => void saveActive(),
    onPreviewActive: () => void previewActive(),
    onExport: () => void exportDocument(),
  });
  labels.apply();

  window.addEventListener("beforeunload", () => void backend.close());
  document
    .querySelector<HTMLButtonElement>("#new-window")!
    .addEventListener("click", () => void openWindow());

  await recompile();
}

/** Keeps the status text in the current language. */
class StatusLine {
  private status: Status = { kind: "starting" };

  constructor(private readonly element: HTMLElement) {
    this.redraw();
  }

  set(status: Status): void {
    this.status = status;
    this.redraw();
  }

  redraw(): void {
    const strings = t();
    switch (this.status.kind) {
      case "starting":
        this.element.textContent = strings.starting;
        break;
      case "compiling":
        this.element.textContent = strings.compiling;
        break;
      case "compiled": {
        const base = strings.compiled(this.status.pages, this.status.ms);
        this.element.textContent =
          this.status.elsewhere > 0
            ? `${base} · ${strings.elsewhere(this.status.elsewhere)}`
            : base;
        break;
      }
      case "stale":
        this.element.textContent = strings.stale(this.status.errors);
        break;
      case "warning":
        this.element.textContent = this.status.text;
        break;
      case "failed":
        this.element.textContent = strings.startupFailed(this.status.error);
        break;
    }
  }
}

/** What the chrome needs to know and do, without reaching into `main`. */
interface Chrome {
  path(): string | undefined;
  dirty(): boolean;
  onOpenProject(): void;
  onSave(): void;
  onPreviewActive(): void;
  onExport(): void;
}

/** Applies translations to the static chrome and drives its buttons. */
class Labels {
  private readonly newWindow = document.querySelector<HTMLButtonElement>("#new-window")!;
  private readonly openProject =
    document.querySelector<HTMLButtonElement>("#open-project")!;
  private readonly save = document.querySelector<HTMLButtonElement>("#save")!;
  private readonly previewThis =
    document.querySelector<HTMLButtonElement>("#preview-this")!;
  private readonly exportButton = document.querySelector<HTMLButtonElement>("#export")!;
  private readonly fileStatus = document.querySelector<HTMLElement>("#file-status")!;
  private readonly picker = document.querySelector<HTMLSelectElement>("#language")!;

  constructor(
    private readonly status: StatusLine,
    private readonly files: FileList,
    private readonly chrome: Chrome,
  ) {
    this.picker.replaceChildren(
      ...LANGUAGES.map((code) => {
        const option = document.createElement("option");
        option.value = code;
        // Each language names itself, as language pickers conventionally do.
        option.textContent = new Intl.DisplayNames([code], { type: "language" }).of(
          code,
        )!;
        return option;
      }),
    );
    this.picker.addEventListener("change", () => {
      setLanguage(this.picker.value as Language);
      this.apply();
    });

    this.openProject.addEventListener("click", () => chrome.onOpenProject());
    this.save.addEventListener("click", () => chrome.onSave());
    this.previewThis.addEventListener("click", () => chrome.onPreviewActive());
    this.exportButton.addEventListener("click", () => chrome.onExport());
  }

  apply(): void {
    const strings = t();
    document.documentElement.lang = language();
    document.title = strings.appName;

    this.newWindow.textContent = strings.newWindow;
    this.openProject.textContent = strings.openProject;
    this.save.textContent = strings.save;
    this.previewThis.textContent = strings.compileThis;
    this.exportButton.textContent = strings.export;

    const path = this.chrome.path();
    this.fileStatus.textContent =
      path === undefined
        ? ""
        : this.chrome.dirty()
          ? `${path} · ${strings.modified}`
          : path;

    this.picker.value = language();
    this.picker.title = strings.languageLabel;
    this.picker.setAttribute("aria-label", strings.languageLabel);
    this.status.redraw();
    this.files.relabel();
  }
}

/** Maps backend diagnostics onto CodeMirror's lint entries. */
function toLint(diagnostics: Diagnostic[], view: EditorView): LintDiagnostic[] {
  const end = view.state.doc.length;
  return diagnostics
    .filter((d) => d.from !== null && d.to !== null)
    .map((d) => ({
      // Offsets are already UTF-16, converted on the Rust side.
      from: Math.min(d.from!, end),
      to: Math.min(Math.max(d.to!, d.from!), end),
      severity: d.error ? ("error" as const) : ("warning" as const),
      message: d.message,
    }));
}

/** Fallback root until the user opens a project. */
async function homeDirectory(): Promise<string> {
  const { homeDir } = await import("@tauri-apps/api/path");
  return homeDir();
}

void main().catch((error: unknown) => {
  // Without this, a failure before the first render leaves a blank window and
  // no hint of what went wrong.
  const element = document.querySelector<HTMLElement>("#status");
  if (element) new StatusLine(element).set({ kind: "failed", error: String(error) });
  console.error(error);
});
