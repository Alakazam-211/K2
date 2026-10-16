/**
 * Freeze-frame for the Browser pane's native page.
 *
 * The page is a native child view drawn above the DOM. When a K2 overlay
 * covers it (see `lib/browser-overlay.ts`), the page must leave the screen.
 * Instead of a blank hole, this takes a snapshot of the native page first,
 * shows it as an `<img>` in the same rect, and only then hides the native
 * view. Uncovering shows the native view again and drops the picture one
 * frame later (the native view paints above it, so there is no flash).
 *
 * Budget: the snapshot never blocks the UI. If it is not on screen within
 * `budgetMs`, the native view hides anyway (the old blank hide). A late
 * picture still fills the hole if the overlay is still open.
 *
 * Pure state machine; BrowserPane supplies the side effects (invoke, the
 * React frame state, timers).
 */

export type BrowserFace = 'live' | 'frozen' | 'hidden'

export interface FrozenFrame {
  /** Object URL (or data URL) of the picture. */
  url: string
  /** Logical (CSS px) size of the page when it was taken. */
  width: number
  height: number
  /** Revoke the URL. The owner of the shown frame calls this. */
  release(): void
}

export interface BrowserFreezeDeps {
  /** Picture of the native page, or null when there is none. */
  snapshot(): Promise<FrozenFrame | null>
  /** `browser_set_visible`. Fire and forget. */
  setNativeVisible(visible: boolean): void
  /** Push current bounds before the native view shows again. */
  beforeShow?(): void
  /** Show this picture (or none). Resolves once it is painted. The
   *  callee owns the frame from here and releases the previous one. */
  showFrame(frame: FrozenFrame | null): Promise<void> | void
  /** Run after the next painted frame (rAF in the app). */
  afterPaint(fn: () => void): void
  setTimer(fn: () => void, ms: number): unknown
  clearTimer(handle: unknown): void
  budgetMs: number
}

/** Snapshot plus paint must land in this long, or the page hides blank. */
export const FREEZE_BUDGET_MS = 150

export class BrowserFreeze {
  private face: BrowserFace
  /** What we last told the native view. */
  private nativeShown: boolean
  private frameShown = false
  /** Bumped on every face change; stale snapshots compare against it. */
  private token = 0
  private timer: unknown = null

  constructor(
    private readonly deps: BrowserFreezeDeps,
    initial: { face: BrowserFace; nativeShown: boolean },
  ) {
    this.face = initial.face
    this.nativeShown = initial.nativeShown
  }

  get current(): BrowserFace {
    return this.face
  }

  /** The native view was just created (add_child shows it). */
  adoptCreatedView(): void {
    this.nativeShown = true
    const want = this.face
    // Re-enter the wanted face from "live" so a pane created while
    // covered still freezes, and one created while hidden hides.
    this.face = 'live'
    if (want !== 'live') this.set(want)
  }

  set(next: BrowserFace): void {
    if (next === this.face) return
    const prev = this.face
    this.face = next
    this.token++
    this.cancelTimer()
    if (next === 'live') {
      this.goLive()
    } else if (next === 'hidden') {
      this.hideNative()
      this.dropFrame()
    } else if (prev === 'live' && this.nativeShown) {
      this.freeze(this.token)
    } else {
      // Hidden → covered (a tab shown while a dialog is open): there is no
      // live page to photograph. Keep it hidden; the overlay is on top.
      this.hideNative()
    }
  }

  /** Unmount: drop the picture, leave the native view to browser_close. */
  dispose(): void {
    this.token++
    this.cancelTimer()
    this.dropFrame()
  }

  private goLive(): void {
    this.deps.beforeShow?.()
    if (!this.nativeShown) {
      this.nativeShown = true
      this.deps.setNativeVisible(true)
    }
    if (this.frameShown) {
      const token = this.token
      // The native view paints above the picture; drop it a frame later so
      // the switch never shows a blank hole.
      this.deps.afterPaint(() => {
        if (token !== this.token) return
        this.dropFrame()
      })
    }
  }

  private freeze(token: number): void {
    this.timer = this.deps.setTimer(() => {
      this.timer = null
      if (token !== this.token) return
      this.hideNative()
    }, this.deps.budgetMs)

    let pending: Promise<FrozenFrame | null>
    try {
      pending = this.deps.snapshot()
    } catch {
      pending = Promise.resolve(null)
    }
    void pending
      .catch(() => null)
      .then(async (frame) => {
        if (token !== this.token || this.face !== 'frozen') {
          frame?.release()
          return
        }
        if (!frame) {
          this.cancelTimer()
          this.hideNative()
          return
        }
        this.frameShown = true
        try {
          await this.deps.showFrame(frame)
        } catch {
          // A picture that failed to paint is still better than nothing.
        }
        if (token !== this.token) return
        this.cancelTimer()
        this.hideNative()
      })
  }

  private hideNative(): void {
    if (!this.nativeShown) return
    this.nativeShown = false
    this.deps.setNativeVisible(false)
  }

  private dropFrame(): void {
    if (!this.frameShown) return
    this.frameShown = false
    void this.deps.showFrame(null)
  }

  private cancelTimer(): void {
    if (this.timer !== null) {
      this.deps.clearTimer(this.timer)
      this.timer = null
    }
  }
}
