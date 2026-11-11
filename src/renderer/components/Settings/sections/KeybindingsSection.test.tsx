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
// The server keeps every write and echoes the shortcuts back; a test that
// wants a refusal rejects one call.
vi.mock('@/lib/daemon-settings', async (orig) => ({
  ...(await orig<typeof import('@/lib/daemon-settings')>()),
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async (u: { keybindings?: Record<string, string> }) => ({ keybindings: u.keybindings ?? {} })),
}))
vi.mock('@/lib/desktop-chrome', async (orig) => ({
  ...(await orig<typeof import('@/lib/desktop-chrome')>()),
  isMacPlatform: () => h.mac,
}))

import { KeybindingsSection, KEYBINDINGS_MANIFEST, KEYBINDING_NAMES } from './KeybindingsSection'
import { HOTKEYS } from '@shared/hotkeys'
import { useSettingsStore } from '@/stores/settings'

const LABELS = [
  'Go to My Home',
  'Go to the Agents page',
  'Go to the Projects page',
  'Go to the Tickets page',
  'Go to the Wiki page',
  'Go to the Settings page',
]
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
    const row = rowFor('Go to the Tickets page')
    expect(keyButton(row).textContent?.replace(/\s/g, '')).toBe('⌃⌘T')
    expect(within(row).getByRole('button', { name: 'Reset Go to the Tickets page to default' }).getAttribute('title')).toBe('Reset to default')
    expect(row.textContent).toContain('Changed. The default is ⌃⌘4.')
  })

  it('Wiki and Settings rebind the same way', () => {
    h.mac = false
    useSettingsStore.setState({ keybindings: { goToWiki: 'Ctrl+Alt+W', goToSettings: 'Ctrl+Alt+S' } })
    render(<KeybindingsSection />)
    expect(keyButton(rowFor('Go to the Wiki page')).textContent?.replace(/\s/g, '')).toBe('Ctrl+Alt+W')
    expect(keyButton(rowFor('Go to the Settings page')).textContent?.replace(/\s/g, '')).toBe('Ctrl+Alt+S')
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

describe('Key Bindings: names', () => {
  beforeEach(() => {
    h.mac = true
  })

  it('every shortcut has a Settings name: sentence case, no slashes, none twice', () => {
    const names = HOTKEYS.map((k) => KEYBINDING_NAMES[k.id]?.name)
    names.forEach((n, i) => expect(n, HOTKEYS[i].id).toBeTruthy())
    for (const n of names as string[]) {
      expect(n).not.toContain('/')
      expect(n[0]).toBe(n[0].toUpperCase())
      // Sentence case: after the first word, only names of K2's own pages and views are capitalised.
      const proper = new Set(['My', 'Home', 'Agents', 'Projects', 'Tickets', 'Wiki', 'Settings', 'Org', 'Chart', 'Thread'])
      for (const w of n.split(' ').slice(1)) if (/^[A-Z]/.test(w)) expect(proper.has(w), `${n}: ${w}`).toBe(true)
    }
    expect(new Set(names).size).toBe(names.length)
    expect(Object.keys(KEYBINDING_NAMES).sort()).toEqual(HOTKEYS.map((k) => k.id).sort())
  })

  it('the split names say which way the pane splits (⌘D stacks, ⇧⌘D sits side by side)', () => {
    render(<KeybindingsSection />)
    expect(within(rowFor('Split the pane top and bottom')).getByRole('button').textContent?.replace(/\s/g, '')).toBe('⌘D')
    expect(within(rowFor('Split the pane side by side')).getByRole('button').textContent?.replace(/\s/g, '')).toBe('⌘⇧D')
  })

  it('a name that needs it carries a one-line hint, and keeps it once changed', () => {
    useSettingsStore.setState({ keybindings: { focusWindow: 'Meta+Shift+J' } })
    render(<KeybindingsSection />)
    expect(rowFor('Show or hide the left panel').textContent).toContain('The panel beside the sidebar')
    const row = rowFor('Open this workspace in its own window')
    expect(row.textContent).toContain('A focus window: just this workspace, nothing else.')
    expect(row.textContent).toContain('Changed. The default is ⌘⇧F.')
  })

  it('off macOS a changed page chord names its Ctrl+Alt default', () => {
    h.mac = false
    useSettingsStore.setState({ keybindings: { goToTickets: 'Ctrl+Alt+T' } })
    render(<KeybindingsSection />)
    expect(rowFor('Go to the Tickets page').textContent).toContain('Changed. The default is Ctrl+Alt+4.')
  })

  it('search uses the same names, and still finds a row by its menu name', () => {
    const entry = KEYBINDINGS_MANIFEST.find((e) => e.id === 'keybindings.toggleLeftPanel')
    expect(entry?.label).toBe('Show or hide the left panel')
    expect(entry?.keywords).toContain('toggle left panel')
    for (const k of HOTKEYS) {
      expect(KEYBINDINGS_MANIFEST.find((e) => e.id === `keybindings.${k.id}`)?.label).toBe(KEYBINDING_NAMES[k.id].name)
    }
  })
})

describe('Key Bindings: recording, clashes and reset', () => {
  beforeEach(() => {
    h.mac = true
  })

  it('records new keys, saves, and says Saved', async () => {
    render(<KeybindingsSection />)
    const row = rowFor('Go to the Tickets page')
    fireEvent.click(keyButton(row))
    expect(within(row).getByRole('button', { name: 'Recording a new shortcut for Go to the Tickets page' }).textContent).toBe('Press keys…')
    expect(row.textContent).toContain('Press the new keys. Escape cancels.')
    await act(async () => {
      fireEvent.keyDown(window, { key: 'y', code: 'KeyY', metaKey: true, ctrlKey: true })
    })
    expect(useSettingsStore.getState().keybindings.goToTickets).toBeTruthy()
    expect(rowFor('Go to the Tickets page').querySelector('[data-save-note]')).toBeTruthy()
  })

  it('a shortcut the server refuses keeps the old keys and never says Saved', async () => {
    const { settingsUpdate } = await import('@/lib/daemon-settings')
    vi.mocked(settingsUpdate).mockRejectedValueOnce(new Error('settings_update 403: forbidden'))
    render(<KeybindingsSection />)
    fireEvent.click(keyButton(rowFor('Go to the Tickets page')))
    await act(async () => {
      fireEvent.keyDown(window, { key: 'y', code: 'KeyY', metaKey: true, ctrlKey: true })
    })
    expect(useSettingsStore.getState().keybindings.goToTickets).toBeUndefined()
    expect(rowFor('Go to the Tickets page').querySelector('[data-save-error]')?.textContent).toContain('Couldn’t save')
    expect(rowFor('Go to the Tickets page').querySelector('[data-save-note]')).toBeNull()
  })

  it('a key the terminal needs is refused out loud, and recording goes on', async () => {
    render(<KeybindingsSection />)
    fireEvent.click(keyButton(rowFor('Go to the Tickets page')))
    await act(async () => {
      fireEvent.keyDown(window, { key: 'c', code: 'KeyC', ctrlKey: true })
    })
    const row = rowFor('Go to the Tickets page')
    expect(within(row).getByRole('alert').textContent).toContain('can’t be a shortcut')
    expect(within(row).getByRole('button', { name: /Recording a new shortcut/ })).toBeTruthy()
    expect(useSettingsStore.getState().keybindings.goToTickets).toBeUndefined()
  })

  it('two shortcuts on the same keys both say so', () => {
    useSettingsStore.setState({ keybindings: { goToWiki: 'Ctrl+Meta+4' } })
    render(<KeybindingsSection />)
    expect(rowFor('Go to the Wiki page').textContent).toContain('Same keys as “Go to the Tickets page”. Change one of them.')
    expect(rowFor('Go to the Tickets page').textContent).toContain('Same keys as “Go to the Wiki page”. Change one of them.')
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
