// @vitest-environment jsdom
// K2 Companion `c29b6db` (Rosson 2026-10-18): the activity drawer's
// expanded detail leads each bit with the same icon its collapsed counter
// shows (read = file, search = magnifier, cmd = prompt, edit = pencil,
// subagents = agent, background = cog), same size and baseline, the words
// and the " · " between them unchanged.
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { act, cleanup, fireEvent, render } from '@testing-library/react'
import { coerceThreadTurn, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { COUNTER_ICON_GAP_PX, COUNTER_ICON_PX, type StripCounterKey } from '@/components/SessionView/threadStrip'
import { ZEN_DRAWER_EXPANDED_KEY, ZenActivityDrawer } from './ZenActivityDrawer'

const T0 = Date.now()

function turn(fields: Record<string, unknown>): ThreadTurn {
  const t = coerceThreadTurn(
    {
      state: 'working',
      phase: 'tool',
      line: 'Ran `npm test`',
      subagents: 0,
      background: 0,
      tally: {},
      end: null,
      rev: 1,
      serverNow: T0,
      turnId: 'icons-1',
      startedAt: T0 - 8_000,
      phaseSince: T0 - 3_000,
      ...fields,
    },
    T0,
  )
  if (!t) throw new Error('fixture is not a turn')
  return t
}

function el(sel: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(sel)
  if (!found) throw new Error(`missing ${sel}`)
  return found
}

/** The icon's drawing (its paths, circles, lines…), size and placement. */
function iconOf(host: Element): { size: [string | null, string | null]; ink: string; align: string; gap: string } {
  const svg = host.querySelector('svg')
  if (!svg) throw new Error(`no icon in ${host.outerHTML.slice(0, 120)}`)
  return {
    size: [svg.getAttribute('width'), svg.getAttribute('height')],
    ink: svg.innerHTML,
    align: svg.style.verticalAlign,
    gap: svg.style.marginRight,
  }
}

const ALL = turn({
  subagents: 2,
  subagentsDone: 1,
  background: 1,
  tally: { read: 3, search: 1, cmd: 2, edit: 1 },
})

beforeEach(() => {
  localStorage.removeItem(ZEN_DRAWER_EXPANDED_KEY)
})

afterEach(() => {
  cleanup()
  localStorage.removeItem(ZEN_DRAWER_EXPANDED_KEY)
})

describe('the expanded detail: an icon beside each bit', () => {
  it('each bit leads with its collapsed counter’s icon, same size and baseline', async () => {
    render(<ZenActivityDrawer turn={ALL} />)
    // Collapsed: every counter's icon, by key.
    const collapsed = new Map<string, ReturnType<typeof iconOf>>()
    for (const c of Array.from(document.querySelectorAll('[data-zen-drawer-count]'))) {
      collapsed.set(c.getAttribute('data-zen-drawer-count') as string, iconOf(c))
    }
    const keys: StripCounterKey[] = ['subagents', 'background', 'read', 'search', 'cmd', 'edit']
    expect([...collapsed.keys()].sort()).toEqual([...keys].sort())

    await act(async () => {
      fireEvent.click(el('[data-zen-drawer-tile]'))
    })
    expect(el('[data-zen-drawer]').getAttribute('data-expanded')).toBe('true')

    const tally = el('[data-zen-drawer-tally]')
    const children = el('[data-zen-drawer-children]')
    const bitKeys = (host: HTMLElement): string[] =>
      Array.from(host.querySelectorAll('[data-zen-drawer-detail-bit]')).map((b) => b.getAttribute('data-zen-drawer-detail-bit') as string)
    expect(bitKeys(tally)).toEqual(['read', 'search', 'cmd', 'edit'])
    expect(bitKeys(children)).toEqual(['subagents', 'background'])

    for (const key of keys) {
      const icon = el(`[data-zen-drawer-detail-icon="${key}"]`)
      const got = iconOf(icon)
      const want = collapsed.get(key)
      if (!want) throw new Error(`no collapsed counter for ${key}`)
      // The collapsed subagents counter also holds the check; its first svg is the agent.
      expect(got, key).toEqual(want)
      expect(got.size, key).toEqual([String(COUNTER_ICON_PX), String(COUNTER_ICON_PX)])
      expect(got.align, key).toBe('-0.2em')
      expect(got.gap, key).toBe(`${COUNTER_ICON_GAP_PX}px`)
      expect(icon.querySelectorAll('svg').length, key).toBe(1)
    }
    // Different keys draw different icons.
    const inks = new Set(keys.map((k) => iconOf(el(`[data-zen-drawer-detail-icon="${k}"]`)).ink))
    expect(inks.size).toBe(keys.length)
  })

  it('the words and the " · " separators are unchanged', async () => {
    localStorage.setItem(ZEN_DRAWER_EXPANDED_KEY, '1')
    render(<ZenActivityDrawer turn={ALL} />)
    expect(el('[data-zen-drawer]').getAttribute('data-expanded')).toBe('true')
    expect(el('[data-zen-drawer-tally]').textContent).toBe('Read 3 files · searched 1 time · ran 2 commands · edited 1 file')
    expect(el('[data-zen-drawer-children]').textContent).toBe('2 subagents running, 1 done · 1 background task')
    // The first bit has no separator; each later one starts with it.
    const bits = Array.from(document.querySelectorAll('[data-zen-drawer-tally] [data-zen-drawer-detail-bit]'))
    expect(bits.map((b) => b.textContent)).toEqual(['Read 3 files', ' · searched 1 time', ' · ran 2 commands', ' · edited 1 file'])
  })

  it('only the bits there are: one tally bit, no children row', async () => {
    localStorage.setItem(ZEN_DRAWER_EXPANDED_KEY, '1')
    render(<ZenActivityDrawer turn={turn({ tally: { cmd: 1 } })} />)
    expect(Array.from(document.querySelectorAll('[data-zen-drawer-detail-icon]')).map((i) => i.getAttribute('data-zen-drawer-detail-icon'))).toEqual(['cmd'])
    expect(el('[data-zen-drawer-tally]').textContent).toBe('Ran 1 command')
    expect(document.querySelector('[data-zen-drawer-children]')).toBeNull()
  })
})
