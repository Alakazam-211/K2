import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const h = vi.hoisted(() => ({ post: vi.fn(), get: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...a: unknown[]) => h.get(...a),
  daemonCliPost: (...a: unknown[]) => h.post(...a),
}))

import { JOIN_TIMEOUT_MS, joinVoiceRoom, sendVoiceSelfState, voiceCodeOfError } from './voice-api'
import type { ServerScope } from '@/kessel/server-scope'

const scope = { hostKey: 'sewlab.k2.dev', id: 'sewlab.k2.dev' } as unknown as ServerScope
const body = { callId: 'vc_9', room: { id: 'room-1', name: 'Standup' }, relayOnly: true, join: { url: 'wss://voice.sewlab.k2.dev', token: 't', media: { relay_only: true } } }

/** How `daemon-cli` throws lane C's refusal: `code` from the top level, the
 *  whole `error` object as `details`, the `hint` as the message. */
function refusal(status: number, code: string, details?: Record<string, unknown>): DaemonHttpError {
  return new DaemonHttpError(`hint for ${code}`, status, code, { code, hint: `hint for ${code}`, ...(details ? { details } : {}) })
}

beforeEach(() => {
  h.post.mockReset()
  h.get.mockReset()
})
afterEach(() => vi.useRealTimers())

describe('joinVoiceRoom', () => {
  it('posts the room to that server and parses the answer', async () => {
    h.post.mockResolvedValue(body)
    const a = await joinVoiceRoom(scope, 'room-1')
    expect(h.post).toHaveBeenCalledWith(scope, 'voice/join', { room: 'room-1' })
    expect(a).toMatchObject({ callId: 'vc_9', relayOnly: true })
  })

  it("join muted goes in Sew's join body", async () => {
    h.post.mockResolvedValue(body)
    await joinVoiceRoom(scope, 'room-1', { selfMute: true })
    expect(h.post).toHaveBeenCalledWith(scope, 'voice/join', { room: 'room-1', join: { self_mute: true } })
  })

  it("waits out the daemon's longest hold (V16), then gives up and leaves a late answer", async () => {
    vi.useFakeTimers()
    // 20 s lane + 8 s level + 8 s member gate + 15 s Sew (voice/bff.rs).
    expect(JOIN_TIMEOUT_MS).toBeGreaterThan(51_000)
    let resolve: (v: unknown) => void = () => {}
    h.post.mockReturnValue(new Promise((r) => (resolve = r)))
    const late = vi.fn()
    const caught = joinVoiceRoom(scope, 'room-1', { onLateAnswer: late }).catch((e: unknown) => e)
    await vi.advanceTimersByTimeAsync(51_000)
    expect(late).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(JOIN_TIMEOUT_MS - 51_000)
    await expect(caught).resolves.toMatchObject({ code: 'join_timeout' })
    resolve(body)
    await vi.advanceTimersByTimeAsync(0)
    expect(late).toHaveBeenCalledWith(expect.objectContaining({ callId: 'vc_9' }))
  })

  it("maps a daemon refusal to its code and keeps the daemon's hint", async () => {
    h.post.mockRejectedValue(refusal(409, 'room_full'))
    await expect(joinVoiceRoom(scope, 'room-1')).rejects.toMatchObject({ code: 'room_full', message: 'hint for room_full' })
  })
})

describe('voiceCodeOfError', () => {
  it("finds lane C's details one level inside daemon-cli's", () => {
    expect(voiceCodeOfError(refusal(503, 'voice_unavailable', { reason: 'voice_unavailable', engine: 'media_path_down' }))).toBe('media_path_down')
    expect(voiceCodeOfError(refusal(503, 'voice_unavailable', { engine: 'unreachable' }))).toBe('voice_unavailable')
    expect(voiceCodeOfError(refusal(422, 'validation_failed', { sew: { code: 'validation_failed', details: { reason: 'e2ee_unsupported_client' } } }))).toBe('e2ee_unsupported')
  })
  it('the login refusal, and anything that is not an answer', () => {
    expect(voiceCodeOfError(refusal(401, 'unauthenticated'))).toBe('signin_required')
    expect(voiceCodeOfError(new Error('network'))).toBe('unknown')
  })
})

describe('sendVoiceSelfState', () => {
  it("wraps Sew's body with the call id (session/state {callId, body})", async () => {
    h.post.mockResolvedValue({ ok: true })
    await sendVoiceSelfState(scope, 'vc_9', true, false)
    expect(h.post).toHaveBeenCalledWith(scope, 'voice/session/state', { callId: 'vc_9', body: { self_mute: true, self_deaf: false } })
  })
})
