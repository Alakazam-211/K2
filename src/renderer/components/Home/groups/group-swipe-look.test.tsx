// @vitest-environment jsdom
//
// Home groups: what the swipe looks like. The strip follows the fingers
// 1:1 with the incoming group riding in beside it (its glyph and name at
// the leading edge, no instruction text), then glides on and lands, or
// glides back; a rubber band at the ends. Synthetic wheel streams, fake
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
import { GroupSwipeStrip } from './GroupSwipeSlide'
import { rowShifts } from './useRowFlip'
import { useHomeGroups } from './useHomeGroups'
import { readFileSync } from 'fs'
import { join } from 'path'

const groupsCss = readFileSync(join(__dirname, 'home-groups.css'), 'utf8')
import { GLIDE_EASE, SWIPE_TUNING as T } from '@/lib/group-swipe'
import { GLIDE_LAND_SLACK_MS, groupSwipeLive, useGroupSwipe, useGroupSwipeStore } from '@/hooks/useGroupSwipe'
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
          renderIncoming={(g: Home) => {
            incomingRenders++
            return <p data-testid="incoming-rows">{g.name} rows</p>
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
const incoming = (): HTMLElement | null => document.querySelector('[data-group-swipe-incoming]')
const peek = (): HTMLElement | null => document.querySelector('[data-group-swipe-peek]')
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
  it('follows the fingers 1:1 (0, 25%, 50%) with the next group riding in at its edge, and no hint text', () => {
    render(<Nav />)
    expect(strip().style.transform).toBe('')
    expect(incoming()).toBeNull()
    for (let i = 0; i < 12; i++) wheel(5) // locks at 10 px, then 5 a frame: 60 px = 25%
    wait(FRAME)
    expect([strip().style.transform, strip().style.transition]).toEqual(['translate3d(-60.00px, 0, 0)', 'none'])
    const inc = incoming() as HTMLElement
    expect(inc.getAttribute('data-group-swipe-incoming')).toBe(ids[1])
    expect([inc.style.left, inc.hasAttribute('inert'), inc.getAttribute('aria-hidden')]).toEqual(['100%', true, 'true'])
    expect(screen.getByTestId('incoming-rows').textContent).toBe('Two rows')
    // The peek is the group's glyph and name, inside the strip, nothing else.
    expect(inc.contains(peek())).toBe(true)
    expect(peek()?.textContent).toMatch(/Two$/)
    expect(peek()?.style.left).toBe('8px')
    expect(document.body.textContent).not.toMatch(HINTS)
    for (let i = 0; i < 12; i++) wheel(5)
    wait(FRAME)
    expect(strip().style.transform).toBe('translate3d(-120.00px, 0, 0)')
    // React drew the incoming group once, not per wheel event.
    expect(incomingRenders).toBeLessThanOrEqual(2)
    expect(document.body.textContent).not.toMatch(HINTS)
  })

  it('past the line it glides the rest of the way, then lands on the group from the top', () => {
    render(<Nav />)
    screen.getByTestId('list').scrollTop = 50
    for (let i = 0; i < 20; i++) wheel(5) // 100 px = 42%
    letGo()
    const ms = glideMs()
    expect(strip().style.transition).toBe(`transform ${ms}ms ${GLIDE_EASE}`)
    expect(strip().style.transform).toBe('translate3d(-240.00px, 0, 0)')
    expect(peek()?.style.opacity).toBe('0')
    expect(document.body.textContent).not.toMatch(HINTS)
    // Still on One while it glides; the switch is the landing.
    expect(current()).toBe('One')
    wait(ms + GLIDE_LAND_SLACK_MS)
    expect(current()).toBe('Two')
    expect([strip().style.transform, strip().style.transition]).toEqual(['', ''])
    expect(incoming()).toBeNull()
    expect(screen.getByTestId('list').textContent).toBe('Two rows')
    expect(screen.getByTestId('list').scrollTop).toBe(0)
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
    expect(incoming()).toBeNull()
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
    expect(incoming()?.style.left).toBe('-100%')
    expect(peek()?.textContent).toMatch(/One$/)
    expect(peek()?.style.right).toBe('8px')
  })

  it('past the first group: a third of the travel, nothing beside it, and back', () => {
    render(<Nav />)
    for (let i = 0; i < 20; i++) wheel(-5) // 100 px of finger
    wait(FRAME)
    expect(strip().style.transform).toBe('translate3d(33.33px, 0, 0)')
    expect(incoming()).toBeNull()
    letGo()
    expect(strip().style.transform).toBe('translate3d(0.00px, 0, 0)')
    wait(1000)
    expect(current()).toBe('One')
    expect(strip().style.transform).toBe('')
  })

  it('the rail peeks the glyph alone', () => {
    render(<Nav variant="rail" />)
    for (let i = 0; i < 12; i++) wheel(5)
    wait(FRAME)
    expect(peek()?.textContent).not.toContain('Two')
    expect(peek()?.style.left).toBe('2px')
    expect(document.body.textContent).not.toMatch(HINTS)
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
        onRename={noop}
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
