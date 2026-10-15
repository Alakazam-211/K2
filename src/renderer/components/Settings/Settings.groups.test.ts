// @vitest-environment jsdom
// Settings sidebar groups (0.45.7): the nav blocks and where old ids land.
import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { settingsNav } from './Settings'
import { useSettingsStore } from '@/stores/settings'

describe('Settings → LLMs group', () => {
  it('is one group with LLMs, Tokens, Token Usage, whether or not Gardens show', () => {
    for (const zenGardens of [true, false]) {
      const nav = settingsNav({ zenGardens })
      const group = nav.find((b) => b.kind === 'group' && b.title === 'LLMs')
      expect(group).toEqual({
        kind: 'group',
        title: 'LLMs',
        items: [
          { id: 'agents', label: 'LLMs' },
          { id: 'llm-tokens', label: 'Tokens' },
          { id: 'token-usage', label: 'Token Usage' },
        ],
      })
      const items = nav.flatMap((b) => (b.kind === 'item' ? [b.id] : []))
      expect(items).not.toContain('agents')
      expect(items).not.toContain('token-usage')
      const titles = nav.flatMap((b) => (b.kind === 'group' ? [b.title] : []))
      expect(titles[zenGardens ? 1 : 0]).toBe('LLMs')
    }
  })

  it('old ids land on the right leaf', () => {
    const s = useSettingsStore.getState()
    s.openSettings('agents')
    expect(useSettingsStore.getState().activeSection).toBe('agents')
    s.openSettings('token-usage')
    expect(useSettingsStore.getState().activeSection).toBe('token-usage')
    s.openSettings('editors-agents')
    expect(useSettingsStore.getState().activeSection).toBe('agents')
    s.setSection('llm-tokens')
    expect(useSettingsStore.getState().activeSection).toBe('llm-tokens')
    s.closeSettings()
  })
})
