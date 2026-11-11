// Voice rooms lane D: the requests, each on the hosting server's own scope
// (`scopeForHost(key)`), so they share that server's `/cli` cap and its one
// revive-and-replay through Home's pool (§4.3, §5: always that server's own
// login, never relayed through the window's server).

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { httpCodeOf, httpDetailsOf } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import type { RecordingPerms } from './recording-controls'
import {
  parseJoinAnswer,
  parseVoiceRooms,
  parseVoiceStatus,
  voiceErrorCode,
  type VoiceErrorCode,
  type VoiceJoinAnswer,
  type VoiceRoomsList,
  type VoiceStatus,
} from './voice-wire'

/** vs-live V16: Home's pool gives up on its own requests at 5 s
 *  (`host-pool-instance.ts` REQUEST_TIMEOUT_MS), so the join has its own
 *  ceiling, above the longest a daemon can take to answer (`voice/bff.rs`):
 *  20 s for a reconnecting lane (§6.6 M11) + 8 s for a level sync + 8 s for
 *  the member gate + 15 s for Sew's answer = 51 s. */
export const JOIN_TIMEOUT_MS = 55_000

export class VoiceRequestError extends Error {
  readonly code: VoiceErrorCode
  constructor(code: VoiceErrorCode, message: string) {
    super(message)
    this.name = 'VoiceRequestError'
    this.code = code
  }
}

/** The §4.6 code of a thrown daemon answer. `daemon-cli` keeps the refusal's
 *  `error` object as `details`, so lane C's own `details` sit one level in. */
export function voiceCodeOfError(err: unknown): VoiceErrorCode {
  if (err instanceof VoiceRequestError) return err.code
  const inner = httpDetailsOf(err)?.details
  const details = inner && typeof inner === 'object' ? (inner as Record<string, unknown>) : null
  return voiceErrorCode(httpCodeOf(err), details)
}

export async function fetchVoiceRooms(scope: ServerScope): Promise<VoiceRoomsList> {
  return parseVoiceRooms(await daemonCliGet(scope, 'voice/rooms'))
}

export interface JoinOptions {
  timeoutMs?: number
  /** Sew's join body: `self_mute` for "join muted". */
  selfMute?: boolean
  /** Called if the server's answer lands after the client gave up: the
   *  shell leaves that call at once so no seat is left behind. */
  onLateAnswer?: (answer: VoiceJoinAnswer) => void
}

/** `POST /cli/voice/join {room, join?}`. Throws a `VoiceRequestError`. */
export async function joinVoiceRoom(scope: ServerScope, roomId: string, opts: JoinOptions = {}): Promise<VoiceJoinAnswer> {
  const timeoutMs = opts.timeoutMs ?? JOIN_TIMEOUT_MS
  let timedOut = false
  const body = opts.selfMute ? { room: roomId, join: { self_mute: true } } : { room: roomId }
  const request = daemonCliPost(scope, 'voice/join', body).then(parseJoinAnswer)
  let timer: ReturnType<typeof setTimeout> | null = null
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      timedOut = true
      reject(new VoiceRequestError('join_timeout', `voice/join: no answer in ${timeoutMs} ms`))
    }, timeoutMs)
  })
  request.then(
    (answer) => {
      if (timedOut) opts.onLateAnswer?.(answer)
    },
    () => {},
  )
  try {
    return await Promise.race([request, timeout])
  } catch (err) {
    if (err instanceof VoiceRequestError) throw err
    throw new VoiceRequestError(voiceCodeOfError(err), err instanceof Error ? err.message : String(err))
  } finally {
    if (timer !== null) clearTimeout(timer)
  }
}

/** `POST /cli/voice/session/leave {callId}`. */
export async function leaveVoiceCall(scope: ServerScope, callId: string): Promise<void> {
  await daemonCliPost(scope, 'voice/session/leave', { callId })
}

/** `POST /cli/voice/session/state {callId, body}`: Sew's self state as is, so
 *  the room list shows it to everyone. */
export async function sendVoiceSelfState(scope: ServerScope, callId: string, muted: boolean, deafened: boolean): Promise<void> {
  await daemonCliPost(scope, 'voice/session/state', { callId, body: { self_mute: muted, self_deaf: deafened } })
}

/** Sew 0.9.0: `POST /cli/voice/recording/controls {room}`, the person's own
 *  permissions in a room, for Record and Stop (`recording-controls.ts`). */
export async function fetchRecordingControls(scope: ServerScope, roomId: string): Promise<RecordingPerms> {
  const v = (await daemonCliPost(scope, 'voice/recording/controls', { room: roomId })) as Record<string, unknown> | null
  if (!v || typeof v !== 'object') throw new Error('voice/recording/controls: not an answer')
  return { recordCalls: v.recordCalls === true, manageChannels: v.manageChannels === true, voiceChatOnly: v.voiceChatOnly === true }
}

/** Settings (§3.3, §4.5): `GET /cli/voice/status`. */
export async function fetchVoiceStatus(scope: ServerScope): Promise<VoiceStatus> {
  return parseVoiceStatus(await daemonCliGet(scope, 'voice/status'))
}

/** Settings (§3, owner/Admin): `POST /cli/voice/settings {count, now?}`. */
export async function setVoiceRoomCount(scope: ServerScope, count: number, now: boolean): Promise<void> {
  await daemonCliPost(scope, 'voice/settings', now ? { count, now: true } : { count })
}

/** Settings (Sew 0.9.0): `POST /cli/voice/recording/settings {required?}`,
 *  the K5 rooms' "require recording" (the owner; refused
 *  `client_consent_missing` until K2's desktop answers consent prompts). */
export async function setVoiceRecordingRequired(scope: ServerScope, required: boolean | null): Promise<void> {
  await daemonCliPost(scope, 'voice/recording/settings', { required })
}

/** Settings (§3.2, owner/Admin): `POST /cli/voice/rooms/rename {room, name}`;
 *  `room` is the slug. */
export async function renameVoiceRoom(scope: ServerScope, roomId: string, name: string): Promise<void> {
  await daemonCliPost(scope, 'voice/rooms/rename', { room: roomId, name })
}
