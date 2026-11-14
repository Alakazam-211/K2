// @vitest-environment jsdom
// Rosson 2026-11-14: "When an agent is monitoring something or working,
// there's a solid or dotted white line along the bottom of the activity
// area. Whatever it's telling us, it's not useful and it looks dumb."
//
// The line meant nothing: the tab on the message box has no bottom edge,
// but its style mixed the `border` shorthand with `borderStyle` and
// `borderBottom: 'none'`. When the turn's tone changed (to or from "No
// update"), React re-set `borderStyle` alone, which brought the bottom edge
// back at the default 3px in the text colour: solid, or dashed while the
// turn read "No update". The state is in the tab's words, its dot and its
// dashed outline; the bottom stays open in every tone.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { coerceThreadTurn, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { ZenActivityDrawer } from './ZenActivityDrawer'

const T0 = Date.now()

function turn(state: string, phase: string, rev: number, extra: Record<string, unknown> = {}): ThreadTurn {
  const t = coerceThreadTurn(
    {
      turnId: 'edge-1',
      startedAt: T0 - 60_000,
      state,
      phase,
      line: phase === 'tool' ? 'Running `cargo test`' : null,
      subagents: 0,
      background: 0,
      tally: { read: 1, cmd: 1 },
      end: null,
      rev,
      serverNow: T0,
      ...extra,
    },
    T0,
  )
  if (!t) throw new Error(`fixture is not a turn: ${state}/${phase}`)
  return t
}

const WORKING = turn('working', 'tool', 1)
const MONITORING = turn('monitoring', 'children', 2, {
  replied: true,
  background: 1,
  children: [{ kind: 'monitor', state: 'running', label: 'the release build', since: T0 - 30_000 }],
})
const NO_UPDATE = turn('unverifiable', 'stale', 3)

function tile(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-zen-drawer-tile]')
  if (!el) throw new Error('no drawer tile')
  return el
}

function tone(): string | null {
  return document.querySelector('[data-zen-drawer]')?.getAttribute('data-tone') ?? null
}

afterEach(cleanup)

describe('the activity tab on the message box has no line along its bottom', () => {
  it('in every tone, and after every change of tone', () => {
    const seq: Array<[ThreadTurn, string]> = [
      [WORKING, 'working'],
      [MONITORING, 'monitoring'],
      [NO_UPDATE, 'unverifiable'],
      [WORKING, 'working'],
      [MONITORING, 'monitoring'],
      [NO_UPDATE, 'unverifiable'],
    ]
    const { rerender } = render(<ZenActivityDrawer turn={seq[0][0]} />)
    for (const [t, want] of seq) {
      rerender(<ZenActivityDrawer turn={t} />)
      expect(tone()).toBe(want)
      const s = tile().style
      expect(s.borderBottomWidth, want).toBe('0px')
      expect(s.borderTopWidth, want).toBe('1px')
      expect(s.borderLeftWidth, want).toBe('1px')
      expect(s.borderRightWidth, want).toBe('1px')
      expect(s.borderBottom, want).not.toMatch(/solid|dashed|dotted/)
    }
  })

  it('the state still shows: in the words, the dot and the outline', () => {
    const { rerender } = render(<ZenActivityDrawer turn={WORKING} />)
    expect(tile().textContent).toContain('cargo test')
    expect(document.querySelector('[data-zen-drawer-dot]')?.hasAttribute('data-zen-pulse')).toBe(true)
    expect(tile().style.borderStyle).toBe('solid')

    rerender(<ZenActivityDrawer turn={MONITORING} />)
    expect(tile().textContent).toMatch(/release build/)
    const ring = document.querySelector<HTMLElement>('[data-zen-drawer-dot]')
    expect(ring?.hasAttribute('data-zen-pulse')).toBe(false)
    expect(ring?.style.border).toMatch(/^2px solid/)

    rerender(<ZenActivityDrawer turn={NO_UPDATE} />)
    expect(tile().textContent).toMatch(/No update/)
    // The "No update" outline: dashed on the sides and top, still open below.
    expect(tile().style.borderTopStyle).toBe('dashed')
    expect(tile().style.borderBottomWidth).toBe('0px')
  })

  it('a free tile (no message box) keeps all four edges', () => {
    const { rerender } = render(<ZenActivityDrawer turn={WORKING} attached={false} />)
    rerender(<ZenActivityDrawer turn={NO_UPDATE} attached={false} />)
    rerender(<ZenActivityDrawer turn={WORKING} attached={false} />)
    const s = tile().style
    expect(s.borderBottomWidth).toBe('1px')
    expect(s.borderBottomStyle).toBe('solid')
    expect(s.borderRadius).not.toMatch(/ 0 0$/)
  })
})
