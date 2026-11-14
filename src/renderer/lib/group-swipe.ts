// Home groups: a two-finger sideways trackpad swipe moves to the next or
// previous group, like Spaces. Pure: the hook (`hooks/useGroupSwipe.ts`)
// feeds it wheel events and acts on its answers.
//
// One gesture moves one group. A trackpad sends a stream of wheel events
// (the fingers, then the momentum tail); the stream counts as one gesture
// until it goes quiet for SWIPE_QUIET_MS. Horizontal travel adds up while
// the event is more sideways than up/down; at SWIPE_THRESHOLD_PX the
// gesture steps once and locks until the stream goes quiet, so the
// momentum tail never steps again.
//
// Direction: fingers moving left (deltaX > 0 with natural scrolling) go to
// the next group, as Spaces does; right goes back.

/** What the group bar's visual follows while the fingers move. */
export interface SwipeProgress {
  /** 0 at rest, 1 at the step. */
  progress: number
  /** +1 toward the next group, -1 toward the previous, 0 at rest. */
  direction: -1 | 0 | 1
}

export const SWIPE_REST: SwipeProgress = { progress: 0, direction: 0 }

/** Sideways travel (CSS px) that moves one group. */
export const SWIPE_THRESHOLD_PX = 120
/** A gap with no wheel events that ends a gesture (ms). */
export const SWIPE_QUIET_MS = 200

export interface WheelSample {
  deltaX: number
  deltaY: number
  /** WheelEvent.deltaMode: 0 pixels, 1 lines, 2 pages. */
  deltaMode?: number
  /** Event time (ms, any monotonic clock). */
  time: number
}

export interface SwipeAnswer {
  swipe: SwipeProgress
  /** +1 / -1 when this sample completes a step, else 0. */
  step: -1 | 0 | 1
}

const LINE_PX = 16
const PAGE_PX = 400

function px(delta: number, mode: number | undefined): number {
  if (mode === 1) return delta * LINE_PX
  if (mode === 2) return delta * PAGE_PX
  return delta
}

export class GroupSwipeTracker {
  private acc = 0
  private last = Number.NEGATIVE_INFINITY
  private locked = false

  constructor(
    private readonly threshold = SWIPE_THRESHOLD_PX,
    private readonly quietMs = SWIPE_QUIET_MS,
  ) {}

  feed(s: WheelSample): SwipeAnswer {
    if (s.time - this.last > this.quietMs) {
      this.acc = 0
      this.locked = false
    }
    this.last = s.time
    if (this.locked) return { swipe: SWIPE_REST, step: 0 }
    const dx = px(s.deltaX, s.deltaMode)
    const dy = px(s.deltaY, s.deltaMode)
    // Mostly up/down: a scroll, not a swipe (it still keeps the gesture alive).
    if (Math.abs(dx) > Math.abs(dy)) this.acc += dx
    if (Math.abs(this.acc) >= this.threshold) {
      const step = this.acc > 0 ? 1 : -1
      this.acc = 0
      this.locked = true
      return { swipe: SWIPE_REST, step }
    }
    return { swipe: this.progress(), step: 0 }
  }

  /** Where the gesture stands; at rest once it has gone quiet by `time`. */
  settle(time: number): SwipeProgress {
    if (time - this.last > this.quietMs) {
      this.acc = 0
      this.locked = false
      return SWIPE_REST
    }
    return this.progress()
  }

  private progress(): SwipeProgress {
    if (this.acc === 0) return SWIPE_REST
    return { progress: Math.min(1, Math.abs(this.acc) / this.threshold), direction: this.acc > 0 ? 1 : -1 }
  }
}

/** The group `step` away from `currentId` in `ids` order, or null at either
 *  end (no wrap, as Spaces) or when `currentId` isn't there. */
export function steppedGroupId(ids: readonly string[], currentId: string, step: -1 | 1): string | null {
  const i = ids.indexOf(currentId)
  if (i < 0) return null
  const j = i + step
  return j >= 0 && j < ids.length ? ids[j] : null
}

/** True when `el` scrolls sideways itself (so a sideways swipe there is its own). */
export function scrollsSideways(el: Element): boolean {
  if (!(el instanceof HTMLElement)) return false
  if (el.scrollWidth <= el.clientWidth + 1) return false
  const ox = getComputedStyle(el).overflowX
  return ox === 'auto' || ox === 'scroll' || ox === 'overlay'
}

/** Whether a wheel event that starts at `target` may switch groups: not
 *  inside an element that scrolls sideways, nor inside one that opts out
 *  (`data-group-swipe="off"`), up to `root` (exclusive of nothing above it). */
export function swipeAllowedFrom(target: EventTarget | null, root: Element | null = null): boolean {
  let el = target instanceof Element ? target : null
  while (el && el !== root) {
    if (el.getAttribute('data-group-swipe') === 'off') return false
    if (scrollsSideways(el)) return false
    el = el.parentElement
  }
  return true
}
