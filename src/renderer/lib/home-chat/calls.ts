// DM calls (prd-home-people-channels-v1 §11a): the client side. The daemon
// owns the call: it holds the person's Sew session, follows `call_ring` and
// `call_updated`, and answers `/cli/voice/join {dm}` and `/cli/voice/call/*`.
// This file reads its frames into small types, says what a call's record
// line reads, and sends the person's gestures (decline, ring again, cancel).
//
// Joining media is the voice controller's (`stores/voice-core.ts`): a DM call
// is a "room" whose id is `dm:<conversation id>`, so mute, leave, devices and
// the native engine work as they do for a voice room.

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliPost } from '@/lib/daemon-cli'
import type { VoiceRoom } from '@/lib/voice/voice-wire'

/** A DM call's room id: `dm:<conversation id>`. */
export const DM_ROOM_PREFIX = 'dm:'

export function dmRoomId(conv: string): string {
  return `${DM_ROOM_PREFIX}${conv}`
}

/** The conversation of a DM call's room id, or null for a voice room. */
export function convOfDmRoom(roomId: string | null | undefined): string | null {
  return roomId && roomId.startsWith(DM_ROOM_PREFIX) ? roomId.slice(DM_ROOM_PREFIX.length) || null : null
}

/** The voice controller's room for a DM call with `name`. */
export function dmCallRoom(conv: string, name: string): VoiceRoom {
  return { id: dmRoomId(conv), name, channelId: conv, limit: 2, occupants: [], closing: false, closesAt: null, unavailable: null }
}

// ── Frames ─────────────────────────────────────────────────────────────

/** A ring for this person (`call_ring`), as Sew sent it. */
export interface HomeChatRing {
  callId: string
  conv: string
  /** The caller's Sew member id. */
  startedBy: string | null
  ringing: boolean
  /** Busy in another call, Do Not Disturb, or a muted DM: no sound. */
  silent: boolean
  /** Why it stopped: accepted, declined, timeout, call_ended, cancelled. */
  reason: string | null
  /** Epoch ms when it times out (Sew's 45 s), when Sew said. */
  expiresAt: number | null
}

export type HomeChatRingState = 'ringing' | 'busy' | 'accepted' | 'declined' | 'missed' | 'cancelled'

/** A DM call as every member sees it (`call_updated`). */
export interface HomeChatCall {
  callId: string
  conv: string
  startedBy: string | null
  ended: boolean
  endReason: string | null
  participantIds: string[]
  rings: Array<{ memberId: string; state: HomeChatRingState | string }>
}

function idOf(v: unknown): string | null {
  if (typeof v === 'string' && v) return v
  if (typeof v === 'number' && Number.isFinite(v)) return String(v)
  return null
}

function ids(v: unknown): string[] {
  return Array.isArray(v) ? v.map(idOf).filter((x): x is string => x !== null) : []
}

function msOf(v: unknown): number | null {
  if (typeof v !== 'string') return null
  const ms = Date.parse(v)
  return Number.isFinite(ms) ? ms : null
}

export function parseRing(d: unknown): HomeChatRing | null {
  const o = d as Record<string, unknown> | null
  const callId = idOf(o?.call_id)
  const conv = idOf(o?.channel_id)
  if (!o || !callId || !conv) return null
  return {
    callId,
    conv,
    startedBy: idOf(o.started_by),
    ringing: o.ringing === true,
    silent: o.silent === true,
    reason: typeof o.reason === 'string' ? o.reason : null,
    expiresAt: msOf(o.expires_at),
  }
}

export function parseCall(v: unknown): HomeChatCall | null {
  const o = v as Record<string, unknown> | null
  const call = (o && typeof o.call === 'object' && o.call ? o.call : o) as Record<string, unknown> | null
  const callId = idOf(call?.id)
  const conv = idOf(call?.channel_id)
  if (!call || !callId || !conv) return null
  const rings = Array.isArray(call.rings)
    ? (call.rings as Array<Record<string, unknown>>)
        .map((r) => ({ memberId: idOf(r.member_id) ?? '', state: typeof r.state === 'string' ? r.state : 'ringing' }))
        .filter((r) => r.memberId)
    : []
  return {
    callId,
    conv,
    startedBy: idOf(call.started_by),
    ended: call.state === 'ended',
    endReason: typeof call.end_reason === 'string' ? call.end_reason : null,
    participantIds: ids(call.participant_ids),
    rings,
  }
}

// ── The call's record line (§11a.4) ────────────────────────────────────

/** Sew's CallSummary on a call's record message (`Message.call`). */
export interface HomeChatCallSummary {
  callId: string
  startedBy: string | null
  ended: boolean
  endReason: string | null
  durationMs: number | null
  joinedIds: string[]
  rungIds: string[]
  declinedIds: string[]
  missedIds: string[]
}

export function parseCallSummary(v: unknown): HomeChatCallSummary | null {
  const o = v as Record<string, unknown> | null
  const callId = idOf(o?.call_id)
  if (!o || !callId) return null
  return {
    callId,
    startedBy: idOf(o.started_by),
    ended: typeof o.ended_at === 'string' || typeof o.end_reason === 'string',
    endReason: typeof o.end_reason === 'string' ? o.end_reason : null,
    durationMs: typeof o.duration_ms === 'number' ? o.duration_ms : null,
    joinedIds: ids(o.joined_ids),
    rungIds: ids(o.rung_ids),
    declinedIds: ids(o.declined_ids),
    missedIds: ids(o.missed_ids),
  }
}

/** `12:04`, or `1:02:03` past an hour. */
export function formatCallDuration(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  const ss = String(s).padStart(2, '0')
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`
}

export type CallRecordKind = 'live' | 'missed' | 'no-answer' | 'declined' | 'cancelled' | 'answered' | 'ended'

export interface CallRecordLine {
  kind: CallRecordKind
  text: string
  /** The line's button once the call is over: "Call back" on a call you
   *  missed, "Call again" on any other (one that happened, one you made);
   *  null while it is live. */
  action: 'Call back' | 'Call again' | null
}

/** What a call's record says to `me` (a Sew member id): "Missed call",
 *  "Declined", "Cancelled" or "Call · 12:04"; "Call in progress" while live.
 *  A cancelled ring is never a missed call. */
export function callRecordLine(s: HomeChatCallSummary, me: string | null): CallRecordLine {
  if (!s.ended) return { kind: 'live', text: 'Call in progress', action: null }
  const again = 'Call again' as const
  if (s.endReason === 'cancelled') return { kind: 'cancelled', text: 'Cancelled', action: again }
  if (s.endReason === 'declined' || (me !== null && s.declinedIds.includes(me))) return { kind: 'declined', text: 'Declined', action: again }
  // Answered: someone besides the caller joined.
  if (s.joinedIds.length >= 2) return { kind: 'answered', text: `Call · ${formatCallDuration(s.durationMs ?? 0)}`, action: again }
  if (s.endReason === 'no_answer' || s.missedIds.length > 0) {
    // Your own call nobody picked up isn't a missed call: it says so plainly.
    if (me !== null && s.startedBy === me) return { kind: 'no-answer', text: 'No answer', action: again }
    return { kind: 'missed', text: 'Missed call', action: 'Call back' }
  }
  return { kind: 'ended', text: 'Call ended', action: again }
}

// ── The person's gestures ──────────────────────────────────────────────

/** `POST /cli/voice/call/decline {dm, callId}`. */
export async function declineDmCall(scope: ServerScope, conv: string, callId: string): Promise<void> {
  await daemonCliPost(scope, 'voice/call/decline', { dm: conv, callId })
}

/** `POST /cli/voice/call/ring {dm, callId}`: ring again (Sew allows 3). */
export async function ringDmCallAgain(scope: ServerScope, conv: string, callId: string): Promise<void> {
  await daemonCliPost(scope, 'voice/call/ring', { dm: conv, callId })
}

/** `POST /cli/voice/call/cancel {dm, callId}`: the caller's Cancel. Without
 *  Sew's `call_cancel` the server leaves instead (`cancelled: false`), and the
 *  other person sees a missed call. */
export async function cancelDmCall(scope: ServerScope, conv: string, callId: string): Promise<{ cancelled: boolean }> {
  const v = await daemonCliPost<{ cancelled?: unknown }>(scope, 'voice/call/cancel', { dm: conv, callId })
  return { cancelled: v?.cancelled === true }
}
