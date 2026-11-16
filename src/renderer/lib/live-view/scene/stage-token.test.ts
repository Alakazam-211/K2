// The stage colour: one Style token, `--color-stage`, the dark surface the
// live 3D picture sits on in every look (Glass #24252a, Terminal #1a1a1a;
// live-scene-v2-notes.md, v2.4b). Every palette carries it, the build
// requires it, the Garden shield covers it, and the stage reads it.

import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { ZEN_STYLE_SHIELD } from '@/lib/zen/zen-style-shield'
import { STAGE_CSS } from '@/components/LiveView/scene/stageCss'

const REPO = join(__dirname, '..', '..', '..', '..', '..')
const STYLES = join(REPO, 'styles')

const SHIPPED: Record<string, Record<string, string>> = {
  glass: { obsidian: '#24252a', veil: '#24252a', yellow: '#24252a' },
  terminal: { charcoal: '#1a1a1a', paper: '#1a1a1a', yellow: '#1a1a1a' },
}

const lum = (hex: string): number => {
  const m = /^#([0-9a-f]{6})$/i.exec(hex)
  expect(m, hex).not.toBeNull()
  const c = [0, 2, 4].map((i) => parseInt(m![1].slice(i, i + 2), 16) / 255).map((v) => (v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4))
  return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

const stageOf = (style: string, file: string): string | undefined => JSON.parse(readFileSync(join(STYLES, style, 'palettes', file), 'utf8')).color?.stage?.$value

describe('the stage token', () => {
  it('is in every palette of every style, dark', () => {
    const styles = readdirSync(STYLES, { withFileTypes: true }).filter((d) => d.isDirectory())
    expect(styles.length).toBeGreaterThanOrEqual(2)
    for (const s of styles)
      for (const f of readdirSync(join(STYLES, s.name, 'palettes'))) {
        const v = stageOf(s.name, f)
        expect(v, `${s.name}/${f}`).toBeDefined()
        expect(lum(v!), `${s.name}/${f}`).toBeLessThan(0.03)
      }
  })

  it('is #24252a in every Glass palette and #1a1a1a in every Terminal one, light looks too', () => {
    for (const [style, pals] of Object.entries(SHIPPED)) for (const [p, want] of Object.entries(pals)) expect(stageOf(style, `${p}.json`), `${style}/${p}`).toBe(want)
  })

  it('is required by the build and generated for every palette', () => {
    expect(readFileSync(join(REPO, 'scripts', 'build-styles.mjs'), 'utf8')).toMatch(/REQUIRED_COLOR_SLOTS = \[[\s\S]*'stage'[\s\S]*\]/)
    const css = readFileSync(join(REPO, 'src', 'renderer', 'styles.generated.css'), 'utf8')
    for (const [style, pals] of Object.entries(SHIPPED))
      for (const [p, want] of Object.entries(pals)) {
        const head = `[data-style="${style}"][data-palette="${p}"] {`
        const at = css.indexOf(head)
        expect(at, head).toBeGreaterThanOrEqual(0)
        expect(css.slice(at, css.indexOf('}', at)), head).toContain(`--color-stage: ${want};`)
      }
  })

  it('is covered by the Garden shield', () => {
    expect(ZEN_STYLE_SHIELD['--color-stage']).toBeDefined()
  })

  it('is what the stage paints, in both styles', () => {
    for (const style of ['glass', 'terminal']) {
      const rule = new RegExp(`\\.lv-sstage\\[data-style="${style}"\\] \\{[^}]*--lv-stage: var\\(--color-stage\\);`)
      expect(STAGE_CSS, style).toMatch(rule)
    }
  })
})
