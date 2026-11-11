// Voice rooms (prd-k2-voice-rooms-v1 v1.9, lane D): what the client reads
// from each hosting server's `/cli/voice/*` routes, and the words it shows.
//
// Pure: parsing and copy only, no requests. The daemon owns the truth
// (rooms, occupants, `hosting`); this client gathers each signed-in server's
// list with that server's own login (§4.3, §5) and never relays one server's
// rooms to another.
//
// The shapes are lane C's (`crates/k2-daemon/src/voice/routes.rs`, `bff.rs`).
// A field this client doesn't know is ignored, a malformed room is dropped
// (never guessed), and a body that isn't a rooms list throws.

/** `/boot-status` `features` key a daemon reports while it hosts rooms
 *  (`voice::occupancy::hosts_rooms`: preview on, Linux, the K5 Space
 *  ready, Pro, at least one room). A daemon with the preview off never lists
 *  it, so the headset shows nothing at all. */
export const VOICE_ROOMS_FEATURE = 'voice-rooms-v1'

/** The daemon's §4.5 state (`voice::Hosting`). The daemon owns the set and
 *  the words; the client branches on only a few of them. */
export interface VoiceHosting {
  /** `ok`, `not_linux`, `sew_off`, `voice_off`, `starting`, `not_pro`,
   *  `count_zero`, `voice_lane_reconnecting`, `voice_lane_down`… */
  state: string
  /** The line people see. */
  line: string | null
  /** The failed check or the reason, when there is one. */
  detail: string | null
}

export interface VoiceOccupant {
  /** Who this is on the box: `owner`, a Connect login's name, or Sew's id
   *  when neither fits (`Occupant.person`). */
  user: string
  /** Display name, when the server sends one. */
  name: string | null
  /** Sew's member id: how the native engine's people (who have no names) are
   *  matched to this list. */
  memberId?: string
  role: string | null
  color: string | null
  muted: boolean
  deafened: boolean
  speaking: boolean
}

export interface VoiceRoom {
  /** The room's slug (`room-1`): what join, rename and close take. Never
   *  changes, unlike the name. */
  id: string
  name: string
  /** Sew's channel id: what the native engine reports after a moderator's move. */
  channelId?: string
  /** At most this many people (§4.3 "count against its limit"). */
  limit: number | null
  occupants: VoiceOccupant[]
  /** Close-when-empty grace or a count change (§3, SW-4): no joins. */
  closing: boolean
  /** "Close now": unix ms when everyone is disconnected (§3.4). */
  closesAt: number | null
  /** Why the room takes no joins, from the daemon: `room_full`,
   *  `media_path_down` (§6.6 M11), or a hosting state. */
  unavailable: string | null
}

/** The daemon says full at the limit; a list read a moment ago still counts. */
export function roomIsFull(r: Pick<VoiceRoom, 'limit' | 'occupants' | 'unavailable'>): boolean {
  return r.unavailable === 'room_full' || (r.limit !== null && r.occupants.length >= r.limit)
}

export interface VoiceRoomsList {
  hosting: VoiceHosting
  rooms: VoiceRoom[]
  /** `voice_rooms_changed {rev}`'s counter. */
  rev: number | null
  /** This login may change rooms here (Owner/Admin, §9.4): the §4.5
   *  footer and Settings' stepper and renames show only then. */
  canManage: boolean
  /** The daemon's occupancy follower is live, so `occupants` is current. */
  feedLive: boolean
  /** 0.46.11: an agent on this server may read the rooms' transcripts, so
   *  K2's recording notice says so. */
  agentReaders?: boolean
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.length > 0 ? v : null
}

/** A missing state reads as `unknown`, never `ok`. */
function parseHosting(raw: unknown): VoiceHosting {
  const h = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  return { state: str(h.state) ?? 'unknown', line: str(h.line), detail: str(h.detail) }
}

function parseOccupant(raw: unknown): VoiceOccupant | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const user = str(r.user)
  if (!user) return null
  return {
    user,
    name: str(r.name),
    memberId: str(r.memberId) ?? undefined,
    role: str(r.role),
    color: str(r.color),
    muted: r.muted === true,
    deafened: r.deafened === true,
    speaking: r.speaking === true,
  }
}

function parseRoom(raw: unknown): VoiceRoom | null {
  if (!raw || typeof raw !== 'object') return null
  const r = raw as Record<string, unknown>
  const id = str(r.id)
  const name = str(r.name)
  if (!id || !name) return null
  const limit = typeof r.limit === 'number' && Number.isFinite(r.limit) && r.limit > 0 ? Math.floor(r.limit) : null
  const occupants = Array.isArray(r.occupants)
    ? r.occupants.map(parseOccupant).filter((o): o is VoiceOccupant => o !== null)
    : []
  const closesAt = typeof r.closesAt === 'number' && Number.isFinite(r.closesAt) ? r.closesAt : null
  const channelId = str(r.channelId)
  return { id, name, ...(channelId ? { channelId } : {}), limit, occupants, closing: r.closing === true, closesAt, unavailable: str(r.unavailable) }
}

/** `GET /cli/voice/rooms`. Throws when the body is not a rooms list. */
export function parseVoiceRooms(body: unknown): VoiceRoomsList {
  if (!body || typeof body !== 'object' || !Array.isArray((body as { rooms?: unknown }).rooms)) {
    throw new Error('voice/rooms: not a rooms list')
  }
  const b = body as { rooms: unknown[]; hosting?: unknown; rev?: unknown; canManage?: unknown; feed?: unknown; agentReaders?: unknown }
  return {
    hosting: parseHosting(b.hosting),
    rooms: b.rooms.map(parseRoom).filter((r): r is VoiceRoom => r !== null),
    rev: typeof b.rev === 'number' && Number.isFinite(b.rev) ? b.rev : null,
    canManage: b.canManage === true,
    feedLive: b.feed === 'live',
    agentReaders: b.agentReaders === true,
  }
}

/** `GET /cli/voice/status` (Owner/Admin; §3.3, §4.5): what Settings needs
 *  beyond the rooms list. Unknown or missing fields read as null. */
export interface VoiceStatus {
  hosting: VoiceHosting
  /** `voice.rooms.count`, 0–10 (§3.1). */
  count: number | null
  /** The media mode (§3.3): tunnel, or the direct fallback. */
  mediaTier: 'tunnel' | 'direct' | null
  /** Sew 0.9.0: the K5 rooms' recording (`recording`), when the daemon has it. */
  recording?: VoiceRecordingSettings
}

/** `GET /cli/voice/status` `.recording` (voice/routes.rs `recording_view`). */
export interface VoiceRecordingSettings {
  spaceId: string | null
  /** null = the server's rule. */
  startLevel: string | null
  required: boolean | null
  locked: boolean
  /** Sew can require recording (`recording_required`). */
  canRequire: boolean
  /** K2's desktop answers consent prompts: until then "require recording"
   *  stays hidden (Rosson's answer 6). */
  consentSheet: boolean
}

function parseRecording(v: unknown): VoiceRecordingSettings | undefined {
  if (!v || typeof v !== 'object') return undefined
  const r = v as Record<string, unknown>
  const flags = r.flags && typeof r.flags === 'object' ? (r.flags as Record<string, unknown>) : {}
  return {
    spaceId: str(r.spaceId),
    startLevel: str(r.startLevel),
    required: typeof r.required === 'boolean' ? r.required : null,
    locked: r.locked === true,
    canRequire: flags.recordingRequired === true,
    consentSheet: r.consentSheet === true,
  }
}

export function parseVoiceStatus(body: unknown): VoiceStatus {
  if (!body || typeof body !== 'object') throw new Error('voice/status: empty answer')
  const b = body as Record<string, unknown>
  const count = b.count
  const media = b.media && typeof b.media === 'object' ? (b.media as Record<string, unknown>) : {}
  const tier = str(media.tier)
  const recording = parseRecording(b.recording)
  return {
    hosting: parseHosting(b.hosting),
    count: typeof count === 'number' && Number.isInteger(count) && count >= 0 && count <= 10 ? count : null,
    mediaTier: tier === 'tunnel' || tier === 'direct' ? tier : null,
    ...(recording ? { recording } : {}),
  }
}

/** What the client keeps from `POST /cli/voice/join`: K2's call id, the room,
 *  and Sew's join answer (`VoiceJoinResponse`, under `join`, §9.2). Passes
 *  never reach the client: only the media token, signalling URL, e2ee init
 *  and policy. */
export interface VoiceJoinAnswer {
  /** K2's call id: the session routes take it (§9.1). */
  callId: string
  roomId: string
  roomName: string
  /** `wss://voice.<label>.k2.dev` (§4.4). */
  signalUrl: string
  /** The ES256 media token (a bearer for the signalling connect). */
  mediaToken: string
  /** `CallKeysInit` for the MLS E2EE: opaque here, the engine reads it. */
  e2ee: unknown
  /** §6.6 M10 (2): Sew's `media.relay_only` (the daemon mirrors it as
   *  `relayOnly`). Absent reads as `false`. */
  relayOnly: boolean
}

/** `POST /cli/voice/join`. Throws when a required field is missing. */
export function parseJoinAnswer(body: unknown): VoiceJoinAnswer {
  if (!body || typeof body !== 'object') throw new Error('voice/join: empty answer')
  const b = body as Record<string, unknown>
  const join = b.join && typeof b.join === 'object' ? (b.join as Record<string, unknown>) : null
  const callId = str(b.callId)
  const room = b.room && typeof b.room === 'object' ? (b.room as Record<string, unknown>) : {}
  const signalUrl = join ? str(join.url) : null
  const mediaToken = join ? str(join.token) : null
  if (!callId || !join || !signalUrl || !mediaToken) throw new Error('voice/join: answer is missing callId, join.url or join.token')
  if (!/^wss:\/\//.test(signalUrl)) throw new Error('voice/join: the signalling URL is not wss://')
  const media = join.media && typeof join.media === 'object' ? (join.media as Record<string, unknown>) : {}
  return {
    callId,
    roomId: str(room.id) ?? '',
    roomName: str(room.name) ?? '',
    signalUrl,
    mediaToken,
    e2ee: join.e2ee ?? null,
    relayOnly: media.relay_only === true || b.relayOnly === true,
  }
}

/** Join refusals the client explains (§4.6, vs-live V16). Anything else
 *  shows the daemon's own hint. */
export type VoiceErrorCode =
  | 'room_full'
  | 'room_closing'
  | 'host_not_pro'
  | 'voice_unavailable'
  | 'media_path_down'
  | 'member_pending'
  | 'e2ee_unsupported'
  | 'server_too_old'
  | 'mic_denied'
  | 'mic_unavailable'
  | 'e2ee_refused'
  | 'turn_unreachable'
  | 'signin_required'
  | 'join_timeout'
  | 'owner_token_cannot_join'
  | 'recorded_room'
  | 'engine_busy'
  | 'unknown'

const CODE_OF: Readonly<Record<string, VoiceErrorCode>> = {
  room_full: 'room_full',
  room_closing: 'room_closing',
  host_not_pro: 'host_not_pro',
  voice_unavailable: 'voice_unavailable',
  voice_member_pending: 'member_pending',
  chat_member_pending: 'member_pending',
  e2ee_unsupported_client: 'e2ee_unsupported',
  unauthenticated: 'signin_required',
  owner_token_cannot_join: 'owner_token_cannot_join',
  // Sew 0.7.3: the room is recorded and the join carried no answer to its
  // notice. The K2 app answers it on its own notice; a window without the
  // app's engine (the web client) can't show one.
  recording_consent_required: 'recorded_room',
  recording_consent_required_to_join: 'recorded_room',
}

/** The §4.6 code of a refusal: lane C's `code`, and the `details` beside it.
 *  `details.engine = media_path_down` is the lane or Sew's media path (§6.6
 *  M11). A Sew refusal the daemon doesn't translate keeps Sew's error under
 *  `details.sew`, where its code or reason may say more. */
export function voiceErrorCode(code: string | null, details: Record<string, unknown> | null): VoiceErrorCode {
  if (details?.engine === 'media_path_down') return 'media_path_down'
  const sew = details?.sew && typeof details.sew === 'object' ? (details.sew as Record<string, unknown>) : null
  const sewDetails = sew?.details && typeof sew.details === 'object' ? (sew.details as Record<string, unknown>) : null
  for (const c of [code, str(sew?.code), str(sewDetails?.reason)]) {
    if (c !== null && Object.prototype.hasOwnProperty.call(CODE_OF, c)) return CODE_OF[c]
  }
  return 'unknown'
}

/** The §4.6 code of a native engine's `join` rejection: the plugin's own codes,
 *  or a server refusal K2's provider passed on as `{hint, details}`. */
export function nativeJoinErrorCode(code: string, detail: Record<string, unknown> | null): VoiceErrorCode {
  switch (code) {
    case 'mic_denied':
    case 'mic_unavailable':
    case 'turn_unreachable':
    case 'signin_required':
    case 'join_timeout':
    case 'e2ee_refused':
      return code
    case 'ca_pin_mismatch':
    case 'relay_only_no_turns':
      return 'turn_unreachable'
    case 'unavailable':
      return 'voice_unavailable'
    case 'voice_server_too_old':
      return 'server_too_old'
  }
  const details = detail?.details && typeof detail.details === 'object' ? (detail.details as Record<string, unknown>) : null
  const mapped = voiceErrorCode(code, details)
  // A native join has its keys: this refusal means the server's Sew is behind.
  return mapped === 'e2ee_unsupported' ? 'server_too_old' : mapped
}

/** §4.6 join errors (with V16's `media_path_down` row). `hint` is the
 *  daemon's own line, for a refusal this client has no words for. */
export function joinErrorText(code: VoiceErrorCode, roomName: string, serverLabel: string, limit: number | null, hint = ''): string {
  switch (code) {
    case 'room_full':
      return limit !== null ? `${roomName} is full (${limit}).` : `${roomName} is full.`
    case 'room_closing':
      return `${roomName} is closing.`
    case 'host_not_pro':
      return `${serverLabel}'s voice rooms are paused.`
    case 'voice_unavailable':
      return `Voice on ${serverLabel} is restarting. Try again in a moment.`
    case 'media_path_down':
      return `Voice on ${serverLabel} is unavailable (its network lane is down).`
    case 'member_pending':
      return `K2 is still setting up your login on ${serverLabel}. Try again in a moment.`
    case 'e2ee_unsupported':
      return 'Call audio arrives in a later K2 update. You can see who is in the rooms now.'
    case 'server_too_old':
      return `${serverLabel} is running an older K2. Ask its owner to update it.`
    case 'mic_denied':
      return "K2 can't use your microphone. Turn it on for K2 in your system's privacy settings, then join again."
    case 'mic_unavailable':
      return "K2 can't open your microphone. Check that one is connected and that no other app is holding it."
    case 'e2ee_refused':
      return `Couldn't join ${roomName}. The secure connection failed. Try again.`
    case 'turn_unreachable':
      return `Can't reach ${serverLabel}'s voice server from this network.`
    case 'signin_required':
      return `Sign in to ${serverLabel} again to join.`
    case 'join_timeout':
      return `${serverLabel} didn't answer in time. Try again in a moment.`
    case 'owner_token_cannot_join':
      return `Sign in to ${serverLabel} with your own login to join.`
    case 'recorded_room':
      return recordedRoomJoinText(roomName)
    case 'engine_busy':
      // The plugin still holds a call that already ended (Sew < 0.8.3).
      return "K2's call engine is still holding your last call. Try again in a minute, or reload the window (Cmd+R / Ctrl+R) if it keeps happening."
    case 'unknown':
      return hint ? `Couldn't join ${roomName} on ${serverLabel}: ${hint}` : `Couldn't join ${roomName} on ${serverLabel}.`
  }
}

/** Why a call ended (§4.4 "a one-line toast says why"): Sew's leave reasons
 *  (`VoiceLeaveReason`) and the client's own `signed_out`, `network` and
 *  `seat_lost`, and the native engine's end reasons. Treats a reason it doesn't
 *  know as "ended" (§14.3). */
export function callEndedText(reason: string, roomName: string, serverLabel: string, native = false): string {
  switch (reason) {
    case 'left':
      return `You left ${roomName}.`
    case 'media_path_down':
      return `The call ended: voice on ${serverLabel} is unavailable (its network lane is down).`
    case 'channel_archived':
    case 'room_closed':
      return `${roomName} closed.`
    case 'perms_lost':
      return `You no longer have access to ${roomName}.`
    case 'timed_out':
      return `You were timed out, so you left ${roomName}.`
    case 'account_removed':
      return `Your account was removed from ${serverLabel}.`
    case 'device_revoked':
      return `This device was signed out of ${serverLabel}.`
    case 'removed_by_moderator':
    case 'removed_from_space':
    case 'kicked':
      return `You were removed from ${roomName}.`
    case 'session_revoked':
    case 'signed_out':
    case 'signin_required':
      return `You were signed out of ${serverLabel}.`
    case 'network':
    case 'disconnected':
      return `The call to ${roomName} dropped: the connection was lost.`
    case 'seat_lost':
    case 'connect_timeout':
      if (native) return `${serverLabel} dropped you from ${roomName}. Join again, or ask its owner if it keeps happening.`
      return `${serverLabel} dropped you from ${roomName}. This K2 can't carry call audio yet.`
    case 'unavailable':
      return `The call to ${roomName} ended: K2 lost contact with ${serverLabel}. Check your internet, then join again.`
    case 'e2ee_refused':
      return `The call to ${roomName} ended: the secure connection failed. Join again to continue.`
    case 'credential_lost':
      // Sew's `replaced`; a Sew older than 0.7.1 also ends a sign-out this way.
      return `The call to ${roomName} ended: you were signed out, or joined from another device.`
    case 'mic_denied':
      return `The call to ${roomName} ended: K2 lost your microphone. Allow it in your system's privacy settings.`
    case 'credential_expired':
      return `The call to ${roomName} ended: your sign-in to ${serverLabel} expired. Join again to continue.`
    case 'app_unlinked':
      // Sew 0.8.0 (VoiceLeaveReason::AppUnlinked, code 29): the App the call
      // was joined through no longer shows this room's Space.
      return `The call to ${roomName} ended: the App you joined it from was unlinked from it.`
    case 'recording_declined':
      return `You left ${roomName}: it's recorded, and you said no to being recorded.`
    case 'recording_withdrawn':
      return `You left ${roomName}: you stopped being recorded, and this room needs everyone recorded.`
    case 'recording_consent_timeout':
      // Rosson 2026-11-10: a plain ended state, not an error.
      return "You left the call because you didn't answer the recording notice."
    case 'recording_consent_required':
      return `The call to ${roomName} ended: it's now recorded. Join again to see its recording notice.`
    default:
      return `The call to ${roomName} ended.`
  }
}

/** A join into a recorded room from a window without the K2 app's call
 *  engine (the web client): it can't show the recording notice, and no answer
 *  is never a yes. The K2 app shows the notice and joins. */
export function recordedRoomJoinText(roomName: string): string {
  return `${roomName} is being recorded. Join it from the K2 app, which shows the recording notice first.`
}

/** The system refused the mic: the person stays in the call and can listen. */
export const MIC_DENIED_LINE = "Your microphone is blocked, so no one can hear you. You can still listen."

/** Why this person's mic is off and not theirs to turn on (live access §11). */
export function micBlockedText(why: 'speak_denied' | 'server_muted', roomName: string): string {
  return why === 'speak_denied' ? `You can't speak in ${roomName}. You can still listen.` : `A moderator muted you in ${roomName}.`
}

/** What an `errors` event from the native engine says during a call. Null: not
 *  something to show (`key_storage_file`, an unknown code): the log has it. */
export function engineErrorText(code: string, serverLabel: string): { line: string; kind: 'error' | 'info' } | null {
  switch (code) {
    case 'mic_denied':
      return { line: MIC_DENIED_LINE, kind: 'error' }
    case 'mic_unavailable':
      return { line: "K2 can't open your microphone. Check that one is connected and that no other app is holding it.", kind: 'error' }
    case 'device_lost':
      return { line: 'Your audio device disconnected. K2 switched to your default device.', kind: 'info' }
    case 'turn_unreachable':
      return { line: `Can't reach ${serverLabel}'s voice server from this network.`, kind: 'error' }
    default:
      return null
  }
}

/** §4.5: the line Settings and the owner footer show: the daemon's own
 *  words. Null while the server hosts normally. */
export function hostingStateText(h: VoiceHosting): string | null {
  if (h.state === 'ok') return null
  return h.line ?? h.detail ?? 'Voice rooms are not available right now.'
}

/** True while the server's own lane is briefly down (< 20 s): joins are
 *  held server-side (§6.6 M11), so the client says "reconnecting", not
 *  "unavailable". */
export function isLaneReconnecting(h: VoiceHosting): boolean {
  return h.state === 'voice_lane_reconnecting'
}
