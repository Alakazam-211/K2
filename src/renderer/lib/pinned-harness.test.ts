// Move the pinned chat (0.46.6): the renderer maps a preset command to a
// harness and asks the daemon; it never moves anything itself.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', async () => {
  const { primaryOnly } = await import('@/test-utils/scope')
  return {
    daemonCliPost: primaryOnly((...args: unknown[]) => daemonCliPost(...args)),
  }
})

import { primaryScope } from '@/kessel/server-scope'
import { harnessOfCommand, harnessLabel, movePinnedChat, movePinnedChatMessage, PINNED_HARNESSES } from './pinned-harness'

describe('harnessOfCommand', () => {
  it('maps preset commands, paths and binaries to the daemon harness keys', () => {
    expect(harnessOfCommand('claude --dangerously-skip-permissions')).toBe('claude')
    expect(harnessOfCommand('grok --always-approve')).toBe('grok')
    expect(harnessOfCommand('/usr/local/bin/cursor-agent')).toBe('cursor')
    expect(harnessOfCommand('C:\\tools\\codex.exe --yolo')).toBe('codex')
    expect(harnessOfCommand('  Gemini ')).toBe('gemini')
  })

  it('answers null for a custom program or nothing', () => {
    expect(harnessOfCommand('my-bot --serve')).toBeNull()
    expect(harnessOfCommand('')).toBeNull()
    expect(harnessOfCommand(undefined)).toBeNull()
  })

  it('every harness has a label', () => {
    for (const h of PINNED_HARNESSES) expect(harnessLabel(h)).not.toBe(h)
  })
})

describe('movePinnedChat', () => {
  beforeEach(() => daemonCliPost.mockReset())

  it('posts agent/harness with the workspace and harness', async () => {
    daemonCliPost.mockResolvedValue({ switched: true, harness: 'claude', fromHarness: 'grok' })
    const r = await movePinnedChat(primaryScope(), 'ws-id', 'claude')
    expect(daemonCliPost).toHaveBeenCalledTimes(1)
    // primaryOnly checked and dropped the scope: (route, body).
    expect(daemonCliPost.mock.calls[0][0]).toBe('agent/harness')
    expect(daemonCliPost.mock.calls[0][1]).toEqual({ workspace: 'ws-id', harness: 'claude' })
    expect(r.switched).toBe(true)
  })
})

describe('movePinnedChatMessage', () => {
  it('says already-on, and names heartbeats that stay on their own session', () => {
    expect(movePinnedChatMessage({ switched: false, alreadyOn: true, harness: 'claude' })).toBe(
      'The pinned chat is already on Claude.',
    )
    const moved = movePinnedChatMessage({ switched: true, harness: 'claude', ownSessionHeartbeats: ['nightly'] })
    expect(moved).toContain('now runs on Claude')
    expect(moved).toContain('old chat is in Chat history')
    expect(moved).toContain('nightly')
  })
})
