// @vitest-environment jsdom
// Settings → Key Bindings lists the page chords (0.46.0, Rosson 2026-10-16:
// "Make sure to add this to the bindings list"): Navigation, rebindable,
// ⌃⌘1–6 on macOS and Ctrl+Alt+1–6 elsewhere (5 Wiki, 6 Settings: 0.46.4).

import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, cleanup, within } from '@testing-library/react'

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

function rowFor(label: string): HTMLElement {
  const span = Array.from(document.querySelectorAll('span')).find((s) => s.textContent === label)
  if (!span?.parentElement) throw new Error(`no row ${label}`)
  return span.parentElement
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
    const order = Array.from(document.querySelectorAll('span'))
      .map((s) => s.textContent ?? '')
      .filter((t) => LABELS.includes(t))
    expect(order).toEqual(LABELS)
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
    const buttons = within(rowFor('Go to Tickets')).getAllByRole('button')
    expect(buttons[0].textContent?.replace(/\s/g, '')).toBe('⌃⌘T')
    expect(buttons[1].getAttribute('title')).toBe('Reset to default')
  })

  it('Wiki and Settings rebind the same way', () => {
    h.mac = false
    useSettingsStore.setState({ keybindings: { goToWiki: 'Ctrl+Alt+W', goToSettings: 'Ctrl+Alt+S' } })
    render(<KeybindingsSection />)
    expect(within(rowFor('Go to Wiki')).getAllByRole('button')[0].textContent?.replace(/\s/g, '')).toBe('Ctrl+Alt+W')
    expect(within(rowFor('Go to Settings')).getAllByRole('button')[0].textContent?.replace(/\s/g, '')).toBe('Ctrl+Alt+S')
  })

  it('the settings search finds them under Navigation', () => {
    for (const id of ['goToHome', 'goToAgents', 'goToProjects', 'goToTickets', 'goToWiki', 'goToSettings']) {
      const entry = KEYBINDINGS_MANIFEST.find((e) => e.id === `keybindings.${id}`)
      expect(entry?.group).toBe('Navigation')
    }
  })
})
