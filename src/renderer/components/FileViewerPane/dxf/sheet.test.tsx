// @vitest-environment jsdom
//
// The sheet a drawing sits on is always dark, as CAD apps draw model space:
// file colours stay true, so yellow (colour 2) text all but vanishes on a
// light look's white sheet. The light looks paint the drawing area with the
// --color-cad-sheet token; the dark looks keep their own background (their
// token is their background), so nothing changes there. Colour 7 draws white
// on every look, on the model path and on the viewer's own reading alike.
// Only the drawing area changes: the header and the stats line keep the look,
// and there is no switch.

import { readFileSync, readdirSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

import { ZEN_STYLE_SHIELD } from '@/lib/zen/zen-style-shield'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { DxfViewer } from '../DxfViewer'
import { fixtureDrawingSource } from './model/source'
import type { ModelHeader, ModelPage } from './model/types'

const REPO = resolve(__dirname, '..', '..', '..', '..', '..')
const STYLES = join(REPO, 'styles')
const FIX = resolve(__dirname, '../__fixtures__')
const PLAN = readFileSync(join(FIX, 'dxf-change-plan.dxf'), 'utf8')
const json = <T,>(n: string): T => JSON.parse(readFileSync(join(FIX, 'drawing-model', `${n}.json`), 'utf8')) as T

/** What each shipped look's drawing area paints. */
const SHIPPED: Record<string, Record<string, string>> = {
  terminal: { paper: '#1a1a1a', charcoal: 'var(--color-bg)', yellow: 'var(--color-bg)' },
  glass: { veil: '#24252a', obsidian: 'var(--color-bg)', yellow: 'var(--color-bg)' },
}

const lum = (hex: string): number => {
  const m = /^#([0-9a-f]{6})$/i.exec(hex)
  expect(m, hex).not.toBeNull()
  const c = [0, 2, 4].map((i) => parseInt(m![1].slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4))
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}
const ratio = (a: number, b: number): number => (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)
const palette = (style: string, file: string) => JSON.parse(readFileSync(join(STYLES, style, 'palettes', file), 'utf8')) as { schemes: string[]; color: Record<string, { $value: string }> }

afterEach(() => cleanup())

describe('the cad-sheet token', () => {
  it('light palettes: a dark sheet where yellow and white both read; dark palettes: their own background', () => {
    for (const s of readdirSync(STYLES, { withFileTypes: true }).filter((d) => d.isDirectory()))
      for (const f of readdirSync(join(STYLES, s.name, 'palettes'))) {
        const p = palette(s.name, f)
        const v = p.color['cad-sheet']?.$value
        expect(v, `${s.name}/${f}`).toBeDefined()
        if (p.schemes.includes('light')) {
          expect(ratio(lum(v), lum('#ffff00')), `${s.name}/${f}`).toBeGreaterThanOrEqual(7)
          expect(ratio(lum(v), lum('#ffffff')), `${s.name}/${f}`).toBeGreaterThanOrEqual(7)
        } else expect(v, `${s.name}/${f}`).toBe('var(--color-bg)')
      }
  })

  it('per look: Terminal light #1a1a1a, Glass Veil #24252a, the dark looks their background', () => {
    for (const [style, pals] of Object.entries(SHIPPED)) for (const [p, want] of Object.entries(pals)) expect(palette(style, `${p}.json`).color['cad-sheet'].$value, `${style}/${p}`).toBe(want)
  })

  it('is required by the build, generated for every palette, and covered by the Garden shield', () => {
    expect(readFileSync(join(REPO, 'scripts', 'build-styles.mjs'), 'utf8')).toMatch(/REQUIRED_COLOR_SLOTS = \[[\s\S]*'cad-sheet'[\s\S]*\]/)
    const css = readFileSync(join(REPO, 'src', 'renderer', 'styles.generated.css'), 'utf8')
    for (const [style, pals] of Object.entries(SHIPPED))
      for (const [p, want] of Object.entries(pals)) {
        const head = `[data-style="${style}"][data-palette="${p}"] {`
        const at = css.indexOf(head)
        expect(at, head).toBeGreaterThanOrEqual(0)
        expect(css.slice(at, css.indexOf('}', at)), head).toContain(`--color-cad-sheet: ${want};`)
      }
    expect(ZEN_STYLE_SHIELD['--color-cad-sheet']).toBeDefined()
  })
})

/** The colour `currentColor` resolves to for an element: the nearest ancestor that sets one. */
function inkOf(el: Element): { by: Element; color: string } {
  for (let e: Element | null = el; e; e = e.parentElement) {
    const c = (e as HTMLElement).style?.color
    if (c) return { by: e, color: c }
  }
  throw new Error('no colour set')
}

describe('the drawing area', () => {
  it('paints the sheet token, with no switch; the header and stats line stay off the sheet', async () => {
    renderInPrimaryRoom(<DxfViewer text={PLAN} filePath="/ws/plan.dxf" model={null} changes={null} />)
    await screen.findByTestId('dxf-shapes')
    const sheet = screen.getByTestId('dxf-sheet')
    expect(sheet.style.background).toBe('var(--dxf-sheet)')
    expect(sheet.style.getPropertyValue('--dxf-sheet')).toBe('var(--color-cad-sheet)')
    expect(screen.queryByRole('switch')).toBeNull()
    expect(screen.queryByText(/dark sheet/i)).toBeNull()
    expect(screen.getByTestId('dxf-stats').closest('[data-testid="dxf-sheet"]')).toBeNull()
    expect(screen.getByTestId('dxf-fit').closest('[data-testid="dxf-sheet"]')).toBeNull()
  })

  it('the viewer’s own reading: colour 7 (layer and entity) draws white', async () => {
    renderInPrimaryRoom(<DxfViewer text={PLAN} filePath="/ws/plan.dxf" model={null} changes={null} />)
    const shapes = await screen.findByTestId('dxf-shapes')
    expect(shapes.getAttribute('data-source')).toBeNull()
    // The walls: layer A-WALL, colour 7 in the file.
    const wall = shapes.querySelector('line')!
    expect(wall.getAttribute('stroke')).toBe('currentColor')
    const ink = inkOf(wall)
    expect(ink.by).toBe(screen.getByTestId('dxf-sheet'))
    expect(ink.color).toBe('white')
  })

  it('the drawing model: a colour 7 row draws white', async () => {
    const model = fixtureDrawingSource({
      header: json<ModelHeader>('sampler-header'),
      pages: { model: [json<ModelPage>('sampler-page-model')], '@h:2D': [json<ModelPage>('sampler-page-door')], '@h:2E': [json<ModelPage>('sampler-page-nest')], '@h:2F': [json<ModelPage>('sampler-page-dimension')] },
    })
    renderInPrimaryRoom(<DxfViewer text={PLAN} filePath="/ws/sampler.dxf" model={model} changes={null} />)
    const shapes = await screen.findByTestId('dxf-shapes')
    expect(shapes.getAttribute('data-source')).toBe('model')
    const dot = shapes.querySelector('[data-h="@h:47"]')!
    expect(dot.getAttribute('stroke')).toBe('currentColor')
    const ink = inkOf(dot)
    expect(ink.by).toBe(screen.getByTestId('dxf-sheet'))
    expect(ink.color).toBe('white')
  })
})
