// @vitest-environment jsdom
//
// Rosson 2026-10-08 — New Garden is one modal: a name and the catalog's
// cards (the Diary first, then the starts). Pick one, name it, Create. No
// permissions (Rosson 2026-10-08): no consent sentence, no scope picker, no
// grant in the create. An older daemon keeps the two starts. "See it in New
// Garden" opens the modal on that entry.
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
import { closeZenNewGarden, useZenNewGardenModal } from '@/lib/zen/zen-new-garden'
import { ZEN_OPEN_NEW_GARDEN_EVENT } from '@/lib/zen/zen-sync'
import { ZenNewGarden } from './ZenNewGarden'
import { ZenNewGardenHost } from './ZenNewGardenModal'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const DIARY = {
  id: 'k2.diary@1',
  short: 'diary',
  label: 'Diary',
  description: 'A haunted journal.',
  section: 'catalog',
  newUsers: true,
}
const STARTS = [
  { id: 'k2.texting@1', short: 'texting', label: 'Start with the default', description: 'Garden 1’s layout.', section: 'start', newUsers: true },
  { id: 'k2.blank@1', short: 'blank', label: 'Start empty and ask my agent', description: 'An empty page.', section: 'start', newUsers: true },
]
const STICKERS = { id: 'k2.stickers@1', short: 'stickers', label: 'Stickers', description: 'Just a look.', section: 'catalog', newUsers: false }

// A Home with an agent on this computer and one on a remote server.
const WORK: Home = {
  id: 'home-work',
  name: 'Work',
  rows: [
    { address: 'alice::local', workspaceId: 'w1', label: 'Alice' },
    { address: 'julie::scout.k2.dev', workspaceId: 'w2', label: 'Julie' },
  ],
}

let creates: Array<{ name: string; template: unknown; opts: unknown }>
let savedHomes: Home[]
let fail: Error | null

function bridge(): ZenWidgetBridge {
  return {
    widgetId: 'template-controls',
    caps: new Set(['gardens:manage']),
    gardens: {
      list: () => [{ id: 'g-test0001', name: 'Garden 1', index: 1 }],
      create: async (name: string, template?: unknown, opts?: unknown) => {
        if (fail) throw fail
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

function cards(): string[] {
  return Array.from(document.querySelectorAll('[data-zen-new-garden-card]')).map((c) => c.getAttribute('data-zen-new-garden-card') ?? '')
}

/** The menu row and the page's host, as Zen mounts them. */
async function openFromMenu(): Promise<ReturnType<typeof vi.fn>> {
  const b = bridge()
  const done = vi.fn()
  render(
    <>
      <ZenNewGarden bridge={b} onDone={done} />
      <ZenNewGardenHost bridge={b} />
    </>,
  )
  await act(async () => void fireEvent.click(q('[data-zen-new-garden]')))
  return done
}

beforeEach(() => {
  creates = []
  fail = null
  h.gets = []
  h.templates = null
  __resetZenTemplatesForTests()
  closeZenNewGarden()
  savedHomes = useHomesStore.getState().homes
  useHomesStore.setState({ homes: [WORK], selectedId: WORK.id })
})

afterEach(() => {
  cleanup()
  closeZenNewGarden()
  useHomesStore.setState({ homes: savedHomes })
})

describe('New Garden: one modal, the catalog as cards (Rosson 2026-10-08)', () => {
  it('the menu row closes its menu and opens the modal: a name, the cards (Diary first, each with a sketch)', async () => {
    h.templates = [...STARTS, DIARY, STICKERS]
    const done = await openFromMenu()
    expect(done).toHaveBeenCalledTimes(1)
    expect(useZenNewGardenModal.getState().open).toBe(true)
    expect(q('[data-testid="zen-new-garden-modal"]').getAttribute('role')).toBe('dialog')
    expect(cards()).toEqual(['diary', 'stickers', 'texting', 'blank'])
    for (const c of cards()) expect(document.querySelector(`[data-zen-new-garden-card="${c}"] [data-zen-garden-sketch="${c}"]`), c).not.toBeNull()
    expect(h.gets.every((g) => g === 'local zen/templates')).toBe(true)
    // The Diary is picked first, its name suggested; nothing to agree to.
    expect(q('[data-zen-new-garden-card="diary"]').getAttribute('aria-checked')).toBe('true')
    expect((q('[data-zen-new-garden-name]') as HTMLInputElement).value).toBe('Diary')
    expect(document.querySelector('[data-zen-new-garden-consent]')).toBeNull()
    expect(q('[data-testid="zen-new-garden-modal"]').textContent).not.toMatch(/permission|allow|post to their Threads/i)
    expect(creates).toEqual([])
  })

  it('Diary: one click creates it, with no grant in the request; no scope picker, no Sending choice', async () => {
    h.templates = [...STARTS, DIARY]
    await openFromMenu()
    // No scope picker, no Sending choice, no agent chooser.
    expect(document.querySelector('[data-zen-new-garden-modal] select, [data-zen-sending-choice], [data-zen-scope-kind]')).toBeNull()
    expect(document.querySelectorAll('[data-testid="zen-new-garden-modal"] select').length).toBe(0)
    fireEvent.change(q('[data-zen-new-garden-name]'), { target: { value: 'Séance' } })
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-create]')))
    expect(creates).toEqual([{ name: 'Séance', template: 'diary', opts: { ask: false } }])
    expect(useZenNewGardenModal.getState().open).toBe(false)
    expect(document.querySelector('[data-testid="zen-new-garden-modal"]')).toBeNull()
  })

  it('the plain starts: Start with the default (Enter in the name) and Start empty (opens Ask my agent), no grant', async () => {
    h.templates = [...STARTS, DIARY]
    await openFromMenu()
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-card="texting"]')))
    expect(document.querySelector('[data-zen-new-garden-consent]')).toBeNull()
    expect((q('[data-zen-new-garden-name]') as HTMLInputElement).value).toBe('Garden 2')
    await act(async () => void fireEvent.keyDown(q('[data-zen-new-garden-name]'), { key: 'Enter' }))
    expect(creates).toEqual([{ name: 'Garden 2', template: 'texting', opts: { ask: false } }])
    await act(async () => void fireEvent.click(q('[data-zen-new-garden]')))
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-card="blank"]')))
    fireEvent.change(q('[data-zen-new-garden-name]'), { target: { value: 'Ideas' } })
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-create]')))
    expect(creates[1]).toEqual({ name: 'Ideas', template: 'blank', opts: { ask: true } })
  })

  it('a name you already have is refused before the create; a daemon refusal stays in the modal', async () => {
    h.templates = [...STARTS, DIARY]
    await openFromMenu()
    fireEvent.change(q('[data-zen-new-garden-name]'), { target: { value: 'garden 1' } })
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-create]')))
    expect(q('[data-zen-new-garden-error]').textContent).toBe('You already have a Garden called “garden 1”.')
    expect(creates).toEqual([])
    fail = Object.assign(new Error('Gardens aren’t set up on this computer yet.'), { code: 'zen_not_set_up' })
    fireEvent.change(q('[data-zen-new-garden-name]'), { target: { value: 'Fresh' } })
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-create]')))
    expect(q('[data-zen-new-garden-error]').textContent).toBe('Gardens aren’t set up on this computer yet.')
    expect(useZenNewGardenModal.getState().open).toBe(true)
  })

  it('another catalog Garden creates the same way: no grant, no consent line', async () => {
    h.templates = [...STARTS, STICKERS]
    await openFromMenu()
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-card="stickers"]')))
    expect(document.querySelector('[data-zen-new-garden-consent]')).toBeNull()
    await act(async () => void fireEvent.click(q('[data-zen-new-garden-create]')))
    expect(creates).toEqual([{ name: 'Stickers', template: 'stickers', opts: { ask: false } }])
  })

  it('an older daemon (no templates route) offers the two starts only', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    await openFromMenu()
    expect(cards()).toEqual(['texting', 'blank'])
    expect(q('[data-zen-new-garden-card="texting"]').getAttribute('aria-checked')).toBe('true')
    warn.mockRestore()
  })

  it('“See it in New Garden” opens the modal on that entry; Esc closes it', async () => {
    h.templates = [...STARTS, DIARY, STICKERS]
    render(<ZenNewGardenHost bridge={bridge()} />)
    expect(document.querySelector('[data-testid="zen-new-garden-modal"]')).toBeNull()
    await act(async () => void window.dispatchEvent(new CustomEvent(ZEN_OPEN_NEW_GARDEN_EVENT, { detail: { short: 'stickers' } })))
    expect(q('[data-zen-new-garden-card="stickers"]').getAttribute('aria-checked')).toBe('true')
    expect((q('[data-zen-new-garden-name]') as HTMLInputElement).value).toBe('Stickers')
    await act(async () => void fireEvent.keyDown(window, { key: 'Escape' }))
    expect(document.querySelector('[data-testid="zen-new-garden-modal"]')).toBeNull()
    expect(creates).toEqual([])
  })
})
