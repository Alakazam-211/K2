// DM calls on the client (prd-home-people-channels-v1 §11a): the frames, the
// call's record line, the ring store, and the door's request bodies.

import { beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({ post: vi.fn(async (..._a: unknown[]): Promise<unknown> => ({ ok: true })), get: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...a: unknown[]) => h.get(...a),
  daemonCliPost: (...a: unknown[]) => h.post(...a),
}))

import type { ServerScope } from '@/kessel/server-scope'
import {
  callRecordLine,
  cancelDmCall,
  convOfDmRoom,
  declineDmCall,
  dmCallRoom,
  formatCallDuration,
  parseCallSummary,
  ringDmCallAgain,
  type HomeChatCallSummary,
} from './calls'
import { parseHomeChatFrame } from './events-socket'
import { parseHomeChatMessage } from './api'
import { applyCallFrame, callerEndedLine, resetHomeChatCallsForTests, ringsNow, useHomeChatCallsStore } from '@/stores/home-chat-calls'
import { joinVoiceRoom } from '@/lib/voice/voice-api'

const scope = { hostKey: 'acme.example.test', id: 'acme.example.test' } as unknown as ServerScope

function summary(over: Partial<Record<string, unknown>>): HomeChatCallSummary {
  const s = parseCallSummary({ call_id: '88', started_by: '42', rung_ids: ['41'], joined_ids: ['42'], declined_ids: [], missed_ids: [], ...over })
  if (!s) throw new Error('summary did not parse')
  return s
}

beforeEach(() => {
  h.post.mockClear()
  resetHomeChatCallsForTests()
})

describe('the call record line', () => {
  it('reads each end reason, and a cancelled ring is never a missed call', () => {
    const at = { ended_at: '2026-11-14T12:01:00Z' }
    expect(callRecordLine(summary({ ...at, end_reason: 'no_answer', missed_ids: ['41'] }), '41')).toEqual({ kind: 'missed', text: 'Missed call', callBack: true })
    expect(callRecordLine(summary({ ...at, end_reason: 'declined', declined_ids: ['41'] }), '41').text).toBe('Declined')
    expect(callRecordLine(summary({ ...at, end_reason: 'cancelled', missed_ids: ['41'] }), '41')).toEqual({ kind: 'cancelled', text: 'Cancelled', callBack: true })
    expect(callRecordLine(summary({ ...at, end_reason: 'empty', joined_ids: ['42', '41'], duration_ms: 724_000 }), '41').text).toBe('Call · 12:04')
    // The caller left without Sew's cancel (an older Sew): a missed call.
    expect(callRecordLine(summary({ ...at, end_reason: 'empty', missed_ids: ['41'] }), '41').kind).toBe('missed')
    expect(callRecordLine(summary({ ...at, end_reason: 'media_failed' }), '41')).toEqual({ kind: 'ended', text: 'Call ended', callBack: true })
    expect(callRecordLine(summary({}), '41')).toEqual({ kind: 'live', text: 'Call in progress', callBack: false })
  })

  it('formats a duration', () => {
    expect(formatCallDuration(0)).toBe('0:00')
    expect(formatCallDuration(65_400)).toBe('1:05')
    expect(formatCallDuration(3_723_000)).toBe('1:02:03')
  })

  it('a message with a call carries its summary', () => {
    const m = parseHomeChatMessage({ id: '7', channel_id: '900', author_id: '42', call: { call_id: '88', end_reason: 'no_answer', missed_ids: ['41'] } })
    expect(m?.call?.callId).toBe('88')
    expect(parseHomeChatMessage({ id: '8', channel_id: '900' })?.call).toBeNull()
  })
})

describe('call frames', () => {
  it('parses call_ring, call_updated, message_updated and the rings in hello', () => {
    const ring = parseHomeChatFrame(JSON.stringify({ type: 'call_ring', conv: '900', d: { call_id: '88', channel_id: '900', started_by: '42', ringing: true, silent: true, expires_at: '2026-11-14T12:00:45Z' } }))
    expect(ring).toMatchObject({ type: 'call_ring', conv: '900', ring: { callId: '88', startedBy: '42', ringing: true, silent: true } })
    const upd = parseHomeChatFrame(JSON.stringify({ type: 'call_updated', conv: '900', d: { call: { id: '88', channel_id: '900', state: 'ended', end_reason: 'declined', rings: [{ member_id: '41', state: 'declined' }] } } }))
    expect(upd).toMatchObject({ type: 'call_updated', call: { callId: '88', ended: true, endReason: 'declined', rings: [{ memberId: '41', state: 'declined' }] } })
    expect(parseHomeChatFrame(JSON.stringify({ type: 'message_updated', conv: '900', message: { id: '7' } }))).toMatchObject({ type: 'message_updated', conv: '900' })
    const hello = parseHomeChatFrame(JSON.stringify({ type: 'hello', chat: 'live', calls: [
      { callId: '88', conv: '900', startedBy: '42', ringing: true, silent: false },
      { callId: '89', conv: '901', startedBy: '41', ringing: false },
    ] }))
    expect(hello).toMatchObject({ type: 'hello', rings: [{ callId: '88', conv: '900' }] })
  })
})

describe('the ring store', () => {
  const ringFrame = (callId: string, ringing: boolean, reason?: string) =>
    parseHomeChatFrame(JSON.stringify({ type: 'call_ring', d: { call_id: callId, channel_id: '900', started_by: '42', ringing, ...(reason ? { reason } : {}) } }))!

  it('a ring shows, then stops on every one of Sew’s reasons', () => {
    for (const reason of ['accepted', 'declined', 'timeout', 'call_ended', 'cancelled']) {
      applyCallFrame('acme', ringFrame('88', true), 1)
      expect(ringsNow(useHomeChatCallsStore.getState().rings).map((r) => r.callId)).toEqual(['88'])
      applyCallFrame('acme', ringFrame('88', false, reason), 2)
      expect(ringsNow(useHomeChatCallsStore.getState().rings)).toEqual([])
    }
  })

  it('hello replaces one server’s rings only; an ended call drops its ring and is remembered', () => {
    applyCallFrame('other', ringFrame('70', true), 1)
    applyCallFrame('acme', ringFrame('88', true), 1)
    applyCallFrame('acme', parseHomeChatFrame(JSON.stringify({ type: 'hello', chat: 'live', calls: [] }))!, 2)
    expect(ringsNow(useHomeChatCallsStore.getState().rings).map((r) => r.callId)).toEqual(['70'])
    applyCallFrame('acme', parseHomeChatFrame(JSON.stringify({ type: 'call_updated', d: { call: { id: '90', channel_id: '900', started_by: '41', state: 'active', rings: [{ member_id: '42', state: 'ringing' }] } } }))!, 3)
    expect(useHomeChatCallsStore.getState().calls['acme|900']?.callId).toBe('90')
    applyCallFrame('acme', parseHomeChatFrame(JSON.stringify({ type: 'call_updated', d: { call: { id: '90', channel_id: '900', state: 'ended', end_reason: 'no_answer' } } }))!, 4)
    expect(useHomeChatCallsStore.getState().calls['acme|900']).toBeUndefined()
    // The end keeps who started it (the live call knew, the end frame may not).
    expect(useHomeChatCallsStore.getState().ended['acme|900']).toEqual({ callId: '90', reason: 'no_answer', at: 4, startedBy: '41' })
  })

  it("only the caller hears that it wasn't answered or was declined: the person called gets the record line, never the caller's bar", () => {
    const ended = { callId: '90', reason: 'no_answer', at: 1_000, startedBy: '41' }
    expect(callerEndedLine(ended, '41', 2_000)).toBe('missed')
    expect(callerEndedLine({ ...ended, reason: 'declined' }, '41', 2_000)).toBe('declined')
    expect(callerEndedLine(ended, '42', 2_000)).toBeNull()
    expect(callerEndedLine({ ...ended, startedBy: null }, '41', 2_000)).toBeNull()
    expect(callerEndedLine(ended, '41', 1_000 + 60_000)).toBeNull()
    expect(callerEndedLine({ ...ended, reason: 'call_ended' }, '41', 2_000)).toBeNull()
    expect(callerEndedLine(undefined, '41', 2_000)).toBeNull()
  })
})

describe('the door', () => {
  it('a DM room joins by dm; decline, ring again and cancel name the DM and the call', async () => {
    expect(dmCallRoom('900', 'Alex')).toMatchObject({ id: 'dm:900', channelId: '900', name: 'Alex' })
    expect(convOfDmRoom('dm:900')).toBe('900')
    expect(convOfDmRoom('room-1')).toBeNull()
    h.post.mockResolvedValueOnce({ callId: 'vc_1', room: { id: 'dm:900' }, join: { url: 'wss://media.example.test', token: 't' } })
    await joinVoiceRoom(scope, 'dm:900', { selfMute: true })
    expect(h.post).toHaveBeenLastCalledWith(scope, 'voice/join', { dm: '900', join: { self_mute: true } })
    await declineDmCall(scope, '900', '88')
    expect(h.post).toHaveBeenLastCalledWith(scope, 'voice/call/decline', { dm: '900', callId: '88' })
    await ringDmCallAgain(scope, '900', '88')
    expect(h.post).toHaveBeenLastCalledWith(scope, 'voice/call/ring', { dm: '900', callId: '88' })
    h.post.mockResolvedValueOnce({ ok: true, cancelled: false, via: 'leave' })
    expect(await cancelDmCall(scope, '900', '88')).toEqual({ cancelled: false })
    expect(h.post).toHaveBeenLastCalledWith(scope, 'voice/call/cancel', { dm: '900', callId: '88' })
  })
})
