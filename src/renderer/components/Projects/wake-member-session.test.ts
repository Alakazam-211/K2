// Wake / live-attach Active obligation (PRD §4.3.1): client-watching
// surfaces must activate the member workspace before/with ensure-pinned-chat
// so active_reaper does not force-close the chat PTY after ~15s grace.

import { describe, it, expect, vi } from 'vitest'
import {
  activateOnLiveSessionAttach,
  resolveAllMemberSessions,
  wakeCanonicalMemberSession,
} from './wake-member-session'

describe('wakeCanonicalMemberSession', () => {
  it('activates the member workspace id before ensure-pinned-chat', async () => {
    const order: string[] = []
    const activateProject = vi.fn((id: string) => {
      order.push(`activate:${id}`)
    })
    const ensurePinnedChat = vi.fn(async (path: string) => {
      order.push(`ensure-pinned-chat:${path}`)
    })

    await wakeCanonicalMemberSession('ws-member-1', '/repos/alpha', {
      activateProject,
      ensurePinnedChat,
    })

    expect(activateProject).toHaveBeenCalledTimes(1)
    expect(activateProject).toHaveBeenCalledWith('ws-member-1')
    expect(ensurePinnedChat).toHaveBeenCalledTimes(1)
    expect(ensurePinnedChat).toHaveBeenCalledWith('/repos/alpha')
    // Order is load-bearing: Active must land before reaper can arm on spawn.
    expect(order).toEqual([
      'activate:ws-member-1',
      'ensure-pinned-chat:/repos/alpha',
    ])
  })

  it('uses the member workspace id (not a project-group id)', async () => {
    const activateProject = vi.fn()
    const ensurePinnedChat = vi.fn(async () => {})
    const memberId = 'proj_abc123'
    const groupId = 'grp_should_not_activate'

    await wakeCanonicalMemberSession(memberId, '/path', {
      activateProject,
      ensurePinnedChat,
    })

    expect(activateProject).toHaveBeenCalledWith(memberId)
    expect(activateProject).not.toHaveBeenCalledWith(groupId)
  })

  it('propagates ensure-pinned-chat failures after activate already ran', async () => {
    const activateProject = vi.fn()
    const ensurePinnedChat = vi.fn(async () => {
      throw new Error('daemon down')
    })

    await expect(
      wakeCanonicalMemberSession('ws-1', '/p', { activateProject, ensurePinnedChat }),
    ).rejects.toThrow('daemon down')
    // Still activated first — partial success is preferable to reaper arming cold.
    expect(activateProject).toHaveBeenCalledWith('ws-1')
  })
})

describe('activateOnLiveSessionAttach', () => {
  it('activates the member workspace id on passive live attach', () => {
    const activateProject = vi.fn()
    activateOnLiveSessionAttach('ws-live', activateProject)
    expect(activateProject).toHaveBeenCalledTimes(1)
    expect(activateProject).toHaveBeenCalledWith('ws-live')
  })
})

describe('resolveAllMemberSessions', () => {
  it('looks up all workspace ids in parallel and maps live/dormant', async () => {
    const started: string[] = []
    const lookup = vi.fn(async (id: string) => {
      started.push(id)
      // Overlapping in-flight: resolve only after both have started.
      await Promise.resolve()
      if (id === 'ws-a') return { sessionAlive: true, sessionId: 'sess-a' }
      return { sessionAlive: false, sessionId: null }
    })

    const map = await resolveAllMemberSessions(['ws-a', 'ws-b', 'ws-a'], lookup)

    expect(lookup).toHaveBeenCalledTimes(2) // deduped
    expect(map['ws-a']).toEqual({ kind: 'live', sessionId: 'sess-a' })
    expect(map['ws-b']).toEqual({ kind: 'dormant' })
    expect(started.sort()).toEqual(['ws-a', 'ws-b'])
  })

  it('captures per-id errors without failing the batch', async () => {
    const lookup = vi.fn(async (id: string) => {
      if (id === 'bad') throw new Error('daemon down')
      return { sessionAlive: true, sessionId: 'ok' }
    })
    const map = await resolveAllMemberSessions(['ok', 'bad'], lookup)
    expect(map['ok']).toEqual({ kind: 'live', sessionId: 'ok' })
    expect(map['bad']).toEqual({ kind: 'error', message: 'daemon down' })
  })
})
