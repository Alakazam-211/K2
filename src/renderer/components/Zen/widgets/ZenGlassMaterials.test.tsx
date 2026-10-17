// @vitest-environment jsdom
//
// 0.46.0 (Rosson 2026-10-16): Garden 1's Glass look uses K2 Companion's real
// materials (`lib/zen/zen-materials.ts`), through the real widgets:
//   - GLASS only on small floating controls: the top band's pills (Garden
//     switcher, Zen toggle, theme, menu), the nav rail (Home | Agents),
//     the Agents header's Home picker and settings cog, server chips;
//   - TILE on the message box and Send, the activity drawer (a sheet on the
//     box, no bottom rim), menus, the settings dialog and its cards;
//   - SOLID iMessage bubbles, plain "Earlier messages", attachment chips,
//     choice pills (white when picked);
//   - the progressive header and footer blur (2/6/12/24px), the list
//     running under both; reduced transparency: no blur, opaque fills;
//   - only the Glass theme turns any of it on: other themes draw as before.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow, ZenThreadView } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import type { OverlayThreadItem, ThreadTurn } from '@/components/SessionView/overlayThread'
import {
  ZEN_GLASS_RADII,
  ZEN_MATERIALS_CSS,
  ZEN_MOBILE_GLASS,
  ZEN_PROGRESSIVE_BLUR_LAYERS,
  ZenMaterialsContext,
  zenMaterialProps,
  zenThemeMaterials,
} from '@/lib/zen/zen-materials'
import { useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import { ThreadAttachmentChip, ThreadBubble } from '@/components/Thread/ThreadZenKit'
import { ZenAgentsWidget } from './ZenAgentsWidget'
import { ZenConversation } from './ZenConversationWidget'
import { ZenNavRailWidget } from './ZenNavRailWidget'
import { ZenGardenSwitcher, ZenToggle } from './ZenChromeControls'
import { __resetZenDraftsForTests } from './ZenCompose'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    if (route !== 'zen/theme/list') throw new Error(`unexpected GET ${route}`)
    return { ok: true, active: 'glass', scope: 'garden', global: 'basic', garden: 'g-1', gardenTheme: null, themes: [] }
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

const ROOT = '[data-zen-root][data-zen-materials="mobile"]'
const REDUCED = '@media (prefers-reduced-transparency: reduce)'

function row(handle: string, over: Partial<ZenAgentRow> = {}): ZenAgentRow {
  return {
    address: `${handle}::local`,
    label: handle,
    index: 0,
    hostKey: 'local',
    server: null,
    role: null,
    reach: 'live',
    auth: null,
    activity: 'idle',
    counts: null,
    working: false,
    needsYou: false,
    state: 'ok',
    stateLabel: null,
    detail: null,
    preview: null,
    people: [],
    selected: false,
    openable: true,
    avatarUrl: null,
    color: null,
    image: null,
    pinned: false,
    ...over,
  }
}

function fakeBridge(rows: ZenAgentRow[]): { bridge: ZenWidgetBridge; pushThread(v: ZenThreadView): void } {
  let threadListener: ((v: ZenThreadView) => void) | null = null
  const bridge = {
    widgetId: 'w',
    caps: new Set<string>(['agents:read', 'thread:read', 'thread:post', 'app:navigate']),
    call(verb: string, ...args: unknown[]): unknown {
      switch (verb) {
        case 'agents.list':
          return rows
        case 'agents.subscribe':
        case 'app.subscribe':
        case 'app.subscribeCurrent':
          return () => {}
        case 'agents.home':
          return 'home-1'
        case 'app.badges':
          return { tickets: { badge: null, stale: false, title: '' } }
        case 'app.current':
          return 'home'
        case 'thread.subscribe':
          threadListener = args[1] as (v: ZenThreadView) => void
          return () => {
            threadListener = null
          }
        default:
          throw new Error(`unexpected bridge verb ${verb}`)
      }
    },
    controls: { bind: () => () => {} },
    gardens: { list: () => [{ id: 'g-1', name: 'Garden 1', index: 1 }], current: () => ({ id: 'g-1', name: 'Garden 1' }) },
    homes: { list: () => [{ id: 'home-1', name: 'Home', rows: rows.length }] },
    zen: { exit: () => {} },
  } as unknown as ZenWidgetBridge
  return {
    bridge,
    pushThread(view) {
      const fn = threadListener
      if (!fn) throw new Error('nobody subscribed to the thread')
      act(() => fn(view))
    },
  }
}

/** A Zen root drawing (or not) Companion's materials. */
function Root({ mobile, reduced = false, children }: { mobile: boolean; reduced?: boolean; children: React.ReactNode }): React.JSX.Element {
  return (
    <div data-zen-root="" data-zen-scheme="dark" data-zen-materials={mobile ? 'mobile' : undefined} data-zen-reduced-transparency={reduced ? '' : undefined}>
      <ZenMaterialsContext.Provider value={{ materials: mobile ? 'mobile' : null, reducedTransparency: reduced }}>{children}</ZenMaterialsContext.Provider>
    </div>
  )
}

function el(sel: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(sel)
  if (!found) throw new Error(`missing ${sel}`)
  return found
}
const material = (sel: string): string | null => el(sel).getAttribute('data-zen-material')
const shape = (sel: string): string | null => el(sel).getAttribute('data-zen-glass-shape')

function liveTurn(): ThreadTurn {
  const now = Date.now()
  return {
    turnId: 't1',
    state: 'working',
    phase: 'tool',
    startedAt: now - 8_000,
    phaseSince: now - 3_000,
    line: 'Read `src/main.ts`',
    subagents: 0,
    subagentsDone: 0,
    background: 0,
    tally: { read: 2, search: 0, cmd: 0, edit: 0 },
    waitingOn: null,
    end: null,
    rev: 1,
    skewMs: 0,
  }
}

function item(id: string, from: string, body: string, seq: number, extra: Partial<OverlayThreadItem['doc']> = {}): OverlayThreadItem {
  return { id, seq, doc: { kind: 'text', from, body, created_at: 1_700_000_000 + seq, ...extra } } as unknown as OverlayThreadItem
}

function threadView(over: Partial<ZenThreadView> = {}): ZenThreadView {
  return {
    address: 'cortana::local',
    phase: 'ready',
    note: null,
    items: [item('m1', 'cortana', 'Hi there', 1), item('m2', 'me', 'Hello', 2, { via: 'compose' } as Partial<OverlayThreadItem['doc']>)],
    loaded: true,
    hasMore: true,
    loadingOlder: false,
    error: null,
    turn: { state: 'working', since: Date.now() - 8_000 },
    liveTurn: liveTurn(),
    viewer: null,
    ...over,
  } as ZenThreadView
}

const OPTIONS = { compose: true, attachments: true, loadOlder: true, activityDrawer: true }

function renderConversation(mobile: boolean, reduced = false): void {
  const fb = fakeBridge([])
  const r = row('cortana', { activity: 'working', working: true, selected: true })
  render(
    <Root mobile={mobile} reduced={reduced}>
      <ZenConversation bridge={fb.bridge} row={r} options={OPTIONS} />
    </Root>,
  )
  fb.pushThread(threadView())
}

beforeEach(() => {
  localStorage.clear()
  useZenRecentFirstStore.getState().reload()
  __resetZenDraftsForTests()
})

afterEach(() => {
  cleanup()
  localStorage.clear()
})

describe('which themes draw Companion’s materials', () => {
  it('Glass only; basic, midnight, paper and haunted draw as before', () => {
    expect(zenThemeMaterials('glass')).toBe('mobile')
    for (const name of ['basic', 'midnight', 'paper', 'haunted', 'k2-light', null, undefined, '']) {
      expect(zenThemeMaterials(name)).toBeNull()
    }
  })

  it('every rule is scoped to a Zen root drawing them (the markers do nothing elsewhere)', () => {
    const selectors = ZEN_MATERIALS_CSS.split('\n')
      .map((l) => l.trim())
      .filter((l) => l && !l.startsWith('@') && !l.startsWith('}') && !/^[a-z-]+:/.test(l) && !l.startsWith('--'))
    expect(selectors.length).toBeGreaterThan(20)
    for (const s of selectors) expect(s.startsWith(ROOT), s).toBe(true)
  })

  it('only glass has a shape', () => {
    expect(zenMaterialProps('glass', 'field')).toEqual({ 'data-zen-material': 'glass', 'data-zen-glass-shape': 'field' })
    expect(() => zenMaterialProps('tile', 'pill')).toThrow('only glass has a shape')
  })
})

describe('the stylesheet: Companion’s values', () => {
  const normal = ZEN_MATERIALS_CSS.slice(0, ZEN_MATERIALS_CSS.indexOf(`${ROOT}[data-zen-reduced-transparency]`))

  it('glass: white 10% over a 28px blur at 180% saturation, a 0.5px edge, a top highlight; pill 999, round 16, field 22', () => {
    expect(ZEN_MOBILE_GLASS).toEqual({
      fill: 'rgba(255, 255, 255, 0.10)',
      blur: 'blur(28px) saturate(180%)',
      edge: '0.5px solid rgba(255, 255, 255, 0.22)',
      highlight: 'inset 0 1px 0 rgba(255, 255, 255, 0.28)',
    })
    expect(ZEN_GLASS_RADII).toEqual({ pill: 999, round: 16, field: 22 })
    const glass = normal.slice(normal.indexOf(`${ROOT} [data-zen-material="glass"]`))
    const rule = glass.slice(0, glass.indexOf('}'))
    expect(rule).toContain('background: rgba(255, 255, 255, 0.10) !important;')
    expect(rule).toContain('-webkit-backdrop-filter: blur(28px) saturate(180%) !important;')
    expect(rule).toContain(' backdrop-filter: blur(28px) saturate(180%) !important;')
    expect(rule).toContain('border: 0.5px solid rgba(255, 255, 255, 0.22) !important;')
    expect(rule).toContain('box-shadow: inset 0 1px 0 rgba(255, 255, 255, 0.28) !important;')
    expect(rule).toContain('border-radius: 999px !important;')
    expect(normal).toContain(`${ROOT} [data-zen-glass-shape="round"] { border-radius: 16px !important; }`)
    expect(normal).toMatch(/\[data-zen-glass-shape="field"\],\n[^{]*\{ border-radius: 22px !important; \}/)
  })

  it('light schemes keep the glass readable: a denser white fill and a dark edge', () => {
    const light = normal.slice(normal.indexOf(`${ROOT}[data-zen-scheme="light"] [data-zen-material="glass"]`))
    const rule = light.slice(0, light.indexOf('}'))
    expect(rule).toContain('background: rgba(255, 255, 255, 0.62) !important;')
    expect(rule).toContain('border: 0.5px solid rgba(0, 0, 0, 0.12) !important;')
  })

  it('tile: solid (the fill over the canvas), no blur, edge + sheen + soft shadow', () => {
    const tile = normal.slice(normal.indexOf(`${ROOT} [data-zen-glass]:not([data-zen-material="glass"])`))
    const rule = tile.slice(0, tile.indexOf('}'))
    expect(rule).toContain(`${ROOT} [data-zen-material="tile"]`)
    expect(rule).toContain('background: linear-gradient(var(--zen-glass, var(--zen-surface)), var(--zen-glass, var(--zen-surface))), var(--zen-canvas) !important;')
    expect(rule).toContain('-webkit-backdrop-filter: none !important; backdrop-filter: none !important;')
    expect(rule).toContain('box-shadow: var(--zen-glass-sheen), var(--zen-glass-shadow) !important;')
    expect(rule).not.toMatch(/blur\(/)
    // A sheet (the drawer on the box): no bottom rim, no drop shadow.
    expect(normal).toContain(
      `${ROOT} [data-zen-material="tile"][data-zen-tile-sheet] { border-bottom-width: 0 !important; box-shadow: var(--zen-glass-sheen) !important; }`,
    )
    // Send: a raised tile, dimmed until it can send.
    expect(normal).toContain(`${ROOT} [data-zen-send][data-zen-material="tile"]:disabled { opacity: 0.45; }`)
  })

  it('solid: bubbles never blur; choice pills outlined, white when picked; "Earlier messages" plain text', () => {
    expect(normal).toContain(`${ROOT} [data-zen-bubble] { -webkit-backdrop-filter: none !important; backdrop-filter: none !important; }`)
    const chip = normal.slice(normal.indexOf(`${ROOT} [data-zen-bubble] [data-testid="thread-choice-chip"] {`))
    expect(chip.slice(0, chip.indexOf('}'))).toContain('background: transparent !important;')
    expect(normal).toMatch(/\[data-testid="thread-choice-chip"\]\[data-selected="true"\] \{\n {2}background: #ffffff !important;/)
    const older = normal.slice(normal.indexOf(`${ROOT} [data-zen-load-older][data-zen-material="solid"],`))
    expect(older.slice(0, older.indexOf('}'))).toContain('background: transparent !important;')
  })

  it('progressive blur: four layers 2/6/12/24px, header heavy at the top, footer heavy at the bottom', () => {
    expect(ZEN_PROGRESSIVE_BLUR_LAYERS.map(([px]) => px)).toEqual([2, 6, 12, 24])
    ZEN_PROGRESSIVE_BLUR_LAYERS.forEach(([px, from, to], i) => {
      for (const [edge, dir] of [
        ['header', 'to top'],
        ['footer', 'to bottom'],
      ] as const) {
        const sel = `${ROOT} .zen-${edge}-blur > .zen-blur-layer-${i + 1} {`
        const rule = ZEN_MATERIALS_CSS.slice(ZEN_MATERIALS_CSS.indexOf(sel))
        expect(ZEN_MATERIALS_CSS.indexOf(sel), sel).toBeGreaterThan(0)
        const body = rule.slice(0, rule.indexOf('}'))
        expect(body).toContain(`-webkit-backdrop-filter: blur(${px}px); backdrop-filter: blur(${px}px);`)
        expect(body).toContain(`mask-image: linear-gradient(${dir}, transparent ${from}%, #000 ${to}%);`)
      }
      if (i > 0) {
        const [, prevFrom, prevTo] = ZEN_PROGRESSIVE_BLUR_LAYERS[i - 1]
        expect(from).toBeGreaterThan(prevFrom)
        expect(from).toBeLessThan(prevTo)
      }
    })
    expect(ZEN_MATERIALS_CSS).toContain(`${ROOT} .zen-footer-blur > .zen-blur-tint { background: linear-gradient(to bottom, `)
    expect(ZEN_MATERIALS_CSS).toContain(`${ROOT} .zen-header-blur > .zen-blur-tint { background: linear-gradient(to top, `)
  })

  it('reduced transparency (media query or the root attribute): no blur, opaque glass, blur layers hidden', () => {
    const attr = ZEN_MATERIALS_CSS.slice(ZEN_MATERIALS_CSS.indexOf(`${ROOT}[data-zen-reduced-transparency] .zen-progressive-blur`))
    const media = ZEN_MATERIALS_CSS.slice(ZEN_MATERIALS_CSS.lastIndexOf(REDUCED))
    for (const block of [attr.slice(0, attr.indexOf(REDUCED)), media]) {
      expect(block).not.toMatch(/blur\(\d/)
      expect(block).toContain('.zen-progressive-blur > [class^="zen-blur-layer-"] { display: none; }')
      expect(block).toContain('background: linear-gradient(var(--zen-surface-raised), var(--zen-surface-raised)), var(--zen-canvas) !important;')
      expect(block).toContain('-webkit-backdrop-filter: none !important; backdrop-filter: none !important;')
      // Glass at rest, hovered and open are all opaque.
      expect(block).toContain('[data-zen-material="glass"]:hover:not(:disabled)')
      expect(block).toContain('[data-zen-material="glass"][aria-expanded="true"]')
    }
    // The tint goes nearly opaque near the light edge.
    expect(ZEN_MATERIALS_CSS).toContain(`${ROOT}[data-zen-reduced-transparency] .zen-footer-blur > .zen-blur-tint { background: linear-gradient(to bottom, `)
  })
})

describe('Glass: each surface’s material', () => {
  it('top band and rail: glass pills (Garden switcher, Zen toggle, Home | Agents rail); the Garden menu is a raised tile', async () => {
    const fb = fakeBridge([])
    render(
      <Root mobile>
        <ZenGardenSwitcher bridge={fb.bridge} />
        <ZenToggle bridge={fb.bridge} />
        <ZenNavRailWidget bridge={fb.bridge} decl={{ id: 'nav', kind: 'nav-rail', column: 0, props: {}, caps: ['app:navigate'], source: 'builtin' } as ZenWidgetDecl} />
      </Root>,
    )
    for (const sel of ['[data-zen-garden-pill]', '[data-zen-switch]', '[data-zen-widget="nav-rail"]']) {
      expect(material(sel), sel).toBe('glass')
      expect(shape(sel), sel).toBe('pill')
    }
    await act(async () => void fireEvent.click(el('[data-zen-garden-pill]')))
    expect(material('[data-zen-garden-menu]')).toBe('tile')
    expect(el('[data-zen-garden-menu]').getAttribute('data-zen-tile')).toBe('raised')
  })

  it('Agents header and rows: the Home picker, the settings cog and server chips are glass; the settings dialog and its cards are tiles', async () => {
    const fb = fakeBridge([row('alpha', { server: 'z3thon.k2.dev' }), row('beta')])
    const decl = { id: 'agents', kind: 'agents', column: 0, props: { 'home-picker': true, settings: true }, caps: ['agents:read'], source: 'builtin' } as ZenWidgetDecl
    render(
      <Root mobile>
        <ZenAgentsWidget bridge={fb.bridge} decl={decl} />
      </Root>,
    )
    expect(material('[data-zen-home-picker-button]')).toBe('glass')
    expect(material('[data-zen-agents-settings]')).toBe('glass')
    expect(material('[data-zen-server-tag]')).toBe('glass')
    expect(shape('[data-zen-server-tag]')).toBe('pill')
    // Rows themselves are no material: plain list rows.
    expect(material('[data-zen-agent-row="beta::local"]')).toBeNull()
    await act(async () => void fireEvent.click(el('[data-zen-agents-settings]')))
    expect(material('[data-testid="zen-garden-settings"]')).toBe('tile')
    const cards = Array.from(document.querySelectorAll('[data-testid="zen-garden-settings"] [data-zen-material="tile"]'))
    expect(cards.length).toBeGreaterThanOrEqual(2)
  })

  it('conversation: the box and Send are raised tiles, the drawer a tile sheet on the box; bubbles solid; "Earlier messages" solid text', () => {
    renderConversation(true)
    expect(material('[data-zen-compose-box]')).toBe('tile')
    expect(el('[data-zen-compose-box]').getAttribute('data-zen-tile')).toBe('raised')
    expect(material('[data-zen-send]')).toBe('tile')
    expect(material('[data-zen-drawer-tile]')).toBe('tile')
    expect(el('[data-zen-drawer-tile]').hasAttribute('data-zen-tile-sheet')).toBe(true)
    expect(material('[data-zen-load-older]')).toBe('solid')
    const agent = el('[data-zen-message="m1"] [data-zen-bubble]')
    const mine = el('[data-zen-message="m2"] [data-zen-bubble]')
    expect(agent.getAttribute('data-zen-bubble')).toBe('agent')
    expect(mine.getAttribute('data-zen-bubble')).toBe('me')
    // iMessage colours come from the theme's bubble tokens; no material, no blur.
    expect(agent.style.background).toBe('var(--zen-bubble-agent)')
    expect(mine.style.background).toBe('var(--zen-bubble-me)')
    for (const b of [agent, mine]) {
      expect(b.hasAttribute('data-zen-material')).toBe(false)
      expect(b.style.backdropFilter).toBe('')
    }
  })

  it('attachment chips are solid; a picked choice pill says so', () => {
    const choice = item('c1', 'cortana', '', 3, {
      kind: 'choice',
      choice: { prompt: 'Ship it?', options: [{ label: 'Yes' }, { label: 'No' }], allow_custom: false, status: 'answered', answer: 'Yes' },
    } as Partial<OverlayThreadItem['doc']>)
    render(
      <Root mobile>
        <ThreadAttachmentChip name="plan.md" onRemove={() => {}} />
        <ThreadBubble item={choice} mine={false} author="cortana" timeLabel="now" viewer={null} />
      </Root>,
    )
    expect(material('[data-zen-attachment="plan.md"]')).toBe('solid')
    const chips = Array.from(document.querySelectorAll<HTMLElement>('[data-testid="thread-choice-chip"]'))
    expect(chips.map((c) => [c.getAttribute('data-label'), c.getAttribute('data-selected')])).toEqual([
      ['Yes', 'true'],
      ['No', null],
    ])
  })

  it('the top bar and the message box float on progressive blurs, the list runs under both', () => {
    renderConversation(true)
    expect(el('[data-zen-conversation]').hasAttribute('data-zen-floating')).toBe(true)
    const header = el('[data-zen-conversation-header]')
    expect(header.style.position).toBe('absolute')
    expect(header.style.top).toBe('0px')
    const slot = el('[data-zen-compose-slot]')
    expect(slot.style.position).toBe('absolute')
    expect(slot.style.bottom).toBe('0px')
    for (const edge of ['header', 'footer']) {
      const blur = el(`[data-zen-progressive-blur="${edge}"]`)
      expect(blur.classList.contains(`zen-${edge}-blur`)).toBe(true)
      expect(blur.getAttribute('aria-hidden')).toBe('true')
      expect(Array.from(blur.children).map((c) => c.className)).toEqual([
        'zen-blur-layer-1',
        'zen-blur-layer-2',
        'zen-blur-layer-3',
        'zen-blur-layer-4',
        'zen-blur-tint',
      ])
    }
    // The header blur sits inside the header, the footer blur inside the slot.
    expect(header.querySelector('.zen-header-blur')).not.toBeNull()
    expect(slot.querySelector('.zen-footer-blur')).not.toBeNull()
    // No rule under the bar: the blur is the edge.
    expect((header.children[1] as HTMLElement).style.borderBottom).toBe('')
  })

  it('reduced transparency: no blur layers, only the tint', () => {
    renderConversation(true, true)
    for (const edge of ['header', 'footer']) {
      expect(Array.from(el(`[data-zen-progressive-blur="${edge}"]`).children).map((c) => c.className)).toEqual(['zen-blur-tint'])
    }
    expect(document.querySelectorAll('[class^="zen-blur-layer-"]')).toHaveLength(0)
  })
})

describe('other themes: unchanged', () => {
  it('nothing floats, no blur, the bar keeps its rule; the markers are inert without the root’s materials', () => {
    renderConversation(false)
    expect(document.querySelector('.zen-progressive-blur')).toBeNull()
    expect(el('[data-zen-conversation]').hasAttribute('data-zen-floating')).toBe(false)
    expect(el('[data-zen-conversation-header]').style.position).toBe('')
    expect(el('[data-zen-compose-slot]').style.position).toBe('')
    expect((el('[data-zen-conversation-header]').children[0] as HTMLElement).style.borderBottom).toBe('1px solid var(--zen-border)')
    expect(el('[data-zen-root]').hasAttribute('data-zen-materials')).toBe(false)
    expect(el('[data-zen-conversation-body]').style.paddingTop).toBe('14px')
    expect(el('[data-zen-conversation-body]').style.paddingBottom).toBe('8px')
  })
})
