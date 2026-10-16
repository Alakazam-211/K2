// @vitest-environment jsdom
// The app font family + size (0.45.7): one setting shown in Settings → General
// (just below the app version) and in Editors / Fonts → Code.
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { AppFontRows } from './AppFontRows'
import { useSettingsStore } from '@/stores/settings'
import { GENERAL_MANIFEST } from '../sections/GeneralSection'
import { CODE_EDITOR_MANIFEST } from '../sections/CodeEditorSettingsSection'

const realUpdate = useSettingsStore.getState().updateEditorSettings
const update = vi.fn()

beforeEach(() => {
  update.mockReset()
  // The store's real update posts to the daemon; here it only applies.
  update.mockImplementation(async (partial: Record<string, unknown>) => {
    useSettingsStore.setState((s) => ({ editor: { ...s.editor, ...partial } }))
  })
  useSettingsStore.setState((s) => ({
    editor: { ...s.editor, fontFamily: 'MesloLGM Nerd Font', fontSize: 13 },
    updateEditorSettings: update as typeof realUpdate,
  }))
})

afterEach(() => {
  cleanup()
  useSettingsStore.setState({ updateEditorSettings: realUpdate })
})

async function pick(row: HTMLElement, trigger: string, option: string): Promise<void> {
  fireEvent.click(within(row).getByRole('button', { name: trigger }))
  const menu = screen.getByTestId('setting-dropdown-menu')
  await act(async () => {
    fireEvent.click(within(menu).getByRole('button', { name: option }))
  })
}

describe('General page order', () => {
  it('the app font sits below the app version and above the CLI version (Rosson 2026-10-16)', () => {
    const src = readFileSync(join(__dirname, '../sections/GeneralSection.tsx'), 'utf8')
    const version = src.indexOf('data-settings-id="general.app-version"')
    const font = src.indexOf('data-settings-id="general.font"')
    const cli = src.indexOf('CLI Version — right under App Version')
    expect(version).toBeGreaterThan(0)
    expect(font).toBeGreaterThan(version)
    expect(cli).toBeGreaterThan(font)
  })
})

describe('AppFontRows', () => {
  it('General and Code show the same setting: a change in one shows in the other', async () => {
    render(
      <>
        <div data-testid="general">
          <AppFontRows where="general" lastHasBorder={false} />
        </div>
        <div data-testid="code">
          <AppFontRows where="code-editor" />
        </div>
      </>,
    )
    const general = screen.getByTestId('general')
    const code = screen.getByTestId('code')
    expect(within(general).getByText('Font family')).toBeTruthy()
    expect(within(general).getByText('Font size')).toBeTruthy()
    expect(within(code).getByText('Font Family')).toBeTruthy()
    expect(within(code).getByText('Font Size')).toBeTruthy()
    expect(within(general).getByRole('button', { name: 'Font size' }).textContent).toContain('13px')
    expect(within(code).getByRole('button', { name: 'Font Size' }).textContent).toContain('13px')

    await pick(general, 'Font size', '16px')
    expect(update).toHaveBeenCalledWith({ fontSize: 16 })
    expect(useSettingsStore.getState().editor.fontSize).toBe(16)
    expect(within(code).getByRole('button', { name: 'Font Size' }).textContent).toContain('16px')

    await pick(code, 'Font Size', '11px')
    expect(update).toHaveBeenLastCalledWith({ fontSize: 11 })
    expect(within(general).getByRole('button', { name: 'Font size' }).textContent).toContain('11px')
  })

  it('search ids sit on the rows; General owns its border', () => {
    render(<AppFontRows where="general" lastHasBorder={false} />)
    const family = document.querySelector('[data-settings-id="general.font-family"]') as HTMLElement
    const size = document.querySelector('[data-settings-id="general.font-size"]') as HTMLElement
    expect(family.className).toContain('border-b')
    expect(size.className).not.toContain('border-b')
    cleanup()
    render(<AppFontRows where="code-editor" />)
    expect(document.querySelector('[data-settings-id="code-editor.font-family"]')).not.toBeNull()
    expect((document.querySelector('[data-settings-id="code-editor.font-size"]') as HTMLElement).className).toContain('border-b')
  })

  it('both pages have search entries, and General shows the rows below the app version', () => {
    const general = GENERAL_MANIFEST.filter((e) => e.id === 'general.font-family' || e.id === 'general.font-size')
    expect(general.map((e) => [e.id, e.section, e.group])).toEqual([
      ['general.font-family', 'general', 'General'],
      ['general.font-size', 'general', 'General'],
    ])
    const code = CODE_EDITOR_MANIFEST.filter((e) => e.id === 'code-editor.font-family' || e.id === 'code-editor.font-size')
    expect(code.map((e) => e.section)).toEqual(['code-editor', 'code-editor'])
    const dir = join(process.cwd(), 'src/renderer/components/Settings/sections')
    const gen = readFileSync(join(dir, 'GeneralSection.tsx'), 'utf8')
    const body = gen.slice(gen.indexOf("{tab === 'general' && ("))
    expect(body.indexOf('<AppFontRows where="general" lastHasBorder={false} />')).toBeGreaterThan(0)
    expect(body.indexOf('<AppFontRows where="general"')).toBeGreaterThan(body.indexOf('data-settings-id="general.app-version"'))
    const codeSrc = readFileSync(join(dir, 'CodeEditorSettingsSection.tsx'), 'utf8')
    expect(codeSrc).toContain('<AppFontRows where="code-editor" />')
  })
})
