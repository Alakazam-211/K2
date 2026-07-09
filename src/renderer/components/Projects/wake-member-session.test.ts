// Wake / live-attach Active obligation (PRD §4.3.1): client-watching
// surfaces must activate the member workspace before/with ensure-pinned-chat
// so active_reaper does not force-close the chat PTY after ~15s grace.

import { describe, it, expect, vi } from 'vitest'
import {
  activateOnLiveSessionAttach,
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
