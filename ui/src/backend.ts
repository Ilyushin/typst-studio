/** Typed wrappers around the Rust commands. */

import { invoke } from "@tauri-apps/api/core";

export interface Diagnostic {
  message: string;
  error: boolean;
  /** Offsets in UTF-16 code units, absent when the diagnostic points elsewhere. */
  from: number | null;
  to: number | null;
}

export interface CompletionItem {
  label: string;
  /** Replacement text, possibly carrying `${placeholder}` snippet markers. */
  apply: string | null;
  detail: string | null;
  kind: string;
}

export interface Completions {
  /** Offset the completion replaces from, in UTF-16 code units. */
  from: number;
  items: CompletionItem[];
}

export interface TooltipInfo {
  text: string;
  /** Whether the text is Typst code rather than prose. */
  code: boolean;
}

export interface CompileResult {
  updated: boolean;
  pages: number;
  diagnostics: Diagnostic[];
}

export class Backend {
  private constructor(private readonly session: number) {}

  static async create(root: string): Promise<Backend> {
    const session = await invoke<number>("create_session", { root });
    return new Backend(session);
  }

  openDocument(text: string, path: string | null = null): Promise<void> {
    return invoke("open_document", { session: this.session, path, text });
  }

  /** Offsets are UTF-16 code units, as CodeMirror reports them. */
  applyEdit(from: number, to: number, text: string): Promise<void> {
    return invoke("apply_edit", { session: this.session, from, to, text });
  }

  compile(): Promise<CompileResult> {
    return invoke("compile", { session: this.session });
  }

  complete(cursor: number, explicit: boolean): Promise<Completions | null> {
    return invoke("complete", { session: this.session, cursor, explicit });
  }

  tooltip(cursor: number): Promise<TooltipInfo | null> {
    return invoke("tooltip", { session: this.session, cursor });
  }

  pageSvg(index: number): Promise<string | null> {
    return invoke("page_svg", { session: this.session, index });
  }

  close(): Promise<boolean> {
    return invoke("close_session", { session: this.session });
  }
}

export function openWindow(): Promise<string> {
  return invoke("open_window");
}
