// @vitest-environment jsdom
// Rosson 2026-10-16: "When I send messages, if the agent is still working we
// don't need to collapse the activity dog ear and then re-open it."
//
// A message sent mid-turn makes the daemon end the open turn `superseded` at
// once (`Tracker::start`, thread_activity.rs); the new turn's first frame
// comes only after delivery. The drawer holds the turn it had across that
// gap (`SUPERSEDE_HOLD_MS`), so the ear stays mounted and open, and the
// glass strip that tracks its top doesn't jump. A turn that really ends
// still collapses.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render } from '@testing-library/react'
import { coerceThreadTurn, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { SUPERSEDE_HOLD_MS, ZEN_DRAWER_MS, ZenActivityDrawer, zenDrawerHeldTurn } from './ZenActivityDrawer'

const T0 = 1_800_000_000_000

function turn(fields: Record<string, unknown>, at: number): ThreadTurn {
  const t = coerceThreadTurn(
    {
      state: 'working',
      phase: 'tool',
      line: 'Running `cargo test`',
      subagents: 1,
      background: 0,
      tally: { read: 2, cmd: 1 },
      end: null,
      rev: 1,
      serverNow: at,
      ...fields,
    },
    at,
  )
  if (!t) throw new Error('fixture is not a turn')
  return t
}

/** One render's view of the ear. */
interface Seen {
  mounted: boolean
  phase: string | null
  tile: Element | null
}

function look(): Seen {
  const drawer = document.querySelector('[data-zen-drawer]')
  return {
    mounted: drawer !== null,
    phase: drawer?.getAttribute('data-phase') ?? null,
    tile: document.querySelector('[data-zen-drawer-tile]'),
  }
}

describe('the activity ear across a message sent while the agent works', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(T0)
  })
  afterEach(() => {
    cleanup()
    vi.useRealTimers()
  })

  it('stays mounted and open from the old turn to the new one; collapses when the new turn ends', () => {
    const live1 = turn({ turnId: 't1', startedAt: T0 - 20_000, phaseSince: T0 - 1_000, rev: 4 }, T0)
    const seen: Seen[] = []
    const { rerender } = render(<ZenActivityDrawer turn={live1} onStop={() => {}} />)
    seen.push(look())

    // The user sends: the old turn ends `superseded` (the daemon's end frame).
    act(() => vi.advanceTimersByTime(300))
    const superseded = turn(
      { turnId: 't1', startedAt: T0 - 20_000, state: 'stopped', rev: 5, end: { reason: 'superseded', detail: null, at: T0 + 300 } },
      T0 + 300,
    )
    rerender(<ZenActivityDrawer turn={superseded} onStop={() => {}} />)
    seen.push(look())

    // Delivery takes a while (the inject's timed sleeps), frames re-render.
    for (let i = 0; i < 4; i++) {
      act(() => vi.advanceTimersByTime(200))
      seen.push(look())
    }

    // The new turn's first frame: the agent is on the new message.
    const live2 = turn({ turnId: 't2', startedAt: T0 + 300, phaseSince: T0 + 1_100, phase: 'thinking', line: null, rev: 1 }, T0 + 1_100)
    rerender(<ZenActivityDrawer turn={live2} onStop={() => {}} />)
    seen.push(look())
    act(() => vi.advanceTimersByTime(1_000))
    seen.push(look())

    const tile = seen[0].tile
    expect(tile).not.toBeNull()
    expect(seen.map((s) => s.mounted)).toEqual(seen.map(() => true))
    expect(seen.map((s) => s.phase)).toEqual(seen.map(() => 'in'))
    // The same tile node throughout: never remounted.
    expect(seen.every((s) => s.tile === tile)).toBe(true)
    // During the hold it still offers Stop (the agent is still working).
    expect(document.querySelector('[data-zen-drawer-stop]')).not.toBeNull()

    // The new turn really ends: the ear slides out, then is gone.
    const done = turn(
      { turnId: 't2', startedAt: T0 + 300, state: 'idle', rev: 9, end: { reason: 'done', detail: null, at: T0 + 2_200 } },
      T0 + 2_200,
    )
    act(() => vi.advanceTimersByTime(100))
    rerender(<ZenActivityDrawer turn={done} onStop={() => {}} />)
    expect(look().phase).toBe('out')
    act(() => vi.advanceTimersByTime(ZEN_DRAWER_MS + 100))
    expect(look().mounted).toBe(false)
  })

  it('a supersede with no new turn collapses once the hold runs out', () => {
    const live1 = turn({ turnId: 't1', startedAt: T0 - 5_000, phaseSince: T0 - 500 }, T0)
    const { rerender } = render(<ZenActivityDrawer turn={live1} />)
    const superseded = turn(
      { turnId: 't1', startedAt: T0 - 5_000, state: 'stopped', rev: 2, end: { reason: 'superseded', detail: null, at: T0 } },
      T0,
    )
    rerender(<ZenActivityDrawer turn={superseded} />)
    expect(look().phase).toBe('in')
    act(() => vi.advanceTimersByTime(SUPERSEDE_HOLD_MS - 200))
    expect(look().phase).toBe('in')
    act(() => vi.advanceTimersByTime(300))
    expect(look().phase).toBe('out')
    act(() => vi.advanceTimersByTime(ZEN_DRAWER_MS + 100))
    expect(look().mounted).toBe(false)
  })

  it('holds for 3s, as K2 Companion does', () => {
    expect(SUPERSEDE_HOLD_MS).toBe(3_000)
  })

  it('a new turn 2.5s after the supersede (a slow tunnel) still swaps in place', () => {
    const live1 = turn({ turnId: 't1', startedAt: T0 - 5_000, phaseSince: T0 - 500 }, T0)
    const { rerender } = render(<ZenActivityDrawer turn={live1} />)
    const tile = look().tile
    expect(tile).not.toBeNull()
    const superseded = turn(
      { turnId: 't1', startedAt: T0 - 5_000, state: 'stopped', rev: 2, end: { reason: 'superseded', detail: null, at: T0 } },
      T0,
    )
    rerender(<ZenActivityDrawer turn={superseded} />)
    const seen: Seen[] = [look()]
    for (let i = 0; i < 5; i++) {
      act(() => vi.advanceTimersByTime(500))
      seen.push(look())
    }
    const live2 = turn({ turnId: 't2', startedAt: T0, phaseSince: T0 + 2_500, phase: 'delivering', line: null, rev: 1 }, T0 + 2_500)
    rerender(<ZenActivityDrawer turn={live2} />)
    seen.push(look())
    act(() => vi.advanceTimersByTime(SUPERSEDE_HOLD_MS))
    seen.push(look())
    expect(seen.map((s) => s.phase)).toEqual(seen.map(() => 'in'))
    expect(seen.every((s) => s.tile === tile)).toBe(true)
    expect(document.querySelector('[data-zen-drawer-step]')?.textContent).toBe('Delivering…')
  })

  it('any other end collapses at once, with no hold', () => {
    for (const reason of ['done', 'reply', 'failed', 'session_gone', 'delivery_failed', 'stale']) {
      const live = turn({ turnId: 't1', startedAt: T0 - 5_000 }, T0)
      const { rerender } = render(<ZenActivityDrawer turn={live} />)
      expect(look().phase, reason).toBe('in')
      const ended = turn(
        {
          turnId: 't1',
          startedAt: T0 - 5_000,
          state: reason === 'done' || reason === 'reply' ? 'idle' : 'stopped',
          rev: 2,
          end: { reason, detail: null, at: T0 },
        },
        T0,
      )
      rerender(<ZenActivityDrawer turn={ended} />)
      expect(look().phase, reason).toBe('out')
      act(() => vi.advanceTimersByTime(ZEN_DRAWER_MS + 100))
      expect(look().mounted, reason).toBe(false)
      cleanup()
    }
  })

  it('a Thread switch drops the hold: no turn collapses at once, and a late end of the old turn is not held', () => {
    const live1 = turn({ turnId: 't1', startedAt: T0 - 5_000 }, T0)
    const { rerender } = render(<ZenActivityDrawer turn={live1} />)
    const superseded = turn(
      { turnId: 't1', startedAt: T0 - 5_000, state: 'stopped', rev: 2, end: { reason: 'superseded', detail: null, at: T0 } },
      T0,
    )
    rerender(<ZenActivityDrawer turn={superseded} />)
    expect(look().phase).toBe('in')
    // The user switches Thread: the hook resets its turn to null.
    act(() => vi.advanceTimersByTime(200))
    rerender(<ZenActivityDrawer turn={null} />)
    expect(look().phase).toBe('out')
    act(() => vi.advanceTimersByTime(ZEN_DRAWER_MS + 100))
    expect(look().mounted).toBe(false)
    // Back on the old Thread, the old turn's end (still inside its hold
    // window) is not held: the live state it would hold was dropped.
    rerender(<ZenActivityDrawer turn={superseded} />)
    expect(look().mounted).toBe(false)
    // The hold's timers went with it.
    expect(vi.getTimerCount()).toBe(0)
  })

  it('unmounting during the hold leaves no timer behind', () => {
    const live1 = turn({ turnId: 't1', startedAt: T0 - 5_000 }, T0)
    const { rerender, unmount } = render(<ZenActivityDrawer turn={live1} />)
    rerender(
      <ZenActivityDrawer
        turn={turn(
          { turnId: 't1', startedAt: T0 - 5_000, state: 'stopped', rev: 2, end: { reason: 'superseded', detail: null, at: T0 } },
          T0,
        )}
      />,
    )
    expect(vi.getTimerCount()).toBeGreaterThan(0)
    unmount()
    expect(vi.getTimerCount()).toBe(0)
  })

  it('holds only a superseded turn it saw live, within the hold', () => {
    const live = turn({ turnId: 't1', startedAt: T0 - 5_000 }, T0)
    const sup = turn({ turnId: 't1', startedAt: T0 - 5_000, state: 'stopped', end: { reason: 'superseded', detail: null, at: T0 } }, T0)
    const done = turn({ turnId: 't1', startedAt: T0 - 5_000, state: 'idle', end: { reason: 'done', detail: null, at: T0 } }, T0)
    const other = turn({ turnId: 't0', startedAt: T0 - 9_000 }, T0)
    expect(zenDrawerHeldTurn(sup, live, T0 + 100)).toBe(live)
    expect(zenDrawerHeldTurn(sup, live, T0 + SUPERSEDE_HOLD_MS)).toBe(sup)
    expect(zenDrawerHeldTurn(sup, other, T0 + 100)).toBe(sup)
    expect(zenDrawerHeldTurn(sup, null, T0 + 100)).toBe(sup)
    expect(zenDrawerHeldTurn(done, live, T0 + 100)).toBe(done)
    expect(zenDrawerHeldTurn(live, live, T0 + 100)).toBe(live)
  })
})
