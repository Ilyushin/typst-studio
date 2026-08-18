/** The preview pane: renders only the pages that are on screen.
 *
 * Rendering the whole document to SVG costs more than the recompilation that
 * produced it (108 ms vs 8 ms on a 49-page document), so pages are fetched
 * lazily as they scroll into view and re-fetched after a recompilation only if
 * they are still visible.
 */

import type { Backend, Spot } from "./backend";

/** Called when the user clicks a spot in the preview. */
export type ClickHandler = (page: number, x: number, y: number) => void;

export class Preview {
  private readonly observer: IntersectionObserver;
  private readonly visible = new Set<HTMLElement>();
  private readonly marker = document.createElement("div");
  private pages: HTMLElement[] = [];
  /** The page the cursor marker belongs to, so a re-render can restore it. */
  private marked: HTMLElement | undefined;
  /** Bumped on every recompilation to discard stale renders. */
  private generation = 0;

  constructor(
    private readonly container: HTMLElement,
    private readonly backend: Backend,
    private readonly onClick: ClickHandler,
  ) {
    this.marker.className = "cursor-marker";
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
    this.marked = undefined;
    this.container.replaceChildren();
    this.pages = Array.from({ length: count }, (_, index) => {
      const page = document.createElement("div");
      page.className = "page";
      page.dataset.index = String(index);
      page.addEventListener("click", (event) => this.click(page, event));
      this.container.append(page);
      this.observer.observe(page);
      return page;
    });
  }

  /** The page currently nearest the top of the viewport. */
  topPage(): number {
    let best = 0;
    let bestDistance = Number.POSITIVE_INFINITY;
    const top = this.container.getBoundingClientRect().top;

    for (const page of this.visible) {
      const distance = Math.abs(page.getBoundingClientRect().top - top);
      if (distance < bestDistance) {
        bestDistance = distance;
        best = Number(page.dataset.index);
      }
    }
    return best;
  }

  /** Marks where the editor cursor appears, scrolling it into view if needed. */
  highlight(spot: Spot | undefined): void {
    if (!spot) {
      this.marker.remove();
      this.marked = undefined;
      return;
    }

    const page = this.pages[spot.page];
    if (!page) return;
    this.marked = page;

    this.marker.style.left = `${spot.x * 100}%`;
    this.marker.style.top = `${spot.y * 100}%`;
    page.append(this.marker);

    // `nearest` leaves the view alone when the spot is already visible, so the
    // preview does not jump around while the user moves the cursor.
    this.marker.scrollIntoView({ block: "nearest", inline: "nearest" });
  }

  private click(page: HTMLElement, event: MouseEvent): void {
    const rect = page.getBoundingClientRect();
    if (rect.width === 0 || rect.height === 0) return;

    this.onClick(
      Number(page.dataset.index),
      (event.clientX - rect.left) / rect.width,
      (event.clientY - rect.top) / rect.height,
    );
  }

  private async render(page: HTMLElement): Promise<void> {
    const generation = this.generation;
    if (page.dataset.rendered === String(generation)) return;
    page.dataset.rendered = String(generation);

    const svg = await this.backend.pageSvg(Number(page.dataset.index));

    // A newer compilation finished while this render was in flight.
    if (generation !== this.generation) return;
    if (svg === null) return;

    page.innerHTML = svg;
    // Replacing the content dropped the marker; put it back where it belongs.
    if (this.marked === page) page.append(this.marker);
  }
}
