// @vitest-environment jsdom
// The Settings search palette on its own: keyboard and mouse picks, the
// empty state, and an Escape that never reaches a window listener behind it.
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { SettingsSearchModal } from './SettingsSearchModal'
import type { SettingEntry } from './searchManifest'

const entries: SettingEntry[] = [
  { id: 'terminal.font-size', section: 'terminal', label: 'Font Size' },
  { id: 'terminal.font-family', section: 'terminal', label: 'Font Family' },
  { id: 'timer.timezone', section: 'timer', label: 'Timezone', group: 'Clock' },
]

function input(): HTMLInputElement {
  return screen.getByPlaceholderText(/Search settings/) as HTMLInputElement
}

Element.prototype.scrollIntoView = vi.fn()
afterEach(() => cleanup())

describe('SettingsSearchModal', () => {
  it('focuses the input and lists matches with their section', () => {
    render(<SettingsSearchModal entries={entries} onPick={vi.fn()} onClose={vi.fn()} />)
    expect(document.activeElement).toBe(input())
    fireEvent.change(input(), { target: { value: 'font' } })
    const rows = document.querySelectorAll('[data-result-index]')
    expect([...rows].map((r) => r.textContent)).toEqual(['TerminalFont Size', 'TerminalFont Family'])
    expect(screen.getByText('2 results')).toBeTruthy()
  })

  it('arrows move the selection and Enter picks it; a click picks too', () => {
    const onPick = vi.fn()
    render(<SettingsSearchModal entries={entries} onPick={onPick} onClose={vi.fn()} />)
    fireEvent.change(input(), { target: { value: 'font' } })
    fireEvent.keyDown(input(), { key: 'ArrowDown' })
    fireEvent.keyDown(input(), { key: 'ArrowDown' })
    fireEvent.keyDown(input(), { key: 'ArrowUp' })
    fireEvent.keyDown(input(), { key: 'ArrowDown' })
    fireEvent.keyDown(input(), { key: 'Enter' })
    expect(onPick).toHaveBeenLastCalledWith(entries[1])
    fireEvent.click(screen.getByText('Font Size'))
    expect(onPick).toHaveBeenLastCalledWith(entries[0])
  })

  it('says when nothing matches, and Enter then picks nothing', () => {
    const onPick = vi.fn()
    render(<SettingsSearchModal entries={entries} onPick={onPick} onClose={vi.fn()} />)
    fireEvent.change(input(), { target: { value: 'zzz' } })
    expect(screen.getByText('No settings match "zzz"')).toBeTruthy()
    fireEvent.keyDown(input(), { key: 'Enter' })
    expect(onPick).not.toHaveBeenCalled()
  })

  it('Escape closes it and never reaches a window keydown listener behind it', () => {
    const behind = vi.fn()
    window.addEventListener('keydown', behind)
    try {
      const onClose = vi.fn()
      render(<SettingsSearchModal entries={entries} onPick={vi.fn()} onClose={onClose} />)
      fireEvent.keyDown(input(), { key: 'Escape' })
      expect(onClose).toHaveBeenCalled()
      fireEvent.keyDown(window, { key: 'Escape' })
      expect(behind).not.toHaveBeenCalled()
      // Other keys still reach it.
      fireEvent.keyDown(window, { key: 'a' })
      expect(behind).toHaveBeenCalledTimes(1)
    } finally {
      window.removeEventListener('keydown', behind)
    }
  })
})
