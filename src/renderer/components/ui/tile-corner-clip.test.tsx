// @vitest-environment jsdom
// A rounded Glass panel keeps what's inside it inside its curve, the Thread's
// blurred top and bottom bands included:
//   - the room's body under the tab strip (and a split pane's body) is
//     rounded at the top and clips to that curve with a clip-path: under a
//     rounded overflow clip alone, a backdrop blur painted square into the
//     corners (the Thread's top band showed your blue bubbles past the
//     curve, over the strip);
//   - a Projects tile and the Project chat can't clip as a whole (the
//     chat's resize edge hangs past its side), so the part that reaches
//     their bottom edge clips to the tile's inner curve, from the tokens:
//     their square fill and fades covered the rim's curve at the bottom
//     corners, so the corners read darker (lighter in Veil).
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { afterEach, describe, expect, it } from 'vitest'
import { cleanup, render } from '@testing-library/react'
import { TILE_FOOT_CLIP } from './Surface'
import { PaneChrome } from '../Projects/PaneChrome'

afterEach(() => cleanup())

const repo = join(dirname(fileURLToPath(import.meta.url)), '..', '..', '..', '..')
const overrides = readFileSync(join(repo, 'styles/glass/overrides.css'), 'utf8')

type Rule = { selector: string; body: string }
const rules: Rule[] = Array.from(
  overrides.replace(/\/\*[\s\S]*?\*\//g, '').matchAll(/([^{}]+)\{([^}]*)\}/g),
  (m) => ({ selector: m[1].trim(), body: m[2] }),
)
const has = (selector: string, decl: RegExp): boolean => rules.some((r) => r.selector === selector && decl.test(r.body))
const classes = (s: string): string[] => s.split(/\s+/).filter(Boolean)

const TOP_CURVE = /clip-path:\s*inset\(0 round var\(--k2-body-r\) var\(--k2-body-r\) 0 0\)/

describe('a rounded Glass body clips its blurred bands to its curve', () => {
  it('the room’s body under the tab strip is rounded at the top and clip-paths to that curve', () => {
    const sel = '[data-style="glass"] [data-toast-host="workspace"]:not([data-zen-root] *)'
    expect(has(sel, /border-radius:\s*var\(--k2-body-r\) var\(--k2-body-r\) 0 0/)).toBe(true)
    expect(has(sel, TOP_CURVE)).toBe(true)
  })

  it('a split pane’s body, rounded the same way, is too', () => {
    const sel = '[data-style="glass"] [data-pane-group-id]:not([data-pane-solo]) > [data-toast-host="pane"]:not([data-zen-root] *)'
    expect(has(sel, /border-radius:\s*var\(--k2-body-r\) var\(--k2-body-r\) 0 0/)).toBe(true)
    expect(has(sel, TOP_CURVE)).toBe(true)
  })
})

describe('a tile’s foot clips to the tile’s inner curve', () => {
  it('Glass clip-paths its bottom corners to the tile’s radius less its rim', () => {
    expect(classes(TILE_FOOT_CLIP)).toEqual(['k2-tile-foot'])
    const sel = '[data-style="glass"] .k2-tile-foot:not([data-zen-root] *)'
    const inner = 'calc\\(var\\(--radius-box\\) - var\\(--tile-edge-width\\)\\)'
    expect(has(sel, /overflow:\s*hidden/)).toBe(true)
    expect(has(sel, new RegExp(`clip-path:\\s*inset\\(0 round 0 0 ${inner} ${inner}\\)`))).toBe(true)
    expect(has(sel, new RegExp(`border-bottom-left-radius:\\s*${inner}`))).toBe(true)
    expect(has(sel, new RegExp(`border-bottom-right-radius:\\s*${inner}`))).toBe(true)
  })

  it('a Projects tile’s body (under its bar) wears it; the bar does not', () => {
    render(
      <PaneChrome title="atlas" shortcutNum={1} readOnly={false} focused={false} onClose={() => {}}>
        <div data-testid="body-content" />
      </PaneChrome>,
    )
    const tile = document.querySelector('[data-tile]') as HTMLElement
    const body = document.querySelector('[data-testid="body-content"]')!.parentElement as HTMLElement
    expect(body.parentElement).toBe(tile)
    for (const c of classes(TILE_FOOT_CLIP)) expect(body.classList).toContain(c)
    const bar = tile.querySelector('[data-tile-bar]') as HTMLElement
    expect(bar.classList).not.toContain('overflow-hidden')
  })
})
