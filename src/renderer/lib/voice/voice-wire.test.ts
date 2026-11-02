import { describe, expect, it } from 'vitest'
import {
  callEndedText,
  hostingStateText,
  isLaneReconnecting,
  joinErrorText,
  parseJoinAnswer,
  parseVoiceRooms,
  parseVoiceStatus,
  roomIsFull,
  voiceErrorCode,
} from './voice-wire'

describe('parseVoiceRooms (voice/routes.rs rooms_view)', () => {
  it("reads lane C's list: the slug is the id, Sew's extra fields are ignored; malformed rooms drop", () => {
    const list = parseVoiceRooms({
      ok: true,
      server: 'sewlab',
      hosting: { state: 'ok', line: 'Voice rooms are on.' },
      canManage: true,
      rev: 7,
      lane: { state: 'not_applicable' },
      feed: 'live',
      rooms: [
        {
          id: 'room-1',
          n: 1,
          slug: 'room-1',
          name: 'Standup',
          channelId: 'ch1',
          limit: 25,
          count: 1,
          closing: false,
          closesAt: null,
          unavailable: null,
          occupants: [
            { user: 'ada', name: null, role: 'admin', color: null, muted: true, deafened: false, speaking: false, memberId: 'm1', connection: 'connected' },
            { name: 'no user' },
          ],
        },
        { id: 'room-2', n: 2, slug: 'room-2', name: 'Room 2', limit: 25, closing: true, closesAt: 9_000, unavailable: null, occupants: [] },
        { id: 'room-3', name: 'Room 3', limit: 25, closing: false, closesAt: null, unavailable: 'media_path_down', occupants: [] },
        { name: 'no id' },
        'junk',
      ],
    })
    expect(list.canManage).toBe(true)
    expect(list.feedLive).toBe(true)
    expect(list.rev).toBe(7)
    expect(list.hosting).toEqual({ state: 'ok', line: 'Voice rooms are on.', detail: null })
    expect(list.rooms.map((r) => r.id)).toEqual(['room-1', 'room-2', 'room-3'])
    expect(list.rooms[0].occupants).toEqual([{ user: 'ada', name: null, role: 'admin', color: null, muted: true, deafened: false, speaking: false }])
    expect(list.rooms[1]).toMatchObject({ closing: true, closesAt: 9_000 })
    expect(list.rooms[2].unavailable).toBe('media_path_down')
  })

  it('a missing hosting state reads as unknown, never ok; a feed that is not live is not', () => {
    const list = parseVoiceRooms({ rooms: [], feed: 'retrying' })
    expect(list.hosting.state).toBe('unknown')
    expect(list.feedLive).toBe(false)
  })

  it('throws on a body that is not a rooms list', () => {
    expect(() => parseVoiceRooms({ ok: false, code: 'voice_rooms_off' })).toThrow('not a rooms list')
    expect(() => parseVoiceRooms(null)).toThrow('not a rooms list')
  })

  it('full is the daemon saying so, or the count at the limit', () => {
    expect(roomIsFull({ limit: 25, occupants: [], unavailable: 'room_full' })).toBe(true)
    expect(roomIsFull({ limit: 0 + 1, occupants: [{ user: 'a', name: null, role: null, color: null, muted: false, deafened: false, speaking: false }], unavailable: null })).toBe(true)
    expect(roomIsFull({ limit: 25, occupants: [], unavailable: 'media_path_down' })).toBe(false)
  })
})

describe('parseVoiceStatus (voice/routes.rs status_view)', () => {
  it('reads the count and media tier; its `rooms` array is the ledger, not the count', () => {
    expect(parseVoiceStatus({ hosting: { state: 'not_pro', line: 'Voice rooms need the Pro plan.' }, count: 3, media: { tier: 'tunnel' }, rooms: [{ n: 1 }] })).toEqual({
      hosting: { state: 'not_pro', line: 'Voice rooms need the Pro plan.', detail: null },
      count: 3,
      mediaTier: 'tunnel',
    })
  })
  it('a count out of 0–10 is null', () => {
    expect(parseVoiceStatus({ count: 11 }).count).toBeNull()
  })
})

describe('parseJoinAnswer (voice/bff.rs join)', () => {
  const ok = {
    ok: true,
    callId: 'vc_1',
    room: { id: 'room-1', n: 1, slug: 'room-1', name: 'Standup', channelId: 'ch1' },
    relayOnly: true,
    heldMs: 0,
    join: { url: 'wss://voice.sewlab.k2.dev', token: 'tok', e2ee: { join_id: 'j' }, media: { relay_only: true } },
  }
  it('reads the call, signalling, token, e2ee and relay-only', () => {
    expect(parseJoinAnswer(ok)).toEqual({
      callId: 'vc_1',
      roomId: 'room-1',
      roomName: 'Standup',
      signalUrl: 'wss://voice.sewlab.k2.dev',
      mediaToken: 'tok',
      e2ee: { join_id: 'j' },
      relayOnly: true,
    })
  })
  it('relay-only absent in both places reads as false (M10)', () => {
    expect(parseJoinAnswer({ ...ok, relayOnly: undefined, join: { ...ok.join, media: {} } }).relayOnly).toBe(false)
    expect(parseJoinAnswer({ ...ok, relayOnly: undefined, join: { url: ok.join.url, token: 'tok' } }).relayOnly).toBe(false)
  })
  it('refuses a non-wss signalling URL and a missing token', () => {
    expect(() => parseJoinAnswer({ ...ok, join: { ...ok.join, url: 'ws://voice.x' } })).toThrow('not wss')
    expect(() => parseJoinAnswer({ ...ok, join: { url: ok.join.url } })).toThrow('missing')
  })
})

describe('voiceErrorCode', () => {
  it("maps lane C's codes", () => {
    expect(voiceErrorCode('room_full', null)).toBe('room_full')
    expect(voiceErrorCode('room_closing', null)).toBe('room_closing')
    expect(voiceErrorCode('host_not_pro', null)).toBe('host_not_pro')
    expect(voiceErrorCode('unauthenticated', null)).toBe('signin_required')
    expect(voiceErrorCode('owner_token_cannot_join', null)).toBe('owner_token_cannot_join')
    expect(voiceErrorCode('voice_member_pending', null)).toBe('member_pending')
    expect(voiceErrorCode('chat_member_pending', null)).toBe('member_pending')
    expect(voiceErrorCode('room_not_found', null)).toBe('unknown')
  })
  it('voice_unavailable with engine media_path_down is the lane or media path being down', () => {
    expect(voiceErrorCode('voice_unavailable', { reason: 'voice_unavailable', engine: 'media_path_down' })).toBe('media_path_down')
    expect(voiceErrorCode('voice_unavailable', { engine: 'unreachable' })).toBe('voice_unavailable')
  })
  it("a client that can't do E2EE yet: Sew's refusal, under details.sew", () => {
    expect(voiceErrorCode('e2ee_unsupported_client', null)).toBe('e2ee_unsupported')
    expect(voiceErrorCode('validation_failed', { sew: { code: 'validation_failed', details: { reason: 'e2ee_unsupported_client' } } })).toBe('e2ee_unsupported')
  })
})

describe('copy', () => {
  it('§4.6 join errors name the room and server', () => {
    expect(joinErrorText('room_full', 'Room 1', 'sewlab', 25)).toBe('Room 1 is full (25).')
    expect(joinErrorText('host_not_pro', 'Room 1', 'sewlab', null)).toBe("sewlab's voice rooms are paused.")
    expect(joinErrorText('media_path_down', 'Room 1', 'sewlab', null)).toBe('Voice on sewlab is unavailable (its network lane is down).')
    expect(joinErrorText('signin_required', 'Room 1', 'sewlab', null)).toBe('Sign in to sewlab again to join.')
    expect(joinErrorText('e2ee_unsupported', 'Room 1', 'sewlab', null)).toContain('Call audio arrives in a later K2 update')
  })
  it("a refusal without words here shows the daemon's hint", () => {
    expect(joinErrorText('unknown', 'Room 1', 'sewlab', null, "no room 'x' on this server")).toBe("Couldn't join Room 1 on sewlab: no room 'x' on this server")
    expect(joinErrorText('unknown', 'Room 1', 'sewlab', null)).toBe("Couldn't join Room 1 on sewlab.")
  })
  it("the hosting line is the daemon's; ok shows none", () => {
    const line = "Voice is reconnecting to K2's network…"
    expect(isLaneReconnecting({ state: 'voice_lane_reconnecting', line, detail: null })).toBe(true)
    expect(isLaneReconnecting({ state: 'voice_lane_down', line, detail: null })).toBe(false)
    expect(hostingStateText({ state: 'voice_lane_down', line, detail: null })).toBe(line)
    expect(hostingStateText({ state: 'ok', line: 'Voice rooms are on.', detail: null })).toBeNull()
    expect(hostingStateText({ state: 'unknown', line: null, detail: null })).toBe('Voice rooms are not available right now.')
  })
  it("call endings use Sew's leave reasons", () => {
    expect(callEndedText('media_path_down', 'Standup', 'sewlab')).toContain('network lane is down')
    expect(callEndedText('signed_out', 'Standup', 'sewlab')).toBe('You were signed out of sewlab.')
    expect(callEndedText('removed_by_moderator', 'Standup', 'sewlab')).toBe('You were removed from Standup.')
    expect(callEndedText('channel_archived', 'Standup', 'sewlab')).toBe('Standup closed.')
    expect(callEndedText('seat_lost', 'Standup', 'sewlab')).toContain('dropped you from Standup')
  })
})
