// The frame loop: the controllers' statistics and eased colour ranges (roads and rail, ferries,
// the terrain tint), and the panels drawn again from them. It runs while there is something to do
// (statistics to compute, a range still easing, panels to draw) and sleeps otherwise.

/** A controller the loop runs each frame. */
export interface FramePart {
  /** A frame: its work, with `k` of the way to each range's target (1 on the loop's first frame:
   * snapped). `panels`: what the panels show has changed; `easing`: a range is still moving. */
  tick(now: number, moving: boolean, k: number): { panels: boolean; easing: boolean };
  /** Work left to do without easing (statistics to compute). */
  readonly due: boolean;
}

export class FrameLoop {
  private last = performance.now();
  private lastPanel = 0;
  private ticking = false;
  private panelsDue = false;
  private snap = true;

  /** `panels`: the panels drawn again, at most every 60 ms. */
  constructor(private parts: FramePart[], private moving: () => boolean, private panels: () => void) {}

  private tick = (now: number) => {
    const dt = now - this.last;
    this.last = now;
    let panels = this.panelsDue;
    let easing = false;
    const k = this.snap ? 1 : 1 - Math.exp(-dt / 140);
    this.snap = false;
    const moving = this.moving();
    for (const p of this.parts) {
      const r = p.tick(now, moving, k);
      panels ||= r.panels;
      easing ||= r.easing;
    }
    this.panelsDue = panels && now - this.lastPanel <= 60;
    if (panels && !this.panelsDue) {
      this.lastPanel = now;
      this.panels();
    }
    if (easing || this.panelsDue || this.parts.some((p) => p.due)) requestAnimationFrame(this.tick);
    else this.ticking = false;
  };

  /** The loop runs (again) from the next frame. */
  wake() {
    if (this.ticking) return;
    this.ticking = true;
    this.last = performance.now();
    requestAnimationFrame(this.tick);
  }
}
