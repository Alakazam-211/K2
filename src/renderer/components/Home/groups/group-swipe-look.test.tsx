// @vitest-environment jsdom
//
// Home groups: the swipe's slide and peek, and the recent-first move
// (K2 Design's mock; no reduced-motion variant, Rosson).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { swipeTargetId, type GroupBarItem } from './HomeGroupBar'
import { useGroupSwipeSlide } from './GroupSwipeSlide'
import { rowShifts } from './useRowFlip'
import { useGroupSwipeStore } from '@/hooks/useGroupSwipe'
import { useHomesStore } from '@/stores/homes'

const G: GroupBarItem[] = [
  { id: 'g1', name: 'Work', icon: 'briefcase', order: 0, recentFirst: false, pinned: [], count: 4 },
  { id: 'g2', name: 'Play', icon: null, order: 1, recentFirst: false, pinned: [], count: 2 },
  { id: 'g3', name: 'Later', icon: 'music', order: 2, recentFirst: false, pinned: [], count: 0 },
]

afterEach(() => {
  cleanup()
  useGroupSwipeStore.setState({ swipe: null, release: null })
})

function Slide(): React.JSX.Element {
  const s = useGroupSwipeSlide('nav')
  return (
    <div>
      <div data-testid="slide" style={s.listStyle} />
      {s.peek}
    </div>
  )
}

describe('the swipe', () => {
  it('slides the list with the finger and peeks the group it would land on', () => {
    const st = useHomesStore.getState()
    const a = st.createHome('Alpha')
    const b = st.createHome('Beta')
    if (!a || !b) throw new Error('createHome refused')
    act(() => useHomesStore.getState().selectHome(a))
    render(<Slide />)
    act(() => useGroupSwipeStore.setState({ swipe: { progress: 0.25, direction: 'next' } }))
    const list = screen.getByTestId('slide')
    expect([list.style.transform, list.style.opacity, list.style.transition]).toEqual(['translateX(-10.00%)', '0.875', 'none'])
    const peek = document.querySelector('[data-group-swipe-peek]') as HTMLElement
    expect(peek.getAttribute('data-group-swipe-peek')).toBe(b)
    expect(peek.textContent).toContain('Beta')
    expect(peek.textContent).toContain('Next group')
    expect(peek.hasAttribute('data-armed')).toBe(false)
    act(() => useGroupSwipeStore.setState({ swipe: { progress: 0.6, direction: 'next' } }))
    expect(peek.textContent).toContain('Let go to switch')
    expect(document.querySelector('[data-group-swipe-peek]')?.hasAttribute('data-armed')).toBe(true)
    // Let go short of the line: it slides back on the fast token.
    act(() => useGroupSwipeStore.setState({ swipe: null }))
    expect(screen.getByTestId('slide').style.transform).toBe('translateX(0)')
    expect(screen.getByTestId('slide').style.transition).toContain('var(--motion-duration-fast)')
    expect(document.querySelector('[data-group-swipe-peek]')).toBeNull()
  })

  it('the bar names the group a swipe heads to (no wrap)', () => {
    expect(swipeTargetId(G, 'g1', { progress: 0.5, direction: 'next' })).toBe('g2')
    expect(swipeTargetId(G, 'g1', { progress: 0.5, direction: 'prev' })).toBeNull()
    expect(swipeTargetId(G, 'g3', { progress: 0.5, direction: 'next' })).toBeNull()
  })
})

describe('the recent-first move', () => {
  it('measures how far each row moved', () => {
    const before = new Map([['a', 0], ['b', 50], ['c', 100]])
    const after = new Map([['c', 0], ['a', 50], ['b', 100]])
    expect(Object.fromEntries(rowShifts(before, after))).toEqual({ c: 100, a: -50, b: -50 })
  })
})
