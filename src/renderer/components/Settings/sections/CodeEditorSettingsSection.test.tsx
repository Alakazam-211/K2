// @vitest-environment jsdom
// Settings → Code: rows say what saved; a custom theme is deleted only after
// a confirmation, and a failed delete says so and keeps the theme.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))
// The preview editor is CodeMirror; the page's rows are what's under test.
vi.mock('@/components/FileViewerPane/CodeEditor', () => ({
  EDITOR_FONTS: [{ id: 'MesloLGM Nerd Font', label: 'MesloLGM Nerd Font' }, { id: 'Fira Code', label: 'Fira Code' }],
  CodeEditor: () => <div data-testid="preview-editor" />,
}))

import { CodeEditorSettingsSection, CODE_EDITOR_MANIFEST } from './CodeEditorSettingsSection'
import { useSettingsStore } from '@/stores/settings'
import { useCustomThemesStore } from '@/stores/custom-themes'

const realUpdate = useSettingsStore.getState().updateEditorSettings
const realDelete = useCustomThemesStore.getState().deleteCustomTheme
const update = vi.fn()
const del = vi.fn()
const theme = (id: string, name: string) => ({ id, name, path: `/tmp/${id}.json`, colors: { bg: '#000000' }, highlight: {}, isLight: false })

beforeEach(() => {
  update.mockReset()
  update.mockImplementation(async (partial: Record<string, unknown>) => {
    useSettingsStore.setState((s) => ({ editor: { ...s.editor, ...partial } }))
  })
  del.mockReset()
  del.mockImplementation(async (id: string) => {
    useCustomThemesStore.setState((s) => ({ customThemes: s.customThemes.filter((t) => t.id !== id) }))
  })
  useSettingsStore.setState((s) => ({
    editor: { ...s.editor, theme: 'custom:midnight', fontFamily: 'MesloLGM Nerd Font', wordWrap: false },
    updateEditorSettings: update as typeof realUpdate,
  }))
  useCustomThemesStore.setState({ customThemes: [theme('custom:midnight', 'Midnight'), theme('custom:mint', 'Mint')] as never, deleteCustomTheme: del })
})
afterEach(() => {
  cleanup()
  useSettingsStore.setState({ updateEditorSettings: realUpdate })
  useCustomThemesStore.setState({ customThemes: [], deleteCustomTheme: realDelete })
})

describe('CodeEditorSettingsSection', () => {
  it('a switch saves and says Saved', async () => {
    render(<CodeEditorSettingsSection />)
    await act(async () => {
      fireEvent.click(screen.getByRole('switch', { name: 'Wrap long lines' }))
    })
    expect(update).toHaveBeenCalledWith({ wordWrap: true })
    expect(document.querySelector('[data-settings-id="code-editor.word-wrap"] [data-save-note]')).toBeTruthy()
  })

  it('ligatures are off for a font without them, and the row says why', () => {
    render(<CodeEditorSettingsSection />)
    expect((screen.getByRole('switch', { name: 'Font ligatures' }) as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(/MesloLGM Nerd Font has no ligatures/)).toBeTruthy()
  })

  it('Delete asks first; deleting the theme in use goes back to Follow app and says so', async () => {
    render(<CodeEditorSettingsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Delete Midnight' }))
    expect(del).not.toHaveBeenCalled()
    expect(screen.getByTestId('code-theme-delete').textContent).toContain('goes back to following the app’s look')
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Delete theme' }))
    })
    expect(del).toHaveBeenCalledWith('custom:midnight')
    expect(update).toHaveBeenCalledWith({ theme: 'follow-app' })
    expect(screen.getByTestId('code-theme-note').textContent).toBe('Midnight is deleted. The editor follows the app’s look again.')
  })

  it('the old default shows as Follow app (default), first in the list, then the named themes and yours', () => {
    useSettingsStore.setState((s) => ({ editor: { ...s.editor, theme: 'k2so-dark' } }))
    render(<CodeEditorSettingsSection />)
    const trigger = screen.getByRole('button', { name: 'Theme' })
    expect(trigger.textContent).toContain('Follow app (default)')
    fireEvent.click(trigger)
    const values = Array.from(screen.getByTestId('setting-dropdown-menu').querySelectorAll('button[data-value]')).map((b) => b.getAttribute('data-value'))
    expect(values).toEqual(['follow-app', 'k2-dark', 'one-dark', 'dracula', 'nord', 'github-dark', 'gruvbox-dark', 'ayu-mirage', 'parchment', 'custom:midnight', 'custom:mint'])
    expect(screen.getByTestId('setting-dropdown-menu').querySelector('button[data-value="follow-app"]')?.hasAttribute('data-selected')).toBe(true)
  })

  it('picking Follow app saves it and says Saved; a named theme saves too', async () => {
    render(<CodeEditorSettingsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Theme' }))
    await act(async () => {
      fireEvent.click(screen.getByTestId('setting-dropdown-menu').querySelector('button[data-value="follow-app"]')!)
    })
    expect(update).toHaveBeenCalledWith({ theme: 'follow-app' })
    expect(useSettingsStore.getState().editor.theme).toBe('follow-app')
    expect(document.querySelector('[data-settings-id="code-editor.theme"] [data-save-note]')).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: 'Theme' }))
    await act(async () => {
      fireEvent.click(screen.getByTestId('setting-dropdown-menu').querySelector('button[data-value="dracula"]')!)
    })
    expect(update).toHaveBeenLastCalledWith({ theme: 'dracula' })
    expect(screen.getByRole('button', { name: 'Theme' }).textContent).toContain('Dracula')
  })

  it('a failed delete keeps the theme and the dialog, with the reason', async () => {
    del.mockImplementation(async () => {
      throw new Error('theme file is locked')
    })
    render(<CodeEditorSettingsSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Delete Mint' }))
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Delete theme' }))
    })
    expect(screen.getByTestId('dialog-error').textContent).toContain('The server said: theme file is locked.')
    expect(screen.getByTestId('code-theme-custom:mint')).toBeTruthy()
    expect(update).not.toHaveBeenCalled()
  })

  it('every search entry points at a row on the page', () => {
    render(<CodeEditorSettingsSection />)
    for (const e of CODE_EDITOR_MANIFEST) expect(document.querySelector(`[data-settings-id="${e.id}"]`)).toBeTruthy()
  })
})
