// @vitest-environment jsdom
// The message box's hint ("Message Ana Ruiz", the agent Thread's own) is
// text the person must read to know what the box is for, so it reads at
// 4.5:1 on the box's fill in every look (WCAG 1.4.3). It is one token,
// --zen-compose-hint: the desktop Thread maps it to the Style's secondary
// text; a Garden, which sets no such token, keeps its own --zen-text-muted.
// The box's fill is the Thread's raised surface (the elevated fill where a
// Style has no thread-raised slot), and Glass's tile fill (the surface).
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { THREAD_ZEN_CSS, THREAD_ZEN_DESKTOP_TOKENS } from './ThreadZenKit'
import { ThreadChatComposer } from './ThreadChat'

const repo = join(__dirname, '..', '..', '..', '..')

const LOOKS = [
  ['terminal', 'charcoal'],
  ['terminal', 'paper'],
  ['glass', 'obsidian'],
  ['glass', 'veil'],
] as const

type Rgb = [number, number, number]

function palette(style: string, id: string): Record<string, string> {
  const json = JSON.parse(readFileSync(join(repo, 'styles', style, 'palettes', `${id}.json`), 'utf8'))
  return Object.fromEntries(Object.entries(json.color as Record<string, { $value: string }>).map(([k, v]) => [k, v.$value]))
}

/** A slot's colour, following `var(--color-…)` to the slot it names. */
function slot(p: Record<string, string>, name: string): string {
  const v = p[name]
  if (v === undefined) throw new Error(`no slot ${name}`)
  const ref = /^var\(--color-([a-z0-9-]+)\)$/.exec(v)
  return ref ? slot(p, ref[1]) : v
}

function hex(value: string): Rgb {
  const m = /^#([0-9a-f]{6})$/i.exec(value)
  if (!m) throw new Error(`not a #rrggbb colour: ${value}`)
  return [0, 2, 4].map((i) => parseInt(m[1].slice(i, i + 2), 16)) as Rgb
}

function luminance([r, g, b]: Rgb): number {
  const lin = (v: number): number => {
    const s = v / 255
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4
  }
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

function contrast(a: Rgb, b: Rgb): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x)
  return (hi + 0.05) / (lo + 0.05)
}

const tokens = THREAD_ZEN_DESKTOP_TOKENS as Record<string, string>

afterEach(cleanup)

describe("the message box's hint", () => {
  it('the desktop box maps its hint to the Style secondary text', () => {
    expect(tokens['--zen-compose-hint']).toBe('var(--color-text-secondary)')
  })

  it.each(LOOKS)('reads 4.5:1 on the box fill (%s %s)', (style, id) => {
    const p = palette(style, id)
    const hint = hex(slot(p, 'text-secondary'))
    const fills = style === 'glass' ? [slot(p, 'bg-surface'), slot(p, 'thread-raised')] : [slot(p, 'bg-elevated')]
    for (const fill of fills) expect(contrast(hint, hex(fill))).toBeGreaterThanOrEqual(4.5)
  })

  it("the drawn hint and the field's own placeholder take that token, falling back to a Garden's muted text", () => {
    const { container } = render(<ThreadChatComposer value="" onChange={() => {}} onSend={() => {}} placeholder="Message Ana Ruiz" />)
    const hint = container.querySelector<HTMLElement>('[data-compose-hint]')!
    expect(hint.style.color).toBe('var(--zen-compose-hint, var(--zen-text-muted))')
    expect(THREAD_ZEN_CSS).toContain('[data-zen-compose-input]::placeholder { color: var(--zen-compose-hint, var(--zen-text-muted)); opacity: 1; }')
  })
})
