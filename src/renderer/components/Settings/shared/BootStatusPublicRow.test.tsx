// @vitest-environment jsdom
// PRD boot-status-ghost §2.3: Settings → K2 Connect → Policies →
// "Hide this server's details from the internet" reads and writes the
// daemon's `bootStatusPublic` (thin client). Off = compat (the 0.46.3
// default), on = ghost.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

import { BootStatusPublicRow, BOOT_STATUS_PUBLIC_DETAIL } from './BootStatusPublicRow'
import { useSettingsStore, normalizeBootStatusPublic } from '@/stores/settings'

const setBootStatusPublic = vi.fn()

beforeEach(() => {
  setBootStatusPublic.mockReset()
  useSettingsStore.setState({ bootStatusPublic: 'compat', setBootStatusPublic })
})

afterEach(() => cleanup())

describe("Hide this server's details from the internet", () => {
  it('defaults off (compat) and turns ghost on', async () => {
    render(<BootStatusPublicRow />)
    const toggle = screen.getByRole('switch', { name: "Hide this server's details from the internet" })
    expect(toggle.getAttribute('aria-checked')).toBe('false')
    expect(screen.getByText(BOOT_STATUS_PUBLIC_DETAIL.compat)).toBeTruthy()
    await act(async () => {
      fireEvent.click(toggle)
    })
    expect(setBootStatusPublic).toHaveBeenCalledWith('ghost')
  })

  it('shows ghost on and turns it back to compat', async () => {
    useSettingsStore.setState({ bootStatusPublic: 'ghost' })
    render(<BootStatusPublicRow />)
    const toggle = screen.getByRole('switch', { name: "Hide this server's details from the internet" })
    expect(toggle.getAttribute('aria-checked')).toBe('true')
    expect(screen.getByText(BOOT_STATUS_PUBLIC_DETAIL.ghost)).toBeTruthy()
    await act(async () => {
      fireEvent.click(toggle)
    })
    expect(setBootStatusPublic).toHaveBeenCalledWith('compat')
  })

  it('an absent or unknown daemon value reads as compat (the 0.46.3 default)', () => {
    expect(normalizeBootStatusPublic(undefined)).toBe('compat')
    expect(normalizeBootStatusPublic(null)).toBe('compat')
    expect(normalizeBootStatusPublic('weird')).toBe('compat')
    expect(normalizeBootStatusPublic('ghost')).toBe('ghost')
  })
})
