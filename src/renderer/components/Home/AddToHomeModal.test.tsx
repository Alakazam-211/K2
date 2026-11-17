// @vitest-environment jsdom
// Add to Home's People and Channels sections (prd-home-people-channels-v1
// S6), inside K2 Design's dialog: a tab bar only once Home chat is on
// somewhere; a person or channel pick adds ONE row and closes, as an
// agent pick does.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))
const h = vi.hoisted(() => ({ offered: true }))
vi.mock('@/stores/home-chat', async (orig) => ({
  ...(await orig<typeof import('@/stores/home-chat')>()),
  useHomeChatOffered: () => h.offered,
}))
vi.mock('./HomeAddPanels', () => ({
  SectionStatus: () => null,
  addableMatches: () => true,
  useAddableAgents: () => ({ sources: [], add: () => null }),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_s: unknown, route: string) => {
    if (route === 'home-chat/status') return { ok: true, on: true, reason: null, flags: {} }
    if (route === 'home-chat/summary') return { ok: true, chat: 'live', convs: [] }
    if (route === 'home-chat/people')
      return { me: { ext: 'k2:connect:me', memberId: '1' }, people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex', ready: true, memberId: '100' }] }
    if (route === 'home-chat/channels') return { channels: [{ conv: '601', name: 'general', open: true, archived: false }] }
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async () => ({ ok: true })),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { ADD_TO_HOME_SECTIONS, AddToHomeModal, addToHomeSections, rememberedAddToHomeTab } from './AddToHomeModal'
import { resetHomeChatForTests } from '@/stores/home-chat'
import type { Home, HomeRow } from '@/stores/homes'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const HOME: Home = { id: 'h1', name: 'Work', rows: [] }

function open(section: 'agents' | 'people' | 'channels' | undefined, rows: HomeRow[], onClose = vi.fn(), onAdded = vi.fn(), home: Home = HOME) {
  render(
    <AddToHomeModal
      home={home}
      section={section}
      onClose={onClose}
      onAdded={onAdded}
      onAddRow={(r) => (rows.push(r), true)}
    />,
  )
  return { onClose, onAdded }
}

beforeEach(() => {
  resetHomeChatForTests()
  h.offered = true
  localStorage.clear()
})
afterEach(() => {
  cleanup()
  resetHomeChatForTests()
})

describe('Add to Home: people and channels', () => {
  it('the sections: Agents alone without Home chat, then People and Channels', () => {
    expect(ADD_TO_HOME_SECTIONS.map((s) => s.id)).toEqual(['agents', 'people', 'channels'])
    expect(addToHomeSections(false).map((s) => s.id)).toEqual(['agents'])
    expect(addToHomeSections(true).map((s) => s.id)).toEqual(['agents', 'people', 'channels'])
  })

  it('a person pick adds one row and closes', async () => {
    const rows: HomeRow[] = []
    const { onClose, onAdded } = open('people', rows)
    expect(screen.getByTestId('add-to-home-tab-channels')).toBeTruthy()
    await act(async () => {
      fireEvent.click(await screen.findByText('Alex'))
    })
    expect(rows).toEqual([{ kind: 'person', address: 'k2:connect:a::local', workspaceId: null, label: 'Alex' }])
    expect(onAdded).toHaveBeenCalledWith('k2:connect:a::local')
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('Channels: pick one (adds and closes), or switch to New channel', async () => {
    const rows: HomeRow[] = []
    const { onClose } = open('channels', rows)
    await act(async () => {
      fireEvent.click(screen.getByTestId('add-to-home-channels-new'))
    })
    expect(await screen.findByTestId('home-chat-new-channel-name')).toBeTruthy()
    await act(async () => {
      fireEvent.click(screen.getByTestId('add-to-home-channels-pick'))
    })
    await act(async () => {
      fireEvent.click(await screen.findByText('general'))
    })
    expect(rows).toEqual([{ kind: 'channel', address: '601::local', workspaceId: null, label: 'general' }])
    expect(onClose).toHaveBeenCalledTimes(1)
  })

  it('with Home chat on no server there is no tab bar, and a People opener lands on Agents', () => {
    h.offered = false
    open('people', [])
    expect(document.querySelector('[data-add-to-home-tabs]')).toBeNull()
    expect(document.querySelector('[data-add-to-home-panel]')?.getAttribute('data-add-to-home-panel')).toBe('agents')
  })

  it('the tabs are the segmented control: Agents, People, Channels, left-aligned', () => {
    open('agents', [])
    const bar = document.querySelector('[data-add-to-home-tabs]')!
    const group = bar.querySelector('[role="radiogroup"]')!
    expect(group.classList.contains('k2-segmented')).toBe(true)
    expect(bar.className).toContain('justify-start')
    expect([...group.querySelectorAll('[role="radio"]')].map((b) => b.textContent)).toEqual(['Agents', 'People', 'Channels'])
    expect(screen.getByTestId('add-to-home-tab-agents').getAttribute('aria-checked')).toBe('true')
  })

  it('→ and ← on the tab bar switch tabs, and focus stays on the tabs', async () => {
    open('agents', [])
    const agents = screen.getByTestId('add-to-home-tab-agents')
    agents.focus()
    await act(async () => {
      fireEvent.keyDown(agents, { key: 'ArrowRight' })
    })
    expect(document.querySelector('[data-add-to-home-panel]')?.getAttribute('data-add-to-home-panel')).toBe('people')
    expect(document.activeElement).toBe(screen.getByTestId('add-to-home-tab-people'))
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('add-to-home-tab-people'), { key: 'ArrowRight' })
    })
    expect(document.querySelector('[data-add-to-home-panel]')?.getAttribute('data-add-to-home-panel')).toBe('channels')
    await act(async () => {
      fireEvent.keyDown(screen.getByTestId('add-to-home-tab-channels'), { key: 'ArrowLeft' })
    })
    expect(document.querySelector('[data-add-to-home-panel]')?.getAttribute('data-add-to-home-panel')).toBe('people')
    // Give a search's own focus a frame: it must not take focus off the tabs.
    await act(async () => {
      await new Promise((r) => requestAnimationFrame(() => r(null)))
    })
    expect(document.activeElement).toBe(screen.getByTestId('add-to-home-tab-people'))
  })

  it('the last tab is remembered per Home', async () => {
    open(undefined, [])
    expect(document.querySelector('[data-add-to-home-panel]')?.getAttribute('data-add-to-home-panel')).toBe('agents')
    await act(async () => {
      fireEvent.click(screen.getByTestId('add-to-home-tab-channels'))
    })
    expect(rememberedAddToHomeTab('h1')).toBe('channels')
    cleanup()
    open(undefined, [])
    expect(document.querySelector('[data-add-to-home-panel]')?.getAttribute('data-add-to-home-panel')).toBe('channels')
    cleanup()
    // Another Home keeps its own (none yet: Agents).
    open(undefined, [], vi.fn(), vi.fn(), { id: 'h2', name: 'Play', rows: [] })
    expect(document.querySelector('[data-add-to-home-panel]')?.getAttribute('data-add-to-home-panel')).toBe('agents')
  })
})

// The title, the tabs, the search field and the list share one left edge:
// the dialog's 20px (px-5). The shared list pads its search and rows for a
// menu (6-8px); inside Add to Home one rule moves them onto the dialog's edge.
describe('Add to Home: one left edge', () => {
  it('the head and the tabs sit on the 20px edge', async () => {
    open('people', [])
    await screen.findByText('Alex')
    expect(document.querySelector('[data-add-to-home-head]')?.className).toContain('pl-5')
    expect(document.querySelector('[data-add-to-home-tabs]')?.className).toContain('px-5')
  })

  it('the search, section heads, rows and notes are marked for the dialog rule', async () => {
    open('people', [])
    await screen.findByText('Alex')
    const panel = document.querySelector('[data-add-to-home-panel]')!
    expect(panel.querySelector('[data-agent-list-search]')).toBeTruthy()
    expect(panel.querySelector('[data-agent-list-head]')).toBeTruthy()
    expect(panel.querySelector('[data-ws-filter-value]')).toBeTruthy()
  })

  it('the rule puts them on the same 20px edge', () => {
    const css = readFileSync(join(__dirname, '..', '..', 'globals.css'), 'utf8').replace(/\/\*[\s\S]*?\*\//g, ' ')
    const edge = 'calc(var(--spacing) * 5)'
    const block = (sel: string): string => {
      const at = css.indexOf(sel)
      expect(at, sel).toBeGreaterThanOrEqual(0)
      return css.slice(at, css.indexOf('}', at))
    }
    expect(block('[data-add-to-home-panel] :is([data-agent-list-search], [data-agent-list-head], [data-ws-filter-value]) {')).toContain(`padding-inline: ${edge}`)
    // A section's note keeps its own 8px, so the box adds the rest.
    expect(block('[data-add-to-home-panel] [data-agent-list-status] {')).toContain(`padding-inline: calc(${edge} - var(--spacing) * 2)`)
  })
})

