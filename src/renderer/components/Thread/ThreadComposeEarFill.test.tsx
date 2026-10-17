// @vitest-environment jsdom
// Rosson 2026-10-16: "As I type when I message the agent on the Thread, as
// soon as I go to a second line, the dog ear's background changes to match
// the text area. It should stay the same colour it was." Then, a later try:
// the change came on the line where the box stacks instead.
//
// The ear (the activity drawer's tab on the message box) had a hover fill,
// the darker surface. The box grows upward and pushes the ear up, so a
// pointer resting on the ear ended up on the field after a line or two (or
// after the stacking jump): the ear lost the darker shade and took the
// field's fill. Rosson chose the darker shade: the ear is always that shade
// (`--zen-surface`), with no hover change, on the box or as a free tile.
//
// The check: in every composer state (1 line, 2 lines, stacked, back to 1),
// with the pointer on the ear or away from it, the ear paints
// `var(--zen-surface)`: its own style says so and no stylesheet rule
// (hover, the field's glass wash, a stacked rule) overrides its fill. The
// square Style's chamfer clears the element's own background and paints
// the shape from `--ch-fill` instead; that rule is the only one allowed.
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { coerceThreadTurn, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { ZEN_DRAWER_EAR_FILL, ZenActivityDrawer } from '@/components/Zen/widgets/ZenActivityDrawer'
import { CHAMFER_SCOPE } from './chamfer'
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

/** The square Style's chamfer clears the element's own background (its
 *  `::after` paints `--ch-fill`); no other rule may touch the ear's fill. */
function allowedRules(style: string): string[] {
  if (style !== 'square') return []
  // The CSSOM reports the shorthand and its longhand.
  return [
    `${CHAMFER_SCOPE} [data-chamfer] { background-color: transparent !important }`,
    `${CHAMFER_SCOPE} [data-chamfer] { background: transparent !important }`,
  ].sort()
}

/** What the ear paints: `--ch-fill` under the square Style's chamfer, its
 *  own background otherwise. Both are the darker surface. */
function expectDarkerShade(tile: Element, style: string): void {
  const own = (tile as HTMLElement).style
  expect(ZEN_DRAWER_EAR_FILL).toBe('var(--zen-surface)')
  expect(own.getPropertyValue('--ch-fill')).toBe('var(--zen-surface)')
  expect(own.background).toBe('var(--zen-surface)')
  const painted = style === 'square' ? own.getPropertyValue('--ch-fill') : own.background
  expect(painted).toBe('var(--zen-surface)')
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
      for (const s of seen) {
        expect({ state: s.state, own: s.own }).toEqual({ state: s.state, own: seen[0].own })
        // A pointer left where the ear was must not recolour it when the
        // box pushes the ear away (or back under it).
        expect({ state: s.state, fill: s.pointerOn }).toEqual({ state: s.state, fill: allowedRules(style) })
        expect({ state: s.state, fill: s.pointerOff }).toEqual({ state: s.state, fill: allowedRules(style) })
      }
      expectDarkerShade(tile, style)
    })
  }

  it('a free tile (no message box under it) is the darker shade too, no hover change', () => {
    document.documentElement.setAttribute('data-style', 'square')
    const { container } = render(
      <div data-k2-thread-zen="" style={THREAD_ZEN_DESKTOP_TOKENS}>
        <ThreadZenStyles />
        <ZenActivityDrawer turn={TURN} attached={false} />
      </div>,
    )
    const tile = earTile(container)
    expect(fillRules(tile, true)).toEqual(allowedRules('square'))
    expect(fillRules(tile, false)).toEqual(allowedRules('square'))
    expectDarkerShade(tile, 'square')
  })

  it('Stop keeps the raised fill, so it still reads as a button on the ear', () => {
    const turn = TURN
    const { container } = render(
      <div data-k2-thread-zen="" style={THREAD_ZEN_DESKTOP_TOKENS}>
        <ThreadZenStyles />
        <ZenActivityDrawer turn={turn} onStop={() => {}} />
      </div>,
    )
    const stop = container.querySelector('[data-zen-drawer-stop]') as HTMLElement | null
    if (!stop) throw new Error('Stop did not render')
    expect(stop.style.background).toBe('var(--zen-surface-raised)')
    expect(stop.style.getPropertyValue('--ch-fill')).toBe('var(--zen-surface-raised)')
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
