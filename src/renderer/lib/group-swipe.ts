// Home groups: a two-finger sideways trackpad swipe moves to the next or
// previous group, like Spaces. Pure: the hook (`hooks/useGroupSwipe.ts`)
// feeds it wheel events and acts on its answers.
//
// A trackpad sends a stream of wheel events (the fingers, then the momentum
// tail); the stream is one gesture until it goes quiet for SWIPE_QUIET_MS,
// which is when the fingers are taken as let go. While it runs, the
// sideways travel (only events more sideways than up/down count) is the
// swipe's progress, 0–1 over SWIPE_DISTANCE_PX, for a visual that follows
// the finger. On release it commits when the progress reached
// SWIPE_COMMIT_AT (35%, as K2 Design's mock), else it cancels. A swipe that
// reaches the full distance commits at once and the rest of that gesture
// is ignored, so one gesture never moves more than one group.
//
// Direction: fingers moving left (deltaX > 0 with natural scrolling) go to
// the next group, as Spaces does; right goes back.

export type SwipeDirection = 'next' | 'prev'

/** What the group bar's visual follows while the fingers move; null at rest. */
export type SwipeProgress = { progress: number; direction: SwipeDirection } | null

/** How a swipe ended: `commit` switched groups, `cancel` springs back
 *  (short of SWIPE_COMMIT_AT, or no group that way). */
export interface SwipeRelease {
  outcome: 'commit' | 'cancel'
  direction: SwipeDirection
}

/** Sideways travel (CSS px) for the whole swipe (progress 1). */
export const SWIPE_DISTANCE_PX = 240
/** On release, the progress that switches groups. */
export const SWIPE_COMMIT_AT = 0.35
/** A gap with no wheel events that ends a gesture (ms): the release. */
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
  /** Set when this answer ends the swipe. */
  release: SwipeRelease | null
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
    private readonly distance = SWIPE_DISTANCE_PX,
    private readonly quietMs = SWIPE_QUIET_MS,
    private readonly commitAt = SWIPE_COMMIT_AT,
  ) {}

  /** One wheel event. A gesture that went quiet before it is released
   *  first (call `settle` on a timer to release without a next event). */
  feed(s: WheelSample): SwipeAnswer {
    const earlier = this.settle(s.time)
    this.last = s.time
    if (earlier.release) {
      // The previous gesture ended in the gap; this event starts a new one.
      const next = this.feed(s)
      return { swipe: next.swipe, release: next.release ?? earlier.release }
    }
    if (this.locked) return { swipe: null, release: null }
    const dx = px(s.deltaX, s.deltaMode)
    const dy = px(s.deltaY, s.deltaMode)
    // Mostly up/down: a scroll, not a swipe (it still keeps the gesture alive).
    if (Math.abs(dx) > Math.abs(dy)) this.acc += dx
    if (Math.abs(this.acc) >= this.distance) {
      const direction: SwipeDirection = this.acc > 0 ? 'next' : 'prev'
      this.acc = 0
      this.locked = true
      return { swipe: null, release: { outcome: 'commit', direction } }
    }
    return { swipe: this.progress(), release: null }
  }

  /** At `time`: if the gesture has gone quiet, it is let go: commit past
   *  SWIPE_COMMIT_AT, else cancel (null when nothing was moving). */
  settle(time: number): SwipeAnswer {
    if (time - this.last <= this.quietMs) return { swipe: this.locked ? null : this.progress(), release: null }
    const p = this.progress()
    this.acc = 0
    this.locked = false
    if (!p) return { swipe: null, release: null }
    return { swipe: null, release: { outcome: p.progress >= this.commitAt ? 'commit' : 'cancel', direction: p.direction } }
  }

  private progress(): SwipeProgress {
    if (this.acc === 0) return null
    return { progress: Math.min(1, Math.abs(this.acc) / this.distance), direction: this.acc > 0 ? 'next' : 'prev' }
  }
}

/** The group one step `direction` from `currentId` in `ids` order, or null
 *  at either end (no wrap) or when `currentId` isn't there. */
export function steppedGroupId(ids: readonly string[], currentId: string, direction: SwipeDirection): string | null {
  const i = ids.indexOf(currentId)
  if (i < 0) return null
  const j = i + (direction === 'next' ? 1 : -1)
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
 *  (`data-group-swipe="off"`), up to `root`. */
export function swipeAllowedFrom(target: EventTarget | null, root: Element | null = null): boolean {
  let el = target instanceof Element ? target : null
  while (el && el !== root) {
    if (el.getAttribute('data-group-swipe') === 'off') return false
    if (scrollsSideways(el)) return false
    el = el.parentElement
  }
  return true
}
