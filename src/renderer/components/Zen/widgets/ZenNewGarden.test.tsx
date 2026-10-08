// @vitest-environment jsdom
//
// prd-zen-user-widgets-v2 R5 (as changed 2026-10-08), UWB22, UWB23 — New
// Garden in Zen reads the starts and the Garden catalog from
// `GET /cli/zen/templates`; a catalog Garden that runs a widget asks for its
// scope (and Sending) in the same click; an older daemon keeps the two
// starts and shows no catalog.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  templates: null as null | unknown[],
  gets: [] as string[],
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (scope: { hostKey: string }, route: string) => {
    h.gets.push(`${scope.hostKey} ${route}`)
    if (route !== 'zen/templates') throw new Error(`unexpected GET ${route}`)
    if (h.templates === null) throw new Error('unknown zen route')
    return { ok: true, templates: h.templates }
  }),
  daemonCliPost: vi.fn(async (_s: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import { useHomesStore, type Home } from '@/stores/homes'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import { __resetZenTemplatesForTests } from '@/lib/zen/zen-templates'
import { ZenNewGarden } from './ZenNewGarden'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const DIARY = {
  id: 'k2.diary@1',
  short: 'diary',
  label: 'Diary',
  description: 'One agent at a time, in ink.',
  section: 'catalog',
  needsGrant: { widget: 'k2:diary@1', caps: ['agents:read', 'thread:read', 'thread:post'] },
  newUsers: false,
}
const STARTS = [
  { id: 'k2.texting@1', short: 'texting', label: 'Start with the default', description: 'Garden 1’s layout.', section: 'start', needsGrant: null, newUsers: true },
  { id: 'k2.blank@1', short: 'blank', label: 'Start empty and ask my agent', description: 'An empty page.', section: 'start', needsGrant: null, newUsers: true },
]
const STICKERS = { id: 'k2.stickers@1', short: 'stickers', label: 'Stickers', description: 'Just a look.', section: 'catalog', needsGrant: null, newUsers: false }

const WORK: Home = { id: 'home-work', name: 'Work', rows: [{ address: 'alice::local', workspaceId: 'w1', label: 'Alice' }] }

let creates: Array<{ name: string; template: unknown; opts: unknown }>
let savedHomes: Home[]

function bridge(): ZenWidgetBridge {
  return {
    widgetId: 'template-controls',
    caps: new Set(['gardens:manage']),
    gardens: {
      list: () => [{ id: 'g-test0001', name: 'Garden 1', index: 1 }],
      create: async (name: string, template?: unknown, opts?: unknown) => {
        creates.push({ name, template, opts })
        return { id: 'g-test0002', name, index: 2 }
      },
    },
  } as unknown as ZenWidgetBridge
}

function q(sel: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(sel)
  if (!el) throw new Error(`missing ${sel}\n${document.body.innerHTML.slice(0, 2000)}`)
  return el
}

async function toStart(name: string): Promise<void> {
  await act(async () => void fireEvent.click(q('[data-zen-new-garden]')))
  fireEvent.change(q('[data-zen-new-garden-name]'), { target: { value: name } })
  await act(async () => void fireEvent.keyDown(q('[data-zen-new-garden-name]'), { key: 'Enter' }))
}

beforeEach(() => {
  creates = []
  h.gets = []
  h.templates = null
  __resetZenTemplatesForTests()
  savedHomes = useHomesStore.getState().homes
  useHomesStore.setState({ homes: [WORK], selectedId: WORK.id })
})

afterEach(() => {
  cleanup()
  useHomesStore.setState({ homes: savedHomes })
})

describe('New Garden: the Garden catalog (R5)', () => {
  it('lists the daemon’s starts, then Ready-made Gardens; Diary asks for its scope and creates with the grant', async () => {
    h.templates = [...STARTS, DIARY, STICKERS]
    const done = vi.fn()
    render(<ZenNewGarden bridge={bridge()} onDone={done} />)
    await toStart('Notebook')
    const choices = Array.from(document.querySelectorAll('[data-zen-new-garden-choice]')).map((c) =>
      c.getAttribute('data-zen-new-garden-choice'),
    )
    expect(choices).toEqual(['texting', 'blank', 'diary', 'stickers'])
    expect(q('[data-zen-new-garden-catalog-title]').textContent).toBe('Ready-made Gardens')
    expect(h.gets).toEqual(['local zen/templates'])
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-choice="diary"]')))
    const grant = q('[data-zen-new-garden-grant="diary"]')
    expect(grant.textContent).toContain('“Notebook” · Diary')
    expect(grant.textContent).toContain('See the agents in Work:')
    expect(creates).toEqual([])
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-create]')))
    expect(creates).toEqual([
      { name: 'Notebook', template: 'diary', opts: { ask: false, grant: { scope: { home: 'home-work' }, sending: true } } },
    ])
    expect(done).toHaveBeenCalledTimes(1)
  })

  it('a catalog Garden with no widget is created in one click, with no grant', async () => {
    h.templates = [...STARTS, STICKERS]
    render(<ZenNewGarden bridge={bridge()} onDone={() => undefined} />)
    await toStart('Look')
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-choice="stickers"]')))
    expect(creates).toEqual([{ name: 'Look', template: 'stickers', opts: { ask: false } }])
  })

  it('an older daemon (no templates route) keeps the two starts and shows no catalog', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    render(<ZenNewGarden bridge={bridge()} onDone={() => undefined} />)
    await toStart('Ideas')
    const choices = Array.from(document.querySelectorAll('[data-zen-new-garden-choice]')).map((c) =>
      c.getAttribute('data-zen-new-garden-choice'),
    )
    expect(choices).toEqual(['texting', 'blank'])
    expect(document.querySelector('[data-zen-new-garden-catalog-title]')).toBeNull()
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-choice="blank"]')))
    expect(creates).toEqual([{ name: 'Ideas', template: 'blank', opts: { ask: true } }])
    warn.mockRestore()
  })
})
