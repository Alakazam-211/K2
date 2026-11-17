// @vitest-environment jsdom
// The Thread's message box (the agent Thread and Home chat) is a control on
// the page, so its edge must read at 3:1 against the page in every look
// (WCAG 1.4.11). The page under it is --color-bg (the scope's --zen-canvas).
// The edge is one token, --zen-compose-edge: the desktop box maps it to the
// Style's control edge, Glass's tile rule draws the same token, and a Garden,
// which sets no such token, keeps its own --zen-border.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { THREAD_ZEN_DESKTOP_TOKENS, ThreadComposeBox } from './ThreadZenKit'

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

describe('the message box edge', () => {
  it('the desktop box maps its edge to the Style control edge', () => {
    expect(tokens['--zen-compose-edge']).toBe('var(--color-control-edge, var(--color-border))')
    expect(tokens['--zen-canvas']).toBe('var(--color-bg)')
  })

  it.each(LOOKS)('the control edge reads 3:1 on the page (%s %s)', (style, id) => {
    const p = palette(style, id)
    expect(contrast(hex(p['control-edge']), hex(p.bg))).toBeGreaterThanOrEqual(3)
  })

  it('the box draws that token, falling back to a Garden\'s own border', () => {
    const { container } = render(
      <ThreadComposeBox>
        <textarea />
      </ThreadComposeBox>,
    )
    const box = container.querySelector<HTMLElement>('[data-zen-compose-box]')!
    expect(box.style.border).toContain('var(--zen-compose-edge, var(--zen-border))')
    // Terminal's cut layers paint the edge from --ch-edge.
    expect(box.style.getPropertyValue('--ch-edge')).toBe('var(--zen-compose-edge, var(--zen-border))')
  })

  it("Glass's tile rule for the box draws the same token, not the tile edge", () => {
    const css = readFileSync(join(repo, 'src/renderer/globals.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, ' ')
    const sel = 'html[data-style="glass"] [data-k2-thread-zen] [data-zen-compose] [data-zen-compose-box]:not([data-zen-root] *) {'
    const at = css.indexOf(sel)
    expect(at).toBeGreaterThanOrEqual(0)
    const body = css.slice(at, css.indexOf('}', at))
    expect(body).toContain('border: 1px solid var(--zen-compose-edge)')
    expect(body).not.toContain('--tile-edge)')
  })
})
