// Voice rooms §4.2: the headset's look and tooltip, from the store. Pure.

import type { VoiceCall, ServerRooms } from '@/stores/voice-core'
import type { VoiceServer } from '@/lib/voice/voice-servers'
import { roomIsFull, type VoiceRoom } from '@/lib/voice/voice-wire'

export type HeadsetLook = 'rest' | 'occupied' | 'in-call' | 'connecting'

export interface HeadsetState {
  look: HeadsetLook
  tooltip: string
  muted: boolean
}

function displayName(user: string, name: string | null): string {
  if (name) return name
  return user === 'owner' ? 'Owner' : user
}

export function headsetState(
  call: VoiceCall | null,
  servers: readonly VoiceServer[],
  rooms: Readonly<Record<string, ServerRooms>>,
): HeadsetState {
  if (call) {
    if (call.phase === 'joining' || call.phase === 'connecting') {
      return { look: 'connecting', tooltip: `Connecting to ${call.roomName}…`, muted: call.muted }
    }
    if (call.phase === 'reconnecting') {
      return { look: 'connecting', tooltip: `Reconnecting to ${call.roomName}…`, muted: call.muted }
    }
    const n = Math.max(1, call.participants.length)
    return {
      look: 'in-call',
      tooltip: `In ${call.roomName} · ${call.serverLabel}, ${n} ${n === 1 ? 'person' : 'people'}`,
      muted: call.muted,
    }
  }
  let people = 0
  let roomCount = 0
  let first: { who: string; room: string; server: string; count: number } | null = null
  for (const s of servers) {
    if (s.kind !== 'live') continue
    for (const r of rooms[s.hostKey]?.list?.rooms ?? []) {
      roomCount += 1
      people += r.occupants.length
      if (!first && r.occupants.length > 0) {
        const o = r.occupants[0]
        first = { who: displayName(o.user, o.name), room: r.name, server: s.label, count: r.occupants.length }
      }
    }
  }
  if (first) {
    const others = first.count - 1
    const who = others === 0 ? `${first.who} is` : `${first.who} and ${others} ${others === 1 ? 'other' : 'others'} are`
    return { look: 'occupied', tooltip: `${who} in ${first.room} · ${first.server}`, muted: false }
  }
  return {
    look: 'rest',
    tooltip: `Voice rooms: ${people} ${people === 1 ? 'person' : 'people'} in ${roomCount} ${roomCount === 1 ? 'room' : 'rooms'}`,
    muted: false,
  }
}

/** "12:04" for the call timer. */
export function callDuration(from: number | null, now: number): string {
  if (from === null) return '0:00'
  const total = Math.max(0, Math.floor((now - from) / 1000))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const sec = String(total % 60).padStart(2, '0')
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${sec}` : `${m}:${sec}`
}

/** What a room says instead of Join, or null when it takes joins (§4.3).
 *  The daemon decides `unavailable`; "Close now" counts its 10 s down. */
export function roomStateText(r: VoiceRoom, now: number): string | null {
  if (r.closing) {
    if (r.closesAt === null) return 'Closing'
    const left = Math.max(0, Math.ceil((r.closesAt - now) / 1000))
    return `Closes in ${left} s`
  }
  if (roomIsFull(r)) return 'Full'
  if (r.unavailable) return 'Unavailable'
  return null
}
