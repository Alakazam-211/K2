// @vitest-environment jsdom
//
// Settings → Styles: the style list, the Colours card, the accent's hex
// field, Reset states, and saves that fail. Bezel is retired: compiled, but
// not in the picker (prd-themes-terminal-glass-v1 §7).

import React from 'react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
const openSettings = vi.fn()
vi.mock('@/stores/settings', () => ({
  useSettingsStore: (pick: (s: { updateStyleSettings: () => Promise<void>; openSettings: typeof openSettings }) => unknown) =>
    pick({ updateStyleSettings: async () => undefined, openSettings }),
}))

import { useStyleStore } from '@/stores/style'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
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
const rowNamed = (name: string): HTMLElement => rows().find((r) => rowName(r) === name)!

const paletteCards = (): string[] =>
  Array.from(document.querySelectorAll('[data-settings-id="styles.palette"] [data-palette-id]')).map(
    (b) => b.querySelector('[data-palette-name]')?.textContent ?? '',
  )

const useTerminalCharcoal = (): void => {
  useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'charcoal', schemeMode: 'dark' })
  useStyleStore.getState().resetAccent()
}

describe('the Styles list', () => {
  it('lists Glass (the default look) then Terminal, and not Bezel, though Bezel still compiles', () => {
    render(<StylesSection />)
    expect(rows().map(rowName)).toEqual(['Glass', 'Terminal'])
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
    fireEvent.click(rowNamed('Glass'))
    expect(useStyleStore.getState().styleId).toBe('glass')
    expect(rows().filter((r) => within(r).queryByText('In use')).map(rowName)).toEqual(['Glass'])
    expect(screen.getByRole('heading', { level: 2, name: 'Glass' })).toBeTruthy()
  })

  it('marks the style in use once, in the list: neutral, so the selected row keeps the one accent', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    expect(screen.getAllByText('In use')).toHaveLength(1)
    expect(within(rowNamed('Terminal')).getByText('In use').getAttribute('data-tone')).toBe('neutral')
  })

  it('list rows show keyboard focus in the theme\'s shape', () => {
    render(<StylesSection />)
    for (const r of rows()) {
      expect(r.hasAttribute('data-k2-focus')).toBe(true)
      expect(r.className).not.toContain('focus-visible:outline-')
    }
  })

  it('rows are tiles in Glass, and Terminal rows run the column’s width so names line up with the heading', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    for (const r of rows()) {
      expect(r.getAttribute('data-material')).toBe('tile')
      // Full width: out past the list's 16px padding on both sides, and the
      // same padding back inside (Glass puts its tile back on the heading's line).
      expect(r.className).toContain('-mx-4')
      expect(r.className).toContain('px-4')
    }
    // The selected row's accent bar is marked, so Glass can leave it out.
    expect(rowNamed('Terminal').querySelector('[data-row-bar]')).not.toBeNull()
    expect(rowNamed('Glass').querySelector('[data-row-bar]')).toBeNull()
  })

  it('describes Glass in plain words', () => {
    useStyleStore.getState().applyStyle({ styleId: 'glass', paletteId: 'obsidian', schemeMode: 'dark' })
    render(<StylesSection />)
    const text = document.body.textContent ?? ''
    expect(text).toContain('Rounded and calm, like the K2 phone app.')
    expect(text).not.toContain('things that float')
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
    // …and offers the way back to a listed style: the first, the default look.
    expect(screen.getByRole('button', { name: 'Use Glass' })).toBeTruthy()
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
    // One accent cue on the chosen card: its edge, no tint.
    const charcoal = pressed[0] as HTMLElement
    expect(charcoal.className).toContain('border-[var(--color-accent-text)]')
    expect(charcoal.className).not.toContain('bg-[var(--color-accent)]')
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
    const reset = screen.getByRole('button', { name: 'Reset the accent colour of Charcoal' }) as HTMLButtonElement
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

  it('names the palette it edits, and saves to that palette alone', () => {
    useTerminalCharcoal()
    localStorage.removeItem('k2.accent.terminal.paper')
    render(<StylesSection />)
    const row = document.querySelector('[data-settings-id="styles.accent"]') as HTMLElement
    expect(row.textContent).toContain('Accent colour, Charcoal')
    fireEvent.change(screen.getByTestId('accent-well'), { target: { value: '#ff00aa' } })
    expect(localStorage.getItem('k2.accent.terminal.charcoal')).toBe('#ff00aa')
    expect(localStorage.getItem('k2.accent.terminal.paper')).toBeNull()
    expect(localStorage.getItem('k2.accent.terminal')).toBeNull()
  })

  it('under Auto, edits the palette applied now and says which', () => {
    useStyleStore.getState().applyStyle({
      styleId: 'glass',
      schemeMode: 'auto',
      autoPalettes: { dark: 'obsidian', light: 'veil' },
    })
    useStyleStore.getState().resetAccent()
    render(<StylesSection />)
    const applied = useStyleStore.getState().resolvedPaletteId
    const name = getStyle('glass')!.palettes.find((p) => p.id === applied)!.name
    expect(screen.getByLabelText(`Accent colour, ${name}`)).toBeTruthy()
    fireEvent.change(screen.getByTestId('accent-well'), { target: { value: '#22aa66' } })
    expect(localStorage.getItem(`k2.accent.glass.${applied}`)).toBe('#22aa66')
    for (const other of ['obsidian', 'veil', 'yellow'].filter((p) => p !== applied)) {
      expect([other, localStorage.getItem(`k2.accent.glass.${other}`)]).toEqual([other, null])
    }
    useStyleStore.getState().resetAccent()
  })

  it('each palette card shows that palette’s own accent', () => {
    useTerminalCharcoal()
    localStorage.setItem('k2.accent.terminal.paper', '#3366cc')
    render(<StylesSection />)
    const swatchOf = (id: string): string => {
      const card = document.querySelector(`[data-palette-id="${id}"]`) as HTMLElement
      return (card.querySelectorAll('span.flex.h-6 > span')[3] as HTMLElement).style.backgroundColor
    }
    expect(swatchOf('paper')).toBe('rgb(51, 102, 204)')
    expect(swatchOf('charcoal')).toBe('rgb(59, 130, 246)')
    localStorage.removeItem('k2.accent.terminal.paper')
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

  it('a colour that saves says Saved in its row', () => {
    useTerminalCharcoal()
    render(<StylesSection />)
    fireEvent.change(screen.getByTestId('accent-well'), { target: { value: '#ff00aa' } })
    const row = document.querySelector('[data-settings-id="styles.accent"]') as HTMLElement
    expect(row.querySelector('[data-save-note]')?.textContent).toBe('Saved')
    expect(document.querySelectorAll('[data-save-note]')).toHaveLength(1)
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

describe('Reset to defaults', () => {
  const realConfirm = useConfirmDialogStore.getState().confirm
  afterEach(() => useConfirmDialogStore.setState({ confirm: realConfirm }))

  /** A customised look: Terminal / Paper / Light, spacious, accents on two
   *  palettes of each style, Frost and text weight changed. */
  const customise = (): void => {
    useStyleStore.getState().applyStyle({ styleId: 'terminal', paletteId: 'paper', schemeMode: 'light', gapsPreset: 'spacious' })
    localStorage.setItem('k2.accent.terminal.paper', '#ff00aa')
    localStorage.setItem('k2.accent.terminal.charcoal', '#00ff88')
    localStorage.setItem('k2.accent.glass.obsidian', '#3366cc')
    localStorage.setItem('k2.accent.glass.yellow', '#22aa66')
    localStorage.setItem('k2.accent.glass', 'yellow')
    localStorage.setItem('k2.dial.glass.frost', '3')
    localStorage.setItem('k2.textGamma.terminal.light', '2')
    localStorage.setItem('k2.textGamma.glass.dark', '1.5')
    useStyleStore.getState().applyStyle({})
  }
  const lookKeys = (): string[] =>
    Object.keys(localStorage).filter(
      (k) => /^k2\.(accent|dial|textGamma)\./.test(k) && !k.startsWith('k2.accent.stamp.'),
    )

  it('asks first, with Reset and the shared confirm; Cancel changes nothing', async () => {
    customise()
    const confirm = vi.fn(async () => false)
    useConfirmDialogStore.setState({ confirm })
    render(<StylesSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Reset to defaults' }))
    await waitFor(() => expect(confirm).toHaveBeenCalledTimes(1))
    expect(confirm).toHaveBeenCalledWith({
      title: 'Put Styles back to how K2 ships?',
      message: 'Your look, light and dark, palettes and accent colours go back to the defaults on this computer.',
      confirmLabel: 'Reset',
    })
    expect(useStyleStore.getState().styleId).toBe('terminal')
    expect(localStorage.getItem('k2.accent.terminal.paper')).toBe('#ff00aa')
  })

  it('after Reset: the shipped look, every palette its own accent, dials and text weight at their defaults', async () => {
    customise()
    useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) })
    render(<StylesSection />)
    fireEvent.click(screen.getByRole('button', { name: 'Reset to defaults' }))
    await waitFor(() => expect(useStyleStore.getState().styleId).toBe('glass'))
    const s = useStyleStore.getState()
    expect([s.styleId, s.schemeMode, s.autoPalettes, s.gapsPreset, s.accent]).toEqual([
      'glass',
      'auto',
      { dark: 'obsidian', light: 'veil' },
      '',
      null,
    ])
    expect(lookKeys()).toEqual([])
    expect(document.documentElement.style.getPropertyValue('--color-accent')).toBe('')
    expect([localStorage.getItem('k2.style'), localStorage.getItem('k2.scheme')]).toEqual(['glass', 'auto'])
    expect(screen.getByRole('heading', { level: 2, name: 'Glass' })).toBeTruthy()
    const row = document.querySelector('[data-settings-id="styles.reset-defaults"]') as HTMLElement
    expect(row.querySelector('[data-save-note]')?.textContent).toBe('Saved')
  })

  it('a reset that can’t be saved says so in its row', async () => {
    customise()
    useConfirmDialogStore.setState({ confirm: vi.fn(async () => true) })
    render(<StylesSection />)
    vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
      throw new Error('SecurityError')
    })
    fireEvent.click(screen.getByRole('button', { name: 'Reset to defaults' }))
    await waitFor(() => expect(screen.getByRole('alert').textContent).toContain("couldn't save it"))
    expect(screen.getByRole('alert').closest('[data-settings-id="styles.reset-defaults"]')).not.toBeNull()
    expect(useStyleStore.getState().styleId).toBe('glass')
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

  it('the hint is short, and the slider keeps a value slot so it ends where Frost does', () => {
    useTerminalSettingsStore.setState({ painter: 'webgl' })
    useTerminalCharcoal()
    render(<StylesSection />)
    const row = document.querySelector('[data-settings-id="styles.text-weight"]') as HTMLElement
    expect(row.textContent).toContain('Drag right for thicker terminal text. Saved for Terminal in dark mode.')
    const slider = screen.getByRole('slider', { name: 'Terminal text weight' })
    expect((slider.nextElementSibling as HTMLElement).className).toContain('w-10 flex-shrink-0')
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
