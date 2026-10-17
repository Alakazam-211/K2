// @vitest-environment jsdom
// Rosson 2026-10-16: "As I type when I message the agent on the Thread, as
// soon as I go to a second line, the dog ear's background changes to match
// the text area. It should stay the same colour it was." Then, a later try:
// the change came on the line where the box stacks instead.
//
// The ear (the activity drawer's tab on the message box) had a hover fill.
// The box grows upward and pushes the ear up, so a pointer resting on the
// ear ends up on the field after a line or two (or after the stacking jump):
// the ear lost its hover fill and took the field's fill. How many lines it
// took depended only on where the pointer rested.
//
// The check: in every composer state (1 line, 2 lines, stacked, back to 1),
// with the pointer on the ear or away from it, the rules that paint the
// ear's fill are the same, and so are its own style and attributes.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { coerceThreadTurn, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'
import {
  THREAD_ZEN_DESKTOP_TOKENS,
  ThreadComposeBox,
  ThreadComposeStackSpacer,
  ThreadZenStyles,
  threadComposeTextSlotStyle,
} from './ThreadZenKit'

const T0 = Date.now()

function workingTurn(): ThreadTurn {
  const t = coerceThreadTurn(
    {
      turnId: 'ear-1',
      startedAt: T0 - 5_000,
      state: 'working',
      phase: 'tool',
      line: 'Running `cargo test`',
      subagents: 1,
      background: 0,
      tally: { read: 2, cmd: 1 },
      end: null,
      rev: 1,
      serverNow: T0,
    },
    T0,
  )
  if (!t) throw new Error('fixture is not a turn')
  return t
}

const TURN = workingTurn()

interface ComposerState {
  name: string
  text: string
  rows: number
  stacked: boolean
}

const STATES: ComposerState[] = [
  { name: 'one line', text: 'hello', rows: 1, stacked: false },
  { name: 'two lines', text: 'hello\nthere', rows: 2, stacked: false },
  { name: 'stacked', text: 'one\ntwo\nthree\nfour', rows: 4, stacked: true },
  { name: 'back to one line', text: 'hi', rows: 1, stacked: false },
]

function Composer({ state }: { state: ComposerState }): React.JSX.Element {
  return (
    <div data-k2-thread-zen="" style={THREAD_ZEN_DESKTOP_TOKENS}>
      <ThreadZenStyles />
      <div data-zen-compose="" data-compose-bar="">
        <ThreadComposeBox stacked={state.stacked} drawer={<ZenActivityDrawer turn={TURN} />}>
          <div style={threadComposeTextSlotStyle(state.stacked)}>
            <textarea data-zen-compose-input="" rows={state.rows} value={state.text} readOnly />
          </div>
          <ThreadComposeStackSpacer stacked={state.stacked} />
        </ThreadComposeBox>
      </div>
    </div>
  )
}

const FILL_PROPS = ['background', 'background-color', '--ch-fill', '--ch-edge'] as const

/** Every style rule in the document's sheets, through @media / @supports. */
function styleRules(): CSSStyleRule[] {
  const out: CSSStyleRule[] = []
  const walk = (rules: CSSRuleList): void => {
    for (const r of Array.from(rules)) {
      if ('selectorText' in r && typeof (r as CSSStyleRule).selectorText === 'string') out.push(r as CSSStyleRule)
      else if ('cssRules' in r && (r as CSSGroupingRule).cssRules) walk((r as CSSGroupingRule).cssRules)
    }
  }
  for (const sheet of Array.from(document.styleSheets)) walk(sheet.cssRules)
  return out
}

/** Top-level commas only (not the ones inside :is(...) / :not(...)). */
function splitSelectors(list: string): string[] {
  const out: string[] = []
  let depth = 0
  let cur = ''
  for (const ch of list) {
    if (ch === '(') depth++
    if (ch === ')') depth--
    if (ch === ',' && depth === 0) {
      out.push(cur.trim())
      cur = ''
    } else cur += ch
  }
  if (cur.trim()) out.push(cur.trim())
  return out
}

/** The fill declarations that reach `el`, with the pointer resting on it
 *  (`:hover` holds) or away from it. Pseudo-element rules are left out
 *  (`::before` / `::after` read the element's `--ch-*`, which is counted). */
function fillRules(el: Element, pointerOn: boolean): string[] {
  const hits: string[] = []
  for (const rule of styleRules()) {
    for (const sel of splitSelectors(rule.selectorText)) {
      if (sel.includes('::')) continue
      const hovers = sel.includes(':hover')
      if (hovers && !pointerOn) continue
      const test = sel.replace(/:hover/g, '')
      let matches: boolean
      try {
        matches = el.matches(test)
      } catch (err) {
        throw new Error(`selector the test can't match: ${test} (${String(err)})`)
      }
      if (!matches) continue
      for (const prop of FILL_PROPS) {
        const value = rule.style.getPropertyValue(prop)
        if (value) hits.push(`${sel} { ${prop}: ${value}${rule.style.getPropertyPriority(prop) ? ' !important' : ''} }`)
      }
    }
  }
  return [...new Set(hits)].sort()
}

/** The ear's own look: inline style, classes and attributes. */
function earOwn(tile: Element): Record<string, string> {
  const own: Record<string, string> = { class: tile.className }
  for (const a of Array.from(tile.attributes)) own[a.name] = a.value
  return own
}

function earTile(container: HTMLElement): Element {
  const tile = container.querySelector('[data-zen-drawer-tile]')
  if (!tile) throw new Error('the ear did not render')
  return tile
}

afterEach(() => {
  cleanup()
  document.documentElement.removeAttribute('data-style')
  document.documentElement.removeAttribute('data-scheme')
})

describe('the activity ear keeps its fill while the message box changes', () => {
  for (const [style, scheme] of [
    ['square', 'dark'],
    ['square', 'light'],
    ['bezel', 'dark'],
    ['glass', 'light'],
  ] as const) {
    it(`${style} / ${scheme}: 1 line → 2 lines → stacked → 1 line, pointer on or off the ear`, () => {
      document.documentElement.setAttribute('data-style', style)
      document.documentElement.setAttribute('data-scheme', scheme)
      const { container, rerender } = render(<Composer state={STATES[0]} />)
      const tile = earTile(container)
      const seen: Array<{ state: string; own: Record<string, string>; pointerOn: string[]; pointerOff: string[] }> = []
      for (const state of STATES) {
        rerender(<Composer state={state} />)
        // Same element throughout: the box's states never remount the ear.
        expect(earTile(container)).toBe(tile)
        const box = container.querySelector('[data-zen-compose-box]')
        expect(box?.getAttribute('data-stacked') ?? null).toBe(state.stacked ? 'true' : null)
        seen.push({ state: state.name, own: earOwn(tile), pointerOn: fillRules(tile, true), pointerOff: fillRules(tile, false) })
      }
      // Something does paint the ear (the check isn't vacuous).
      expect(seen[0].pointerOff.length).toBeGreaterThan(0)
      for (const s of seen) {
        expect({ state: s.state, own: s.own }).toEqual({ state: s.state, own: seen[0].own })
        // A pointer left where the ear was must not recolour it when the
        // box pushes the ear away (or back under it).
        expect({ state: s.state, fill: s.pointerOn }).toEqual({ state: s.state, fill: seen[0].pointerOff })
        expect({ state: s.state, fill: s.pointerOff }).toEqual({ state: s.state, fill: seen[0].pointerOff })
      }
    })
  }

  it('a free tile (no message box under it) keeps its hover fill', () => {
    document.documentElement.setAttribute('data-style', 'square')
    const { container } = render(
      <div data-k2-thread-zen="" style={THREAD_ZEN_DESKTOP_TOKENS}>
        <ThreadZenStyles />
        <ZenActivityDrawer turn={TURN} attached={false} />
      </div>,
    )
    const tile = earTile(container)
    const added = fillRules(tile, true).filter((r) => !fillRules(tile, false).includes(r))
    expect(added).toEqual([
      ':is([data-zen-root], [data-k2-thread-zen]) [data-zen-drawer-attached="false"] [data-zen-drawer-tile]:hover { background: var(--zen-surface) }',
      'html[data-style="square"] [data-k2-thread-zen] [data-zen-drawer-attached="false"] [data-zen-drawer-tile][data-chamfer]:hover { --ch-fill: var(--zen-surface) !important }',
    ])
  })

  it('the box itself does change when it stacks (the toggle is real)', () => {
    const { container, rerender } = render(<Composer state={STATES[0]} />)
    const box = container.querySelector('[data-zen-compose-box]') as HTMLElement
    const before = box.getAttribute('style')
    rerender(<Composer state={STATES[2]} />)
    expect(box.getAttribute('data-stacked')).toBe('true')
    expect(box.getAttribute('style')).not.toBe(before)
  })
})
