// The store over the native engine (N2): every event comes through a fake of the
// plugin, with the shapes pinned in native-voice-contract.json and Sew's own
// sew-native-voice-contract.json.

import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import type { HostEntry } from '@/lib/host-pool'
import type { ServerScope } from '@/kessel/server-scope'
import { NativeVoiceEngine, PLUGIN_COMMAND_PREFIX, type NativeVoiceBridge } from '@/lib/voice/native-voice-engine'
import { SeatOnlyEngine } from '@/lib/voice/voice-engine'
import type { VoiceRoom, VoiceRoomsList } from '@/lib/voice/voice-wire'
import { createVoiceController, type VoiceDeps } from './voice-core'

const sewContract = JSON.parse(readFileSync(resolve(__dirname, '../lib/voice/sew-native-voice-contract.json'), 'utf8'))

const SEW = 'sewlab.k2.dev'
const ME = '48831765147549699'
const P = PLUGIN_COMMAND_PREFIX

const person = (member_id: string, is_self: boolean, over: Partial<{ muted: boolean; deafened: boolean; server_muted: boolean; server_deafened: boolean }> = {}) => ({
  member_id,
  is_self,
  muted: false,
  deafened: false,
  server_muted: false,
  server_deafened: false,
  ...over,
})

const occupant = (user: string, memberId: string) => ({ user, name: null, memberId, role: null, color: null, muted: false, deafened: false, speaking: false })
const room1: VoiceRoom = { id: 'room-1', name: 'Standup', limit: 25, occupants: [occupant('ada', ME), occupant('grace', '7')], closing: false, closesAt: null, unavailable: null }
const room2: VoiceRoom = { id: 'room-2', name: 'Room 2', limit: 25, occupants: [], closing: false, closesAt: null, unavailable: null }
const list: VoiceRoomsList = { hosting: { state: 'ok', line: null, detail: null }, rooms: [room1, room2], rev: 1, canManage: false, feedLive: true }

function entry(hostKey: string): HostEntry {
  return {
    hostKey,
    saved: true,
    hostId: 'host-1',
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.46.8', protocol: 1, instanceId: 'i', features: ['voice-rooms-v1'], view: 'full', at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: 1,
  }
}

interface Plugin {
  state: unknown
  call: { target: string; room: string } | null
  joinResult: unknown
  joinError: unknown
  devices: unknown
}

function setup(over: Partial<VoiceDeps> & { plugin?: Partial<Plugin> } = {}) {
  const plugin: Plugin = {
    state: { state: 'idle', participants: [], call_id: null },
    call: null,
    joinResult: { call_id: 'r_x' },
    joinError: null,
    devices: { inputs: [{ id: 'in:1', name: 'Mic', default: true }], outputs: [{ id: 'out:1', name: 'Speakers', default: true }], input: null, output: null },
    ...over.plugin,
  }
  const calls: Array<[string, Record<string, unknown> | undefined]> = []
  const listeners = new Map<string, (p: unknown) => void>()
  const bridge: NativeVoiceBridge = {
    invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push([command, args])
      switch (command) {
        case 'voice_native_status':
          return { available: true, call: plugin.call } as T
        case `${P}state`:
          return plugin.state as T
        case `${P}join`:
          if (plugin.joinError) throw plugin.joinError
          return plugin.joinResult as T
        case `${P}devices`:
          return plugin.devices as T
        default:
          return null as T
      }
    },
    listen: async (event, handler) => {
      listeners.set(event, handler)
      return () => listeners.delete(event)
    },
  }
  const fire = (name: string, payload: unknown): void => {
    const h = listeners.get(`sew-voice://${name}`)
    if (!h) throw new Error(`nothing listens to ${name}`)
    h(payload)
  }
  const toasts: string[] = []
  const api = {
    rooms: vi.fn(async (_s: ServerScope): Promise<VoiceRoomsList> => list),
    join: vi.fn(async () => {
      throw new Error('the store must not call /cli/voice/join with the native engine')
    }),
    leave: vi.fn(async () => {
      throw new Error('the engine tells Sew itself')
    }),
    selfState: vi.fn(async () => {
      throw new Error('the engine sends self state itself')
    }),
  }
  const authFns: Array<(k: string, a: 'signin-required') => void> = []
  const openMicSettings = vi.fn()
  const entries: Record<string, HostEntry> = { [SEW]: entry(SEW) }
  const ctl = createVoiceController({
    keys: () => ['local', SEW],
    windowKey: () => 'local',
    labelOf: (k) => (k === SEW ? 'sewlab' : k),
    entries: () => entries,
    check: async () => undefined,
    onPoolChange: () => () => {},
    onAuth: (fn) => {
      authFns.push(fn as (k: string, a: 'signin-required') => void)
      return () => {}
    },
    onWindowRoomsChanged: () => () => {},
    onRoomsChanged: () => () => {},
    selfNames: () => ['ada'],
    scopeFor: (k) => ({ hostKey: k, id: k, label: k }) as unknown as ServerScope,
    api: api as unknown as VoiceDeps['api'],
    makeEngine: () => new SeatOnlyEngine(),
    toast: (t) => toasts.push(t),
    loadReported: () => new Set(),
    saveReported: () => {},
    loadPrefs: () => ({ inputDeviceId: null, outputDeviceId: null, joinMuted: false }),
    savePrefs: () => {},
    now: () => 5_000,
    openMicSettings,
    loadNative: async () => new NativeVoiceEngine(bridge),
    targetFor: (k) => (k === SEW ? 'host-1' : null),
    hostKeyForTarget: (t) => (t === 'host-1' ? SEW : null),
    micPreflight: async () => 'granted',
    ...over,
  })
  ctl.recompute()
  return { ctl, api, calls, fire, toasts, plugin, openMicSettings, authFns, joins: () => calls.filter(([c]) => c === `${P}join`) }
}

const settle = async (): Promise<void> => {
  for (let i = 0; i < 20; i += 1) await Promise.resolve()
  await new Promise((r) => setTimeout(r, 0))
}

async function inCall() {
  const s = setup()
  s.plugin.state = { state: 'connected', participants: [], call_id: 'r_x' }
  await s.ctl.join(SEW, room1)
  s.plugin.call = { target: 'host-1', room: 'room-1' }
  await settle()
  return s
}

describe('native join', () => {
  it("calls the plugin's join, with the saved host's id and the slug, and never /cli/voice/join", async () => {
    const { ctl, api, joins } = await inCall()
    expect(joins()).toEqual([[`${P}join`, { target: 'host-1', room: 'room-1', muted: false }]])
    expect(api.join).not.toHaveBeenCalled()
    expect(ctl.store.getState().call).toMatchObject({ phase: 'connected', callId: 'r_x', hostKey: SEW, roomId: 'room-1', connectedAt: 5_000 })
    expect(ctl.store.getState().engineKind).toBe('native')
  })

  it('follows the engine: connecting, connected, reconnecting', async () => {
    const s = setup()
    s.plugin.state = { state: 'connecting', participants: [], call_id: 'r_x' }
    await s.ctl.join(SEW, room1)
    expect(s.ctl.store.getState().call?.phase).toBe('connecting')
    s.fire('state', { state: 'connected' })
    expect(s.ctl.store.getState().call).toMatchObject({ phase: 'connected', connectedAt: 5_000 })
    s.fire('state', { state: 'reconnecting', attempt: 2 })
    expect(s.ctl.store.getState().call?.phase).toBe('reconnecting')
    s.fire('state', { state: 'connected' })
    expect(s.ctl.store.getState().call?.phase).toBe('connected')
  })

  it("a window with no call of its own shows another window's, from the snapshot", async () => {
    const s = setup({ plugin: { state: { state: 'connected', participants: [person(ME, true, { muted: true })], call_id: 'r_x' }, call: { target: 'host-1', room: 'room-1' } } })
    s.ctl.start()()
    await settle()
    // start()'s stop ran; a fresh start attaches again.
    const stop = s.ctl.start()
    await settle()
    expect(s.ctl.store.getState().call).toMatchObject({ phase: 'connected', hostKey: SEW, roomId: 'room-1', callId: 'r_x', muted: true })
    stop()
  })

  it('this computer has no login to join with, so nothing is sent', async () => {
    const s = setup()
    await s.ctl.join('local', room1)
    expect(s.joins()).toEqual([])
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.ctl.store.getState().notice?.text).toContain('with your own login')
  })

  it('a server refusal gets its §4.6 words', async () => {
    const s = setup({ plugin: { joinError: { code: 'room_full', detail: { hint: 'full', details: null } } } })
    await s.ctl.join(SEW, room1)
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.ctl.store.getState().notice).toMatchObject({ text: 'Standup is full (25).', kind: 'error' })
  })

  it("the seat path's 'later K2 update' line never shows: an old Sew says so", async () => {
    const s = setup({ plugin: { joinError: { code: 'e2ee_unsupported_client', detail: { hint: '', details: null } } } })
    await s.ctl.join(SEW, room1)
    const text = s.ctl.store.getState().notice?.text ?? ''
    expect(text).toContain('sewlab is running an older K2')
    expect(text).not.toContain('later K2 update')
  })

  it('signin_required says sign in', async () => {
    const s = setup({ plugin: { joinError: { code: 'signin_required', detail: null } } })
    await s.ctl.join(SEW, room1)
    expect(s.ctl.store.getState().notice?.text).toBe('Sign in to sewlab again to join.')
  })

  it("another window's leave cancelled this join: the call goes, quietly", async () => {
    const s = setup({ plugin: { joinError: { code: 'left', detail: null } } })
    await s.ctl.join(SEW, room1)
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.ctl.store.getState().notice).toBeNull()
  })

  it('attaching again: a call the engine no longer has goes away', async () => {
    const s = setup()
    s.plugin.state = { state: 'connected', participants: [], call_id: 'r_x' }
    s.plugin.call = { target: 'host-1', room: 'room-1' }
    const stop = s.ctl.start()
    await settle()
    expect(s.ctl.store.getState().call?.callId).toBe('r_x')
    stop()
    s.plugin.state = { state: 'ended', reason: 'network', participants: [], call_id: null }
    s.plugin.call = null
    const stop2 = s.ctl.start()
    await settle()
    expect(s.ctl.store.getState().call).toBeNull()
    stop2()
  })

  it('another window joined first: show that call, no error', async () => {
    const s = setup({ plugin: { joinError: { code: 'already_in_call', detail: null }, state: { state: 'connected', participants: [], call_id: 'r_y' }, call: { target: 'host-1', room: 'room-2' } } })
    await s.ctl.join(SEW, room1)
    await settle()
    expect(s.ctl.store.getState().notice).toBeNull()
    expect(s.ctl.store.getState().call).toMatchObject({ callId: 'r_y', roomId: 'room-2' })
  })
})

describe('microphone', () => {
  it('mic_denied at preflight: no join, the §4.6 line, the privacy pane opens', async () => {
    const s = setup({ micPreflight: async () => 'denied' })
    await s.ctl.join(SEW, room1)
    expect(s.joins()).toEqual([])
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.ctl.store.getState().notice?.text).toContain("K2 can't use your microphone")
    expect(s.openMicSettings).toHaveBeenCalledTimes(1)
  })

  it('mic_denied from the engine at join: the join answer explains it once, not the events too', async () => {
    const s = setup({ plugin: { joinError: { code: 'mic_denied', detail: null } } })
    const joining = s.ctl.join(SEW, room1)
    await settle()
    s.fire('errors', { code: 'mic_denied', detail: null })
    s.fire('state', { state: 'ended', reason: 'mic_denied' })
    await joining
    expect(s.toasts.filter((t) => t.includes('microphone'))).toHaveLength(1)
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.openMicSettings).toHaveBeenCalledTimes(1)
  })

  it('mic_denied mid-call: says so and opens the pane, and the call stays', async () => {
    const s = await inCall()
    s.fire('errors', { code: 'mic_denied', detail: { audio: 'no_permission' } })
    expect(s.ctl.store.getState().notice).toMatchObject({ kind: 'error' })
    expect(s.ctl.store.getState().notice?.text).toContain("K2 can't use your microphone")
    expect(s.openMicSettings).toHaveBeenCalledTimes(1)
    expect(s.ctl.store.getState().call?.phase).toBe('connected')
  })

  it('device_lost mid-call: an info line, the call stays', async () => {
    const s = await inCall()
    s.fire('errors', { code: 'device_lost', detail: { audio: 'stream_lost' } })
    expect(s.ctl.store.getState().notice).toMatchObject({ text: 'Your audio device disconnected. K2 switched to your default device.', kind: 'info' })
    expect(s.ctl.store.getState().call?.phase).toBe('connected')
    expect(s.openMicSettings).not.toHaveBeenCalled()
  })

  it('a warning people can do nothing about stays in the log', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const s = await inCall()
    s.fire('errors', { code: 'key_storage_file', detail: null })
    expect(s.ctl.store.getState().notice).toBeNull()
    expect(warn).toHaveBeenCalledWith('[voice] engine says: key_storage_file')
    warn.mockRestore()
  })
})

// What a re-join looks like from here: the engine goes `reconnecting`, K2's
// provider is asked (src-tauri's tests hold that side for both RejoinReasons),
// and the call goes on or ends with the provider's outcome.
describe.each(['admit_lost', 'credential_expired'])('a re-join for %s', (reason) => {
  it('goes on: reconnecting, then connected, the same call', async () => {
    const s = await inCall()
    s.fire('state', { state: 'reconnecting', attempt: 1 })
    expect(s.ctl.store.getState().call?.phase).toBe('reconnecting')
    s.fire('state', { state: 'connected' })
    expect(s.ctl.store.getState().call).toMatchObject({ phase: 'connected', callId: 'r_x', roomId: 'room-1' })
    expect(s.joins(), reason).toHaveLength(1)
  })

  it.each([
    ['signin_required', 'You were signed out of sewlab.'],
    ['network', 'The call to Standup dropped: the connection was lost.'],
    ['credential_lost', 'The call to Standup ended: you were signed out, or joined from another device.'],
    ['room_closing', 'The call to Standup ended.'],
  ])('ends %s with its own line', async (ending, line) => {
    const s = await inCall()
    s.fire('state', { state: 'reconnecting', attempt: 1 })
    s.fire('state', { state: 'ended', reason: ending })
    expect(s.ctl.store.getState().call, reason).toBeNull()
    expect(s.ctl.store.getState().notice).toMatchObject({ text: line, kind: 'error' })
  })
})

describe('the call', () => {
  it('names people from the rooms list, marks speaking, and takes its own mute from the engine', async () => {
    const s = await inCall()
    await s.ctl.refreshRooms(SEW)
    s.ctl.store.setState({ servers: [{ kind: 'live', hostKey: SEW, label: 'sewlab', isWindow: false }] as never })
    await s.ctl.refreshRooms(SEW)
    s.fire('participants', [person(ME, true, { muted: true }), person('7', false), person('99', false, { server_muted: true })])
    await settle()
    expect(s.ctl.store.getState().call?.participants).toEqual([
      { user: 'ada', speaking: false, muted: true },
      { user: 'grace', speaking: false, muted: false },
      { user: 'Someone', speaking: false, muted: true },
    ])
    expect(s.ctl.store.getState().call?.muted).toBe(true)
    s.fire('speaking', [{ member_id: '7', level: 0.4 }, { member_id: ME, level: 0.01 }])
    expect(s.ctl.store.getState().call?.participants.map((p) => p.speaking)).toEqual([false, true, false])
  })

  it('mute and deafen go to the plugin, not the server; deafen undone keeps what the engine says', async () => {
    const s = await inCall()
    s.ctl.setMuted(true)
    s.ctl.setDeafened(true)
    expect(s.calls).toContainEqual([`${P}mute`, { muted: true }])
    expect(s.calls).toContainEqual([`${P}mute`, { deafened: true }])
    expect(s.ctl.store.getState().call).toMatchObject({ muted: true, deafened: true })
    s.ctl.setDeafened(false)
    s.fire('participants', [person(ME, true, { muted: false })])
    expect(s.ctl.store.getState().call).toMatchObject({ muted: false, deafened: false })
    expect(s.api.selfState).not.toHaveBeenCalled()
  })

  it('leave goes to the plugin and says so once', async () => {
    const s = await inCall()
    await s.ctl.leave()
    s.fire('state', { state: 'ended', reason: 'left' })
    expect(s.calls).toContainEqual([`${P}leave`, undefined])
    expect(s.api.leave).not.toHaveBeenCalled()
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.toasts).toEqual(['You left Standup.'])
  })

  it("signed out on the call's server: the plugin leaves", async () => {
    const s = await inCall()
    s.authFns[0](SEW, 'signin-required')
    await settle()
    expect(s.calls).toContainEqual([`${P}leave`, undefined])
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.ctl.store.getState().notice?.text).toBe('You were signed out of sewlab.')
  })

  it('Sew ending the call (a kick) ends it here, with its line', async () => {
    const s = await inCall()
    s.fire('state', { state: 'ended', reason: 'removed_by_moderator' })
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.ctl.store.getState().notice?.text).toBe('You were removed from Standup.')
  })

  it("the seat check doesn't run: an empty list doesn't end a native call", async () => {
    const s = await inCall()
    s.ctl.store.setState({ servers: [{ kind: 'live', hostKey: SEW, label: 'sewlab', isWindow: false }] as never })
    s.api.rooms.mockImplementation(async () => ({ ...list, rooms: [{ ...room1, occupants: [] }, room2] }))
    await s.ctl.refreshRooms(SEW)
    await s.ctl.refreshRooms(SEW)
    await s.ctl.refreshRooms(SEW)
    expect(s.ctl.store.getState().call?.phase).toBe('connected')
  })

  it('a moderator moved me: the list says where, the call follows', async () => {
    const s = await inCall()
    s.fire('participants', [person(ME, true)])
    s.ctl.store.setState({ servers: [{ kind: 'live', hostKey: SEW, label: 'sewlab', isWindow: false }] as never })
    s.api.rooms.mockImplementation(async () => ({ ...list, rooms: [{ ...room1, occupants: [] }, { ...room2, occupants: [occupant('ada', ME)] }] }))
    await s.ctl.refreshRooms(SEW)
    expect(s.ctl.store.getState().call).toMatchObject({ roomId: 'room-2', roomName: 'Room 2', phase: 'connected' })
  })
})

describe('devices', () => {
  it('lists through the plugin, hot-plug updates, picks go to set_device', async () => {
    const s = await inCall()
    await s.ctl.loadDevices()
    expect(s.ctl.store.getState().devices).toMatchObject({ outputSelectable: true, inputs: [{ id: 'in:1', label: 'Mic', isDefault: true }] })
    s.fire('devices', { inputs: [{ id: 'in:2', name: 'USB', default: true }], outputs: [], input: null, output: null })
    expect(s.ctl.store.getState().devices?.inputs[0].id).toBe('in:2')
    s.ctl.setPrefs({ inputDeviceId: 'in:2' })
    await settle()
    expect(s.calls).toContainEqual([`${P}set_device`, { input: 'in:2' }])
  })

  it('"System default" picks the device the OS calls default', async () => {
    const s = await inCall()
    s.ctl.setPrefs({ outputDeviceId: null })
    await settle()
    expect(s.calls).toContainEqual([`${P}set_device`, { output: 'out:1' }])
  })
})

describe('a build without the native engine', () => {
  it('stays on the seat path', async () => {
    const answer = { callId: 'c', roomId: 'room-1', roomName: 'Standup', signalUrl: 'wss://v', mediaToken: 't', e2ee: null, relayOnly: true }
    const join = vi.fn(async () => answer)
    const s = setup({ loadNative: async () => null, api: { rooms: async () => list, join, leave: async () => {}, selfState: async () => {} } as unknown as VoiceDeps['api'] })
    await s.ctl.join(SEW, room1)
    expect(join).toHaveBeenCalledTimes(1)
    expect(s.joins()).toEqual([])
    expect(s.ctl.store.getState().engineKind).toBe('seat')
  })
})

// Live access §11, against Sew's exact JSON.
describe('live access: how a call ends', () => {
  const lines: Record<string, string> = {
    perms_lost: 'You no longer have access to Standup.',
    removed_from_space: 'You were removed from Standup.',
    timed_out: 'You were timed out, so you left Standup.',
    channel_archived: 'Standup closed.',
    account_removed: 'Your account was removed from sewlab.',
    removed_by_moderator: 'You were removed from Standup.',
    session_revoked: 'You were signed out of sewlab.',
    device_revoked: 'This device was signed out of sewlab.',
    kicked: 'You were removed from Standup.',
    room_closed: 'Standup closed.',
    credential_lost: 'The call to Standup ended: you were signed out, or joined from another device.',
  }

  it("has a line for every reason in Sew's contract", () => {
    const named = [...sewContract.ended.sew_reasons, ...sewContract.ended.fallbacks].map((r: { reason: string }) => r.reason).sort()
    expect(named).toEqual(Object.keys(lines).sort())
    expect(sewContract.ended.event).toBe('sew-voice://state')
  })

  it.each([...sewContract.ended.sew_reasons, ...sewContract.ended.fallbacks].map((r: { reason: string; payload: unknown }) => [r.reason, r.payload]))('%s ends the call with its line', async (reason, payload) => {
    const s = await inCall()
    s.fire('state', payload)
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.ctl.store.getState().notice).toMatchObject({ text: lines[reason as string], kind: 'error' })
  })

  it('a reason nobody has heard of still ends the call, in plain words', async () => {
    const s = await inCall()
    s.fire('state', { state: 'ended', reason: 'some_new_reason' })
    expect(s.ctl.store.getState().call).toBeNull()
    expect(s.ctl.store.getState().notice?.text).toBe('The call to Standup ended.')
  })
})

describe('live access: the mic', () => {
  const p = sewContract.participants

  it('speaking taken away: the call stays, the mic is blocked, it says so once', async () => {
    const s = await inCall()
    s.fire('participants', p.joined)
    expect(s.ctl.store.getState().call).toMatchObject({ speakDenied: false, muted: false })
    s.fire('participants', p.self_speak_denied)
    s.fire('participants', p.self_speak_denied)
    expect(s.ctl.store.getState().call).toMatchObject({ phase: 'connected', speakDenied: true, serverMuted: false, muted: true })
    expect(s.toasts.filter((t) => t === "You can't speak in Standup. You can still listen.")).toHaveLength(1)
    s.ctl.setMuted(false)
    expect(s.calls.filter(([c]) => c === `${P}mute`)).toEqual([])
    expect(s.ctl.store.getState().call?.muted).toBe(true)
  })

  it('speaking back: the same call, one line, no re-join', async () => {
    const s = await inCall()
    s.fire('participants', p.self_speak_denied)
    s.fire('participants', p.self_speak_back_unmuted)
    expect(s.ctl.store.getState().call).toMatchObject({ speakDenied: false, muted: false, callId: 'r_x' })
    expect(s.ctl.store.getState().notice).toMatchObject({ text: 'You can speak in Standup again.', kind: 'info' })
    expect(s.joins()).toHaveLength(1)
  })

  it('speaking back honours a self-mute', async () => {
    const s = await inCall()
    s.fire('participants', p.self_speak_denied)
    s.fire('participants', p.self_speak_back_still_self_muted)
    expect(s.ctl.store.getState().call).toMatchObject({ speakDenied: false, muted: true })
  })

  it('a moderator mute is its own line, not speak_denied', async () => {
    const s = await inCall()
    s.fire('participants', p.self_server_muted)
    expect(s.ctl.store.getState().call).toMatchObject({ speakDenied: false, serverMuted: true, muted: true })
    expect(s.ctl.store.getState().notice?.text).toBe('A moderator muted you in Standup.')
    s.ctl.setMuted(false)
    expect(s.calls.filter(([c]) => c === `${P}mute`)).toEqual([])
  })

  it('someone else who lost speaking shows muted, and says nothing to me', async () => {
    const s = await inCall()
    s.fire('participants', p.remote_speak_denied)
    const call = s.ctl.store.getState().call
    expect(call?.participants.map((x) => x.muted)).toEqual([false, true])
    expect(call).toMatchObject({ speakDenied: false, muted: false })
    expect(s.ctl.store.getState().notice).toBeNull()
  })

  it('a window opened mid-call reads it from the snapshot, with no line', async () => {
    const s = setup({ plugin: { state: sewContract.state_snapshot.speak_denied, call: { target: 'host-1', room: 'room-1' } } })
    const stop = s.ctl.start()
    await settle()
    expect(s.ctl.store.getState().call).toMatchObject({ speakDenied: true, muted: true, phase: 'connected' })
    expect(s.toasts).toEqual([])
    stop()
  })
})
