// @vitest-environment jsdom
// homes-sync-v1 — the sync-server badge on the Home tab icon.

import { describe, it, expect, afterEach } from 'vitest'
import { render, screen, cleanup, act } from '@testing-library/react'
import { HomesSyncBadge, homesSyncBadgeView } from './HomesSyncBadge'
import { INITIAL_STATUS, useHomesSyncStatus, type HomesSyncStatus } from '@/lib/homes-sync/runner'

function st(patch: Partial<HomesSyncStatus>): HomesSyncStatus {
  return { ...INITIAL_STATUS, ...patch }
}

describe('HomesSyncBadge', () => {
  afterEach(() => {
    cleanup()
    useHomesSyncStatus.setState({ ...INITIAL_STATUS })
  })

  it('hides while sync is off or unsupported', () => {
    expect(homesSyncBadgeView(st({ phase: 'off' }))).toBeNull()
    expect(homesSyncBadgeView(st({ phase: 'unsupported' }))).toBeNull()
  })

  it('shows this computer as the sync server, with a tone for how it is going', () => {
    expect(homesSyncBadgeView(st({ phase: 'synced' }))).toEqual({
      tone: 'ok',
      glyph: 'computer',
      title: 'Homes sync with This computer',
    })
    expect(homesSyncBadgeView(st({ phase: 'synced', pending: 2 }))?.tone).toBe('paused')
    const off = homesSyncBadgeView(st({ phase: 'offline', pending: 3 }))
    expect(off?.tone).toBe('paused')
    expect(off?.title).toBe('Sync paused · 3 changes waiting for This computer')
    expect(homesSyncBadgeView(st({ phase: 'removed' }))?.tone).toBe('problem')
    // Another home server shows its initial.
    expect(homesSyncBadgeView(st({ phase: 'synced', serverLabel: 'rosson.k2.dev' }))?.glyph).toBe('R')
  })

  it('renders from the status store and follows it', () => {
    render(<HomesSyncBadge />)
    expect(screen.queryByTestId('homes-sync-badge')).toBeNull()
    act(() => useHomesSyncStatus.setState(st({ phase: 'synced' })))
    const badge = screen.getByTestId('homes-sync-badge')
    expect(badge.getAttribute('data-tone')).toBe('ok')
    expect(badge.getAttribute('title')).toBe('Homes sync with This computer')
    act(() => useHomesSyncStatus.setState(st({ phase: 'offline', pending: 1 })))
    expect(screen.getByTestId('homes-sync-badge').getAttribute('data-tone')).toBe('paused')
    expect(screen.getByTestId('homes-sync-badge').getAttribute('title')).toBe('Sync paused · 1 change waiting for This computer')
  })
})
