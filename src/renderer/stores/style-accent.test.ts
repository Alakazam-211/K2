// @vitest-environment jsdom
//
// The accent override and Auto picks, against the real store, the real <html>
// and index.html's real pre-paint bootstrap.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { beforeEach, describe, expect, it, vi } from 'vitest'

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

let osLight = false
Object.defineProperty(window, 'matchMedia', {
  configurable: true,
  value: (q: string) => ({
    matches: q.includes('light') ? osLight : !osLight,
    addEventListener: () => {},
  }),
})

const html = document.documentElement
const accentVar = (): string => html.style.getPropertyValue('--color-accent')

const page = readFileSync(resolve(__dirname, '../index.html'), 'utf8')
const script = page.match(/<script>(\s*\/\/ Stamp Style attributes[\s\S]*?)<\/script>/)
if (!script) throw new Error('the Style bootstrap is not in index.html')
const runBootstrap = new Function(script[1])

function clearHtml(): void {
  for (const a of ['data-style', 'data-palette', 'data-scheme', 'data-gaps', 'style']) html.removeAttribute(a)
}

async function freshStore() {
  vi.resetModules()
  return (await import('./style')).useStyleStore
}

/** A client that already saved Terminal / Charcoal / Dark. */
function seedTerminal(): void {
  mem.set('k2.style', 'terminal')
  mem.set('k2.palette', 'charcoal')
  mem.set('k2.scheme', 'dark')
}

beforeEach(() => {
  mem.clear()
  seedTerminal()
  clearHtml()
  osLight = false
})

describe('accent override', () => {
  it('applies live and is saved', async () => {
    const store = await freshStore()
    store.getState().setAccent('#FF00AA')
    expect(accentVar()).toBe('#ff00aa')
    expect(store.getState().accent).toBe('#ff00aa')
    expect(mem.get('k2.accent.terminal')).toBe('#ff00aa')
  })

  it('still applies live when storage refuses to save it', async () => {
    const store = await freshStore()
    const setItem = vi.spyOn(localStorage, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError')
    })
    try {
      store.getState().setAccent('#ff00aa')
    } finally {
      setItem.mockRestore()
    }
    expect(accentVar()).toBe('#ff00aa')
    expect(store.getState().accent).toBe('#ff00aa')
    expect(mem.has('k2.accent.terminal')).toBe(false)
  })

  it('ignores an invalid colour', async () => {
    const store = await freshStore()
    store.getState().setAccent('not-a-colour')
    expect(accentVar()).toBe('')
    expect(store.getState().accent).toBeNull()
    expect(mem.has('k2.accent.terminal')).toBe(false)
  })

  it('an invalid stored value falls back to the default at boot', async () => {
    mem.set('k2.accent.terminal', 'url(evil)')
    const store = await freshStore()
    expect(store.getState().accent).toBeNull()
    expect(accentVar()).toBe('')
  })

  it('Reset removes it at once, leaving the palette in charge', async () => {
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    store.getState().resetAccent()
    expect(accentVar()).toBe('')
    expect(html.style.getPropertyValue('--color-on-accent')).toBe('')
    expect(store.getState().accent).toBeNull()
    expect(mem.has('k2.accent.terminal')).toBe(false)
    expect(mem.has('k2.accent.stamp.dark')).toBe(false)
  })

  it('is remembered per style', async () => {
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    store.getState().applyStyle({ styleId: 'glass', paletteId: 'obsidian' })
    expect(accentVar()).toBe('')
    expect(store.getState().accent).toBeNull()
    store.getState().setAccent('#00ff88')
    store.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal' })
    expect(accentVar()).toBe('#ff00aa')
    expect(store.getState().accent).toBe('#ff00aa')
  })

  it('a hover preview of another style shows that style’s accent, and leaving restores', async () => {
    const store = await freshStore()
    const { stampStyleAttributes } = await import('./style')
    store.getState().setAccent('#ff00aa')
    mem.set('k2.accent.glass', '#00ff88')
    const { styleId, paletteId, schemeMode, gapsPreset } = store.getState()
    stampStyleAttributes({ styleId: 'glass', paletteId: 'obsidian', schemeMode, gapsPreset })
    expect(accentVar()).toBe('#00ff88')
    stampStyleAttributes({ styleId, paletteId, schemeMode, gapsPreset })
    expect(accentVar()).toBe('#ff00aa')
    expect(store.getState().accent).toBe('#ff00aa')
  })

  it('picks readable ink for a bright colour', async () => {
    const store = await freshStore()
    store.getState().setAccent('#ffee00')
    expect(html.style.getPropertyValue('--color-on-accent')).toBe('#141414')
  })

  it('a peer window’s change arrives through the stamp key', async () => {
    const { STYLE_MIRROR_STORAGE_KEYS } = await import('./style')
    expect(STYLE_MIRROR_STORAGE_KEYS).toEqual(
      expect.arrayContaining(['k2.accent.stamp.dark', 'k2.accent.stamp.light']),
    )
  })
})

describe('accent pre-paint (index.html)', () => {
  it('paints the saved accent before any module runs', async () => {
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    clearHtml()
    runBootstrap()
    expect(accentVar()).toBe('#ff00aa')
  })

  it('paints nothing without a saved accent', async () => {
    await freshStore()
    clearHtml()
    runBootstrap()
    expect(accentVar()).toBe('')
  })

  const stamp = (over: Record<string, unknown>): void => {
    mem.set('k2.style', 'terminal')
    mem.set('k2.palette', 'charcoal')
    mem.set('k2.scheme', 'dark')
    mem.set(
      'k2.accent.stamp.dark',
      JSON.stringify({ style: 'terminal', palette: 'charcoal', vars: { '--color-accent': '#ff00aa' }, ...over }),
    )
  }

  it('skips a stamp for another style or palette', () => {
    stamp({ style: 'glass' })
    runBootstrap()
    expect(accentVar()).toBe('')
    clearHtml()
    stamp({ palette: 'paper' })
    runBootstrap()
    expect(accentVar()).toBe('')
  })

  it('skips any value that is not a plain colour', () => {
    stamp({ vars: { '--color-accent': 'url(evil)', '--color-accent-hover': '#123456', 'color': '#123456' } })
    runBootstrap()
    expect(accentVar()).toBe('')
    expect(html.style.getPropertyValue('--color-accent-hover')).toBe('#123456')
    expect(html.style.getPropertyValue('color')).toBe('')
  })

  it('survives a damaged stamp', () => {
    mem.set('k2.scheme', 'dark')
    mem.set('k2.accent.stamp.dark', '{not json')
    expect(() => runBootstrap()).not.toThrow()
    expect(html.getAttribute('data-palette')).toBe('charcoal')
  })
})

describe('Auto picks at boot', () => {
  const seedAuto = (dark: string, light: string): void => {
    mem.set('k2.style', 'terminal')
    mem.set('k2.scheme', 'auto')
    mem.set('k2.palette', 'charcoal')
    mem.set('k2.palette.dark', dark)
    mem.set('k2.palette.light', light)
    mem.set('k2.style.migrated', '1')
  }

  it('paints the pick for this appearance, with the palette’s own polarity', () => {
    seedAuto('charcoal', 'yellow')
    osLight = true
    runBootstrap()
    expect([html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual(['yellow', 'dark'])
  })

  it('the store agrees with the bootstrap, and keeps both picks', async () => {
    seedAuto('charcoal', 'yellow')
    osLight = true
    const store = await freshStore()
    expect([html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual(['yellow', 'dark'])
    expect(store.getState().autoPalettes).toEqual({ dark: 'charcoal', light: 'yellow' })
    expect(mem.get('k2.palette.light')).toBe('yellow')
  })

  it('a fixed mode leaves the picks for when Auto returns', async () => {
    seedAuto('yellow', 'paper')
    const store = await freshStore()
    store.getState().applyStyle({ schemeMode: 'light', paletteId: 'paper' })
    store.getState().applyStyle({ schemeMode: 'auto' })
    expect(store.getState().autoPalettes).toEqual({ dark: 'yellow', light: 'paper' })
    expect(html.getAttribute('data-palette')).toBe('yellow')
  })

  it('a fixed mode paints its own palette, not an Auto pick left in that side’s key', () => {
    mem.set('k2.style', 'terminal')
    mem.set('k2.scheme', 'light')
    mem.set('k2.palette', 'paper')
    mem.set('k2.palette.light', 'yellow')
    runBootstrap()
    expect([html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual(['paper', 'light'])
  })

  it('the accent stamp follows the pick for this appearance', async () => {
    seedAuto('charcoal', 'paper')
    const store = await freshStore()
    store.getState().setAccent('#ff00aa')
    const light = JSON.parse(mem.get('k2.accent.stamp.light')!)
    const dark = JSON.parse(mem.get('k2.accent.stamp.dark')!)
    expect([light.palette, dark.palette]).toEqual(['paper', 'charcoal'])
    clearHtml()
    osLight = true
    runBootstrap()
    expect(accentVar()).toBe('#ff00aa')
    expect(html.getAttribute('data-palette')).toBe('paper')
  })
})

describe('the default look on a client with nothing saved', () => {
  const YELLOW_ON_OBSIDIAN = '#ffc700'
  const YELLOW_ON_VEIL = '#e6b400'

  beforeEach(() => {
    mem.clear()
  })

  it('boots to Glass under Auto, Obsidian and Veil, with the Yellow accent saved once', async () => {
    const store = await freshStore()
    const s = store.getState()
    expect([s.styleId, s.paletteId, s.schemeMode, s.autoPalettes]).toEqual([
      'glass',
      'obsidian',
      'auto',
      { dark: 'obsidian', light: 'veil' },
    ])
    expect(s.accent).toBe('yellow')
    expect(mem.get('k2.accent.glass')).toBe('yellow')
    expect([html.getAttribute('data-style'), html.getAttribute('data-palette')]).toEqual(['glass', 'obsidian'])
    expect(accentVar()).toBe(YELLOW_ON_OBSIDIAN)
    expect([mem.get('k2.style'), mem.get('k2.scheme'), mem.get('k2.palette.light')]).toEqual(['glass', 'auto', 'veil'])
  })

  it('on a light system it is Veil with Yellow', async () => {
    osLight = true
    const store = await freshStore()
    expect(store.getState().resolvedPaletteId).toBe('veil')
    expect(accentVar()).toBe(YELLOW_ON_VEIL)
  })

  it('the first paint is already the default, with no flash of Terminal or blue', () => {
    runBootstrap()
    expect([html.getAttribute('data-style'), html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual([
      'glass',
      'obsidian',
      'dark',
    ])
    expect(accentVar()).toBe(YELLOW_ON_OBSIDIAN)
    clearHtml()
    osLight = true
    runBootstrap()
    expect([html.getAttribute('data-palette'), html.getAttribute('data-scheme')]).toEqual(['veil', 'light'])
    expect(accentVar()).toBe(YELLOW_ON_VEIL)
  })

  it('the bootstrap’s Yellow is exactly what the store derives, on both palettes', async () => {
    const { accentVarsFor } = await import('@/lib/style-accent')
    const { getStyle } = await import('@/styles.generated')
    const glass = getStyle('glass')!
    for (const [palette, light] of [['obsidian', false], ['veil', true]] as const) {
      clearHtml()
      osLight = light
      runBootstrap()
      const want = accentVarsFor('yellow', glass.palettes.find((p) => p.id === palette)!, light ? 'light' : 'dark')
      const got = Object.fromEntries(Object.keys(want).map((k) => [k, html.style.getPropertyValue(k)]))
      expect([palette, got]).toEqual([palette, want])
    }
  })

  it('the bootstrap and the store agree after the first launch', async () => {
    await freshStore()
    clearHtml()
    runBootstrap()
    expect(accentVar()).toBe(YELLOW_ON_OBSIDIAN)
    expect(html.getAttribute('data-style')).toBe('glass')
  })

  it('an existing Terminal client keeps it, and no accent is written', async () => {
    seedTerminal()
    const store = await freshStore()
    expect([store.getState().styleId, store.getState().resolvedPaletteId, store.getState().schemeMode]).toEqual([
      'terminal',
      'charcoal',
      'dark',
    ])
    expect(store.getState().accent).toBeNull()
    expect([...mem.keys()].filter((k) => k.startsWith('k2.accent.') && !k.startsWith('k2.accent.stamp.'))).toEqual([])
    expect(accentVar()).toBe('')
    clearHtml()
    runBootstrap()
    expect([html.getAttribute('data-style'), accentVar()]).toEqual(['terminal', ''])
  })

  it('an existing Glass client with no accent saved stays on the palette’s accent', async () => {
    mem.set('k2.style', 'glass')
    mem.set('k2.palette', 'obsidian')
    mem.set('k2.scheme', 'dark')
    const store = await freshStore()
    expect(store.getState().accent).toBeNull()
    expect(mem.has('k2.accent.glass')).toBe(false)
    expect(accentVar()).toBe('')
    clearHtml()
    runBootstrap()
    expect(accentVar()).toBe('')
  })

  it('never replaces an accent already saved for Glass', async () => {
    mem.set('k2.accent.glass', '#00ff88')
    const store = await freshStore()
    expect(store.getState().accent).toBe('#00ff88')
    expect(mem.get('k2.accent.glass')).toBe('#00ff88')
  })

  it('a person who resets to the palette’s accent keeps that', async () => {
    const store = await freshStore()
    store.getState().resetAccent()
    expect(mem.has('k2.accent.glass')).toBe(false)
    const again = await freshStore()
    expect(again.getState().accent).toBeNull()
    expect(accentVar()).toBe('')
  })
})

describe('the daemon’s style echo on a new client', () => {
  beforeEach(() => {
    mem.clear()
  })

  it('the daemon’s never-set default (square / charcoal / dark) does not replace the default look', async () => {
    const store = await freshStore()
    const { migrateStyleFromDaemon, __setPreBootMirrorCompleteForTests } = await import('./style')
    __setPreBootMirrorCompleteForTests(false)
    migrateStyleFromDaemon({ id: 'square', palette: 'charcoal', scheme: 'dark', gaps: '' })
    expect([store.getState().styleId, store.getState().accent]).toEqual(['glass', 'yellow'])
    expect(mem.get('k2.style.migrated')).toBe('1')
  })

  it('a style someone chose on the daemon still seeds a new client, with its own accent', async () => {
    const store = await freshStore()
    const { migrateStyleFromDaemon, __setPreBootMirrorCompleteForTests } = await import('./style')
    __setPreBootMirrorCompleteForTests(false)
    migrateStyleFromDaemon({ id: 'glass', palette: 'veil', scheme: 'light', gaps: '' })
    expect([store.getState().styleId, store.getState().resolvedPaletteId]).toEqual(['glass', 'veil'])
    expect(store.getState().accent).toBeNull()
    expect(mem.has('k2.accent.glass')).toBe(false)
  })
})

describe('Gardens', () => {
  it('the shield re-points every accent property at a Zen token', async () => {
    const { ZEN_STYLE_SHIELD } = await import('@/lib/zen/zen-style-shield')
    const { ACCENT_VARS } = await import('@/lib/style-accent')
    for (const name of ACCENT_VARS) {
      if (name === '--glass-accent') continue // read only by the palette's own slots, which are shielded
      expect([name, ZEN_STYLE_SHIELD[name]?.startsWith('var(--zen-')]).toEqual([name, true])
    }
  })
})
