/** The preview pane: renders only the pages that are on screen.
 *
 * Rendering the whole document to SVG costs more than the recompilation that
 * produced it (108 ms vs 8 ms on a 49-page document), so pages are fetched
 * lazily as they scroll into view and re-fetched after a recompilation only if
 * they are still visible.
 */

import type { Backend } from "./backend";

export class Preview {
  private readonly observer: IntersectionObserver;
  private readonly visible = new Set<HTMLElement>();
  private pages: HTMLElement[] = [];
  /** Bumped on every recompilation to discard stale renders. */
  private generation = 0;

  constructor(
    private readonly container: HTMLElement,
    private readonly backend: Backend,
  ) {
    this.observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          const page = entry.target as HTMLElement;
          if (entry.isIntersecting) {
            this.visible.add(page);
            void this.render(page);
          } else {
            this.visible.delete(page);
          }
        }
      },
      // Start rendering slightly before a page scrolls into view.
      { root: this.container, rootMargin: "200px" },
    );
  }

  /** Reflects a new compilation, keeping scroll position where possible. */
  update(count: number): void {
    this.generation++;

    if (count === this.pages.length) {
      // Same layout: only what the user is actually looking at needs redrawing.
      for (const page of this.visible) void this.render(page);
      return;
    }

    this.observer.disconnect();
    this.visible.clear();
    this.container.replaceChildren();
    this.pages = Array.from({ length: count }, (_, index) => {
      const page = document.createElement("div");
      page.className = "page";
      page.dataset.index = String(index);
      this.container.append(page);
      this.observer.observe(page);
      return page;
    });
  }

  private async render(page: HTMLElement): Promise<void> {
    const generation = this.generation;
    if (page.dataset.rendered === String(generation)) return;
    page.dataset.rendered = String(generation);

    const svg = await this.backend.pageSvg(Number(page.dataset.index));

    // A newer compilation finished while this render was in flight.
    if (generation !== this.generation) return;
    if (svg !== null) page.innerHTML = svg;
  }
}
