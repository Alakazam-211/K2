// @vitest-environment jsdom
// Settings → Key Bindings lists the page chords (0.46.0, Rosson 2026-10-16:
// "Make sure to add this to the bindings list"): Navigation, rebindable,
// ⌃⌘1–6 on macOS and Ctrl+Alt+1–6 elsewhere (5 Wiki, 6 Settings: 0.46.4).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { act, render, cleanup, fireEvent, screen, within } from '@testing-library/react'

const h = vi.hoisted(() => ({ mac: true }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  isMacPlatform: () => h.mac,
}))

import { KeybindingsSection, KEYBINDINGS_MANIFEST } from './KeybindingsSection'
import { useSettingsStore } from '@/stores/settings'

const LABELS = ['Go to My Home', 'Go to Agents', 'Go to Projects', 'Go to Tickets', 'Go to Wiki', 'Go to Settings']
const IDS = ['goToHome', 'goToAgents', 'goToProjects', 'goToTickets', 'goToWiki', 'goToSettings']

function rowFor(label: string): HTMLElement {
  const row = Array.from(document.querySelectorAll<HTMLElement>('[data-testid^="keybinding-"]')).find(
    (r) => r.querySelector('.type-row')?.textContent === label,
  )
  if (!row) throw new Error(`no row ${label}`)
  return row
}

function keyButton(row: HTMLElement): HTMLElement {
  return within(row).getByRole('button', { name: /\. Change$/ })
}

beforeEach(() => {
  useSettingsStore.setState({ keybindings: {} })
})
afterEach(() => cleanup())

describe('Key Bindings: page chords', () => {
  it('macOS: six Navigation rows, ⌃⌘1–6, in top-bar order', () => {
    h.mac = true
    render(<KeybindingsSection />)
    LABELS.forEach((label, i) => {
      const row = rowFor(label)
      expect(within(row).getByRole('button').textContent?.replace(/\s/g, '')).toBe(`⌃⌘${i + 1}`)
    })
    const order = Array.from(document.querySelectorAll('[data-testid^="keybinding-"]'))
      .map((r) => (r.getAttribute('data-testid') ?? '').replace('keybinding-', ''))
      .filter((id) => IDS.includes(id))
    expect(order).toEqual(IDS)
  })

  it('Linux / Windows: Ctrl+Alt+1–6', () => {
    h.mac = false
    render(<KeybindingsSection />)
    LABELS.forEach((label, i) => {
      expect(within(rowFor(label)).getByRole('button').textContent?.replace(/\s/g, '')).toBe(`Ctrl+Alt+${i + 1}`)
    })
  })

  it('a rebind shows, with a reset', () => {
    h.mac = true
    useSettingsStore.setState({ keybindings: { goToTickets: 'Ctrl+Meta+T' } })
    render(<KeybindingsSection />)
    const row = rowFor('Go to Tickets')
    expect(keyButton(row).textContent?.replace(/\s/g, '')).toBe('⌃⌘T')
    expect(within(row).getByRole('button', { name: 'Reset Go to Tickets to default' }).getAttribute('title')).toBe('Reset to default')
    expect(row.textContent).toContain('Changed. The default is ⌃⌘4.')
  })

  it('Wiki and Settings rebind the same way', () => {
    h.mac = false
    useSettingsStore.setState({ keybindings: { goToWiki: 'Ctrl+Alt+W', goToSettings: 'Ctrl+Alt+S' } })
    render(<KeybindingsSection />)
    expect(keyButton(rowFor('Go to Wiki')).textContent?.replace(/\s/g, '')).toBe('Ctrl+Alt+W')
    expect(keyButton(rowFor('Go to Settings')).textContent?.replace(/\s/g, '')).toBe('Ctrl+Alt+S')
  })

  it('the settings search finds them under Navigation', () => {
    for (const id of ['goToHome', 'goToAgents', 'goToProjects', 'goToTickets', 'goToWiki', 'goToSettings']) {
      const entry = KEYBINDINGS_MANIFEST.find((e) => e.id === `keybindings.${id}`)
      expect(entry?.group).toBe('Navigation')
    }
  })

  it('every search entry points at a row on the page', () => {
    render(<KeybindingsSection />)
    for (const e of KEYBINDINGS_MANIFEST) expect(document.querySelector(`[data-settings-id="${e.id}"]`)).toBeTruthy()
  })
})

describe('Key Bindings: recording, clashes and reset', () => {
  beforeEach(() => {
    h.mac = true
  })

  it('records new keys, saves, and says Saved', async () => {
    render(<KeybindingsSection />)
    const row = rowFor('Go to Tickets')
    fireEvent.click(keyButton(row))
    expect(within(row).getByRole('button', { name: 'Recording a new shortcut for Go to Tickets' }).textContent).toBe('Press keys…')
    expect(row.textContent).toContain('Press the new keys. Escape cancels.')
    await act(async () => {
      fireEvent.keyDown(window, { key: 'y', code: 'KeyY', metaKey: true, ctrlKey: true })
    })
    expect(useSettingsStore.getState().keybindings.goToTickets).toBeTruthy()
    expect(rowFor('Go to Tickets').querySelector('[data-save-note]')).toBeTruthy()
  })

  it('a key the terminal needs is refused out loud, and recording goes on', async () => {
    render(<KeybindingsSection />)
    fireEvent.click(keyButton(rowFor('Go to Tickets')))
    await act(async () => {
      fireEvent.keyDown(window, { key: 'c', code: 'KeyC', ctrlKey: true })
    })
    const row = rowFor('Go to Tickets')
    expect(within(row).getByRole('alert').textContent).toContain('can’t be a shortcut')
    expect(within(row).getByRole('button', { name: /Recording a new shortcut/ })).toBeTruthy()
    expect(useSettingsStore.getState().keybindings.goToTickets).toBeUndefined()
  })

  it('two shortcuts on the same keys both say so', () => {
    useSettingsStore.setState({ keybindings: { goToWiki: 'Ctrl+Meta+4' } })
    render(<KeybindingsSection />)
    expect(rowFor('Go to Wiki').textContent).toContain('Same keys as Go to Tickets. Change one of them.')
    expect(rowFor('Go to Tickets').textContent).toContain('Same keys as Go to Wiki. Change one of them.')
  })

  it('Reset all asks first, then says it is done', async () => {
    useSettingsStore.setState({ keybindings: { goToWiki: 'Ctrl+Alt+W', goToSettings: 'Ctrl+Alt+S' } })
    render(<KeybindingsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Reset all…' }))
    expect(screen.getByTestId('keys-reset-confirm').textContent).toContain('The 2 shortcuts you changed go back')
    expect(Object.keys(useSettingsStore.getState().keybindings)).toHaveLength(2)
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Reset all' }))
    })
    expect(useSettingsStore.getState().keybindings).toEqual({})
    expect(screen.getByTestId('keys-reset-note').textContent).toBe('All shortcuts are back to their defaults.')
    expect((screen.getByRole('button', { name: 'Reset all…' }) as HTMLButtonElement).disabled).toBe(true)
  })
})
