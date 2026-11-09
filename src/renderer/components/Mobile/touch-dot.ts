// The touch dot over the device (fix 4): where the person's finger is,
// fading on release. Drawn outside React: a pointer move sets the dot's
// style directly, so dragging never re-renders the Mobile pane.
//
// React renders the `<span>` once and hands it over with `attach`; it gives
// the span no `style` and no `data-mobile-touch-dot`, so a re-render never
// undoes what this sets. The look is the old React dot's: the same classes,
// a 200 ms fade, scale 1.5 on release, then gone.

/** The fade after release, ms (the span's `duration-200`). */
export const DOT_FADE_MS = 200

export interface DotTimers {
  set(fn: () => void, ms: number): number
  clear(id: number): void
}

const windowTimers: DotTimers = {
  set: (fn, ms) => window.setTimeout(fn, ms),
  clear: (id) => window.clearTimeout(id),
}

export class TouchDot {
  private el: HTMLElement | null = null
  private timer: number | null = null

  constructor(private readonly timers: DotTimers = windowTimers) {}

  /** The span React rendered (null when it unmounts). A stable callback
   *  ref, so a re-render doesn't reset a dot mid-drag. */
  attach(el: HTMLElement | null): void {
    if (el === this.el) return
    this.cancel()
    this.el = el
    if (el) this.hide()
  }

  /** The finger at (x, y) in the screen's own pixels; `down` false lifts
   *  it: scale 1.5, fade, then gone. */
  place(x: number, y: number, down: boolean): void {
    const el = this.el
    if (!el) return
    this.cancel()
    el.style.display = ''
    el.style.transform = `translate3d(${x}px, ${y}px, 0) translate(-50%, -50%)`
    el.style.scale = down ? '1' : '1.5'
    el.style.opacity = down ? '1' : '0'
    el.setAttribute('data-mobile-touch-dot', down ? 'down' : 'up')
    if (!down) {
      this.timer = this.timers.set(() => {
        this.timer = null
        this.hide()
      }, DOT_FADE_MS)
    }
  }

  hide(): void {
    this.cancel()
    const el = this.el
    if (!el) return
    el.style.display = 'none'
    el.style.opacity = '0'
    el.removeAttribute('data-mobile-touch-dot')
  }

  dispose(): void {
    this.cancel()
    this.el = null
  }

  private cancel(): void {
    if (this.timer !== null) this.timers.clear(this.timer)
    this.timer = null
  }
}
