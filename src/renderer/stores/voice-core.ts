// Voice rooms (prd-k2-voice-rooms-v1 v1.9 §4, §5; lane D S4/S5): the
// client's rooms list and the one call, with every dependency injected so
// tests run without a daemon, a pool or a webview. `stores/voice.ts` builds
// the app's instance over the real pool, scopes and engine.
//
// Daemon-first check (§5): each server's daemon owns its rooms, occupants
// and hosting state, and keeps them current headless. What lives here is
// per-person VIEW state (which of MY servers have rooms, my one call), the
// same split as Home: no shared truth in the renderer, and no loop the
// headless daemon needs.
//
// Two kinds of call. With the K2 app's native engine (§14, N2) the call lives in
// the app's Rust process: the plugin's `join` reaches the server through K2's
// provider, the engine's events are the call's truth, and every window of the
// app shows the same call. Without it (a build without the feature, the web
// client) a call is the seat the server gave, on the signalling path only, and
// it runs in the window that joined.

import { createStore, type StoreApi } from 'zustand/vanilla'
import type { HostEntry, PoolAuth } from '@/lib/host-pool'
import type { ServerScope } from '@/kessel/server-scope'
import type { VoiceEngine, EngineParticipant } from '@/lib/voice/voice-engine'
import { EngineError } from '@/lib/voice/voice-engine'
import {
  callEndedText,
  MIC_DENIED_LINE,
  engineErrorText,
  joinErrorText,
  micBlockedText,
  nativeJoinErrorCode,
  type VoiceJoinAnswer,
  type VoiceErrorCode,
  type VoiceRoom,
  type VoiceRoomsList,
} from '@/lib/voice/voice-wire'
import { reportsVoice, voiceServers, type VoiceServer } from '@/lib/voice/voice-servers'
import { canonicalHostKey } from '@/lib/host-key'
import { VoiceRequestError, voiceCodeOfError, type JoinOptions } from '@/lib/voice/voice-api'
import {
  DEFAULT_DEVICE,
  NativeVoiceError,
  type NativeCallState,
  type NativeConsentPrompt,
  type NativeDevices,
  type NativeEvents,
  type NativeLevel,
  type NativeParticipant,
  type NativeRecording,
  type NativeSnapshot,
  type NativeVoiceEngine,
} from '@/lib/voice/native-voice-engine'
import type { VoiceDevices } from '@/lib/voice/voice-engine'
import type { RecordingPerms } from '@/lib/voice/recording-controls'

/** §4.3: a 30 s check while the popover is open or a call runs. */
export const VOICE_ROOMS_POLL_MS = 30_000
/** Which servers report rooms at all (boot-status only), when nothing is open. */
export const VOICE_DISCOVERY_MS = 60_000
/** The seat check (below) ignores lists read this soon after connecting: the
 *  daemon's occupancy feed may not have the seat yet. */
export const VOICE_SEAT_GRACE_MS = 10_000
/** Lists in a row without the seat before the call counts as gone. */
export const VOICE_SEAT_MISSES = 2
/** A native `speaking` level above this draws the ring (0 to 1). Not yet tuned
 *  on real microphones (N4). */
export const SPEAKING_LEVEL = 0.05

export interface ServerRooms {
  list: VoiceRoomsList | null
  error: string | null
  loading: boolean
  at: number | null
}

/** The window's own server while it doesn't host rooms but has the preview
 *  on: its `/cli/voice/rooms` answer, for the dimmed headset's reason line. */
export interface WindowHosting {
  hostKey: string
  label: string
  list: VoiceRoomsList | null
  error: string | null
  loading: boolean
}

export type CallPhase = 'joining' | 'connecting' | 'connected' | 'reconnecting'

export interface VoiceCall {
  hostKey: string
  serverLabel: string
  roomId: string
  roomName: string
  /** Null until the server answered the join. */
  callId: string | null
  phase: CallPhase
  muted: boolean
  deafened: boolean
  /** Native: this person's role or a room rule took away speaking. */
  speakDenied: boolean
  /** Native: a moderator muted this person. */
  serverMuted: boolean
  /** Native: the system refused the microphone. They're in the call and can listen. */
  micDenied: boolean
  relayOnly: boolean
  /** When media came up (the call timer). */
  connectedAt: number | null
  participants: EngineParticipant[]
  /** Native, Sew 0.9.0: the room's active recording (the red badge), or null. */
  recording: NativeRecording | null
  /** K2's recording notice waiting for the person's answer ("OK" or leave).
   *  Never answered by itself: no answer is never a yes. */
  recordingNotice: RecordingNotice | null
  /** The person's own Sew permissions in the room (Record and Stop), once read. */
  recordingPerms: RecordingPerms | null
}

/** A notice on screen: the recording it is for, its slots, and where it was
 *  met (a held join, or in the call). */
export interface RecordingNotice {
  recordingId: string
  prompt: NativeConsentPrompt
  ask: 'join' | 'in_call'
}

export interface VoicePrefs {
  inputDeviceId: string | null
  outputDeviceId: string | null
  /** Join with the mic muted. */
  joinMuted: boolean
}

export interface VoiceState {
  servers: VoiceServer[]
  rooms: Record<string, ServerRooms>
  call: VoiceCall | null
  /** The last one-line reason a join failed or a call ended. */
  notice: { text: string; at: number; kind: 'error' | 'info'; micSettings?: boolean } | null
  popoverOpen: boolean
  prefs: VoicePrefs
  /** `native` once this app's native engine is attached; else the seat only. */
  engineKind: 'seat' | 'native'
  /** The audio devices, while a native call's panel has asked (hot-plug updates it). */
  devices: VoiceDevices | null
  /** The window's server's hosting answer, read by `readWindowHosting`. */
  windowHosting: WindowHosting | null
}

export interface VoiceDeps {
  /** Every server this client knows (window's, this computer's, saved). */
  keys: () => string[]
  windowKey: () => string
  labelOf: (hostKey: string) => string
  entries: () => Readonly<Record<string, HostEntry | undefined>>
  /** Home's pool: `check(key, {bootOnly})`. */
  check: (hostKey: string, opts?: { bootOnly?: boolean }) => Promise<unknown>
  onPoolChange: (fn: () => void) => () => void
  onAuth: (fn: (hostKey: string, auth: PoolAuth) => void) => () => void
  /** `voice_rooms_changed {rev}` from the window's server, whichever it is. */
  onWindowRoomsChanged: (fn: (rev: number | null) => void) => () => void
  /** `voice_rooms_changed {rev}` from one server's own bus. */
  onRoomsChanged: (hostKey: string, fn: (rev: number | null) => void) => () => void
  /** What this login looks like in that server's occupant list. */
  selfNames: (hostKey: string) => string[]
  scopeFor: (hostKey: string) => ServerScope
  api: {
    rooms: (scope: ServerScope) => Promise<VoiceRoomsList>
    join: (scope: ServerScope, roomId: string, opts: JoinOptions) => Promise<VoiceJoinAnswer>
    leave: (scope: ServerScope, callId: string) => Promise<void>
    selfState: (scope: ServerScope, callId: string, muted: boolean, deafened: boolean) => Promise<void>
    /** Sew 0.9.0: the person's own permissions in a room (Record and Stop). */
    recordingControls?: (scope: ServerScope, roomId: string) => Promise<RecordingPerms>
  }
  makeEngine: () => VoiceEngine
  toast: (text: string, kind: 'error' | 'info') => void
  loadReported: () => Set<string>
  saveReported: (keys: Set<string>) => void
  loadPrefs: () => VoicePrefs
  savePrefs: (p: VoicePrefs) => void
  now: () => number
  /** Opens the OS privacy pane, when the person asks (§4.6). False: this OS has none. */
  openMicSettings?: () => Promise<boolean>
  /** The app's native engine, when this build has one. Null: seats only. */
  loadNative?: () => Promise<NativeVoiceEngine | null>
  /** The plugin's `target` for a server: its saved host's id. Null: this client
   *  has no login of its own there. */
  targetFor?: (hostKey: string) => string | null
  /** The way back, for a window opened mid-call. */
  hostKeyForTarget?: (target: string) => string | null
}

export interface VoiceController {
  store: StoreApi<VoiceState>
  /** Re-derive the sections from the pool (no requests). */
  recompute(): void
  /** Boot-status sweep of every known server, then a full check of the ones
   *  that report rooms (role + login) — the discovery tick. */
  discover(): Promise<void>
  /** Read `/cli/voice/rooms` from every live section (or just `hostKey`). */
  refreshRooms(hostKey?: string): Promise<void>
  /** `voice_rooms_changed {rev}` from a server's bus: refetch, unless the list
   *  already has that `rev`. */
  noteRoomsChanged(hostKey: string, rev: number | null): void
  setPopoverOpen(open: boolean): void
  /** Read the window's own server's `/cli/voice/rooms` into `windowHosting`:
   *  why it doesn't host (the dimmed headset, §4.5). */
  readWindowHosting(): Promise<void>
  /** Join `room` on `hostKey`. A call in progress is left first (the UI
   *  asks before calling this, §4.3). */
  join(hostKey: string, room: VoiceRoom): Promise<void>
  leave(reason?: string): Promise<void>
  setMuted(muted: boolean): void
  setDeafened(deafened: boolean): void
  setPrefs(patch: Partial<VoicePrefs>): void
  engine(): VoiceEngine | null
  /** Read the audio devices for the call panel (`state.devices`). */
  loadDevices(): Promise<void>
  /** The person's click on "Open privacy settings". */
  openMicSettings(): Promise<void>
  /** The answer to K2's recording notice: "OK" grants (with the notice's
   *  version), "leave" leaves the call (or gives up the held join). */
  answerRecordingNotice(answer: 'ok' | 'leave'): Promise<void>
  /** Record and Stop, the person's own acts (Sew decides who may). */
  startRecording(): Promise<void>
  stopRecording(): Promise<void>
  /** Start the discovery and refresh timers; returns the stop. */
  start(): () => void
}

function canon(k: string): string {
  try {
    return canonicalHostKey(k)
  } catch {
    return k
  }
}

export function createVoiceController(deps: VoiceDeps): VoiceController {
  const store = createStore<VoiceState>(() => ({
    servers: [],
    rooms: {},
    call: null,
    notice: null,
    popoverOpen: false,
    prefs: deps.loadPrefs(),
    engineKind: 'seat',
    devices: null,
    windowHosting: null,
  }))
  let reported = deps.loadReported()
  let engine: VoiceEngine | null = null
  /** The native engine, once loaded (one per window; the call is the app's). */
  let native: NativeVoiceEngine | null = null
  let nativeLoad: Promise<NativeVoiceEngine | null> | null = null
  let nativeAttached = false
  /** This window is inside `native.join`: its answer explains a failure, so the
   *  `ended` and `errors` events of that failure stay quiet here. */
  let nativeJoining = false
  /** The call in the store came from the native engine (joined or adopted).
   *  It stays true while every headset button is unmounted (`native` is null
   *  then), so such a call never takes the seat path's `/cli/voice/leave`. */
  let nativeCall = false
  /** The engine's own list and levels; `participants` is drawn from them. */
  let nativePeople: NativeParticipant[] = []
  let nativeSpeaking = new Set<string>()
  /** The notice (recording and version) this window already put up. */
  let toldRecording: string | null = null
  /** The plugin takes `set_recording_rights` (Sew 0.9.2): null until tried. */
  let rightsCommand: boolean | null = null
  /** Bumped per join/leave so a stale async step never touches a newer call. */
  let callGen = 0
  /** Lists in a row that didn't show this login in its room. */
  let seatMisses = 0
  /** One subscription per live server, while started. */
  const subs = new Map<string, () => void>()
  let started = false
  /** One read per server at a time; a change during it reads once more. */
  const reading = new Map<string, Promise<void>>()
  const readAgain = new Set<string>()

  const setRooms = (hostKey: string, patch: Partial<ServerRooms>): void => {
    store.setState((s) => {
      const prev = s.rooms[hostKey] ?? { list: null, error: null, loading: false, at: null }
      return { rooms: { ...s.rooms, [hostKey]: { ...prev, ...patch } } }
    })
  }

  const recompute = (): void => {
    const entries = deps.entries()
    // Remember who had rooms while signed in, for the signed-out row.
    let changed = false
    const next = new Set(reported)
    for (const key of deps.keys()) {
      const e = entries[key]
      const k = canon(key)
      if (e && reportsVoice(e) && (e.auth === 'ok' || k === canon(deps.windowKey()) || k === 'local') && !next.has(k)) {
        next.add(k)
        changed = true
      }
    }
    if (changed) {
      reported = next
      deps.saveReported(next)
    }
    const servers = voiceServers({
      keys: deps.keys(),
      windowKey: deps.windowKey(),
      entries,
      labelOf: deps.labelOf,
      reportedBefore: reported,
    })
    const prev = store.getState().servers
    if (JSON.stringify(prev) !== JSON.stringify(servers)) store.setState({ servers })
    syncSubs()
  }

  const syncSubs = (): void => {
    if (!started) return
    const live = new Map(
      store
        .getState()
        .servers.filter((s) => s.kind === 'live')
        .map((s) => [canon(s.hostKey), s.hostKey] as const),
    )
    for (const [k, off] of subs) {
      if (live.has(k)) continue
      off()
      subs.delete(k)
    }
    for (const [k, key] of live) {
      if (!subs.has(k)) subs.set(k, deps.onRoomsChanged(key, (rev) => noteRoomsChanged(key, rev)))
    }
  }

  /** With no audio connection, nothing tells the client when the server drops
   *  its seat (a moderator's kick, Sew's connect timeout). The list does: a
   *  call whose login stays missing from its room has ended. Only a live feed
   *  counts, since a stale one shows nobody. */
  const checkSeat = (hostKey: string, list: VoiceRoomsList): void => {
    const c = store.getState().call
    if (native || nativeCall || !c || c.phase !== 'connected' || canon(c.hostKey) !== canon(hostKey) || !list.feedLive) return
    if (c.connectedAt === null || deps.now() - c.connectedAt < VOICE_SEAT_GRACE_MS) return
    const names = deps.selfNames(hostKey).map((n) => n.toLowerCase())
    if (names.length === 0) return
    const room = list.rooms.find((r) => r.id === c.roomId)
    if (room?.occupants.some((o) => names.includes(o.user.toLowerCase()))) {
      seatMisses = 0
      return
    }
    seatMisses += 1
    if (seatMisses >= VOICE_SEAT_MISSES) void endCall(callGen, 'seat_lost', { tellServer: true, toast: true })
  }

  const readRooms = async (hostKey: string): Promise<void> => {
    setRooms(hostKey, { loading: true })
    try {
      const list = await deps.api.rooms(deps.scopeFor(hostKey))
      setRooms(hostKey, { list, error: null, loading: false, at: deps.now() })
      checkSeat(hostKey, list)
      nameNativeRoom(hostKey, list)
      drawNativeParticipants()
    } catch (err) {
      setRooms(hostKey, { error: err instanceof Error ? err.message : String(err), loading: false })
    }
  }

  const refreshOne = (hostKey: string): Promise<void> => {
    const running = reading.get(hostKey)
    if (running) {
      readAgain.add(hostKey)
      return running
    }
    const p = readRooms(hostKey).finally(() => {
      reading.delete(hostKey)
      if (readAgain.delete(hostKey)) void refreshOne(hostKey)
    })
    reading.set(hostKey, p)
    return p
  }

  const refreshRooms = async (hostKey?: string): Promise<void> => {
    const live = store
      .getState()
      .servers.filter((s) => s.kind === 'live')
      .map((s) => s.hostKey)
    const keys = hostKey === undefined ? live : live.filter((k) => canon(k) === canon(hostKey))
    await Promise.all(keys.map(refreshOne))
  }

  const noteRoomsChanged = (hostKey: string, rev: number | null): void => {
    const known = store.getState().servers.find((s) => s.kind === 'live' && canon(s.hostKey) === canon(hostKey))
    if (!known) {
      // Its hosting moved: it may list `voice-rooms-v1` now.
      void deps.check(hostKey, { bootOnly: true }).catch(() => undefined)
      return
    }
    // Not "older than": `rev` starts over when the daemon restarts.
    if (rev !== null && store.getState().rooms[known.hostKey]?.list?.rev === rev) return
    void refreshRooms(hostKey)
  }

  const discover = async (): Promise<void> => {
    const windowKey = canon(deps.windowKey())
    const keys = deps.keys()
    await Promise.all(keys.map((k) => deps.check(k, { bootOnly: true }).catch(() => undefined)))
    // A server that reports rooms needs its login and role read (the
    // signed-in rule, §4.3). The window's own login is the window's.
    const entries = deps.entries()
    const full = keys.filter((k) => canon(k) !== windowKey && canon(k) !== 'local' && reportsVoice(entries[k]))
    await Promise.all(full.map((k) => deps.check(k).catch(() => undefined)))
    recompute()
    // The headset's look reads the lists, with or without the popover open.
    await refreshRooms()
  }

  const readWindowHosting = async (): Promise<void> => {
    const hostKey = deps.windowKey()
    const label = deps.labelOf(hostKey)
    const prev = store.getState().windowHosting
    const keep = prev && canon(prev.hostKey) === canon(hostKey) ? prev : null
    store.setState({ windowHosting: { hostKey, label, list: keep?.list ?? null, error: null, loading: true } })
    try {
      const list = await deps.api.rooms(deps.scopeFor(hostKey))
      store.setState({ windowHosting: { hostKey, label, list, error: null, loading: false } })
    } catch (err) {
      store.setState({ windowHosting: { hostKey, label, list: null, error: err instanceof Error ? err.message : String(err), loading: false } })
    }
  }

  const notice = (text: string, kind: 'error' | 'info', micSettings = false): void => {
    store.setState({ notice: { text, at: deps.now(), kind, ...(micSettings ? { micSettings } : {}) } })
    deps.toast(text, kind)
  }

  const endCall = async (gen: number, reason: string, opts: { tellServer: boolean; toast: boolean }): Promise<void> => {
    const call = store.getState().call
    if (!call || gen !== callGen) return
    callGen += 1
    const e = engine
    engine = null
    store.setState({ call: null })
    if (opts.toast) {
      notice(callEndedText(reason, call.roomName, call.serverLabel, native !== null || nativeCall), reason === 'left' ? 'info' : 'error')
    }
    if (!native && nativeCall) {
      // A native call while this window has no engine attached (every headset
      // button unmounted). The seat path's leave isn't this call's: the plugin
      // leaves, with the call credential.
      console.warn(`[voice] ending a native call (${reason}) with no engine attached: leaving through the plugin, not /cli/voice/leave`)
      clearNativeCall()
      const n = await (deps.loadNative?.() ?? Promise.resolve(null)).catch((err: unknown) => {
        console.warn('[voice] loading the native engine to leave failed:', err)
        return null
      })
      if (n) await n.leave().catch((err: unknown) => console.warn('[voice] leave failed:', err))
      void refreshRooms(call.hostKey)
      return
    }
    if (native) {
      // The engine tells Sew itself, with the call credential.
      clearNativeCall()
      await native.leave().catch((err: unknown) => {
        console.warn('[voice] leave failed:', err)
      })
      void refreshRooms(call.hostKey)
      return
    }
    if (e) await e.disconnect().catch(() => undefined)
    if (opts.tellServer && call.callId) {
      await deps.api.leave(deps.scopeFor(call.hostKey), call.callId).catch((err: unknown) => {
        console.warn('[voice] leave failed:', err)
      })
    }
    void refreshRooms(call.hostKey)
  }

  const failJoin = (gen: number, code: VoiceErrorCode, room: VoiceRoom, label: string, hint = ''): void => {
    if (gen !== callGen) return
    callGen += 1
    const e = engine
    engine = null
    store.setState({ call: null })
    if (e) void e.disconnect().catch(() => undefined)
    notice(joinErrorText(code, room.name, label, room.limit, hint), 'error', code === 'mic_denied')
  }

  // ── the native engine ────────────────────────────────────────────

  const clearNativeCall = (): void => {
    nativePeople = []
    nativeSpeaking = new Set()
    toldRecording = null
    store.setState({ devices: null })
  }

  /** The name of a Sew member: the rooms list is the only place that knows. */
  const nameOfMember = (hostKey: string, memberId: string): string | null => {
    for (const r of store.getState().rooms[hostKey]?.list?.rooms ?? []) {
      const o = r.occupants.find((x) => x.memberId === memberId)
      if (o) return o.name ?? o.user
    }
    return null
  }

  const drawNativeParticipants = (): void => {
    const c = store.getState().call
    if (!native || !c) return
    const self = nativePeople.find((p) => p.isSelf)
    const list: EngineParticipant[] = nativePeople.map((p) => ({
      user: nameOfMember(c.hostKey, p.memberId) ?? (p.isSelf ? deps.selfNames(c.hostKey)[0] ?? 'You' : 'Someone'),
      speaking: nativeSpeaking.has(p.memberId),
      muted: p.muted || p.serverMuted || p.speakDenied,
      memberId: p.memberId,
      isSelf: p.isSelf,
    }))
    const muted = self ? self.muted : c.muted
    const deafened = self ? self.deafened : c.deafened
    const speakDenied = self ? self.speakDenied : c.speakDenied
    const serverMuted = self ? self.serverMuted : c.serverMuted
    const same =
      JSON.stringify(list) === JSON.stringify(c.participants) &&
      muted === c.muted &&
      deafened === c.deafened &&
      speakDenied === c.speakDenied &&
      serverMuted === c.serverMuted
    if (same) return
    store.setState({ call: { ...c, participants: list, muted, deafened, speakDenied, serverMuted } })
    // Each says so once, when it changes.
    if (speakDenied && !c.speakDenied) notice(micBlockedText('speak_denied', c.roomName), 'error')
    else if (!speakDenied && c.speakDenied) notice(`You can speak in ${c.roomName} again.`, 'info')
    if (serverMuted && !c.serverMuted && !speakDenied) notice(micBlockedText('server_muted', c.roomName), 'error')
  }

  /** A room as the engine reports it: the slug K2 joined by, or Sew's decimal
   *  channel id after a moderator's move. */
  const roomOnList = (hostKey: string, room: string): VoiceRoom | null =>
    store.getState().rooms[hostKey]?.list?.rooms.find((r) => r.id === room || r.channelId === room) ?? null

  /** Sew 0.7.2: a `state` event names the room, and a moderator's move changes it
   *  with no break in the call. */
  const followNativeRoom = (room: string | undefined): void => {
    const c = store.getState().call
    if (!room || !c || c.roomId === room) return
    const known = roomOnList(c.hostKey, room)
    if (known) {
      if (known.id !== c.roomId) store.setState({ call: { ...c, roomId: known.id, roomName: known.name } })
      return
    }
    // Not on the list we hold: the next read names it.
    store.setState({ call: { ...c, roomId: room } })
    void refreshRooms(c.hostKey)
  }

  /** A list came in: a call still shown by channel id gets its room's slug and name. */
  const nameNativeRoom = (hostKey: string, list: VoiceRoomsList): void => {
    const c = store.getState().call
    if (!native || !c || canon(c.hostKey) !== canon(hostKey)) return
    const known = list.rooms.find((r) => r.channelId === c.roomId)
    if (known) store.setState({ call: { ...c, roomId: known.id, roomName: known.name } })
  }

  /** The plugin holds a call that's up, on a server and room it names. */
  const isLiveSnapshot = (snap: NativeSnapshot): boolean => {
    const phase = snap.state.state
    return Boolean(snap.target && snap.room) && (phase === 'connecting' || phase === 'connected' || phase === 'reconnecting')
  }

  /** A call this window didn't start: another window's, or one from before a
   *  reload. The engine and K2's Rust side say what it is. */
  const adoptNativeCall = async (): Promise<void> => {
    const n = native
    if (!n || store.getState().call || nativeJoining) return
    const gen = callGen
    const snap = await n.snapshot()
    if (gen !== callGen || store.getState().call || !snap.target || !snap.room) return
    const phase = snap.state.state
    if (phase !== 'connecting' && phase !== 'connected' && phase !== 'reconnecting') return
    const hostKey = deps.hostKeyForTarget?.(snap.target) ?? null
    if (!hostKey) {
      console.warn('[voice] the live call is on a server this window has no entry for')
      return
    }
    const onList = roomOnList(hostKey, snap.room)
    const roomId = onList?.id ?? snap.room
    const roomName = onList?.name ?? snap.room
    nativePeople = snap.participants
    const self = snap.participants.find((p) => p.isSelf)
    callGen += 1
    nativeCall = true
    store.setState({
      call: {
        hostKey,
        serverLabel: deps.labelOf(hostKey),
        roomId,
        roomName,
        callId: snap.callId,
        phase,
        muted: self?.muted ?? false,
        deafened: self?.deafened ?? false,
        speakDenied: self?.speakDenied ?? false,
        serverMuted: self?.serverMuted ?? false,
        micDenied: false,
        relayOnly: false,
        connectedAt: phase === 'connected' ? deps.now() : null,
        participants: [],
        recording: null,
        recordingNotice: null,
        recordingPerms: null,
      },
    })
    drawNativeParticipants()
    void refreshRooms(hostKey)
  }

  const onNativeState = (s: NativeCallState): void => {
    if (s.state === 'idle') return
    if (s.state === 'ended') {
      // This window's own failed join is explained by the join's answer.
      if (nativeJoining) {
        console.warn(`[voice] dropped while joining: ended (${s.reason})`)
        return
      }
      const c = store.getState().call
      if (!c) return
      callGen += 1
      engine = null
      clearNativeCall()
      store.setState({ call: null })
      // Not answering K2's recording notice in time (Sew drops the person): a plain end, not an error.
      if (s.reason !== 'left') notice(callEndedText(s.reason, c.roomName, c.serverLabel, true), s.reason === 'recording_consent_timeout' ? 'info' : 'error')
      void refreshRooms(c.hostKey)
      return
    }
    if (s.state === 'consent_required') {
      // A join held for the recording notice: no mic, no media until the
      // person answers K2's notice (the `recording` event carries it). No
      // answer is never a yes, and Sew drops a silent person in K2's rooms.
      if (nativeJoining) return
      const held = store.getState().call
      if (held && held.phase !== 'joining') store.setState({ call: { ...held, phase: 'joining' } })
      return
    }
    const c = store.getState().call
    if (!c) {
      void adoptNativeCall().catch((err: unknown) => console.warn('[voice] reading the live call failed:', err))
      return
    }
    followNativeRoom(s.room)
    const now = store.getState().call ?? c
    if (s.state === 'connected') {
      store.setState({ call: { ...now, phase: 'connected', connectedAt: now.connectedAt ?? deps.now() } })
      // A held join that the person answered: now in the room, read what they may do.
      if (!now.recordingPerms) void readRecordingPerms()
    }
    else if (s.state === 'reconnecting') store.setState({ call: { ...now, phase: 'reconnecting' } })
    else if (now.phase !== 'joining') store.setState({ call: { ...now, phase: 'connecting' } })
  }

  const onNativeParticipants = (list: NativeParticipant[]): void => {
    const c = store.getState().call
    const unknown = c && list.some((p) => !p.isSelf && nameOfMember(c.hostKey, p.memberId) === null)
    nativePeople = list
    drawNativeParticipants()
    // Someone the last list didn't have: the server's list names them.
    if (c && unknown) void refreshRooms(c.hostKey)
  }

  const onNativeSpeaking = (levels: NativeLevel[]): void => {
    // Sound from this device's own mic: the person allowed it, and the engine
    // opened it by itself.
    const c = store.getState().call
    const self = nativePeople.find((p) => p.isSelf)
    if (c?.micDenied && self && levels.some((l) => l.memberId === self.memberId && l.level > 0)) {
      store.setState({ call: { ...c, micDenied: false } })
      notice('Your microphone is working again.', 'info')
    }
    const next = new Set(levels.filter((l) => l.level > SPEAKING_LEVEL).map((l) => l.memberId))
    if (next.size === nativeSpeaking.size && [...next].every((m) => nativeSpeaking.has(m))) return
    nativeSpeaking = next
    drawNativeParticipants()
  }

  const onNativeError = (code: string): void => {
    const c = store.getState().call
    // The call goes on without the mic. The call panel keeps saying why, with a
    // button to the privacy pane.
    if (code === 'mic_denied' && c) {
      if (c.micDenied) return
      store.setState({ call: { ...c, micDenied: true } })
      if (!nativeJoining) notice(MIC_DENIED_LINE, 'error', true)
      return
    }
    // A failed join says this itself; with no call there's nothing to explain.
    if (nativeJoining || !c) {
      console.warn(`[voice] dropped engine error ${code}: ${nativeJoining ? 'a join is in flight' : 'no call'}`)
      return
    }
    const text = engineErrorText(code, c.serverLabel)
    if (text) notice(text.line, text.kind)
    else console.warn(`[voice] engine says: ${code}`)
  }

  /** The person's own Sew permissions in the call's room, for Record and
   *  Stop (Sew 0.9.0's plugin has no `can_record` / `can_stop` until 0.9.2). */
  const readRecordingPerms = async (): Promise<void> => {
    const c = store.getState().call
    const read = deps.api.recordingControls
    if (!c || !read) return
    const gen = callGen
    try {
      const perms = await read(deps.scopeFor(c.hostKey), c.roomId)
      const now = store.getState().call
      if (gen === callGen && now) store.setState({ call: { ...now, recordingPerms: perms } })
      // Sew 0.9.2's plugin decides Record and Stop from these facts (passed
      // through as read); 0.6.0's has no such command, and the derived rule
      // stands in (`recordingButtons`).
      if (native && perms.rights && rightsCommand !== false) {
        rightsCommand = await native.setRecordingRights(perms.rights)
      }
    } catch (err) {
      console.warn('[voice] reading the recording controls failed:', err)
    }
  }

  /** The call's recording (plugin 0.2.0+): the badge while there is one, and
   *  K2's notice while this person hasn't answered it. Joining or staying is
   *  the person's answer, given on the notice: nothing here answers for them. */
  const onNativeRecording = (r: NativeRecording | null): void => {
    const c = store.getState().call
    if (!c) return
    const changed = (c.recording?.recordingId ?? null) !== (r?.recordingId ?? null)
    let recordingNotice = c.recordingNotice
    if (r && r.myConsent === 'pending' && r.prompt) {
      const key = `${r.recordingId}:${r.prompt.noticeVersion}`
      if (toldRecording !== key || !recordingNotice) {
        toldRecording = key
        recordingNotice = { recordingId: r.recordingId, prompt: r.prompt, ask: c.phase === 'joining' ? 'join' : 'in_call' }
      }
    } else if (!r || r.myConsent !== 'pending') {
      recordingNotice = null
    }
    store.setState({ call: { ...c, recording: r, recordingNotice } })
    if (changed) void readRecordingPerms()
  }

  const answerRecordingNotice = async (answer: 'ok' | 'leave'): Promise<void> => {
    const c = store.getState().call
    const n = native
    if (!c?.recordingNotice) return
    const shown = c.recordingNotice
    store.setState({ call: { ...c, recordingNotice: null } })
    if (answer === 'leave' || !n) {
      await leave('left')
      return
    }
    try {
      await n.consent(true, shown.prompt.noticeVersion)
    } catch (err) {
      const e = err instanceof NativeVoiceError ? err : null
      console.warn('[voice] recording answer failed:', err)
      const now = store.getState().call
      // Still in the call (or still held): ask again, never assume a yes.
      if (now) store.setState({ call: { ...now, recordingNotice: shown } })
      notice(e?.code === 'not_in_call' ? 'That call ended.' : "K2 couldn't send your answer to the recording notice. Try again, or leave the call.", 'error')
    }
  }

  const recordingCommand = async (what: 'start' | 'stop'): Promise<void> => {
    const n = native
    if (!n || !store.getState().call) return
    try {
      if (what === 'start') await n.startRecording()
      else await n.stopRecording()
    } catch (err) {
      const code = err instanceof NativeVoiceError ? err.code : 'failed'
      console.warn(`[voice] ${what} recording failed:`, err)
      notice(
        code === 'recording_required'
          ? 'This room records every call with more than one person, so its recording keeps going while two or more are here.'
          : code === 'recording_active'
            ? 'This call is already being recorded.'
            : code === 'recording_disabled'
              ? "Recording is off on this server."
              : what === 'start'
                ? "K2 couldn't start recording this call."
                : "K2 couldn't stop the recording.",
        'error',
      )
    }
  }

  const nativeEvents: NativeEvents = {
    onState: onNativeState,
    onRecording: onNativeRecording,
    onParticipants: onNativeParticipants,
    onSpeaking: onNativeSpeaking,
    onDevices: (d: NativeDevices) => {
      if (store.getState().devices) store.setState({ devices: d })
    },
    onError: onNativeError,
  }

  /** Attaching again (the headset button came back): events were missed, so a
   *  call this window still shows takes its truth from the engine. */
  const reconcileNativeCall = async (n: NativeVoiceEngine): Promise<void> => {
    const c = store.getState().call
    if (!c || nativeJoining) return
    const snap = await n.snapshot()
    const s = snap.state.state
    if (s === 'connecting' || s === 'connected' || s === 'reconnecting') {
      nativePeople = snap.participants
      onNativeState(snap.state)
      drawNativeParticipants()
      return
    }
    callGen += 1
    clearNativeCall()
    store.setState({ call: null })
    void refreshRooms(c.hostKey)
  }

  /** Load the native engine once and attach this window to it. */
  const loadNativeEngine = (): Promise<NativeVoiceEngine | null> => {
    if (nativeLoad) return nativeLoad
    nativeLoad = (async () => {
      const n = (await deps.loadNative?.()) ?? null
      if (!n) return null
      await n.start(nativeEvents)
      native = n
      nativeAttached = true
      store.setState({ engineKind: 'native' })
      await reconcileNativeCall(n)
      await adoptNativeCall()
      return n
    })().catch((err: unknown) => {
      // Reported, not hidden; this window then holds seats only.
      console.error('[voice] the native engine did not start:', err)
      nativeLoad = null
      return null
    })
    return nativeLoad
  }

  /** The plugin refused the join: it holds a call. Another window's live call is
   *  shown. A call that already ended but that the plugin still holds (Sew
   *  < 0.8.3, e.g. after the lid closed on a call) can't be left from here, so
   *  say so instead of doing nothing. */
  const settleAlreadyInCall = async (gen: number, room: VoiceRoom, label: string, hostKey: string): Promise<void> => {
    const n = native
    const snap = n
      ? await n.snapshot().catch((err: unknown) => {
          console.warn('[voice] reading the live call failed:', err)
          return null
        })
      : null
    if (gen !== callGen) return
    if (snap && isLiveSnapshot(snap)) {
      // Another window got there first: show that call.
      store.setState({ call: null })
      callGen += 1
      await adoptNativeCall().catch((failure: unknown) => console.warn('[voice] reading the live call failed:', failure))
      return
    }
    console.error('[voice] join refused: already_in_call with no live call')
    failJoin(gen, 'engine_busy', room, label)
    void refreshRooms(hostKey)
  }

  const failNativeJoin = (gen: number, err: unknown, room: VoiceRoom, label: string, hostKey: string): void => {
    if (gen !== callGen) return
    const e = err instanceof NativeVoiceError ? err : new NativeVoiceError('failed', err instanceof Error ? err.message : String(err))
    // A leave from another window cancelled it (our own leave moved `callGen`).
    if (e.code === 'left') {
      callGen += 1
      clearNativeCall()
      store.setState({ call: null })
      void refreshRooms(hostKey)
      return
    }
    if (e.code === 'already_in_call') {
      nativeJoining = false
      void settleAlreadyInCall(gen, room, label, hostKey)
      return
    }
    const detail = e.detail && typeof e.detail === 'object' ? (e.detail as Record<string, unknown>) : null
    const hint = typeof detail?.hint === 'string' ? detail.hint : ''
    failJoin(gen, nativeJoinErrorCode(e.code, detail), room, label, hint)
    void refreshRooms(hostKey)
  }

  const joinNative = async (n: NativeVoiceEngine, hostKey: string, room: VoiceRoom): Promise<void> => {
    const current = store.getState().call
    if (current) await endCall(callGen, 'left', { tellServer: true, toast: false })
    const gen = ++callGen
    const label = deps.labelOf(hostKey)
    const prefs = store.getState().prefs
    clearNativeCall()
    nativeCall = true
    store.setState({
      notice: null,
      call: {
        hostKey,
        serverLabel: label,
        roomId: room.id,
        roomName: room.name,
        callId: null,
        phase: 'joining',
        muted: prefs.joinMuted,
        deafened: false,
        speakDenied: false,
        serverMuted: false,
        micDenied: false,
        relayOnly: false,
        connectedAt: null,
        participants: [],
        recording: null,
        recordingNotice: null,
        recordingPerms: null,
      },
    })
    const target = deps.targetFor?.(hostKey) ?? null
    if (target === null) {
      failJoin(gen, canon(hostKey) === 'local' ? 'owner_token_cannot_join' : 'signin_required', room, label)
      return
    }
    nativeJoining = true
    try {
      // Saved picks apply to the next call. A pick that's gone is no reason to
      // refuse the call: it runs on the default device.
      if (prefs.inputDeviceId || prefs.outputDeviceId) {
        await n
          .setDevice({ ...(prefs.inputDeviceId ? { input: prefs.inputDeviceId } : {}), ...(prefs.outputDeviceId ? { output: prefs.outputDeviceId } : {}) })
          .catch((err: unknown) => console.warn('[voice] saved audio device not applied:', err))
      }
      const { callId } = await n.join({ target, room: room.id, muted: prefs.joinMuted })
      if (gen !== callGen) return
      const after = (await n.snapshot()).state
      const phase = after.state
      if (gen !== callGen) return
      const c = store.getState().call
      if (!c) return
      if (after.state === 'ended') {
        // Ended as it was joined: its reason is the only word on why.
        console.warn(`[voice] the call ended as it was joined: ${after.reason}`)
        store.setState({ call: null })
        callGen += 1
        clearNativeCall()
        if (after.reason !== 'left') notice(callEndedText(after.reason, c.roomName, c.serverLabel, true), 'error')
        void refreshRooms(hostKey)
        return
      }
      if (phase === 'consent_required') {
        // Held for the recording notice: the call waits for the person's
        // answer to K2's notice (OK joins, recorded; "Don't join" gives up).
        store.setState({ call: { ...c, callId: callId || null, phase: 'joining' } })
        void refreshRooms(hostKey)
        return
      }
      store.setState({
        call: {
          ...c,
          callId,
          phase: phase === 'connected' ? 'connected' : phase === 'reconnecting' ? 'reconnecting' : 'connecting',
          connectedAt: phase === 'connected' ? (c.connectedAt ?? deps.now()) : c.connectedAt,
        },
      })
      drawNativeParticipants()
      if (store.getState().call?.micDenied) notice(MIC_DENIED_LINE, 'error', true)
      void refreshRooms(hostKey)
      void readRecordingPerms()
    } catch (err) {
      failNativeJoin(gen, err, room, label, hostKey)
    } finally {
      nativeJoining = false
    }
  }

  const join = async (hostKey: string, room: VoiceRoom): Promise<void> => {
    const n = deps.loadNative ? await loadNativeEngine() : null
    if (n) return joinNative(n, hostKey, room)
    const current = store.getState().call
    if (current) await endCall(callGen, 'left', { tellServer: true, toast: false })
    const gen = ++callGen
    seatMisses = 0
    nativeCall = false
    const label = deps.labelOf(hostKey)
    const prefs = store.getState().prefs
    store.setState({
      notice: null,
      call: {
        hostKey,
        serverLabel: label,
        roomId: room.id,
        roomName: room.name,
        callId: null,
        phase: 'joining',
        muted: prefs.joinMuted,
        deafened: false,
        speakDenied: false,
        serverMuted: false,
        micDenied: false,
        relayOnly: false,
        connectedAt: null,
        participants: [],
        recording: null,
        recordingNotice: null,
        recordingPerms: null,
      },
    })
    const scope = deps.scopeFor(hostKey)
    let answer: VoiceJoinAnswer
    try {
      answer = await deps.api.join(scope, room.id, {
        selfMute: prefs.joinMuted,
        // V16: a held join answered after we gave up is left at once.
        onLateAnswer: (late) => void deps.api.leave(scope, late.callId).catch(() => undefined),
      })
    } catch (err) {
      failJoin(gen, err instanceof VoiceRequestError ? err.code : voiceCodeOfError(err), room, label, err instanceof Error ? err.message : '')
      return
    }
    if (gen !== callGen) {
      // Left (or joined elsewhere) while the server answered.
      void deps.api.leave(scope, answer.callId).catch(() => undefined)
      return
    }
    store.setState((s) =>
      s.call
        ? { call: { ...s.call, callId: answer.callId, phase: 'connecting', relayOnly: answer.relayOnly, roomName: answer.roomName || s.call.roomName } }
        : s,
    )
    const e = deps.makeEngine()
    engine = e
    try {
      await e.connect(
        {
          signalUrl: answer.signalUrl,
          mediaToken: answer.mediaToken,
          e2ee: answer.e2ee,
          relayOnly: answer.relayOnly,
          inputDeviceId: prefs.inputDeviceId,
          outputDeviceId: prefs.outputDeviceId,
          startMuted: prefs.joinMuted,
        },
        {
          onState: (state, reason) => {
            if (gen !== callGen) return
            if (state === 'reconnecting') store.setState((s) => (s.call ? { call: { ...s.call, phase: 'reconnecting' } } : s))
            else if (state === 'connected')
              store.setState((s) => (s.call ? { call: { ...s.call, phase: 'connected', connectedAt: s.call.connectedAt ?? deps.now() } } : s))
            else if (state === 'ended' && store.getState().call?.phase !== 'connecting')
              void endCall(gen, reason ?? 'ended', { tellServer: true, toast: true })
          },
          onParticipants: (list) => {
            if (gen !== callGen) return
            store.setState((s) => (s.call ? { call: { ...s.call, participants: list } } : s))
          },
        },
      )
    } catch (err) {
      const code: VoiceErrorCode = err instanceof EngineError && err.code !== 'failed' ? err.code : 'unknown'
      void deps.api.leave(scope, answer.callId).catch(() => undefined)
      failJoin(gen, code, room, label)
      return
    }
    void refreshRooms(hostKey)
  }

  const leave = async (reason = 'left'): Promise<void> => {
    await endCall(callGen, reason, { tellServer: true, toast: true })
  }

  const sendSelf = (): void => {
    const c = store.getState().call
    // The native engine sends Sew's self state itself.
    if (!c?.callId || native || nativeCall) return
    void deps.api.selfState(deps.scopeFor(c.hostKey), c.callId, c.muted, c.deafened).catch((err: unknown) => {
      console.warn('[voice] self state failed:', err)
    })
  }

  const setMuted = (muted: boolean): void => {
    const c = store.getState().call
    if (!c) return
    if (native) {
      // The mic isn't this person's to turn on right now.
      if (c.speakDenied || c.serverMuted) return
      // Optimistic; the engine's `participants` event is the truth.
      store.setState({ call: { ...c, muted } })
      void native.mute({ muted }).catch((err: unknown) => console.warn('[voice] mute failed:', err))
      return
    }
    engine?.setMuted(muted)
    store.setState({ call: { ...c, muted } })
    sendSelf()
  }

  const setDeafened = (deafened: boolean): void => {
    const c = store.getState().call
    if (!c) return
    if (native) {
      store.setState({ call: { ...c, deafened, muted: deafened ? true : c.muted } })
      void native.mute({ deafened }).catch((err: unknown) => console.warn('[voice] deafen failed:', err))
      return
    }
    engine?.setDeafened(deafened)
    // Deafen mutes too (the usual rule); undeafen leaves the mic as it was.
    store.setState({ call: { ...c, deafened, muted: deafened ? true : c.muted } })
    if (deafened) engine?.setMuted(true)
    sendSelf()
  }

  const setPrefs = (patch: Partial<VoicePrefs>): void => {
    const prefs = { ...store.getState().prefs, ...patch }
    store.setState({ prefs })
    deps.savePrefs(prefs)
    if (native) {
      void applyNativeDevice(native, patch).catch((err: unknown) => {
        console.warn('[voice] switching audio device failed:', err)
        notice("K2 couldn't switch to that audio device.", 'error')
      })
      return
    }
    if (patch.inputDeviceId !== undefined) void engine?.setInputDevice(patch.inputDeviceId)
    if (patch.outputDeviceId !== undefined) void engine?.setOutputDevice(patch.outputDeviceId)
  }

  /** "System default" (`null`) follows the OS's default from then on. */
  const applyNativeDevice = async (n: NativeVoiceEngine, patch: Partial<VoicePrefs>): Promise<void> => {
    const pick = (id: string | null | undefined): string | undefined => (id === undefined ? undefined : (id ?? DEFAULT_DEVICE))
    const input = pick(patch.inputDeviceId)
    const output = pick(patch.outputDeviceId)
    if (input === undefined && output === undefined) return
    await n.setDevice({ ...(input !== undefined ? { input } : {}), ...(output !== undefined ? { output } : {}) })
  }

  const loadDevices = async (): Promise<void> => {
    if (native) {
      store.setState({ devices: await native.devices() })
      return
    }
    if (engine) store.setState({ devices: await engine.listDevices() })
  }

  const openMicSettings = async (): Promise<void> => {
    let opened = false
    try {
      opened = (await deps.openMicSettings?.()) === true
    } catch (err) {
      console.warn('[voice] opening the privacy settings failed:', err)
    }
    if (!opened) notice("K2 couldn't open your settings. Open your system's sound settings and allow K2.", 'error')
  }

  // §4.4: signed out, kicked or removed on the call's server ends the call.
  deps.onAuth((hostKey, auth) => {
    const c = store.getState().call
    if (!c || canon(c.hostKey) !== canon(hostKey)) return
    if (auth === 'signin-required' || auth === 'kicked') {
      void endCall(callGen, 'signed_out', { tellServer: false, toast: true })
    }
  })

  return {
    store,
    recompute,
    discover,
    refreshRooms,
    noteRoomsChanged,
    setPopoverOpen(open) {
      store.setState({ popoverOpen: open })
      if (open) void refreshRooms()
    },
    readWindowHosting,
    join,
    leave,
    setMuted,
    setDeafened,
    setPrefs,
    engine: () => engine,
    loadDevices,
    openMicSettings,
    answerRecordingNotice,
    startRecording: () => recordingCommand('start'),
    stopRecording: () => recordingCommand('stop'),
    start() {
      started = true
      void loadNativeEngine()
      const unsubPool = deps.onPoolChange(recompute)
      const unsubWindow = deps.onWindowRoomsChanged((rev) => noteRoomsChanged(deps.windowKey(), rev))
      recompute()
      void discover()
      const discoverTimer = setInterval(() => void discover(), VOICE_DISCOVERY_MS)
      const roomsTimer = setInterval(() => {
        const s = store.getState()
        if (s.popoverOpen || s.call) void refreshRooms()
      }, VOICE_ROOMS_POLL_MS)
      return () => {
        started = false
        unsubPool()
        unsubWindow()
        for (const off of subs.values()) off()
        subs.clear()
        clearInterval(discoverTimer)
        clearInterval(roomsTimer)
        if (native && nativeAttached) {
          native.stop()
          native = null
          nativeAttached = false
          nativeLoad = null
          store.setState({ engineKind: 'seat' })
        }
      }
    },
  }
}
