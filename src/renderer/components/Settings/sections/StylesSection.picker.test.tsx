// @vitest-environment jsdom
//
// Bezel is retired: compiled, but not in the picker
// (prd-themes-terminal-glass-v1 §7).

import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (pick: (s: { updateStyleSettings: () => Promise<void> }) => unknown) =>
    pick({ updateStyleSettings: async () => undefined }),
}))

import { useStyleStore } from '@/stores/style'
import { getStyle } from '@/styles.generated'
import { STYLES_MANIFEST, StylesSection } from './StylesSection'

afterEach(cleanup)

const cards = (): HTMLElement[] =>
  Array.from(document.querySelectorAll('[data-settings-id="styles.style-picker"] button')) as HTMLElement[]
const cardNames = (): (string | null)[] => cards().map((c) => c.querySelector('span')?.textContent ?? null)

describe('the Styles picker', () => {
  it('lists Terminal and Glass, and not Bezel, though Bezel still compiles', () => {
    render(<StylesSection />)
    expect(cardNames().sort()).toEqual(['Glass', 'Terminal'])
    expect(getStyle('bezel')).toBeDefined()
  })

  it('a user arriving from Bezel sees Glass as the active card', () => {
    useStyleStore.getState().applyStyle({ styleId: 'bezel', paletteId: 'graphite' })
    render(<StylesSection />)
    const active = cards().find((c) => within(c).queryByText('Active'))
    expect(active?.querySelector('span')?.textContent).toBe('Glass')
    expect(screen.queryByText('Bezel')).toBeNull()
  })

  it('checks no scheme while the detail shows a style that is not the active one', () => {
    // A style with no card: applyStyle can't reach this any more (old Glass
    // aliases to Glass), so the state is set directly.
    useStyleStore.setState({ styleId: 'glass-classic', paletteId: 'obsidian', schemeMode: 'dark' })
    render(<StylesSection />)
    const group = screen.getByRole('radiogroup', { name: 'Scheme' })
    expect(within(group).getAllByRole('radio').filter((r) => r.getAttribute('aria-checked') === 'true')).toHaveLength(0)
  })

  it('arrow keys on the scheme move focus without committing a change', () => {
    useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'dark' })
    render(<StylesSection />)
    const group = screen.getByRole('radiogroup', { name: 'Scheme' })
    const dark = within(group).getByRole('radio', { name: 'Dark' })
    expect(dark.getAttribute('aria-checked')).toBe('true')
    dark.focus()
    fireEvent.keyDown(dark, { key: 'ArrowRight' })
    expect(document.activeElement).toBe(within(group).getByRole('radio', { name: 'Auto' }))
    expect(useStyleStore.getState().schemeMode).toBe('dark')
    fireEvent.click(within(group).getByRole('radio', { name: 'Auto' }))
    expect(useStyleStore.getState().schemeMode).toBe('auto')
  })

  const paletteCards = (): string[] =>
    Array.from(document.querySelectorAll('[data-settings-id="styles.palette"] button')).map(
      (b) => b.querySelector(':scope > span:last-child')?.textContent ?? '',
    )

  it('shows three palette cards for each style, with no scheme restriction label', () => {
    useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'dark' })
    render(<StylesSection />)
    expect(paletteCards()).toEqual(['Charcoal', 'Paper', 'Yellow'])
    expect(screen.queryByText('Light only')).toBeNull()
    cleanup()
    useStyleStore.getState().applyStyle({ styleId: 'glass', paletteId: 'obsidian', schemeMode: 'dark' })
    render(<StylesSection />)
    expect(paletteCards()).toEqual(['Obsidian', 'Veil', 'Yellow'])
  })

  it('Dark and Light show no Auto choosers; Auto shows one per appearance, each with all three', () => {
    useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'dark' })
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

  it('the accent well sets a colour; Reset to default returns to the theme, and starts disabled', () => {
    useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'dark' })
    useStyleStore.getState().resetAccent()
    render(<StylesSection />)
    const reset = screen.getByRole('button', { name: 'Reset to default' }) as HTMLButtonElement
    expect(reset.disabled).toBe(true)
    fireEvent.change(screen.getByTestId('accent-well'), { target: { value: '#ff00aa' } })
    expect(useStyleStore.getState().accent).toBe('#ff00aa')
    expect(document.documentElement.style.getPropertyValue('--color-accent')).toBe('#ff00aa')
    expect(reset.disabled).toBe(false)
    fireEvent.click(reset)
    expect(useStyleStore.getState().accent).toBeNull()
    expect(document.documentElement.style.getPropertyValue('--color-accent')).toBe('')
  })

  it('search finds Terminal, and finds it by its old name, but not Bezel', () => {
    const picker = STYLES_MANIFEST.find((e) => e.id === 'styles.style-picker')!
    expect(picker.keywords).toEqual(expect.arrayContaining(['terminal', 'square', 'glass']))
    expect(picker.keywords).not.toContain('bezel')
  })
})
