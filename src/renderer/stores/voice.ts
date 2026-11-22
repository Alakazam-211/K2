// Voice rooms lane D: the app's voice controller over Home's pool, each
// server's own scope and the installed media engine. The logic and its
// tests live in `stores/voice-core.ts`.
//
// Gated end to end by the servers themselves: a daemon with
// `voice_rooms_preview` off never reports `voice-rooms-v1`, so no section
// exists, the headset never shows, and nothing but the boot-status sweep
// (which Home's pool already makes) is ever sent.

import { useStore } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { hostPool } from '@/lib/host-pool-instance'
import { homeHostKey, LOCAL_HOME_HOST, savedHostForKey } from '@/lib/host-key'
import { primaryScope, scopeForHost } from '@/kessel/server-scope'
import { isWebClient } from '@/lib/is-web'
import { useConnectHostStore } from '@/stores/connect-host'
import { onVoiceRoomsChanged } from '@/stores/session-events'
import { useToastStore } from '@/stores/toast'
import { fetchRecordingControls, fetchVoiceRooms, joinVoiceRoom, leaveVoiceCall, sendVoiceSelfState } from '@/lib/voice/voice-api'
import { makeVoiceEngine } from '@/lib/voice/voice-engine'
import { NativeVoiceEngine, type NativeStatus, type NativeVoiceBridge } from '@/lib/voice/native-voice-engine'
import { setUpdateInstallGuard } from '@/stores/update'
import { createVoiceController, type VoicePrefs, type VoiceState } from './voice-core'
import { subscribeVoiceEvents } from '@/lib/voice/voice-events-socket'
import { hostIsVoiceChatOnly, isVoiceChatOnly } from '@/lib/voice-chat-only'
import { callPersonName, dmCallRoomName } from '@/lib/home-chat/call-names'
import { refreshHomeChatPeople, refreshHomeChatSummary, useHomeChatStore } from '@/stores/home-chat'
import { PRODUCT_NAME } from '@/lib/brand'

const REPORTED_KEY = 'k2.voice.reportedServers.v1'
const PREFS_KEY = 'k2.voice.prefs.v1'

const DEFAULT_PREFS: VoicePrefs = { inputDeviceId: null, outputDeviceId: null, joinMuted: false, noiseRemoval: true }

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key)
    return raw ? (JSON.parse(raw) as T) : fallback
  } catch {
    return fallback
  }
}

function writeJson(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value))
  } catch {
    // Per-device convenience only: the list still works without it.
  }
}

function windowKey(): string {
  return homeHostKey(useConnectHostStore.getState().activeHost)
}

function knownKeys(): string[] {
  const keys = [windowKey()]
  if (!isWebClient()) keys.push(LOCAL_HOME_HOST)
  for (const h of useConnectHostStore.getState().hosts) keys.push(homeHostKey(h))
  return keys
}

function labelOf(hostKey: string): string {
  if (hostKey === LOCAL_HOME_HOST) return 'This computer'
  const saved = savedHostForKey(useConnectHostStore.getState().hosts, hostKey)
  return saved ? saved.label || saved.hostname : hostKey
}

const tauriBridge: NativeVoiceBridge = {
  invoke: (command, args) => invoke(command, args),
  listen: (event, handler) => listen(event, (e) => handler(e.payload)),
}

/** The native engine, when this desktop build has it (the `native-voice`
 *  feature). The web client has no plugin, and holds seats only (V42). */
async function loadNative(): Promise<NativeVoiceEngine | null> {
  if (isWebClient()) return null
  const status = await invoke<NativeStatus>('voice_native_status')
  return status.available ? new NativeVoiceEngine(tauriBridge) : null
}

function targetFor(hostKey: string): string | null {
  if (hostKey === LOCAL_HOME_HOST) return null
  return savedHostForKey(useConnectHostStore.getState().hosts, hostKey)?.id ?? null
}

/** Whether this client's login on `hostKey` is Voice & Chat only (the
 *  pool's whoami, else the saved host's hint). */
function voiceChatOnlyHost(hostKey: string): boolean {
  if (isVoiceChatOnly(hostPool.entry(hostKey)?.role)) return true
  return hostIsVoiceChatOnly(savedHostForKey(useConnectHostStore.getState().hosts, hostKey))
}

function hostKeyForTarget(target: string): string | null {
  const host = useConnectHostStore.getState().hosts.find((h) => h.id === target)
  return host ? homeHostKey(host) : null
}

/** A DM call's people are named from Home's people (their room is on no
 *  rooms list). A window that never opened Home (the call's own window)
 *  reads them now; the names follow when they arrive. */
const NAMES_RETRY_MS = 30_000
const namesAsked = new Map<string, number>()
function chatNames(hostKey: string, roomId: string | null) {
  const h = useHomeChatStore.getState().byHost[hostKey]
  // At most one read per server per NAMES_RETRY_MS: the names are drawn on
  // every speaking change, and a server with no Home chat never answers.
  if ((!h?.people || (roomId && !h.summary)) && Date.now() - (namesAsked.get(hostKey) ?? 0) > NAMES_RETRY_MS) {
    namesAsked.set(hostKey, Date.now())
    if (!h?.people) void refreshHomeChatPeople(hostKey)
    if (roomId && !h?.summary) void refreshHomeChatSummary(hostKey)
  }
  return h
}

export const voice = createVoiceController({
  keys: knownKeys,
  windowKey,
  labelOf,
  entries: () => hostPool.store.getState().entries,
  check: (key, opts) => hostPool.check(key, opts),
  onPoolChange: (fn) => {
    const a = hostPool.store.subscribe(fn)
    const b = useConnectHostStore.subscribe((s, prev) => {
      if (s.hosts !== prev.hosts || s.activeHost !== prev.activeHost) fn()
    })
    return () => {
      a()
      b()
    }
  },
  onAuth: (fn) => hostPool.onAuth(fn),
  // The window's own server pushes on the primary bus (its app socket), which
  // survives a server switch. Any other server's bus has frames only while a
  // Home room's socket carries them; the 60 s sweep covers the rest.
  onWindowRoomsChanged: (fn) => onVoiceRoomsChanged(primaryScope(), fn),
  // A Voice & Chat only login never gets that server's app socket: its
  // rooms changes come on `/cli/voice/events` (prd-voice-chat-only-members-v1
  // §6.3), with the 60 s sweep still behind it.
  onRoomsChanged: (key, fn) =>
    voiceChatOnlyHost(key) ? subscribeVoiceEvents(scopeForHost(key), fn) : onVoiceRoomsChanged(scopeForHost(key), fn),
  selfNames: (key) => {
    const saved = savedHostForKey(useConnectHostStore.getState().hosts, key)
    const role = hostPool.entry(key)?.role ?? null
    // A linked owner login shows in the list as `owner`, not its login name.
    return [...(saved?.username ? [saved.username] : []), ...(saved && (role === null || role === 'owner') ? ['owner'] : [])]
  },
  scopeFor: (key) => scopeForHost(key),
  api: {
    rooms: fetchVoiceRooms,
    join: joinVoiceRoom,
    leave: leaveVoiceCall,
    selfState: sendVoiceSelfState,
    recordingControls: fetchRecordingControls,
  },
  makeEngine: makeVoiceEngine,
  toast: (text, kind) => useToastStore.getState().addToast(text, kind === 'error' ? 'error' : 'info'),
  loadReported: () => new Set(readJson<string[]>(REPORTED_KEY, [])),
  saveReported: (keys) => writeJson(REPORTED_KEY, [...keys]),
  loadPrefs: () => {
    const saved = readJson<Partial<VoicePrefs>>(PREFS_KEY, {})
    // Noise removal is on unless the person turned it off (Sew 0.10.2): prefs
    // saved before it existed have no field, and anything but `false` is on.
    return { ...DEFAULT_PREFS, ...saved, noiseRemoval: saved.noiseRemoval !== false }
  },
  savePrefs: (p) => writeJson(PREFS_KEY, p),
  now: () => Date.now(),
  openMicSettings: () => invoke<boolean>('permissions_open_microphone_settings'),
  loadNative,
  targetFor,
  hostKeyForTarget,
  personName: (key, memberId) => callPersonName(chatNames(key, null), memberId),
  dmRoomName: (key, roomId) => dmCallRoomName(chatNames(key, roomId), roomId),
})

// Home's people or DMs arrived or changed: name the call's people again.
useHomeChatStore.subscribe((s, prev) => {
  if (s.byHost !== prev.byHost && voice.store.getState().call) voice.renamePeople()
})

// vs-live V36: installing an update restarts the app, which ends a call.
setUpdateInstallGuard(async () => {
  const call = voice.store.getState().call
  if (!call || voice.store.getState().engineKind !== 'native') return true
  return window.confirm(`Updating ${PRODUCT_NAME} ends your call in ${call.roomName}. Update now?`)
})

let starts = 0
let stop: (() => void) | null = null

/** Start the sweep while at least one headset button is mounted. */
export function retainVoice(): () => void {
  starts += 1
  if (starts === 1) stop = voice.start()
  return () => {
    starts -= 1
    if (starts === 0 && stop) {
      stop()
      stop = null
    }
  }
}

export function useVoice<T>(sel: (s: VoiceState) => T): T {
  return useStore(voice.store, sel)
}
