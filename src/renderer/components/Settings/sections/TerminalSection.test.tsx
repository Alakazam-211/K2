// @vitest-environment jsdom
// Settings → Terminal: every row changes what terminals use, and says so.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-settings')>()
  return { ...actual, settingsGet: vi.fn(async () => ({})), settingsUpdate: vi.fn(async () => ({})) }
})

import { settingsUpdate } from '@/lib/daemon-settings'
import { TerminalSection, TERMINAL_MANIFEST } from './TerminalSection'
import { useSettingsStore } from '@/stores/settings'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { defaultKesselConfig, terminalFontStack } from '@/kessel/config'

const realUpdate = useSettingsStore.getState().updateTerminalSettings
const update = vi.fn()

beforeEach(() => {
  update.mockReset()
  update.mockImplementation(async (partial: Record<string, unknown>) => {
    useSettingsStore.setState((s) => ({ terminal: { ...s.terminal, ...partial } }))
  })
  useSettingsStore.setState((s) => ({
    terminal: { ...s.terminal, fontFamily: 'MesloLGM Nerd Font', naturalTextEditing: true },
    updateTerminalSettings: update as typeof realUpdate,
  }))
  useTerminalSettingsStore.setState({ fontSize: 13, painter: 'webgl', lineHeightMultiplier: 1.2, charTracking: 1.2 })
})
afterEach(() => {
  cleanup()
  useSettingsStore.setState({ updateTerminalSettings: realUpdate })
})

describe('terminalFontStack', () => {
  it('is the default stack for the default font or none, and puts a picked font first', () => {
    expect(terminalFontStack(undefined)).toBe(defaultKesselConfig.font.family)
    expect(terminalFontStack('MesloLGM Nerd Font')).toBe(defaultKesselConfig.font.family)
    expect(terminalFontStack('Fira Code')).toBe(`'Fira Code', ${defaultKesselConfig.font.family}`)
    expect(terminalFontStack("Evil'; }")).toBe(`'Evil', ${defaultKesselConfig.font.family}`)
  })
})

describe('TerminalSection', () => {
  it('Size is the size ⌘+ and ⌘− change, clamped, and says Saved', () => {
    render(<TerminalSection />)
    expect(document.querySelector('[data-settings-id="terminal.font-size"]')?.textContent).toContain(
      '⌘+ and ⌘− change it too. On this computer only.',
    )
    for (const gone of ['Cursor style', 'Scrollback', 'Renderer']) expect(screen.queryByText(gone)).toBeNull()
    fireEvent.change(screen.getByRole('spinbutton', { name: 'Size, exact value' }), { target: { value: '16' } })
    expect(useTerminalSettingsStore.getState().fontSize).toBe(16)
    expect(document.querySelector('[data-save-note]')).toBeTruthy()
    useTerminalSettingsStore.getState().setFontSize(99)
    expect(useTerminalSettingsStore.getState().fontSize).toBe(24)
  })

  it('spacing needs the GPU: off, the rows are disabled and say why', () => {
    useTerminalSettingsStore.setState({ painter: 'dom' })
    render(<TerminalSection />)
    expect((screen.getByRole('slider', { name: 'Line spacing' }) as HTMLInputElement).disabled).toBe(true)
    expect((screen.getByRole('slider', { name: 'Letter spacing' }) as HTMLInputElement).disabled).toBe(true)
    expect(screen.getAllByText('Needs Draw with the GPU, below.')).toHaveLength(2)
    expect((screen.getByRole('button', { name: 'Reset spacing' }) as HTMLButtonElement).disabled).toBe(true)
  })

  it('Reset spacing puts both back to the default', () => {
    useTerminalSettingsStore.setState({ lineHeightMultiplier: 1.4, charTracking: 1.1 })
    render(<TerminalSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Reset spacing' }))
    expect(useTerminalSettingsStore.getState().lineHeightMultiplier).toBe(1.2)
    expect(useTerminalSettingsStore.getState().charTracking).toBe(1.2)
  })

  it('Mac text shortcuts saves to the server and says Saved', async () => {
    render(<TerminalSection />)
    await act(async () => {
      fireEvent.click(screen.getByRole('switch', { name: 'Mac text shortcuts' }))
    })
    expect(update).toHaveBeenCalledWith({ naturalTextEditing: false })
    expect(document.querySelector('[data-save-note]')).toBeTruthy()
  })

  it('a refused save says so', async () => {
    update.mockImplementation(async () => {})
    render(<TerminalSection />)
    await act(async () => {
      fireEvent.click(screen.getByRole('switch', { name: 'Mac text shortcuts' }))
    })
    expect(document.querySelector('[data-save-error]')?.textContent).toContain('Couldn’t save')
  })

  describe('with the real store setter', () => {
    beforeEach(() => {
      vi.mocked(settingsUpdate).mockReset()
      useSettingsStore.setState({ updateTerminalSettings: realUpdate })
    })

    it('a refused save puts the switch back and says so, never Saved', async () => {
      vi.mocked(settingsUpdate).mockRejectedValueOnce(new Error('settings_update 403: forbidden'))
      render(<TerminalSection />)
      const toggle = screen.getByRole('switch', { name: 'Mac text shortcuts' })
      await act(async () => {
        fireEvent.click(toggle)
      })
      expect(settingsUpdate).toHaveBeenCalledWith({ terminal: expect.objectContaining({ naturalTextEditing: false }) })
      expect(useSettingsStore.getState().terminal.naturalTextEditing).toBe(true)
      expect(toggle.getAttribute('aria-checked')).toBe('true')
      expect(document.querySelector('[data-save-error]')?.textContent).toContain('Couldn’t save')
      expect(document.querySelector('[data-save-note]')).toBeNull()
    })

    it('a save the server keeps says Saved', async () => {
      vi.mocked(settingsUpdate).mockImplementationOnce(async (u: Record<string, unknown>) => ({ terminal: u.terminal }) as never)
      render(<TerminalSection />)
      await act(async () => {
        fireEvent.click(screen.getByRole('switch', { name: 'Mac text shortcuts' }))
      })
      expect(useSettingsStore.getState().terminal.naturalTextEditing).toBe(false)
      expect(document.querySelector('[data-save-note]')).toBeTruthy()
      expect(document.querySelector('[data-save-error]')).toBeNull()
    })
  })

  it('every search entry points at a row on the page', () => {
    render(<TerminalSection />)
    for (const e of TERMINAL_MANIFEST) expect(document.querySelector(`[data-settings-id="${e.id}"]`)).toBeTruthy()
  })
})
