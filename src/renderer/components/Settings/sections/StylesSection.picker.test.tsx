// @vitest-environment jsdom
//
// Bezel is retired: compiled, but not in the picker
// (prd-themes-terminal-glass-v1 §7).

import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen, within } from '@testing-library/react'

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

  it('search finds Terminal, and finds it by its old name, but not Bezel', () => {
    const picker = STYLES_MANIFEST.find((e) => e.id === 'styles.style-picker')!
    expect(picker.keywords).toEqual(expect.arrayContaining(['terminal', 'square', 'glass']))
    expect(picker.keywords).not.toContain('bezel')
  })
})
