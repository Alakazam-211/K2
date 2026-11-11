// @vitest-environment jsdom
//
// Settings → Terminal → Font reaches Kessel terminals: App.tsx mounts
// StyledKesselProvider above the app (and Home's rooms), the provider puts
// the picked font first in the Kessel config, and TerminalPane draws with
// useKesselConfig().font.family. The real settings store and the setter the
// Terminal page calls are used; only the server is faked.

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import React from 'react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
// The server keeps every settings write and echoes it back.
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async (u: Record<string, unknown>) => ({ ...u })),
  settingsReset: vi.fn(async () => ({})),
}))

import { StyledKesselProvider } from './styled-kessel-provider'
import { useKesselConfig } from '@/kessel/config-context'
import { defaultKesselConfig } from '@/kessel/config'
import { useSettingsStore } from '@/stores/settings'

/** What a terminal reads: the same hook TerminalPane uses. */
function FontProbe(): React.JSX.Element {
  return <span data-testid="family">{useKesselConfig().font.family}</span>
}

function family(): string {
  return screen.getByTestId('family').textContent ?? ''
}

beforeEach(() => {
  useSettingsStore.setState((s) => ({ terminal: { ...s.terminal, fontFamily: 'MesloLGM Nerd Font' } }))
})
afterEach(() => cleanup())

describe('Terminal font → Kessel config', () => {
  it('the default font keeps the default stack; a picked font goes first and follows each change', async () => {
    render(
      <StyledKesselProvider>
        <FontProbe />
      </StyledKesselProvider>,
    )
    expect(family()).toBe(defaultKesselConfig.font.family)

    await act(async () => {
      await useSettingsStore.getState().updateTerminalSettings({ fontFamily: 'Fira Code' })
    })
    expect(family()).toBe(`'Fira Code', ${defaultKesselConfig.font.family}`)

    await act(async () => {
      await useSettingsStore.getState().updateTerminalSettings({ fontFamily: 'JetBrains Mono' })
    })
    expect(family()).toBe(`'JetBrains Mono', ${defaultKesselConfig.font.family}`)

    await act(async () => {
      await useSettingsStore.getState().updateTerminalSettings({ fontFamily: 'MesloLGM Nerd Font' })
    })
    expect(family()).toBe(defaultKesselConfig.font.family)
  })

  it('the app mounts the provider above the app and Home rooms, and terminals draw with its font', () => {
    const renderer = join(process.cwd(), 'src/renderer')
    const app = readFileSync(join(renderer, 'App.tsx'), 'utf8')
    expect(app).toContain("import { StyledKesselProvider } from './lib/styled-kessel-provider'")
    const root = app.slice(app.indexOf('export default function App('))
    expect(root.slice(0, root.indexOf('\n}\n'))).toMatch(/<StyledKesselProvider>\s*<AppRoot \/>\s*<\/StyledKesselProvider>/)
    const rooms = app.slice(app.indexOf('export function HomeRoomsHost('))
    expect(rooms.slice(0, rooms.indexOf('\n}\n'))).toMatch(/<StyledKesselProvider>\s*<HomeRoomsPortal \/>\s*<\/StyledKesselProvider>/)

    const pane = readFileSync(join(renderer, 'kessel-term/TerminalPane.tsx'), 'utf8')
    expect(pane).toContain('const config = useKesselConfig()')
    expect(pane).toContain('fontFamily: config.font.family')
  })
})
