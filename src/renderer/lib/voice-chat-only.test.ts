// prd-voice-chat-only-members-v1: the client's helpers for the role.

import { describe, expect, it } from 'vitest'
import {
  accessForRole,
  hostIsVoiceChatOnly,
  isRoleRequired,
  isVoiceChatOnly,
  openableHosts,
  voiceChatAddedText,
  voiceChatGateText,
} from './voice-chat-only'
import { isPossibleAuthFailure } from './remote-session'
import {
  addUserBody,
  canSetRole,
  parseK2Role,
  parseShares,
  roleOptions,
  splitShareList,
} from '@/components/Settings/sections/connect-user-roles'

const ROLE_REQUIRED = JSON.stringify({
  error: 'role_required',
  required: 'member',
  role: 'voice_chat',
  message: 'This server gives you Voice & Chat only: you can join its voice rooms, not open it.',
})

describe('the role and its hint', () => {
  it('only voice_chat is Voice & Chat only', () => {
    expect(isVoiceChatOnly('voice_chat')).toBe(true)
    for (const r of ['member', 'admin', 'owner', 'viewer', null, undefined]) expect(isVoiceChatOnly(r)).toBe(false)
    expect(accessForRole('voice_chat')).toBe('voice_chat')
    expect(accessForRole('member')).toBeUndefined()
    expect(hostIsVoiceChatOnly({ access: 'voice_chat' })).toBe(true)
    expect(hostIsVoiceChatOnly({})).toBe(false)
  })

  it('openableHosts drops only the Voice & Chat only hosts, in order', () => {
    const hosts = [{ id: 'a' }, { id: 'b', access: 'voice_chat' }, { id: 'c' }]
    expect(openableHosts(hosts).map((h) => h.id)).toEqual(['a', 'c'])
  })

  it('a role_required 403 is a role refusal, never a dead login (no revive loop)', () => {
    expect(isRoleRequired(403, ROLE_REQUIRED)).toBe(true)
    expect(isPossibleAuthFailure(403, ROLE_REQUIRED)).toBe(false)
    expect(isRoleRequired(403, '{"error":"Invalid or missing auth token"}')).toBe(false)
    expect(isRoleRequired(401, ROLE_REQUIRED)).toBe(false)
    expect(isRoleRequired(403, 'not json')).toBe(false)
  })

  it('copy names the server', () => {
    expect(voiceChatGateText('Review', false)).toBe(
      "Review gives you Voice & Chat only. Join its rooms from the headset. You can't open this server or talk to its agents.",
    )
    expect(voiceChatGateText('Review', true)).toContain('Use the K2 desktop app to join its voice rooms.')
    expect(voiceChatAddedText('Review')).toBe('Added Review. It gives you Voice & Chat only: its rooms are in the headset.')
  })
})

describe('Settings: the role picker and the add body', () => {
  it('offers Voice & Chat only first, and only for a server that reports the feature', () => {
    expect(roleOptions(false).map((o) => o.value)).toEqual(['member', 'admin', 'owner'])
    expect(roleOptions(true).map((o) => o.value)).toEqual(['voice_chat', 'member', 'admin', 'owner'])
    expect(roleOptions(true)[0].label).toBe('Voice & Chat only')
    expect(parseK2Role('voice_chat')).toBe('voice_chat')
    expect(parseK2Role('viewer')).toBeNull()
  })

  it('an Admin may move people only between Member and Voice & Chat only (Q1)', () => {
    expect(canSetRole('admin', 'member', 'voice_chat')).toBe(true)
    expect(canSetRole('admin', 'voice_chat', 'member')).toBe(true)
    expect(canSetRole('admin', 'member', 'admin')).toBe(false)
    expect(canSetRole('admin', 'admin', 'voice_chat')).toBe(false)
    expect(canSetRole('owner', 'admin', 'voice_chat')).toBe(true)
    expect(canSetRole('member', 'member', 'voice_chat')).toBe(false)
  })

  it('the role and shares ride the add body itself (V5): never a Member first', () => {
    expect(
      addUserBody({
        username: 'reviewer',
        password: 'pw',
        mustChangePassword: false,
        role: 'voice_chat',
        shareAgents: ['demo'],
        shareGardens: ['g-1'],
      }),
    ).toEqual({ username: 'reviewer', password: 'pw', mustChangePassword: false, role: 'voice_chat', shareAgents: ['demo'], shareGardens: ['g-1'] })
    // A Member add stays today's body: no role, no shares.
    expect(addUserBody({ username: 'm', password: 'pw', mustChangePassword: true, role: 'member', shareAgents: ['demo'] })).toEqual({
      username: 'm',
      password: 'pw',
      mustChangePassword: true,
    })
  })

  it('reads shares from a users row and splits typed lists', () => {
    expect(
      parseShares({ agents: [{ id: 'p1', address: 'demo', name: 'Demo' }, { address: '' }, 7], gardens: [{ id: 'g-1', name: 'Phone' }, {}] }),
    ).toEqual({ agents: [{ id: 'p1', address: 'demo', name: 'Demo' }], gardens: [{ id: 'g-1', name: 'Phone' }] })
    expect(parseShares(undefined)).toEqual({ agents: [], gardens: [] })
    expect(splitShareList(' demo, sales  demo,,g-1 ')).toEqual(['demo', 'sales', 'g-1'])
  })
})
