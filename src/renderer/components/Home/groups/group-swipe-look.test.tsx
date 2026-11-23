// @vitest-environment jsdom
//
// Home groups: what the swipe looks like. The strip follows the fingers
// 1:1 with the neighbouring groups' rows beside it, drawn ahead of the
// gesture and never during it (no tile naming the group, no instruction
// text), then glides on and lands, or glides back; a rubber band at the
// ends. A landing holds until the list shows the new group. Synthetic wheel streams, fake
// timers and rAF. Also the bar reading the live swipe, and the
// recent-first move.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import HomeGroupBar, { swipeTargetId, type GroupBarItem } from './HomeGroupBar'
import { GroupSwipeStrip, STRIP_HOLD_MAX_MS, paintStrip } from './GroupSwipeSlide'
import { rowShifts } from './useRowFlip'
import { useHomeGroups } from './useHomeGroups'
import { readFileSync } from 'fs'
import { join } from 'path'

const groupsCss = readFileSync(join(__dirname, 'home-groups.css'), 'utf8')
import { GLIDE_EASE, SWIPE_TUNING as T } from '@/lib/group-swipe'
import { GLIDE_LAND_SLACK_MS, groupSwipeLive, useGroupSwipe, useGroupSwipeStore, type SwipeFrame } from '@/hooks/useGroupSwipe'
import { selectedHome, useHomesStore, type Home } from '@/stores/homes'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: false, pinned: [], count: 4 },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [], count: 2 },
  { id: 'g3', name: 'Later', icon: 'music', order: 2, recentFirst: false, pinned: [], count: 0 },
]

/** The nav is 240 px wide: the swipe's width, and the strip's. */
const NAV = 240
const FRAME = 16
const HINTS = /let go|release|switch|next group|previous group|swipe/i

let incomingRenders = 0
function Nav({ variant = 'nav' }: { variant?: 'nav' | 'rail' }): React.JSX.Element {
  useGroupSwipe(true)
  const home = useHomesStore(selectedHome)
  return (
    <nav data-group-swipe-zone="">
      <div data-group-swipe-stage="">
        <GroupSwipeStrip
          variant={variant}
          groupId={home.id}
          renderNeighbour={(g: Home) => {
            incomingRenders++
            return <p data-testid={`neighbour-${g.name}`}>{g.name} rows</p>
          }}
        >
          <div data-scroll-edges-list="" data-testid="list">
            {home.name} rows
          </div>
        </GroupSwipeStrip>
      </div>
    </nav>
  )
}

const ids: string[] = []
beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'performance', 'requestAnimationFrame', 'cancelAnimationFrame'] })
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(NAV)
  vi.spyOn(HTMLElement.prototype, 'clientWidth', 'get').mockReturnValue(NAV)
  localStorage.clear()
  sessionStorage.clear()
  useGroupSwipeStore.setState({ swipe: null, release: null })
  const st = useHomesStore.getState()
  const before = st.homes.map((h) => h.id)
  ids.length = 0
  for (const n of ['One', 'Two', 'Three']) {
    const id = st.createHome(n)
    if (id === null) throw new Error(`createHome refused ${n}`)
    ids.push(id)
  }
  for (const id of before) if (!useHomesStore.getState().deleteHome(id)) throw new Error('deleteHome refused')
  act(() => useHomesStore.getState().selectHome(ids[0]))
  incomingRenders = 0
})
afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  vi.useRealTimers()
  useGroupSwipeStore.setState({ swipe: null, release: null })
})

const strip = (): HTMLElement => document.querySelector('[data-group-swipe-strip]') as HTMLElement
const side = (s: 'next' | 'prev'): HTMLElement | null => document.querySelector(`[data-group-swipe-side="${s}"]`)
/** Rosson: no tile naming the group the swipe heads to. Nothing floats
 *  over the strip: only the two lists' rows are on screen. */
const expectNoTile = (rows: string): void => {
  expect(document.querySelector('[data-group-swipe-peek], [data-group-float], [data-k2-chamfer-float]')).toBeNull()
  expect(document.body.textContent).toBe(rows)
}
const current = (): string => selectedHome(useHomesStore.getState()).name
/** One wheel event over the list, a frame after the last; then that frame paints. */
const wheel = (deltaX: number): void => {
  act(() => {
    vi.advanceTimersByTime(FRAME)
    screen.getByTestId('list').dispatchEvent(new WheelEvent('wheel', { deltaX, deltaY: 0, bubbles: true }))
  })
}
const wait = (ms: number): void => {
  act(() => {
    vi.advanceTimersByTime(ms)
  })
}
/** Let the fingers go, and the glide's first frame paint. */
const letGo = (): void => wait(T.idleMs + 4 + FRAME)
const glideMs = (): number => {
  const f = groupSwipeLive.get()
  if (f.kind !== 'glide') throw new Error(`not gliding: ${f.kind}`)
  return f.ms
}

describe('the strip', () => {
  it('the neighbours sit just out of view at rest, drawn ahead of the gesture: none left of the first group', () => {
    act(() => useHomesStore.getState().selectHome(ids[1]))
    render(<Nav />)
    expect(strip().style.transform).toBe('')
    const [prev, next] = [side('prev') as HTMLElement, side('next') as HTMLElement]
    expect([prev.getAttribute('data-group-swipe-incoming'), prev.style.left]).toEqual([ids[0], '-100%'])
    expect([next.getAttribute('data-group-swipe-incoming'), next.style.left]).toEqual([ids[2], '100%'])
    for (const el of [prev, next]) expect([el.hasAttribute('inert'), el.getAttribute('aria-hidden')]).toEqual([true, 'true'])
    cleanup()
    act(() => useHomesStore.getState().selectHome(ids[0]))
    render(<Nav />)
    expect(side('prev')).toBeNull()
    expect(side('next')?.getAttribute('data-group-swipe-incoming')).toBe(ids[1])
  })

  it('follows the fingers 1:1 (0, 30%, 60%) with the next group’s rows at its edge, drawing nothing while it moves, and no tile or hint text', () => {
    render(<Nav />)
    expectNoTile('One rowsTwo rows')
    const drawn = incomingRenders
    for (let i = 0; i < 12; i++) wheel(6) // locks at 10 px, then 6 a frame: 72 px = 30%
    wait(FRAME)
    expect([strip().style.transform, strip().style.transition]).toEqual(['translate3d(-72.00px, 0, 0)', 'none'])
    const inc = side('next') as HTMLElement
    expect(inc.getAttribute('data-group-swipe-incoming')).toBe(ids[1])
    expect(screen.getByTestId('neighbour-Two').textContent).toBe('Two rows')
    // The incoming group is its rows alone.
    expect(inc.children.length).toBe(1)
    expectNoTile('One rowsTwo rows')
    expect(document.body.textContent).not.toMatch(HINTS)
    for (let i = 0; i < 12; i++) wheel(6)
    wait(FRAME)
    expect(strip().style.transform).toBe('translate3d(-144.00px, 0, 0)')
    expectNoTile('One rowsTwo rows')
    // React drew no group while the fingers moved.
    expect(incomingRenders).toBe(drawn)
    expect(document.body.textContent).not.toMatch(HINTS)
  })

  it('past the line it glides the rest of the way, then lands on the group from the top, with its own neighbours', () => {
    render(<Nav />)
    screen.getByTestId('list').scrollTop = 50
    for (let i = 0; i < 20; i++) wheel(5) // 100 px = 42%
    letGo()
    const ms = glideMs()
    expect(strip().style.transition).toBe(`transform ${ms}ms ${GLIDE_EASE}`)
    expect(strip().style.transform).toBe('translate3d(-240.00px, 0, 0)')
    expectNoTile('One rowsTwo rows')
    // Still on One while it glides; the switch is the landing.
    expect(current()).toBe('One')
    wait(ms + GLIDE_LAND_SLACK_MS)
    expect(current()).toBe('Two')
    expect([strip().style.transform, strip().style.transition]).toEqual(['', ''])
    expect(screen.getByTestId('list').textContent).toBe('Two rows')
    expect(screen.getByTestId('list').scrollTop).toBe(0)
    expect([side('prev'), side('next')].map((el) => el?.getAttribute('data-group-swipe-incoming'))).toEqual([ids[0], ids[2]])
  })

  it('short of the line and slow, it glides back', () => {
    render(<Nav />)
    for (let i = 0; i < 8; i++) wheel(5) // 40 px = 17%
    letGo()
    const ms = glideMs()
    expect(strip().style.transform).toBe('translate3d(0.00px, 0, 0)')
    expect(strip().style.transition).toBe(`transform ${ms}ms ${GLIDE_EASE}`)
    wait(ms + GLIDE_LAND_SLACK_MS)
    expect(current()).toBe('One')
    expect(strip().style.transform).toBe('')
    expect(side('next')?.getAttribute('data-group-swipe-incoming')).toBe(ids[1])
  })

  it('a quick flick short of the line still goes', () => {
    render(<Nav />)
    for (let i = 0; i < 4; i++) wheel(12) // 48 px = 20% at 0.75 px/ms
    letGo()
    expect(strip().style.transform).toBe('translate3d(-240.00px, 0, 0)')
    wait(1000)
    expect(current()).toBe('Two')
  })

  it('back to the previous group: it rides in from the left', () => {
    act(() => useHomesStore.getState().selectHome(ids[1]))
    render(<Nav />)
    for (let i = 0; i < 6; i++) wheel(-5)
    wait(FRAME)
    expect(strip().style.transform).toBe('translate3d(30.00px, 0, 0)')
    expect(side('prev')?.style.left).toBe('-100%')
    expectNoTile('Two rowsOne rowsThree rows')
  })

  it('past the first group: a third of the travel, nothing beside it, and back', () => {
    render(<Nav />)
    for (let i = 0; i < 20; i++) wheel(-5) // 100 px of finger
    wait(FRAME)
    expect(strip().style.transform).toBe('translate3d(33.33px, 0, 0)')
    expect(side('prev')).toBeNull()
    letGo()
    expect(strip().style.transform).toBe('translate3d(0.00px, 0, 0)')
    wait(1000)
    expect(current()).toBe('One')
    expect(strip().style.transform).toBe('')
  })

  it('the rail shows no tile either', () => {
    render(<Nav variant="rail" />)
    for (let i = 0; i < 12; i++) wheel(5)
    wait(FRAME)
    expect(strip().style.transform).toBe('translate3d(-60.00px, 0, 0)')
    expectNoTile('One rowsTwo rows')
  })
})

describe('the landing waits for the list', () => {
  const glide: SwipeFrame = { kind: 'glide', from: 0.5, to: 1, ms: 200, easing: GLIDE_EASE, outcome: 'commit' }
  const motion = (groupId: string): Parameters<typeof paintStrip>[2] => ({ width: NAV, applied: null, groupId, glideFrom: null })

  it('a landed rest holds the strip where it glided to until the list shows the new group; a cancel rests at once', () => {
    const el = document.createElement('div')
    const m = motion('g1')
    expect(paintStrip(el, glide, m)).toBe(true)
    expect(el.style.transform).toBe('translate3d(-240.00px, 0, 0)')
    const landed: SwipeFrame = { kind: 'rest', landed: true }
    // The list still draws g1 (it draws the switch a moment later).
    expect(paintStrip(el, landed, m)).toBe(false)
    expect(el.style.transform).toBe('translate3d(-240.00px, 0, 0)')
    m.groupId = 'g2'
    expect(paintStrip(el, landed, m)).toBe(true)
    expect([el.style.transform, el.style.transition, el.style.willChange]).toEqual(['', '', ''])
    // A cancel never waits.
    const c = motion('g1')
    paintStrip(el, { ...glide, to: 0, outcome: 'cancel' }, c)
    expect(paintStrip(el, { kind: 'rest', landed: false }, c)).toBe(true)
    expect(el.style.transform).toBe('')
  })

  it('a glide from where the fingers left the strip starts there without a forced style flush', () => {
    const el = document.createElement('div')
    const m = motion('g1')
    const flush = vi.spyOn(window, 'getComputedStyle')
    paintStrip(el, { kind: 'track', offset: 0.5 }, m)
    paintStrip(el, glide, m)
    expect(flush).not.toHaveBeenCalled()
    expect([el.style.transition, el.style.transform]).toEqual([`transform 200ms ${GLIDE_EASE}`, 'translate3d(-240.00px, 0, 0)'])
    // From anywhere else it sets the start first, then flushes once.
    paintStrip(el, { kind: 'track', offset: 0.2 }, m)
    paintStrip(el, { ...glide }, m)
    expect(flush).toHaveBeenCalledTimes(1)
  })

  it('in the strip: held while the list is behind, never longer than STRIP_HOLD_MAX_MS', () => {
    // A list that never catches up (its group was picked back meanwhile).
    function Stuck(): React.JSX.Element {
      useGroupSwipe(true)
      return (
        <nav data-group-swipe-zone="">
          <div data-group-swipe-stage="">
            <GroupSwipeStrip variant="nav" groupId={ids[0]} renderNeighbour={(g: Home) => <p>{g.name} rows</p>}>
              <div data-testid="list">One rows</div>
            </GroupSwipeStrip>
          </div>
        </nav>
      )
    }
    render(<Stuck />)
    for (let i = 0; i < 20; i++) wheel(5)
    letGo()
    wait(glideMs() + GLIDE_LAND_SLACK_MS + FRAME)
    expect(current()).toBe('Two')
    expect(strip().style.transform).toBe('translate3d(-240.00px, 0, 0)')
    wait(STRIP_HOLD_MAX_MS)
    expect(strip().style.transform).toBe('')
  })
})

describe('the group bar during a swipe', () => {
  it('reads the live swipe: the target steps up past the line and while gliding to land, not on a cancel', () => {
    const noop = (): void => undefined
    render(
      <HomeGroupBar
        groups={G}
        currentId="g1"
        onSelect={noop}
        onRename={() => 'saved'}
        onSetIcon={noop}
        onCreate={() => null}
        onDelete={noop}
        onSetRecentFirst={noop}
        onReorderGroups={noop}
      />,
    )
    const bar = screen.getByRole('toolbar')
    const pending = (): string | null => document.querySelector('[data-pending]')?.getAttribute('data-group-id') ?? null
    act(() => useGroupSwipeStore.setState({ swipe: { progress: 0.2, direction: 'next' } }))
    expect([bar.getAttribute('data-swipe-direction'), bar.getAttribute('data-swipe-armed'), pending()]).toEqual(['next', null, null])
    act(() => useGroupSwipeStore.setState({ swipe: { progress: 0.34, direction: 'next' } }))
    expect([bar.getAttribute('data-swipe-armed'), pending()]).toEqual(['', 'g2'])
    act(() => useGroupSwipeStore.setState({ swipe: { progress: 1, direction: 'next', settling: 'commit' } }))
    expect(pending()).toBe('g2')
    act(() => useGroupSwipeStore.setState({ swipe: { progress: 0, direction: 'next', settling: 'cancel' } }))
    expect([bar.getAttribute('data-swipe-armed'), pending()]).toEqual([null, null])
    act(() => useGroupSwipeStore.setState({ swipe: null }))
    expect(bar.getAttribute('data-swipe-direction')).toBe('none')
  })

  it('names the group a swipe heads to (no wrap)', () => {
    expect(swipeTargetId(G, 'g1', { progress: 0.5, direction: 'next' })).toBe('g2')
    expect(swipeTargetId(G, 'g1', { progress: 0.5, direction: 'prev' })).toBeNull()
    expect(swipeTargetId(G, 'g3', { progress: 0.5, direction: 'next' })).toBeNull()
  })
})

function BarNav(): React.JSX.Element {
  useGroupSwipe(true)
  const g = useHomeGroups()
  return (
    <nav data-group-swipe-zone="">
      <div data-group-swipe-stage="">
        <div data-testid="list">rows</div>
      </div>
      <HomeGroupBar {...g.bar} />
    </nav>
  )
}

describe('the group bar’s colours follow the swipe (Rosson: no white, then a sudden blue)', () => {
  const bar = (): HTMLElement => screen.getByRole('toolbar')
  const icon = (i: number): HTMLElement => bar().querySelectorAll<HTMLElement>('[data-group-id]')[i]
  const blend = (): string => bar().style.getPropertyValue('--group-swipe-progress')
  /** The accent's share (%) a rule in home-groups.css gives an icon at progress `p`. */
  const accentShare = (role: 'from' | 'to', p: number): number => {
    const rule = new RegExp(`\\[data-swipe-role="${role}"\\]\\s*\\{\\s*color:\\s*color-mix\\(in oklab, var\\(--color-accent-text\\) calc\\(([^;]*?)\\* 100%\\), var\\(--color-text-muted\\)\\);`)
    const m = rule.exec(groupsCss)
    if (!m) throw new Error(`no ${role} rule`)
    const expr = m[1].replace(/var\(--group-swipe-progress\)/g, String(p))
    if (!/^[\d\s.()+*/-]+$/.test(expr)) throw new Error(`odd calc: ${expr}`)
    return Math.round(Number(new Function(`return ${expr}`)()) * 100)
  }

  it('at 50% both icons are mid-blend, set outside React; the rest stay muted', () => {
    render(<BarNav />)
    expect(bar().hasAttribute('data-swiping')).toBe(false)
    for (let i = 0; i < 24; i++) wheel(5) // locks at 10 px, then 5 a frame: 120 px = 50%
    wait(FRAME)
    expect(blend()).toBe('0.5')
    expect(bar().hasAttribute('data-swiping')).toBe(true)
    expect([icon(0), icon(1), icon(2)].map((el) => el.getAttribute('data-swipe-role'))).toEqual(['from', 'to', null])
    expect([accentShare('from', 0.5), accentShare('to', 0.5)]).toEqual([50, 50])
    expect([accentShare('from', 0), accentShare('to', 0), accentShare('from', 1), accentShare('to', 1)]).toEqual([100, 0, 0, 100])
    // The bright "about to switch" ink is gone.
    expect(icon(1).className.split(/\s+/)).not.toContain('text-[var(--color-text-primary)]')
  })

  it('no hover ink and no colour transition on any icon while a swipe is under way', () => {
    expect(groupsCss).toMatch(/\[data-home-group-bar\]\[data-swiping\] \[data-group-id\]:not\(\[data-current\]\) \{ color: var\(--color-text-muted\); \}/)
    expect(groupsCss).toMatch(/\[data-home-group-bar\]\[data-swiping\] \[data-group-id\] \{ transition: none; \}/)
    // Three attributes and a :not() (0,4,0) outrank Tailwind's hover:text-… (0,2,0).
    render(<BarNav />)
    expect(icon(2).className).toContain('hover:text-[var(--color-text-primary)]')
    for (let i = 0; i < 12; i++) wheel(5)
    expect(bar().hasAttribute('data-swiping')).toBe(true)
  })

  it('on release the blend glides with the strip: on to the new group, or back', () => {
    render(<BarNav />)
    for (let i = 0; i < 23; i++) wheel(5)
    letGo()
    const f = groupSwipeLive.get()
    if (f.kind !== 'glide') throw new Error('no glide')
    expect(bar().style.transition).toBe(`--group-swipe-progress ${f.ms}ms ${f.easing}`)
    expect(blend()).toBe('1')
    wait(f.ms + GLIDE_LAND_SLACK_MS + FRAME)
    expect(icon(1).hasAttribute('data-current')).toBe(true)
    expect(bar().hasAttribute('data-swiping')).toBe(false)
    expect(blend()).toBe('')
    // A short one glides back.
    for (let i = 0; i < 8; i++) wheel(5)
    letGo()
    expect(blend()).toBe('0')
    wait(1000)
    expect(icon(1).hasAttribute('data-current')).toBe(true)
  })

  it('"current" is never empty, nor doubled, through a commit', () => {
    render(<BarNav />)
    const counts = new Set<number>()
    const look = (): void => {
      counts.add(bar().querySelectorAll('[data-current]').length)
    }
    const mo = new MutationObserver(look)
    mo.observe(bar(), { subtree: true, attributes: true, childList: true })
    for (let i = 0; i < 23; i++) {
      wheel(5)
      look()
    }
    letGo()
    look()
    for (let t = 0; t < 400; t += 8) {
      wait(8)
      look()
    }
    mo.disconnect()
    expect(current()).toBe('Two')
    expect([...counts]).toEqual([1])
  })
})

describe('the recent-first move', () => {
  it('measures how far each row moved', () => {
    const before = new Map([['a', 0], ['b', 50], ['c', 100]])
    const after = new Map([['c', 0], ['a', 50], ['b', 100]])
    expect(Object.fromEntries(rowShifts(before, after))).toEqual({ c: 100, a: -50, b: -50 })
  })
})
