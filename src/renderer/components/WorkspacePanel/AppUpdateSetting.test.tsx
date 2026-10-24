// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest'
import { render, screen, fireEvent, cleanup, waitFor } from '@testing-library/react'

const daemonCliGet = vi.fn()
const daemonCliPost = vi.fn()

vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliGet: primaryOnly((...args: unknown[]) => daemonCliGet(...args)),
    daemonCliPost: primaryOnly((...args: unknown[]) => daemonCliPost(...args)),
  }
})

import { primaryScope } from '@/kessel/server-scope'
import { AppUpdateSetting, parseRehost } from './AppUpdateSetting'

const answer = (whenNoOverlap: string, wave: unknown = null) => ({
  ok: true,
  app: 'hearth',
  rehost: { policy: 'auto', window: null, whenNoOverlap, fingerprint: null, due: null, sessions: 'sealed', heldFailed: false, wave },
})

beforeEach(() => {
  daemonCliGet.mockReset()
  daemonCliPost.mockReset()
})

afterEach(() => cleanup())

describe('the App panel update setting (Rosson Q4 + Q16)', () => {
  it('defaults to "pause briefly and update" and the opt-out posts defer', async () => {
    daemonCliGet.mockResolvedValue(answer('pause'))
    daemonCliPost.mockResolvedValue(answer('defer'))
    render(<AppUpdateSetting scope={primaryScope()} app="hearth" />)
    const pause = (await screen.findByLabelText('Pause briefly and update')) as HTMLInputElement
    const defer = screen.getByLabelText('Skip it and let me update it myself') as HTMLInputElement
    expect(daemonCliGet).toHaveBeenCalledWith('app/rehost', { app: 'hearth' })
    expect([pause.checked, defer.checked]).toEqual([true, false])
    fireEvent.click(defer)
    await waitFor(() => expect(daemonCliPost).toHaveBeenCalledWith('app/rehost/policy', { app: 'hearth', whenNoOverlap: 'defer' }))
    await waitFor(() => expect((screen.getByLabelText('Skip it and let me update it myself') as HTMLInputElement).checked).toBe(true))
  })

  it('a refused change goes back and says why; the wave’s word shows', async () => {
    daemonCliGet.mockResolvedValue(answer('pause', { state: 'deferred', code: 'no_headroom', detail: 'it needs 900 MiB' }))
    daemonCliPost.mockRejectedValue(new Error('apps_manage_off'))
    render(<AppUpdateSetting scope={primaryScope()} app="hearth" />)
    await screen.findByText('Last update: deferred: it needs 900 MiB')
    fireEvent.click(screen.getByLabelText('Skip it and let me update it myself'))
    await screen.findByText('apps_manage_off')
    expect((screen.getByLabelText('Pause briefly and update') as HTMLInputElement).checked).toBe(true)
  })

  it('an older daemon (no rehost block) shows nothing', () => {
    expect(parseRehost({ ok: true })).toBeNull()
    expect(parseRehost(answer('later'))).toBeNull()
    expect(parseRehost(answer('defer'))?.whenNoOverlap).toBe('defer')
  })
})
