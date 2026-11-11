// The Desktop tab reads `desktop/status` the same way Settings does: an odd
// reply is a plain error, never a crash on `status.you`.
import { describe, expect, it, vi } from 'vitest'

const get = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: (...a: unknown[]) => get(...a), daemonCliPost: vi.fn() }))

import { fetchDesktopStatus } from './desktop-api'
import type { ServerScope } from '@/kessel/server-scope'
import { mayStartOf } from './desktop-state'

const scope = {} as ServerScope

describe('fetchDesktopStatus', () => {
  it('reads a full reply', async () => {
    get.mockResolvedValueOnce({ enabled: true, you: { role: 'owner', mayView: true, mayControl: true, mayManage: true } })
    const s = await fetchDesktopStatus(scope)
    expect(s.enabled).toBe(true)
    expect(s.you.mayManage).toBe(true)
  })

  it('keeps a virtual desktop\'s fields and the start right', async () => {
    get.mockResolvedValueOnce({
      enabled: true,
      desktop: 'virtual:ws-1',
      kind: 'agent',
      name: 'Builder',
      workspaceId: 'ws-1',
      agentsAllowed: true,
      reason: null,
      virtual: { hasBrowser: false, hold: { by: 'k2::builder', until: 5 } },
      you: { role: 'owner', mayView: true, mayControl: true, mayManage: true, mayStart: true },
    })
    const s = await fetchDesktopStatus(scope, 'virtual:ws-1')
    expect(get).toHaveBeenLastCalledWith(scope, 'desktop/status', { desktop: 'virtual:ws-1' })
    expect(s.desktop).toBe('virtual:ws-1')
    expect(s.kind).toBe('agent')
    expect(s.name).toBe('Builder')
    expect(s.workspaceId).toBe('ws-1')
    expect(s.agentsAllowed).toBe(true)
    expect(s.reason).toBeNull()
    expect(s.virtual).toEqual({ hasBrowser: false, hold: { by: 'k2::builder', until: 5 } })
    expect(mayStartOf(s)).toBe(true)
  })

  it('turns a reply without `you` into a plain error', async () => {
    get.mockResolvedValueOnce({ enabled: true })
    await expect(fetchDesktopStatus(scope)).rejects.toThrow('the reply didn’t say what you may do here')
  })

  it('turns an empty reply into a plain error', async () => {
    get.mockResolvedValueOnce({})
    await expect(fetchDesktopStatus(scope)).rejects.toThrow('the reply didn’t say whether Remote Desktop is on')
  })
})
