// @vitest-environment jsdom
//
// 0.46.5 WL7 (prd-widget-size-and-shared-libs-v1): the daemon resolves a
// widget's libraries once and sends `libKeys`; the frame loads exactly
// those, in that order. An older daemon sends only `libs`, which the loader
// resolves itself. Through the real ZenPage and ZenCustomWidget; the
// daemon, Tauri and the library loader's texts are faked (a recorder).
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ asked: [] as unknown[][] }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main', startDragging: async () => undefined, listen: async () => () => undefined }),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string, params?: { widget?: string }) => {
    if (route !== 'zen/widget/bundle') throw new Error(`unexpected GET ${route}`)
    return {
      ok: true,
      widget: params?.widget,
      hash: 'hash-1',
      nonce: 'q83vFzPq1N3V0aZ8k2LmTw',
      html: '<!doctype html><html><body><script nonce="q83vFzPq1N3V0aZ8k2LmTw">k2.ready()</script></body></html>',
      bytes: 120,
    }
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/zen/zen-lib-loader', async (orig) => {
  const real = (await orig()) as Record<string, unknown>
  return {
    ...real,
    zenLib: {
      texts: vi.fn(async (refs: readonly unknown[]) => {
        h.asked.push([...refs])
        return refs.map(() => '/* lib */')
      }),
    },
  }
})

import { act } from 'react'
import { cleanup, render } from '@testing-library/react'
import { useZenGardensStore } from '@/lib/zen/zen-gardens'
import { useZenWindowStore } from '@/lib/zen/zen-window'
import { __setZenGeometryForTests } from '@/lib/zen/zen-monitor'
import { parseZenGet, ZEN_CUSTOM_KIND, type ZenResolvedPage } from '@/lib/zen/zen-page'
import { __resetZenCustomRunForTests } from '@/lib/zen/zen-custom-run'
import { resetZenPausedStartForTests, takeZenPausedAtBoot } from '@/lib/zen/zen-widgets-running'
import { registerZenWidget } from './zen-registry'
import { ZenCustomWidget } from './widgets/ZenCustomWidget'
import { ZenPage } from './ZenPage'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const GARDEN = 'g-test0002'

function page(extra: Record<string, unknown>): ZenResolvedPage {
  return parseZenGet({
    ok: true,
    schema: 1,
    version: 'v1',
    garden: { id: GARDEN, name: 'Libs', index: 1 },
    page: {
      template: 'k2.blank@1',
      layout: { kind: 'columns', split: [100], minWidths: [0] },
      widgets: [
        {
          id: 'notes',
          kind: 'custom',
          slot: 'column',
          column: 0,
          props: {},
          widget: 'notes',
          name: 'Notes',
          caps: [],
          requested: [],
          source: 'user',
          description: null,
          hash: 'hash-1',
          state: 'ok',
          errors: [],
          warnings: [],
          origin: 'local',
          paused: null,
          libs: ['k2-markdown@1'],
          ...extra,
        },
      ],
      controls: ['garden-switcher', 'drag-region', 'zen-toggle'],
      chrome: {
        from: 'garden',
        items: [
          { id: 'garden-switcher', kind: 'garden-switcher', slot: 'top', align: 'start', props: {}, caps: ['gardens:manage'] },
          { id: 'zen-toggle', kind: 'zen-toggle', slot: 'top', align: 'end', props: {}, caps: [] },
        ],
      },
      bands: { top: { start: ['garden-switcher'], center: [], end: ['zen-toggle'] }, bottom: null },
      edges: [],
      menus: {},
    },
    theme: { name: 'basic' },
    themes: [],
    chrome: {},
    motion: {},
    errors: [],
    warnings: [],
    lastGoodAt: null,
  })
}

async function show(p: ZenResolvedPage): Promise<void> {
  render(
    <div data-zen-root="">
      <ZenPage page={p} safe={false} banner={null} onControlFailure={() => undefined} />
    </div>,
  )
  await act(async () => {
    await new Promise((r) => setTimeout(r, 0))
  })
}

let unregister: () => void

beforeEach(() => {
  h.asked = []
  unregister = registerZenWidget(ZEN_CUSTOM_KIND, ZenCustomWidget)
  useZenGardensStore.setState({ gardens: [{ id: GARDEN, name: 'Libs', template: 'k2.blank@1', index: 1 } as never] })
  useZenWindowStore.setState({ on: true, garden: GARDEN, view: 'home' })
  __setZenGeometryForTests({
    rect: () => ({ left: 120, top: 60, width: 80, height: 30 }),
    style: () => ({ display: 'block', visibility: 'visible', opacity: 1 }),
    viewport: () => ({ width: 1200, height: 800 }),
    elementFromPoint: () => null,
  })
  __resetZenCustomRunForTests()
  resetZenPausedStartForTests()
  takeZenPausedAtBoot(null, { label: 'main', storage: null })
})

afterEach(() => {
  cleanup()
  unregister()
  __setZenGeometryForTests(null)
})

describe('the frame loads the daemon’s resolved libKeys (WL7)', () => {
  it('libKeys present: exactly those keys, in that order', async () => {
    const keys = ['marked@18.0.14', 'dompurify@3.4.17', 'k2-markdown@1.0.0']
    await show(page({ libKeys: keys }))
    expect(h.asked).toEqual([keys])
  })

  it('an older daemon (no libKeys): the manifest’s libs, resolved by the loader', async () => {
    await show(page({}))
    expect(h.asked).toEqual([['k2-markdown@1']])
  })
})

// Security review finding 5 (widget-libs): the frame's caps, sources and
// agent rows come from the page payload, so it runs only the bundle whose
// hash is the payload's. A bundle that changed in between never runs.
describe('the frame runs only the version its payload describes', () => {
  it('a bundle whose hash is not the payload’s: nothing runs, a load card says why', async () => {
    await show(page({ hash: 'hash-2', caps: ['sources:read'] }))
    expect(document.querySelector('iframe')).toBeNull()
    const card = document.querySelector('[data-zen-custom-card="load-failed"]')
    expect(card?.textContent).toContain('This widget changed while its page was loading.')
  })

  it('the same hash: the frame mounts and no load card shows', async () => {
    await show(page({ hash: 'hash-1' }))
    expect(document.querySelector('[data-zen-custom-card="load-failed"]')).toBeNull()
    expect(document.querySelector('iframe')).not.toBeNull()
  })
})
