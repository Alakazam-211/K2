// @vitest-environment jsdom
//
// Settings → Styles: the style list, the Colours card, the accent's hex
// field, Reset states, and saves that fail. Bezel is retired: compiled, but
// not in the picker (prd-themes-terminal-glass-v1 §7).

import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
const openSettings = vi.fn()
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (pick: (s: { updateStyleSettings: () => Promise<void>; openSettings: typeof openSettings }) => unknown) =>
    pick({ updateStyleSettings: async () => undefined, openSettings }),
}))

import { useStyleStore } from '@/stores/style'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { getStyle } from '@/styles.generated'
import { STYLES_MANIFEST, StylesSection } from './StylesSection'

afterEach(() => {
  cleanup()
  vi.restoreAllMocks()
  openSettings.mockClear()
})

const rows = (): HTMLElement[] =>
  Array.from(document.querySelectorAll('[data-settings-id="styles.style-picker"] [data-style-row]')) as HTMLElement[]
const rowName = (row: HTMLElement): string | null => row.querySelector('[data-style-name]')?.textContent ?? null

const paletteCards = (): string[] =>
  Array.from(document.querySelectorAll('[data-settings-id="styles.palette"] [data-palette-id]')).map(
    (b) => b.querySelector('[data-palette-name]')?.textContent ?? '',
  )

const useTerminalCharcoal = (): void => {
  useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'dark' })
  useStyleStore.getState().resetAccent()
}

describe('the Styles list', () => {
  it('lists Terminal then Glass, and not Bezel, though Bezel still compiles', () => {
    render(<StylesSection />)
    expect(rows().map(rowName)).toEqual(['Terminal', 'Glass'])
    expect(getStyle('bezel')).toBeDefined()
  })

  it('a user arriving from Bezel sees Glass tagged In use, and marked as shown', () => {
    useStyleStore.getState().applyStyle({ styleId: 'bezel', paletteId: 'graphite' })
    render(<StylesSection />)
    const inUse = rows().filter((r) => within(r).queryByText('In use'))
    expect(inUse.map(rowName)).toEqual(['Glass'])
    expect(inUse[0].getAttribute('aria-current')).toBe('true')
    expect(screen.queryByText('Bezel')).toBeNull()
  })

  it('clicking a style uses it and moves the In use tag', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    fireEvent.click(rows()[1])
    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(rows().filter((r) => within(r).queryByText('In use')).map(rowName)).toEqual(['Glass'])
    expect(screen.getByRole('heading', { level: 2, name: 'Glass' })).toBeTruthy()
  })

  it('describes Terminal in plain words', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    const text = document.body.textContent ?? ''
    expect(text).toContain('Square and compact')
    for (const jargon of ['Style contract', 'zero-radius', 'polarity', 'painter =', 'Lower = thicker']) {
      expect(text).not.toContain(jargon)
    }
  })
})

describe('Colours', () => {
  it('shows no light or dark choice while the detail shows a style that is not in use', () => {
    // A style with no row: applyStyle can't reach this any more (old Glass
    // aliases to Glass), so the state is set directly.
    useStyleStore.setState({ styleId: 'glass-classic', paletteId: 'obsidian', schemeMode: 'dark' })
    render(<StylesSection />)
    const group = screen.getByRole('radiogroup', { name: 'Light or dark' })
    expect(within(group).getAllByRole('radio').filter((r) => r.getAttribute('aria-checked') === 'true')).toHaveLength(0)
    // …and offers the way back to a listed style.
    expect(screen.getByRole('button', { name: 'Use Terminal' })).toBeTruthy()
  })

  it('arrow keys on Light or dark move focus without committing a change', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    const group = screen.getByRole('radiogroup', { name: 'Light or dark' })
    const dark = within(group).getByRole('radio', { name: 'Dark' })
    expect(dark.getAttribute('aria-checked')).toBe('true')
    dark.focus()
    fireEvent.keyDown(dark, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(within(group).getByRole('radio', { name: 'Auto' }))
    expect(useStyleStore.getState().schemeMode).toBe('dark')
    fireEvent.click(within(group).getByRole('radio', { name: 'Auto' }))
    expect(useStyleStore.getState().schemeMode).toBe('auto')
  })

  it('shows three palette cards for each style, the one in use pressed', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    expect(paletteCards()).toEqual(['Charcoal', 'Paper', 'Yellow'])
    const pressed = Array.from(document.querySelectorAll('[data-palette-id][aria-pressed="true"]'))
    expect(pressed.map((b) => b.getAttribute('data-palette-id'))).toEqual(['charcoal'])
    expect(screen.queryByText('Light only')).toBeNull()
    cleanup()
    useStyleStore.getState().applyStyle({ styleId: 'glass', paletteId: 'obsidian', schemeMode: 'dark' })
    render(<StylesSection />)
    expect(paletteCards()).toEqual(['Obsidian', 'Veil', 'Yellow'])
  })

  it('a palette card is a focusable button with the shared focus ring', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    const paper = document.querySelector('[data-palette-id="paper"]') as HTMLButtonElement
    expect(paper.tagName).toBe('BUTTON')
    expect(paper.className).toContain('k2-focus')
    expect(paper.getAttribute('data-k2-chamfer')).toBe('surface')
    fireEvent.click(paper)
    expect(useStyleStore.getState().resolvedPaletteId).toBe('paper')
  })

  it('Dark and Light show no Auto choosers; Auto shows one per appearance, each with all three', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    expect(screen.queryByRole('radiogroup', { name: 'When my system is light' })).toBeNull()
    cleanup()
    useStyleStore.getState().applyStyle({ schemeMode: 'auto' })
    render(<StylesSection />)
    for (const name of ['When my system is light', 'When my system is dark']) {
      const group = screen.getByRole('radiogroup', { name })
      expect(within(group).getAllByRole('radio').map((r) => r.textContent)).toEqual(['Charcoal', 'Paper', 'Yellow'])
    }
  })

  it('an Auto chooser saves that side and keeps Auto, even a dark theme for a light system', () => {
    useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'auto' })
    render(<StylesSection />)
    const group = screen.getByRole('radiogroup', { name: 'When my system is light' })
    fireEvent.click(within(group).getByRole('radio', { name: 'Yellow' }))
    expect(useStyleStore.getState().schemeMode).toBe('auto')
    expect(useStyleStore.getState().autoPalettes?.light).toBe('yellow')
    expect(localStorage.getItem('k2.palette.light')).toBe('yellow')
  })
})

describe('the accent', () => {
  it('the colour well sets a colour; Reset returns to the theme, and starts disabled', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    const reset = screen.getByRole('button', { name: 'Reset' }) as HTMLButtonElement
    expect(reset.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('accent-well'), { target: { value: '#ff00aa' } })
    expect(useStyleStore.getState().accent).toBe('#ff00aa')
    expect(document.documentElement.style.getPropertyValue('--color-accent')).toBe('#ff00aa')
    expect((screen.getByTestId('accent-hex') as HTMLInputElement).value).toBe('#ff00aa')
    expect(reset.disabled).toBe(false)
    fireEvent.click(reset)
    expect(useStyleStore.getState().accent).toBeNull()
    expect(document.documentElement.style.getPropertyValue('--color-accent')).toBe('')
    expect(reset.disabled).toBe(true)
  })

  it('a typed hex applies at once, with or without the #', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    const hex = screen.getByTestId('accent-hex') as HTMLInputElement
    fireEvent.change(hex, { target: { value: '22AA66' } })
    expect(useStyleStore.getState().accent).toBe('#22aa66')
    fireEvent.blur(hex)
    expect(hex.value).toBe('#22aa66')
    expect(screen.queryByRole('alert')).toBeNull()
  })

  it('an invalid hex says so when you leave the field, and changes nothing', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    const hex = screen.getByTestId('accent-hex') as HTMLInputElement
    fireEvent.change(hex, { target: { value: '#12zz' } })
    expect(screen.queryByRole('alert')).toBeNull()
    fireEvent.blur(hex)
    expect(screen.getByRole('alert').textContent).toContain('Use six hex digits')
    expect(hex.getAttribute('aria-invalid')).toBe('true')
    expect(useStyleStore.getState().accent).toBeNull()
    fireEvent.change(hex, { target: { value: '#123456' } })
    expect(screen.queryByRole('alert')).toBeNull()
    expect(useStyleStore.getState().accent).toBe('#123456')
  })

  it('a colour that applies but cannot be saved says so in its row', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
      throw new Error('QuotaExceededError')
    })
    fireEvent.change(screen.getByTestId('accent-well'), { target: { value: '#ff00aa' } })
    expect(document.documentElement.style.getPropertyValue('--color-accent')).toBe('#ff00aa')
    const alert = screen.getByRole('alert')
    expect(alert.textContent).toContain("couldn't save it")
    expect(alert.closest('[data-settings-id="styles.accent"]')).not.toBeNull()
  })
})

describe('Terminals', () => {
  it('text weight: drag right for thicker (a lower glyph gamma); Reset only once changed', () => {
    useTerminalSettingsStore.setState({ painter: 'webgl' })
    useTerminalCharcoal()
    useStyleStore.getState().resetTextGamma()
    render(<StylesSection />)
    const reset = screen.getByRole('button', { name: 'Reset text weight' }) as HTMLButtonElement
    expect(reset.disabled).toBe(true)
    const slider = screen.getByRole('slider', { name: 'Terminal text weight' }) as HTMLInputElement
    const before = useStyleStore.getState().textGamma
    fireEvent.change(slider, { target: { value: String(Number(slider.value) + 0.5) } })
    expect(useStyleStore.getState().textGamma).toBeLessThan(before)
    expect(reset.disabled).toBe(false)
    fireEvent.click(reset)
    expect(useStyleStore.getState().textGamma).toBe(before)
    expect(reset.disabled).toBe(true)
  })

  it('with WebGL off the slider is disabled and points to Terminal settings', () => {
    useTerminalSettingsStore.setState({ painter: 'dom' })
    useTerminalCharcoal()
    render(<StylesSection />)
    const slider = screen.getByRole('slider', { name: 'Terminal text weight' }) as HTMLInputElement
    expect(slider.disabled).toBe(true)
    fireEvent.click(screen.getByRole('button', { name: 'Terminal settings' }))
    expect(openSettings).toHaveBeenCalledWith('terminal')
    useTerminalSettingsStore.setState({ painter: 'webgl' })
  })
})

describe('search', () => {
  it('finds Terminal, and finds it by its old name, but not Bezel', () => {
    const picker = STYLES_MANIFEST.find((e) => e.id === 'styles.style-picker')!
    expect(picker.keywords).toEqual(expect.arrayContaining(['terminal', 'square', 'glass']))
    expect(picker.keywords).not.toContain('bezel')
  })

  it('every entry points at a row on the page', () => {
    useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'auto' })
    render(<StylesSection />)
    for (const entry of STYLES_MANIFEST) {
      if (entry.id === 'styles.dials') continue // Glass only: checked below
      expect(document.querySelector(`[data-settings-id="${entry.id}"]`), entry.id).not.toBeNull()
    }
    cleanup()
    useStyleStore.getState().applyStyle({ styleId: 'glass', paletteId: 'obsidian', schemeMode: 'dark' })
    render(<StylesSection />)
    expect(document.querySelector('[data-settings-id="styles.dials"]')).not.toBeNull()
  })
})
