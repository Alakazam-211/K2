// @vitest-environment jsdom
//
// Retired style ids against the real boot path and the real <html>
// (prd-themes-terminal-glass-v1 §7). A saved `square` or `bezel` must never
// be stamped: not by the inline bootstrap in index.html, which runs before
// any module, and not by the store afterwards.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { getStyle } from '@/styles.generated'
import { STYLE_ID_ALIASES } from '@/lib/style-resolve'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
  key: (i: number) => Array.from(mem.keys())[i] ?? null,
  get length() {
    return mem.size
  },
})

const html = document.documentElement
const ATTRS = ['data-style', 'data-palette', 'data-scheme', 'data-gaps', 'style']
const stamped = (): Record<string, string | null> => ({
  style: html.getAttribute('data-style'),
  palette: html.getAttribute('data-palette'),
  scheme: html.getAttribute('data-scheme'),
  gaps: html.getAttribute('data-gaps'),
})

function seed(style: string, palette: string, scheme: string, gaps = ''): void {
  // Past 0.46.11's one-time switch to the default look, or that would win.
  mem.set('k2.style.migrated-0-46-11-glass', '1')
  mem.set('k2.style', style)
  mem.set('k2.palette', palette)
  mem.set(`k2.palette.${scheme}`, palette)
  mem.set('k2.scheme', scheme)
  mem.set('k2.gaps', gaps)
}

beforeEach(() => {
  mem.clear()
  for (const a of ATTRS) html.removeAttribute(a)
  vi.resetModules()
})

afterEach(() => {
  for (const a of ATTRS) html.removeAttribute(a)
})

describe('pre-paint bootstrap (index.html)', () => {
  const page = readFileSync(resolve(__dirname, '../index.html'), 'utf8')
  const script = page.match(/<script>(\s*\/\/ Stamp Style attributes[\s\S]*?)<\/script>/)
  if (!script) throw new Error('the Style bootstrap is not in index.html')
  const runBootstrap = new Function(script[1])

  it('a first boot with nothing saved is the default look, Glass on Yellow (dark system)', () => {
    runBootstrap()
    expect(stamped()).toEqual({ style: 'glass', palette: 'yellow', scheme: 'dark', gaps: null })
  })

  it('maps a saved square, keeping palette, scheme and gaps', () => {
    seed('square', 'paper', 'light', 'spacious')
    runBootstrap()
    expect(stamped()).toEqual({ style: 'terminal', palette: 'paper', scheme: 'light', gaps: 'spacious' })
  })

  it('a saved bezel never stamps bezel, its palette, or its gaps', () => {
    seed('bezel', 'graphite', 'dark', 'spacious')
    runBootstrap()
    expect(stamped()).toEqual({ style: 'glass', palette: 'yellow', scheme: 'dark', gaps: null })
    for (const a of ATTRS) html.removeAttribute(a)
    seed('bezel', 'porcelain', 'light')
    runBootstrap()
    expect(stamped()).toEqual({ style: 'glass', palette: 'veil', scheme: 'light', gaps: null })
  })

  it('a half-migrated mirror (Terminal with a Bezel palette) still paints Terminal', () => {
    seed('terminal', 'graphite', 'dark')
    runBootstrap()
    expect(stamped().palette).toBe('charcoal')
  })

  it('a saved glass-next or old Glass paints as Glass', () => {
    seed('glass-next', 'yellow', 'dark')
    runBootstrap()
    expect(stamped()).toEqual({ style: 'glass', palette: 'yellow', scheme: 'dark', gaps: null })
    mem.clear()
    seed('glass-classic', 'veil', 'light')
    runBootstrap()
    expect(stamped()).toEqual({ style: 'glass', palette: 'veil', scheme: 'light', gaps: null })
  })

  it('a palette that is on Terminal survives, and Glass is untouched', () => {
    seed('terminal', 'paper', 'light')
    runBootstrap()
    expect(stamped().palette).toBe('paper')
    mem.clear()
    seed('glass', 'obsidian', 'dark', 'spacious')
    runBootstrap()
    expect(stamped()).toEqual({ style: 'glass', palette: 'obsidian', scheme: 'dark', gaps: 'spacious' })
  })

  it('stays in step with the alias table and every palette of Terminal and Glass', () => {
    for (const [id, alias] of STYLE_ID_ALIASES) {
      mem.clear()
      seed(id, 'nonsense', 'dark')
      runBootstrap()
      expect([id, stamped().style]).toEqual([id, alias.to])
    }
    for (const styleId of ['terminal', 'glass']) {
      for (const p of getStyle(styleId)!.palettes) {
        mem.clear()
        seed(styleId, p.id, p.schemes[0])
        runBootstrap()
        expect([styleId, stamped().palette, stamped().scheme]).toEqual([styleId, p.id, p.schemes[0]])
      }
    }
  })
})

describe('store boot from a retired id', () => {
  it('a saved Porcelain reaches <html> as Veil with the gold accent, and the mirror is rewritten', async () => {
    seed('bezel', 'porcelain', 'light', 'spacious')
    const { useStyleStore } = await import('./style')
    expect(stamped()).toEqual({ style: 'glass', palette: 'veil', scheme: 'light', gaps: null })
    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(useStyleStore.getState().schemeMode).toBe('light')
    expect([mem.get('k2.style'), mem.get('k2.palette'), mem.get('k2.gaps')]).toEqual(['glass', 'veil', ''])
    expect(html.style.getPropertyValue('--color-accent')).toBe('#e6b400')
    expect(html.style.getPropertyValue('--color-accent-hover')).toBe('#c79c00')
    expect(html.style.getPropertyValue('--color-accent-soft')).toBe('#8a6d00')
    expect(html.style.getPropertyValue('--color-on-accent')).toBe('#1a1400')
  })

  it('a saved Graphite reaches <html> as Glass Yellow, with no override', async () => {
    seed('bezel', 'graphite', 'dark')
    await import('./style')
    expect(stamped()).toEqual({ style: 'glass', palette: 'yellow', scheme: 'dark', gaps: null })
    expect(html.style.getPropertyValue('--color-accent')).toBe('')
  })

  it('a saved square keeps its density and palette on Terminal', async () => {
    seed('square', 'charcoal', 'dark', 'regular')
    await import('./style')
    expect(stamped()).toEqual({ style: 'terminal', palette: 'charcoal', scheme: 'dark', gaps: 'regular' })
  })

  it('the migration runs before the first apply, so a saved Square text weight is live at boot', async () => {
    seed('square', 'charcoal', 'dark')
    mem.set('k2.textGamma.square.dark', '1.75')
    const { useStyleStore } = await import('./style')
    expect(useStyleStore.getState().textGamma).toBe(1.75)
    expect(mem.get('k2.textGamma.terminal.dark')).toBe('1.75')
    expect(mem.has('k2.textGamma.square.dark')).toBe(false)
  })

  it('a hover preview of a retired id stamps Glass', async () => {
    const { stampStyleAttributes } = await import('./style')
    stampStyleAttributes({ styleId: 'bezel', paletteId: 'graphite', schemeMode: 'dark', gapsPreset: 'spacious' })
    expect(stamped()).toEqual({ style: 'glass', palette: 'yellow', scheme: 'dark', gaps: null })
  })
})
