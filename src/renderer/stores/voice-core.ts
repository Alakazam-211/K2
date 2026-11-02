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
// 0.46.8 carries no call audio: a call is the seat the server gave, on the
// signalling path only. It runs in the window that joined. Native audio
// (`sew-call-client`, 0.46.9) plugs in behind the same engine seam.

import { createStore, type StoreApi } from 'zustand/vanilla'
import type { HostEntry, PoolAuth } from '@/lib/host-pool'
import type { ServerScope } from '@/kessel/server-scope'
import type { VoiceEngine, EngineParticipant } from '@/lib/voice/voice-engine'
import { EngineError } from '@/lib/voice/voice-engine'
import {
  callEndedText,
  joinErrorText,
  type VoiceJoinAnswer,
  type VoiceErrorCode,
  type VoiceRoom,
  type VoiceRoomsList,
} from '@/lib/voice/voice-wire'
import { reportsVoice, voiceServers, type VoiceServer } from '@/lib/voice/voice-servers'
import { canonicalHostKey } from '@/lib/host-key'
import { VoiceRequestError, voiceCodeOfError, type JoinOptions } from '@/lib/voice/voice-api'

/** §4.3: a 30 s check while the popover is open or a call runs. */
export const VOICE_ROOMS_POLL_MS = 30_000
/** Which servers report rooms at all (boot-status only), when nothing is open. */
export const VOICE_DISCOVERY_MS = 60_000
/** The seat check (below) ignores lists read this soon after connecting: the
 *  daemon's occupancy feed may not have the seat yet. */
export const VOICE_SEAT_GRACE_MS = 10_000
/** Lists in a row without the seat before the call counts as gone. */
export const VOICE_SEAT_MISSES = 2

export interface ServerRooms {
  list: VoiceRoomsList | null
  error: string | null
  loading: boolean
  at: number | null
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
  relayOnly: boolean
  /** When media came up (the call timer). */
  connectedAt: number | null
  participants: EngineParticipant[]
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
  notice: { text: string; at: number; kind: 'error' | 'info' } | null
  popoverOpen: boolean
  prefs: VoicePrefs
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
  }
  makeEngine: () => VoiceEngine
  toast: (text: string, kind: 'error' | 'info') => void
  loadReported: () => Set<string>
  saveReported: (keys: Set<string>) => void
  loadPrefs: () => VoicePrefs
  savePrefs: (p: VoicePrefs) => void
  now: () => number
  /** "Opens the OS privacy pane" on `mic_denied` (§4.6). */
  openMicSettings?: () => void
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
  /** Join `room` on `hostKey`. A call in progress is left first (the UI
   *  asks before calling this, §4.3). */
  join(hostKey: string, room: VoiceRoom): Promise<void>
  leave(reason?: string): Promise<void>
  setMuted(muted: boolean): void
  setDeafened(deafened: boolean): void
  setPrefs(patch: Partial<VoicePrefs>): void
  engine(): VoiceEngine | null
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
  }))
  let reported = deps.loadReported()
  let engine: VoiceEngine | null = null
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
    if (!c || c.phase !== 'connected' || canon(c.hostKey) !== canon(hostKey) || !list.feedLive) return
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

  const notice = (text: string, kind: 'error' | 'info'): void => {
    store.setState({ notice: { text, at: deps.now(), kind } })
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
      notice(callEndedText(reason, call.roomName, call.serverLabel), reason === 'left' ? 'info' : 'error')
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
    notice(joinErrorText(code, room.name, label, room.limit, hint), 'error')
    if (code === 'mic_denied') deps.openMicSettings?.()
  }

  const join = async (hostKey: string, room: VoiceRoom): Promise<void> => {
    const current = store.getState().call
    if (current) await endCall(callGen, 'left', { tellServer: true, toast: false })
    const gen = ++callGen
    seatMisses = 0
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
        relayOnly: false,
        connectedAt: null,
        participants: [],
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
    if (!c?.callId) return
    void deps.api.selfState(deps.scopeFor(c.hostKey), c.callId, c.muted, c.deafened).catch((err: unknown) => {
      console.warn('[voice] self state failed:', err)
    })
  }

  const setMuted = (muted: boolean): void => {
    const c = store.getState().call
    if (!c) return
    engine?.setMuted(muted)
    store.setState({ call: { ...c, muted } })
    sendSelf()
  }

  const setDeafened = (deafened: boolean): void => {
    const c = store.getState().call
    if (!c) return
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
    if (patch.inputDeviceId !== undefined) void engine?.setInputDevice(patch.inputDeviceId)
    if (patch.outputDeviceId !== undefined) void engine?.setOutputDevice(patch.outputDeviceId)
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
    join,
    leave,
    setMuted,
    setDeafened,
    setPrefs,
    engine: () => engine,
    start() {
      started = true
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
      }
    },
  }
}
