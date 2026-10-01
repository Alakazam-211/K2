// @vitest-environment jsdom
// prd-tickets-badge-orphans TB8/TB17 + Appa A3 — what the Tickets tab badge
// paints for each store state: a fresh count; the last count dimmed with
// "may be out of date"; a dimmed `?` on a server too old to count; nothing
// for a 0 (fresh or stale).
import { afterEach, describe, expect, it, vi } from 'vitest'
import { cleanup, render, screen } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main' }),
}))

import { PageTab, badgeText, ticketsBadgeProps } from './PageTabs'

afterEach(() => cleanup())

function renderTickets(state: { waitingCount: number; waitingStale: boolean; waitingUnsupported: boolean }): void {
  const p = ticketsBadgeProps(state)
  render(
    <PageTab
      selected={false}
      onSelect={vi.fn()}
      badge={p.badge}
      badgeStale={p.badgeStale}
      badgeTitle={p.badgeTitle}
      title="Tickets"
    >
      Tickets
    </PageTab>,
  )
}

describe('Tickets badge', () => {
  it('a fresh count shows the number, not dimmed, no tooltip', () => {
    renderTickets({ waitingCount: 2, waitingStale: false, waitingUnsupported: false })
    const b = screen.getByTestId('page-tab-badge')
    expect(b.textContent).toBe('2')
    expect(b.dataset.stale).toBe('false')
    expect(b.className).not.toContain('opacity-50')
    expect(b.getAttribute('title')).toBeNull()
  })

  it('a stale count keeps the number, dimmed, with "may be out of date"', () => {
    renderTickets({ waitingCount: 2, waitingStale: true, waitingUnsupported: false })
    const b = screen.getByTestId('page-tab-badge')
    expect(b.textContent).toBe('2')
    expect(b.dataset.stale).toBe('true')
    expect(b.className).toContain('opacity-50')
    expect(b.getAttribute('title')).toBe('Tickets count may be out of date')
  })

  it('a server without tickets-list-all shows a dimmed ? with "too old to count tickets"', () => {
    renderTickets({ waitingCount: 0, waitingStale: false, waitingUnsupported: true })
    const b = screen.getByTestId('page-tab-badge')
    expect(b.textContent).toBe('?')
    expect(b.dataset.stale).toBe('true')
    expect(b.className).toContain('opacity-50')
    expect(b.getAttribute('title')).toBe('This server is too old to count tickets')
  })

  it('a 0 shows no badge, fresh or stale (the reset after a server switch)', () => {
    renderTickets({ waitingCount: 0, waitingStale: false, waitingUnsupported: false })
    expect(screen.queryByTestId('page-tab-badge')).toBeNull()
    cleanup()
    renderTickets({ waitingCount: 0, waitingStale: true, waitingUnsupported: false })
    expect(screen.queryByTestId('page-tab-badge')).toBeNull()
  })

  it('badgeText caps at 99+', () => {
    expect(badgeText(100)).toBe('99+')
    expect(badgeText(99)).toBe('99')
    expect(badgeText(undefined)).toBeNull()
    expect(badgeText('?')).toBe('?')
  })
})
