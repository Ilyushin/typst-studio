/** Completion and hover, served by the compiler through the backend. */

import {
  autocompletion,
  snippetCompletion,
  type Completion as CmCompletion,
  type CompletionContext,
  type CompletionResult,
} from "@codemirror/autocomplete";
import { hoverTooltip, type Tooltip } from "@codemirror/view";
import type { Extension } from "@codemirror/state";

import type { Backend, CompletionItem } from "./backend";

/** Maps Typst's completion kinds onto CodeMirror's icon set. */
const KIND_TO_TYPE: Record<string, string> = {
  syntax: "keyword",
  func: "function",
  type: "type",
  param: "property",
  constant: "constant",
  path: "text",
  package: "namespace",
  label: "variable",
  font: "text",
  symbol: "text",
};

/**
 * Builds the completion and hover extensions.
 *
 * `settled` resolves once every pending edit has reached the compiler; without
 * awaiting it, a query could race ahead of the text it is asking about.
 */
export function ideExtensions(backend: Backend, settled: () => Promise<void>): Extension[] {
  return [
    autocompletion({ override: [(context) => complete(context, backend, settled)] }),
    hoverTooltip((_view, pos) => hover(pos, backend, settled)),
  ];
}

async function complete(
  context: CompletionContext,
  backend: Backend,
  settled: () => Promise<void>,
): Promise<CompletionResult | null> {
  await settled();
  const result = await backend.complete(context.pos, context.explicit);
  if (!result || result.items.length === 0) return null;

  return { from: result.from, options: result.items.map(toOption) };
}

function toOption(item: CompletionItem): CmCompletion {
  const type = KIND_TO_TYPE[item.kind] ?? "text";
  const detail = item.detail ?? undefined;

  // Typst marks placeholders as `${name}`, which is also CodeMirror's snippet
  // syntax, so such completions become real snippets with tab stops.
  if (item.apply !== null && item.apply.includes("${")) {
    return snippetCompletion(item.apply, { label: item.label, detail, type });
  }

  return { label: item.label, apply: item.apply ?? undefined, detail, type };
}

async function hover(
  pos: number,
  backend: Backend,
  settled: () => Promise<void>,
): Promise<Tooltip | null> {
  await settled();
  const info = await backend.tooltip(pos);
  if (!info) return null;

  return {
    pos,
    above: true,
    create: () => {
      const dom = document.createElement("div");
      dom.className = info.code ? "typst-tooltip code" : "typst-tooltip";
      // The compiler returns plain text; assigning textContent keeps it that way.
      dom.textContent = info.text;
      return { dom };
    },
  };
}
