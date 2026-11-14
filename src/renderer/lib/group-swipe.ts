// Home groups: a two-finger sideways trackpad swipe pages between groups,
// like iOS paging. Pure: the hook (`hooks/useGroupSwipe.ts`) feeds it wheel
// events and timer ticks and acts on its answers.
//
// One gesture goes through three phases:
//   - track: the fingers are down. The first few pixels lock the axis:
//     sideways is a swipe, up/down is the list's own scroll and the rest
//     of that gesture is left alone. Sideways, the strip follows the
//     fingers 1:1: `offset` is the travel as a share of the swipe's width
//     (positive towards the next group). Past the first or last group it
//     moves a third as far (the rubber band).
//   - release: wheel events have no "fingers up", so the fingers count as
//     lifted when the stream goes quiet for `idleMs`, when it sends an
//     empty event, or when the deltas start to fall away smoothly (macOS's
//     momentum, which can follow a lift with no gap at all). It commits
//     past `commitAt` of the width or on a flick (`flickPxPerMs`, measured
//     over the last few events), else it cancels. Where there is no group
//     that way it always cancels (no wrap).
//   - absorb: the momentum tail after a release never drags the strip or
//     switches again. A fresh gesture is a quiet gap over `momentumGapMs`,
//     a turn the other way, an up/down scroll, or deltas that grow twice
//     in a row (momentum only ever falls away; new fingers speed up). So
//     the tail never holds off a real swipe, back or onwards, and nothing
//     here waits on the pointer leaving or entering the nav.
// A swipe that travels the whole width commits at once, so one gesture
// never moves more than one group.
//
// Direction: fingers moving left (deltaX > 0 with natural scrolling) go to
// the next group, as Spaces does; right goes back.

export type SwipeDirection = 'next' | 'prev'

/** What readers of the swipe see: how far it has gone (0–1 of the width)
 *  and which way; `settling` while the strip glides home after a release.
 *  Null at rest. */
export type SwipeProgress = {
  progress: number
  direction: SwipeDirection
  settling?: 'commit' | 'cancel'
} | null

/** How a swipe ended: `commit` moves to the next or previous group,
 *  `cancel` glides back. */
export interface SwipeRelease {
  outcome: 'commit' | 'cancel'
  direction: SwipeDirection
}

/** A release, with what the glide needs. */
export interface SwipeDecision extends SwipeRelease {
  /** Where the strip was (signed share of the width, rubber band applied). */
  from: number
  /** The fingers' speed at the release (signed share of the width per ms). */
  velocity: number
}

/** The feel. Tune here; the tests read these names. */
export const SWIPE_TUNING = {
  /** On release, travel past this share of the width commits. */
  commitAt: 0.33,
  /** On release, a flick this fast (px/ms, 500 px/s) commits, short of commitAt. */
  flickPxPerMs: 0.5,
  /** ...once it has travelled this share of the width (no twitch flicks). */
  flickMinShare: 0.04,
  /** The fingers count as lifted after this long with no wheel event (ms). */
  idleMs: 80,
  /** After a release, a quiet gap this long ends the momentum tail (ms). */
  momentumGapMs: 150,
  /** The axis locks once the gesture has moved this far (px). */
  axisLockPx: 6,
  /** The release speed is measured over this much of the last events (ms). */
  velocityWindowMs: 64,
  /** Momentum onset: this many steps in a row, each smaller than the last, */
  decaySteps: 4,
  /** ...each at least this share of the one before (a smooth fall, not a stop), */
  decayMinRatio: 0.7,
  /** ...from a delta at least this big (px). */
  decayMinPx: 8,
  /** In the tail, deltas that grow this much twice in a row are new fingers, */
  riseRatio: 1.25,
  /** ...once they reach this size (px). */
  riseMinPx: 3,
  /** Past the first or last group the strip moves this share of the travel. */
  rubberBand: 1 / 3,
} as const

/** On release, the share of the width that commits. */
export const SWIPE_COMMIT_AT = SWIPE_TUNING.commitAt
/** The swipe's width (px) when the nav can't be measured. */
export const SWIPE_DISTANCE_PX = 240
/** The swipe never needs less travel than this (the 48 px rail). */
export const SWIPE_MIN_DISTANCE_PX = 160

/** The glide after a release: a decelerating curve with no bounce, its
 *  length scaled by how far is left to go. */
export const GLIDE_MIN_MS = 180
export const GLIDE_MAX_MS = 280
export const GLIDE_EASE = 'cubic-bezier(0.25, 1, 0.5, 1)'

export interface WheelSample {
  deltaX: number
  deltaY: number
  /** WheelEvent.deltaMode: 0 pixels, 1 lines, 2 pages. */
  deltaMode?: number
  /** Event time (ms, any monotonic clock). */
  time: number
}

export interface SwipeAnswer {
  /** The strip's new place (signed share of the width), or null for no change. */
  offset: number | null
  /** Set when this answer releases the swipe. */
  release: SwipeDecision | null
}

/** What the tracker asks the page, once per gesture as it starts. */
export interface SwipeEnv {
  /** The swipe's width (px): the nav's width, at least SWIPE_MIN_DISTANCE_PX. */
  distance?(): number
  /** Whether there is a group that way. */
  canGo?(direction: SwipeDirection): boolean
  /** A new gesture is starting (anything still gliding should land now). */
  onStart?(): void
}

const LINE_PX = 16
const PAGE_PX = 400
const NONE: SwipeAnswer = { offset: null, release: null }

function px(delta: number, mode: number | undefined): number {
  if (mode === 1) return delta * LINE_PX
  if (mode === 2) return delta * PAGE_PX
  return delta
}

type Phase = 'idle' | 'track' | 'absorb'

export class GroupSwipeTracker {
  private phase: Phase = 'idle'
  private axis: 'x' | 'y' | null = null
  private sumX = 0
  private sumY = 0
  private acc = 0
  private last = Number.NEGATIVE_INFINITY
  private samples: { t: number; dx: number }[] = []
  private width = SWIPE_DISTANCE_PX
  private can = { next: true, prev: true }
  /** The sign of the fingers' last sideways move (the tail's direction). */
  private tailSign = 0
  /** In the tail: the last delta's size, and how many times in a row it grew. */
  private tailLast = 0
  private tailRises = 0

  constructor(
    private readonly env: SwipeEnv = {},
    private readonly tune: typeof SWIPE_TUNING = SWIPE_TUNING,
  ) {}

  /** One wheel event. */
  feed(s: WheelSample): SwipeAnswer {
    const dx = px(s.deltaX, s.deltaMode)
    const dy = px(s.deltaY, s.deltaMode)
    const gap = s.time - this.last
    // The fingers were lifted in the gap before this event.
    const earlier = this.phase === 'track' && gap > this.tune.idleMs ? this.lift() : null
    this.last = s.time
    if (this.phase === 'absorb') {
      if (gap <= this.tune.momentumGapMs && !this.fresh(dx, dy)) return { offset: null, release: earlier }
      this.phase = 'idle'
    }
    if (this.phase === 'idle') {
      if (dx === 0 && dy === 0) return { offset: null, release: earlier }
      this.start()
    }
    const now = this.track(s.time, dx, dy)
    return { offset: now.offset, release: now.release ?? earlier }
  }

  /** A timer tick at `time`: releases a gesture that has gone quiet. */
  settle(time: number): SwipeAnswer {
    if (this.phase === 'track' && time - this.last > this.tune.idleMs) return { offset: null, release: this.lift() }
    return NONE
  }

  private start(): void {
    this.env.onStart?.()
    this.phase = 'track'
    this.axis = null
    this.sumX = 0
    this.sumY = 0
    this.acc = 0
    this.samples = []
    this.width = Math.max(1, this.env.distance?.() ?? SWIPE_DISTANCE_PX)
    this.can = { next: this.env.canGo?.('next') ?? true, prev: this.env.canGo?.('prev') ?? true }
  }

  private track(t: number, dx: number, dy: number): SwipeAnswer {
    let move = dx
    if (this.axis === null) {
      this.sumX += dx
      this.sumY += dy
      if (Math.abs(this.sumX) + Math.abs(this.sumY) < this.tune.axisLockPx) return NONE
      this.axis = Math.abs(this.sumX) > Math.abs(this.sumY) ? 'x' : 'y'
      move = this.sumX
    }
    if (this.axis === 'y') return NONE
    if (dx === 0 && dy === 0) return this.acc === 0 ? NONE : { offset: null, release: this.lift() }
    if (move !== 0) this.tailSign = Math.sign(move)
    this.acc += move
    this.samples.push({ t, dx: move })
    if (this.samples.length > 16) this.samples.shift()
    if (Math.abs(this.acc) >= this.width) {
      this.acc = Math.sign(this.acc) * this.width
      if (this.can[this.acc > 0 ? 'next' : 'prev']) return { offset: null, release: this.lift() }
    }
    if (this.decaying()) return { offset: null, release: this.lift() }
    return { offset: this.band(this.acc / this.width), release: null }
  }

  /** The fingers are up: decide, then absorb what follows. */
  private lift(): SwipeDecision | null {
    const share = this.acc / this.width
    const velocity = this.velocity()
    const sideways = this.axis === 'x'
    const from = this.band(share)
    this.tailLast = Math.abs(this.samples[this.samples.length - 1]?.dx ?? 0)
    this.tailRises = 0
    // Back where it started: a cancel that has nowhere to glide.
    const direction: SwipeDirection = (share === 0 ? this.tailSign : share) > 0 ? 'next' : 'prev'
    const can = this.can[direction]
    this.phase = sideways ? 'absorb' : 'idle'
    this.axis = null
    this.acc = 0
    this.samples = []
    if (!sideways) return null
    const along = velocity * Math.sign(share)
    const t = this.tune
    let outcome: SwipeRelease['outcome'] = 'cancel'
    if (can && Math.abs(share) >= t.commitAt && along > -t.flickPxPerMs) outcome = 'commit'
    else if (can && along >= t.flickPxPerMs && Math.abs(share) >= t.flickMinShare) outcome = 'commit'
    return { outcome, direction, from, velocity: velocity / this.width }
  }

  /** In the momentum tail: does this event start a new gesture? */
  private fresh(dx: number, dy: number): boolean {
    if (Math.abs(dy) >= 2 && Math.abs(dy) > 2 * Math.abs(dx)) return true
    if (dx !== 0 && Math.abs(dx) >= Math.abs(dy) && Math.sign(dx) !== this.tailSign) return true
    const m = Math.abs(dx)
    this.tailRises = m >= this.tune.riseMinPx && m > this.tailLast * this.tune.riseRatio ? this.tailRises + 1 : 0
    this.tailLast = m
    return this.tailRises >= 2
  }

  /** The travel as the strip shows it: 1:1 towards a group, a third past the ends. */
  private band(share: number): number {
    if (share === 0) return 0
    const clamped = Math.max(-1, Math.min(1, share))
    return this.can[share > 0 ? 'next' : 'prev'] ? clamped : clamped * this.tune.rubberBand
  }

  /** Speed over the last events (px/ms, signed). */
  private velocity(): number {
    const n = this.samples.length
    if (n === 0) return 0
    const lastT = this.samples[n - 1].t
    const w = this.samples.filter((s) => s.t >= lastT - this.tune.velocityWindowMs)
    if (w.length < 2) return w[0].dx / 16
    const span = lastT - w[0].t
    if (span <= 0) return 0
    let sum = 0
    for (let i = 1; i < w.length; i++) sum += w[i].dx
    return sum / span
  }

  /** The last deltas fall away smoothly, the shape of macOS's momentum. */
  private decaying(): boolean {
    const n = this.tune.decaySteps
    if (this.samples.length < n + 1) return false
    const tail = this.samples.slice(-(n + 1)).map((s) => s.dx)
    const sign = Math.sign(tail[0])
    if (tail.some((d) => Math.sign(d) !== sign)) return false
    const m = tail.map(Math.abs)
    if (m[0] < this.tune.decayMinPx) return false
    for (let i = 1; i <= n; i++) if (!(m[i] < m[i - 1] && m[i] >= m[i - 1] * this.tune.decayMinRatio)) return false
    return true
  }
}

/** The glide after a release: where the strip ends (signed share: ±1 the
 *  next or previous group, 0 back home), how long it takes and its curve.
 *  The curve's first handle is set so the glide starts at the fingers'
 *  speed when they were moving that way faster than the plain curve. */
export function glideFor(d: SwipeDecision): { to: number; ms: number; easing: string } {
  const to = d.outcome === 'commit' ? (d.direction === 'next' ? 1 : -1) : 0
  const remaining = Math.abs(to - d.from)
  if (remaining < 0.002) return { to, ms: 0, easing: GLIDE_EASE }
  const ms = Math.round(GLIDE_MIN_MS + (GLIDE_MAX_MS - GLIDE_MIN_MS) * Math.min(1, remaining))
  const toward = d.velocity * Math.sign(to - d.from)
  // cubic-bezier(x1, 1, 0.5, 1) opens at a slope of 1 / x1 (4 for the plain curve).
  const slope = toward > 0 ? (toward * ms) / remaining : 0
  if (slope <= 4) return { to, ms, easing: GLIDE_EASE }
  const x1 = Math.max(0.05, 1 / slope)
  return { to, ms, easing: `cubic-bezier(${x1.toFixed(3)}, 1, 0.5, 1)` }
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
