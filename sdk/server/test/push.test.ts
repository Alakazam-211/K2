// App chat pushes (prd-home-chat-pushes-v1 HC-25): a chat push's data is
// read back exactly, anything else is null, and push.viewing's chat
// conversation is built only from a Sew id.

import { describe, expect, it } from 'vitest'

import { chatConversation, chatPushFrom, CHAT_ANDROID_CHANNEL, CHAT_CONVERSATION_PREFIX } from '../src/index.ts'

const DATA = {
  k2App: 'svc_jobs',
  k2Send: 'snd_00000000000000000000000000000001',
  k2Kind: 'chat',
  k2Who: 'who_abc',
  k2Reason: 'mention',
  k2Conv: '901',
  k2ConvKind: 'channel',
  k2Msg: '2001',
  k2Count: '2',
  k2Mention: '1',
}

describe('chatPushFrom', () => {
  it('reads a chat push into one typed value', () => {
    expect(chatPushFrom(DATA)).toEqual({
      app: 'svc_jobs',
      reason: 'mention',
      conversation: '901',
      kind: 'channel',
      messageId: '2001',
      count: 2,
      mention: true,
      who: 'who_abc',
    })
    expect(chatPushFrom({ ...DATA, k2ConvKind: 'dm', k2Reason: 'dm', k2Mention: '0', k2Count: '1' })).toMatchObject({ kind: 'dm', reason: 'dm', mention: false, count: 1 })
  })

  it('is null for any other push or a malformed field', () => {
    expect(chatPushFrom(null)).toBeNull()
    expect(chatPushFrom({ ...DATA, k2Kind: 'agent' })).toBeNull()
    expect(chatPushFrom({ ...DATA, k2Kind: 'app' })).toBeNull()
    for (const [k, v] of [
      ['k2Reason', 'shout'],
      ['k2Conv', '../x'],
      ['k2Msg', ''],
      ['k2ConvKind', 'room'],
      ['k2Count', '0'],
      ['k2Count', '1.5'],
      ['k2Mention', 'yes'],
      ['k2Who', ''],
    ] as const) {
      expect(chatPushFrom({ ...DATA, [k]: v }), `${k}=${v}`).toBeNull()
    }
    const { k2App: _gone, ...noApp } = DATA
    expect(chatPushFrom(noApp)).toBeNull()
  })
})

describe('chatConversation', () => {
  it('prefixes a Sew id and refuses anything else', () => {
    expect(chatConversation('901')).toBe('chat:901')
    expect(CHAT_CONVERSATION_PREFIX).toBe('chat:')
    expect(CHAT_ANDROID_CHANNEL).toBe('chat')
    expect(() => chatConversation('')).toThrow(/Sew channel id/)
    expect(() => chatConversation('chat:901')).toThrow(/Sew channel id/)
  })
})
