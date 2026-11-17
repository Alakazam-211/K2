// @vitest-environment jsdom
//
// The Home and Agents navs never dim a row's text for being unselected
// (Rosson 2026-11-17: "Can we not dim the text? The selector and background
// highlight already do the work of showing what's active. Same with hover:
// the background highlights."). Every nav row (agents, people, channels,
// Grok Bots) paints through AgentRowButton, so the rule lives there and in
// globals.css:
//   - the name reads in the primary text colour, resting, hovered or
//     selected;
//   - the second line's quiet text (the number, an action word, a note)
//     reads in the secondary colour in every state: the muted tier steps up
//     on the row at rest too, not only once it is lit;
//   - hover and selection change the background (and the accent bar), never
//     the text;
//   - a row whose server can't be reached still fades, through its own
//     `dimmed` flag.
// Colours are resolved the way the browser does it here: the nearest text
// class up the tree (a hover: class only on a hovered row), then the
// globals.css rule that steps muted to secondary, then each shipped look's
// palette. jsdom has no pointer, so :hover reads as a [data-test-hover] mark.

import { afterEach, describe, expect, it, vi } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { cleanup, render } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => ({})),
  daemonCliPost: vi.fn(async () => ({})),
  withHostCliSlot: async <T,>(_scope: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => undefined),
}))
vi.mock('@/lib/context-menu', () => ({ showContextMenu: vi.fn(async () => null) }))

import { AgentRowButton } from './Sidebar'

const dir = dirname(fileURLToPath(import.meta.url))
const repo = join(dir, '..', '..', '..', '..')
const strip = (css: string): string => css.replace(/\/\*[\s\S]*?\*\//g, ' ')
const globals = strip(readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8'))

afterEach(() => {
  cleanup()
  document.body.innerHTML = ''
})

/** The selector of the one rule that steps muted text up to secondary. */
function mutedStepSelector(): string {
  const body = '{\n  --color-text-muted: var(--color-text-secondary);\n}'
  const at = globals.indexOf(body)
  expect(at, 'the muted-to-secondary rule').toBeGreaterThanOrEqual(0)
  expect(globals.indexOf(body, at + 1), 'only one muted-to-secondary rule').toBe(-1)
  return globals.slice(globals.lastIndexOf('}', at) + 1, at).trim().replaceAll(':hover', '[data-test-hover]')
}

const LOOKS = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
] as const

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

type Tier = 'primary' | 'secondary' | 'muted'
const TEXT_CLASS = /^(hover:)?text-\[var\(--color-text-(primary|secondary|muted)\)\]$/

/** The text tier `el` paints in: its nearest text class up the tree (a
 *  hover: class counts only on a hovered element), muted stepped up where
 *  the globals.css rule reaches. */
function tierOf(el: Element): Tier {
  const step = mutedStepSelector()
  let tier: Tier | null = null
  for (let n: Element | null = el; n && !tier; n = n.parentElement) {
    const hovered = n.closest('[data-test-hover]') !== null
    let plain: Tier | null = null
    let onHover: Tier | null = null
    for (const c of Array.from(n.classList)) {
      const m = TEXT_CLASS.exec(c)
      if (!m) continue
      if (m[1]) onHover = m[2] as Tier
      else plain = m[2] as Tier
    }
    tier = (hovered && onHover) || plain
  }
  if (!tier) throw new Error('no text colour class up the tree')
  if (tier === 'muted') {
    for (let n: Element | null = el; n; n = n.parentElement) if (n.matches(step)) return 'secondary'
  }
  return tier
}

/** The colour `el`'s text computes to in each shipped look. */
function colours(el: Element): string[] {
  const t = tierOf(el)
  return LOOKS.map(([style, id]) => `${style}/${id} ${palette(style, id)[`text-${t}`]}`)
}

type State = 'rest' | 'hover' | 'selected'

function row(state: State, dimmed = false): HTMLElement {
  const { container } = render(
    <AgentRowButton
      isActive={state === 'selected'}
      color="#0f766e"
      avatar={<span />}
      name="Harbor"
      subline={
        <>
          <span data-t="note" className="text-[10px] text-[var(--color-text-muted)]">No chat on this server</span>
          <span data-t="number" className="font-mono text-[10px] text-[var(--color-text-muted)]">3</span>
        </>
      }
      onClick={() => undefined}
      onContextMenu={() => undefined}
      dimmed={dimmed}
    />,
  )
  const btn = container.querySelector('[data-agent-row]') as HTMLElement
  if (state === 'hover') btn.setAttribute('data-test-hover', '')
  return btn
}

const nameOf = (btn: HTMLElement): Element => Array.from(btn.querySelectorAll('span')).find((s) => s.textContent === 'Harbor')!

describe('nav rows never dim their text for being unselected', () => {
  it('the name computes to the same colour resting, hovered and selected, in every look: the primary text', () => {
    const got = (['rest', 'hover', 'selected'] as const).map((s) => {
      const btn = row(s)
      const c = colours(nameOf(btn))
      cleanup()
      return c
    })
    expect(got[0]).toEqual(got[2])
    expect(got[1]).toEqual(got[2])
    expect(tierOf(nameOf(row('rest')))).toBe('primary')
  })

  it('the second line computes to the same colour resting, hovered and selected: the secondary text', () => {
    for (const t of ['note', 'number']) {
      const got = (['rest', 'hover', 'selected'] as const).map((s) => {
        const btn = row(s)
        const c = colours(btn.querySelector(`[data-t="${t}"]`)!)
        cleanup()
        return c
      })
      expect(got[0], t).toEqual(got[2])
      expect(got[1], t).toEqual(got[2])
      expect(tierOf(row('rest').querySelector(`[data-t="${t}"]`)!), t).toBe('secondary')
      cleanup()
    }
  })

  it('hover changes only the background: no text class turns on under the pointer', () => {
    const btn = row('rest')
    const hoverClasses = Array.from(btn.classList).filter((c) => c.startsWith('hover:'))
    expect(hoverClasses.length).toBeGreaterThan(0)
    for (const c of hoverClasses) expect(c, c).toMatch(/^hover:bg-/)
  })

  it('selection changes only the background and the accent bar, never a text class', () => {
    const rest = new Set(Array.from(row('rest').classList))
    cleanup()
    const sel = row('selected')
    const added = Array.from(sel.classList).filter((c) => !rest.has(c))
    for (const c of added) expect(c, c).toMatch(/^bg-/)
    expect(sel.style.borderLeft).toContain('2px solid')
  })

  it('the muted step reaches a resting nav row but not a resting row elsewhere, or a nav row in a Garden', () => {
    const step = mutedStepSelector()
    expect(row('rest').matches(step)).toBe(true)
    cleanup()
    document.body.innerHTML = '<button data-t data-agent-row></button><div data-zen-root><button data-g data-agent-row data-nav-agent-row></button></div>'
    expect(document.querySelector('[data-t]')!.matches(step)).toBe(false)
    expect(document.querySelector('[data-g]')!.matches(step)).toBe(false)
  })
})

describe('states that still look different, through their own rules', () => {
  it('a row whose server is unreachable still fades', () => {
    expect(row('rest', true).className).toMatch(/\bopacity-60\b/)
    cleanup()
    expect(row('selected', true).className).toMatch(/\bopacity-60\b/)
    cleanup()
    expect(row('rest').className).not.toMatch(/\bopacity-/)
    cleanup()
    expect(row('selected').className).not.toMatch(/\bopacity-/)
  })
})
