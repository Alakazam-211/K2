// @vitest-environment jsdom
// 0.46.1 — completion sound volume + choice (General → Workspaces, under
// the completion-sound switch). Both are daemon settings written through
// `/cli/settings/update`; Preview plays the chosen sound at the chosen
// volume.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'

const settingsGet = vi.hoisted(() => vi.fn())
const settingsUpdate = vi.hoisted(() => vi.fn())
const settingsReset = vi.hoisted(() => vi.fn())
const previewCompletionSound = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
vi.mock('@/lib/daemon-settings', () => ({ settingsGet, settingsUpdate, settingsReset }))
vi.mock('@/lib/completion-sound', () => ({ previewCompletionSound }))

import { GENERAL_MANIFEST, WorkspaceBehaviorRows } from './GeneralSection'
import { COMPLETION_SOUND_VOLUME_SAVE_DELAY_MS, useSettingsStore } from '@/stores/settings'

/** The daemon echoes the merged settings; this one keeps what it was sent. */
function echo(updates: Record<string, unknown>): Record<string, unknown> {
  const s = useSettingsStore.getState()
  return {
    terminal: s.terminal,
    keybindings: s.keybindings,
    completionSoundEnabled: true,
    completionSoundVolume: s.completionSoundVolume,
    completionSound: s.completionSound,
    ...updates,
  }
}

beforeEach(() => {
  vi.useFakeTimers()
  settingsGet.mockReset()
  settingsUpdate.mockReset()
  settingsUpdate.mockImplementation(async (updates: Record<string, unknown>) => echo(updates))
  previewCompletionSound.mockReset()
  useSettingsStore.setState({
    completionSoundEnabled: true,
    completionSoundVolume: 50,
    completionSound: 'chime',
  })
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

function slider(): HTMLInputElement {
  return screen.getByRole('slider', { name: 'Completion sound volume' }) as HTMLInputElement
}

function soundRow(): HTMLElement {
  const el = document.querySelector<HTMLElement>('[data-settings-id="general.completion-sound-choice"]')
  if (!el) throw new Error('completion sound choice row not rendered')
  return el
}

describe('Completion sound volume and choice', () => {
  it('sits right under the completion-sound switch with today\'s defaults', () => {
    render(<WorkspaceBehaviorRows />)
    const ids = Array.from(document.querySelectorAll('[data-settings-id]')).map((el) =>
      el.getAttribute('data-settings-id'),
    )
    const sw = ids.indexOf('general.completion-sound')
    expect(sw).toBeGreaterThanOrEqual(0)
    expect(ids.slice(sw, sw + 3)).toEqual([
      'general.completion-sound',
      'general.completion-sound-choice',
      'general.completion-sound-volume',
    ])
    expect(slider().value).toBe('50')
    expect(screen.getByText('50%')).toBeTruthy()
    expect(within(soundRow()).getByRole('button', { name: 'Completion sound' }).textContent).toContain(
      'Chime (default)',
    )
  })

  it('is findable in settings search', () => {
    const volume = GENERAL_MANIFEST.find((e) => e.id === 'general.completion-sound-volume')
    const choice = GENERAL_MANIFEST.find((e) => e.id === 'general.completion-sound-choice')
    expect(volume?.group).toBe('Workspaces')
    expect(volume?.keywords).toContain('volume')
    expect(choice?.keywords).toContain('ping')
  })

  it('the volume slider moves at once and writes the daemon once, after the drag rests', async () => {
    render(<WorkspaceBehaviorRows />)

    fireEvent.change(slider(), { target: { value: '60' } })
    fireEvent.change(slider(), { target: { value: '75' } })
    fireEvent.change(slider(), { target: { value: '80' } })

    expect(useSettingsStore.getState().completionSoundVolume).toBe(80)
    expect(screen.getByText('80%')).toBeTruthy()
    expect(settingsUpdate).not.toHaveBeenCalled()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(COMPLETION_SOUND_VOLUME_SAVE_DELAY_MS)
    })

    expect(settingsUpdate).toHaveBeenCalledTimes(1)
    expect(settingsUpdate).toHaveBeenCalledWith({ completionSoundVolume: 80 })
    expect(useSettingsStore.getState().completionSoundVolume).toBe(80)
  })

  it('takes the daemon\'s clamped value from the echo', async () => {
    settingsUpdate.mockImplementation(async () => echo({ completionSoundVolume: 100 }))
    useSettingsStore.getState().setCompletionSoundVolume(100)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(COMPLETION_SOUND_VOLUME_SAVE_DELAY_MS)
    })
    expect(useSettingsStore.getState().completionSoundVolume).toBe(100)

    // Out-of-range input is clamped before it is ever sent.
    settingsUpdate.mockClear()
    settingsUpdate.mockImplementation(async (updates: Record<string, unknown>) => echo(updates))
    useSettingsStore.getState().setCompletionSoundVolume(140)
    await act(async () => {
      await vi.advanceTimersByTimeAsync(COMPLETION_SOUND_VOLUME_SAVE_DELAY_MS)
    })
    expect(settingsUpdate).toHaveBeenCalledWith({ completionSoundVolume: 100 })
  })

  it('offers the five sounds and saves the choice', async () => {
    render(<WorkspaceBehaviorRows />)
    fireEvent.click(within(soundRow()).getByRole('button', { name: 'Completion sound' }))
    const menu = screen.getByTestId('setting-dropdown-menu')
    const labels = within(menu).getAllByRole('button').map((b) => b.textContent?.trim())
    expect(labels).toEqual(['Chime (default)', 'Soft chime', 'Pop', 'Ding', 'Subtle tick'])

    await act(async () => {
      fireEvent.click(within(menu).getByRole('button', { name: 'Pop' }))
    })

    expect(settingsUpdate).toHaveBeenCalledTimes(1)
    expect(settingsUpdate).toHaveBeenCalledWith({ completionSound: 'pop' })
    expect(useSettingsStore.getState().completionSound).toBe('pop')
  })

  it('Preview plays the chosen sound at the chosen volume', () => {
    useSettingsStore.setState({ completionSound: 'ding', completionSoundVolume: 70 })
    render(<WorkspaceBehaviorRows />)

    fireEvent.click(screen.getByRole('button', { name: 'Preview' }))

    expect(previewCompletionSound).toHaveBeenCalledTimes(1)
    expect(previewCompletionSound).toHaveBeenCalledWith('ding', 70)
  })

  it('reads the daemon values, and an older daemon without them reads as the defaults', async () => {
    settingsGet.mockResolvedValueOnce(echo({ completionSoundVolume: 35, completionSound: 'soft' }))
    await act(async () => {
      await useSettingsStore.getState().fetchSettings()
    })
    expect(useSettingsStore.getState().completionSoundVolume).toBe(35)
    expect(useSettingsStore.getState().completionSound).toBe('soft')

    settingsGet.mockResolvedValueOnce({
      terminal: useSettingsStore.getState().terminal,
      keybindings: {},
    })
    await act(async () => {
      await useSettingsStore.getState().fetchSettings()
    })
    expect(useSettingsStore.getState().completionSoundVolume).toBe(50)
    expect(useSettingsStore.getState().completionSound).toBe('chime')

    settingsGet.mockResolvedValueOnce(echo({ completionSoundVolume: 'x', completionSound: 'airhorn' }))
    await act(async () => {
      await useSettingsStore.getState().fetchSettings()
    })
    expect(useSettingsStore.getState().completionSoundVolume).toBe(50)
    expect(useSettingsStore.getState().completionSound).toBe('chime')
  })
})
