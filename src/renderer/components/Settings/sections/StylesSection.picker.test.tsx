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
    expect(cardNames().sort()).toEqual(['Liquid Glass', 'Terminal'])
    expect(getStyle('bezel')).toBeDefined()
  })

  it('a user arriving from Bezel sees Terminal as the active card', () => {
    useStyleStore.getState().applyStyle({ styleId: 'bezel', paletteId: 'graphite' })
    render(<StylesSection />)
    const active = cards().find((c) => within(c).queryByText('Active'))
    expect(active?.querySelector('span')?.textContent).toBe('Terminal')
    expect(screen.queryByText('Bezel')).toBeNull()
  })

  it('checks no scheme while the detail shows a style that is not the active one', () => {
    useStyleStore.getState().applyStyle({ styleId: 'glass-next', paletteId: 'obsidian', schemeMode: 'dark' })
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

  it('search finds Terminal, and finds it by its old name, but not Bezel', () => {
    const picker = STYLES_MANIFEST.find((e) => e.id === 'styles.style-picker')!
    expect(picker.keywords).toEqual(expect.arrayContaining(['terminal', 'square', 'glass']))
    expect(picker.keywords).not.toContain('bezel')
  })
})
