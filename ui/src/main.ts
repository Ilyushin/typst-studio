/** Wires the editor, the compiler backend, and the preview together. */

import { EditorState } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import {
  lintGutter,
  setDiagnostics,
  type Diagnostic as LintDiagnostic,
} from "@codemirror/lint";

import { Backend, openWindow, type Diagnostic } from "./backend";
import { ideExtensions } from "./ide";
import { Preview } from "./preview";
import { LANGUAGES, language, setLanguage, t, type Language } from "./i18n";
import "./styles.css";

/** Compilation takes ~8 ms on a 49-page document, so the wait can be short. */
const DEBOUNCE_MS = 40;

/** What the status line currently shows, so it can be redrawn on a language switch. */
type Status =
  | { kind: "starting" }
  | { kind: "compiled"; pages: number; ms: number }
  | { kind: "stale"; errors: number }
  | { kind: "failed"; error: string };

async function main(): Promise<void> {
  const status = new StatusLine(document.querySelector<HTMLElement>("#status")!);
  const backend = await Backend.create(await projectRoot());

  const initial = t().sampleDocument;
  await backend.openDocument(initial);

  const preview = new Preview(document.querySelector<HTMLElement>("#preview")!, backend);

  // Edits are shipped to Rust one by one, in order. The queue keeps them from
  // interleaving with each other or with a compilation.
  let queue: Promise<unknown> = Promise.resolve();
  let timer: number | undefined;

  const view = new EditorView({
    parent: document.querySelector<HTMLElement>("#editor")!,
    state: EditorState.create({
      doc: initial,
      extensions: [
        lineNumbers(),
        highlightActiveLine(),
        history(),
        lintGutter(),
        keymap.of([...defaultKeymap, ...historyKeymap]),
        EditorView.lineWrapping,
        ...ideExtensions(backend, () => queue.then(() => undefined)),
        EditorView.updateListener.of((update) => {
          if (!update.docChanged) return;

          // Apply from the end backwards, so earlier offsets stay valid.
          const edits: Array<[number, number, string]> = [];
          update.changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
            edits.push([fromA, toA, inserted.toString()]);
          });
          for (const [from, to, text] of edits.reverse()) {
            queue = queue.then(() => backend.applyEdit(from, to, text));
          }

          window.clearTimeout(timer);
          timer = window.setTimeout(recompile, DEBOUNCE_MS);
        }),
      ],
    }),
  });

  async function recompile(): Promise<void> {
    queue = queue.then(async () => {
      const started = performance.now();
      const result = await backend.compile();
      const ms = Math.round(performance.now() - started);

      view.dispatch(setDiagnostics(view.state, toLint(result.diagnostics, view)));
      preview.update(result.pages);

      status.set(
        result.updated
          ? { kind: "compiled", pages: result.pages, ms }
          : { kind: "stale", errors: result.diagnostics.filter((d) => d.error).length },
      );
    });
    await queue;
  }

  const labels = new Labels(status);
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
      case "compiled":
        this.element.textContent = strings.compiled(this.status.pages, this.status.ms);
        break;
      case "stale":
        this.element.textContent = strings.stale(this.status.errors);
        break;
      case "failed":
        this.element.textContent = strings.startupFailed(this.status.error);
        break;
    }
  }
}

/** Applies translations to the static chrome and drives the language picker. */
class Labels {
  private readonly button = document.querySelector<HTMLButtonElement>("#new-window")!;
  private readonly picker = document.querySelector<HTMLSelectElement>("#language")!;

  constructor(private readonly status: StatusLine) {
    this.picker.replaceChildren(
      ...LANGUAGES.map((code) => {
        const option = document.createElement("option");
        option.value = code;
        // Each language names itself, as language pickers conventionally do.
        option.textContent = new Intl.DisplayNames([code], { type: "language" })
          .of(code)!;
        return option;
      }),
    );
    this.picker.addEventListener("change", () => {
      setLanguage(this.picker.value as Language);
      this.apply();
    });
  }

  apply(): void {
    const strings = t();
    document.documentElement.lang = language();
    document.title = strings.appName;
    this.button.textContent = strings.newWindow;
    this.picker.value = language();
    this.picker.title = strings.languageLabel;
    this.picker.setAttribute("aria-label", strings.languageLabel);
    this.status.redraw();
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

/** The directory that imports and packages resolve against. */
async function projectRoot(): Promise<string> {
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
