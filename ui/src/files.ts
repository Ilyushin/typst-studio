/** The project file list. */

import { t } from "./i18n";

/** Called when the user picks a file to edit. */
export type SelectHandler = (path: string) => void;

export class FileList {
  private paths: string[] = [];
  private active: string | undefined;
  private compiled: string | undefined;

  constructor(
    private readonly container: HTMLElement,
    private readonly onSelect: SelectHandler,
  ) {
    this.render();
  }

  /** Replaces the listing, keeping the selection if it still exists. */
  update(paths: string[]): void {
    this.paths = paths;
    if (this.active !== undefined && !paths.includes(this.active)) {
      this.active = undefined;
    }
    this.render();
  }

  /** Marks which file is being edited and which one is being compiled. */
  mark(active: string | undefined, compiled: string | undefined): void {
    this.active = active;
    this.compiled = compiled;
    this.render();
  }

  /** Redraws labels after a language switch. */
  relabel(): void {
    this.render();
  }

  private render(): void {
    if (this.paths.length === 0) {
      const empty = document.createElement("p");
      empty.className = "empty";
      empty.textContent = t().noProject;
      this.container.replaceChildren(empty);
      return;
    }

    this.container.replaceChildren(
      ...this.paths.map((path) => {
        const item = document.createElement("button");
        item.type = "button";
        item.className = "file";
        item.textContent = path;
        item.classList.toggle("active", path === this.active);

        if (path === this.compiled) {
          const badge = document.createElement("span");
          badge.className = "badge";
          badge.textContent = t().compiledFile;
          item.append(badge);
        }

        item.addEventListener("click", () => this.onSelect(path));
        return item;
      }),
    );
  }
}
