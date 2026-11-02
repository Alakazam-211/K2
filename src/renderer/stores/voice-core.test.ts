import { describe, expect, it, vi } from 'vitest'
import type { HostEntry, PoolAuth } from '@/lib/host-pool'
import type { ServerScope } from '@/kessel/server-scope'
import { FakeVoiceEngine, type FakeEngineScript } from '@/lib/voice/fake-voice-engine'
import { VoiceRequestError } from '@/lib/voice/voice-api'
import type { VoiceJoinAnswer, VoiceRoom, VoiceRoomsList } from '@/lib/voice/voice-wire'
import { createVoiceController, type VoiceDeps } from './voice-core'

const SEW = 'sewlab.k2.dev'
const OTHER = 'other.k2.dev'

function entry(hostKey: string, features: string[] = ['voice-rooms-v1']): HostEntry {
  return {
    hostKey,
    saved: true,
    hostId: hostKey,
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.46.8', protocol: 1, instanceId: 'i', features, view: 'full', at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: 1,
  }
}

const room1: VoiceRoom = { id: 'room-1', name: 'Standup', limit: 25, occupants: [], closing: false, closesAt: null, unavailable: null }
const room2: VoiceRoom = { id: 'room-2', name: 'Room 2', limit: 25, occupants: [], closing: false, closesAt: null, unavailable: null }
const list: VoiceRoomsList = { hosting: { state: 'ok', line: null, detail: null }, rooms: [room1, room2], rev: 1, canManage: false, feedLive: true }

function answer(callId: string, relayOnly = true): VoiceJoinAnswer {
  return { callId, roomId: 'room-1', roomName: 'Standup', signalUrl: 'wss://voice.sewlab.k2.dev', mediaToken: 'tok', e2ee: null, relayOnly }
}

function setup(over: { script?: FakeEngineScript; join?: VoiceDeps['api']['join'] } = {}) {
  const authFns: Array<(k: string, a: PoolAuth) => void> = []
  const clock = { now: 5_000 }
  const windowEvent: Array<(rev: number | null) => void> = []
  const serverEvent = new Map<string, (rev: number | null) => void>()
  const engines: FakeVoiceEngine[] = []
  const toasts: string[] = []
  const scopes = new Map<string, ServerScope>()
  const scopeFor = (k: string): ServerScope => {
    let s = scopes.get(k)
    if (!s) {
      s = { hostKey: k, id: k, label: k } as unknown as ServerScope
      scopes.set(k, s)
    }
    return s
  }
  const entries: Record<string, HostEntry> = { [SEW]: entry(SEW), [OTHER]: entry(OTHER) }
  const check = vi.fn(async (_key: string, _opts?: { bootOnly?: boolean }) => undefined)
  const api = {
    rooms: vi.fn(async (_s: ServerScope): Promise<VoiceRoomsList> => list),
    join: vi.fn(over.join ?? (async (_s: ServerScope, roomId: string) => ({ ...answer('call-1'), roomId }))),
    leave: vi.fn(async () => {}),
    selfState: vi.fn(async () => {}),
  }
  const ctl = createVoiceController({
    keys: () => ['local', SEW, OTHER],
    windowKey: () => 'local',
    labelOf: (k) => (k === SEW ? 'sewlab' : k),
    entries: () => entries,
    check,
    onPoolChange: () => () => {},
    onAuth: (fn) => {
      authFns.push(fn)
      return () => {}
    },
    onWindowRoomsChanged: (fn) => {
      windowEvent.push(fn)
      return () => {
        windowEvent.splice(windowEvent.indexOf(fn), 1)
      }
    },
    onRoomsChanged: (key, fn) => {
      serverEvent.set(key, fn)
      return () => {
        serverEvent.delete(key)
      }
    },
    selfNames: () => ['ada'],
    scopeFor,
    api,
    makeEngine: () => {
      const e = new FakeVoiceEngine(over.script ?? {})
      engines.push(e)
      return e
    },
    toast: (t) => toasts.push(t),
    loadReported: () => new Set(),
    saveReported: () => {},
    loadPrefs: () => ({ inputDeviceId: null, outputDeviceId: null, joinMuted: false }),
    savePrefs: () => {},
    now: () => clock.now,
  })
  ctl.recompute()
  return { ctl, api, engines, toasts, authFns, scopeFor, clock, entries, windowEvent, serverEvent, check }
}

describe('voice controller: join and leave', () => {
  it("joins with that server's own scope, connects relay-only, and leaves", async () => {
    const { ctl, api, engines, scopeFor } = setup()
    await ctl.join(SEW, room1)
    expect(api.join).toHaveBeenCalledWith(scopeFor(SEW), 'room-1', expect.objectContaining({ selfMute: false }))
    const call = ctl.store.getState().call
    expect(call?.phase).toBe('connected')
    expect(call?.callId).toBe('call-1')
    expect(call?.relayOnly).toBe(true)
    expect(call?.connectedAt).toBe(5_000)
    expect(engines[0].relayOnly).toBe(true)

    await ctl.leave()
    expect(ctl.store.getState().call).toBeNull()
    expect(engines[0].calls).toContain('disconnect')
    expect(api.leave).toHaveBeenCalledWith(scopeFor(SEW), 'call-1')
    expect(ctl.store.getState().notice?.text).toBe('You left Standup.')
  })

  it('a refused join shows the §4.6 line and leaves no call', async () => {
    const { ctl, engines } = setup({
      join: async () => {
        throw new VoiceRequestError('room_full', 'full')
      },
    })
    await ctl.join(SEW, room1)
    expect(ctl.store.getState().call).toBeNull()
    expect(ctl.store.getState().notice).toMatchObject({ text: 'Standup is full (25).', kind: 'error' })
    expect(engines).toHaveLength(0)
  })

  it('media_path_down (lane down ≥ 20 s) refuses with its own line', async () => {
    const { ctl } = setup({
      join: async () => {
        throw new VoiceRequestError('media_path_down', 'down')
      },
    })
    await ctl.join(SEW, room1)
    expect(ctl.store.getState().notice?.text).toBe('Voice on sewlab is unavailable (its network lane is down).')
  })

  it('a held join that times out says so (V16)', async () => {
    const { ctl } = setup({
      join: async () => {
        throw new VoiceRequestError('join_timeout', 'late')
      },
    })
    await ctl.join(SEW, room1)
    expect(ctl.store.getState().notice?.text).toBe("sewlab didn't answer in time. Try again in a moment.")
  })

  it('an engine refusal (mic denied) leaves the server seat and explains', async () => {
    const { ctl, api, scopeFor } = setup({ script: { failWith: 'mic_denied' } })
    await ctl.join(SEW, room1)
    expect(ctl.store.getState().call).toBeNull()
    expect(api.leave).toHaveBeenCalledWith(scopeFor(SEW), 'call-1')
    expect(ctl.store.getState().notice?.text).toContain("K2 can't use your microphone")
  })

  it("Sew refusing a client without E2EE says audio comes later, and takes no seat", async () => {
    const { ctl, engines } = setup({
      join: async () => {
        throw new VoiceRequestError('e2ee_unsupported', 'hint')
      },
    })
    await ctl.join(SEW, room1)
    expect(ctl.store.getState().call).toBeNull()
    expect(ctl.store.getState().notice?.text).toContain('Call audio arrives in a later K2 update')
    expect(engines).toHaveLength(0)
  })

  it("a refusal the client has no words for shows the daemon's hint", async () => {
    const { ctl } = setup({
      join: async () => {
        throw new VoiceRequestError('unknown', "no room 'room-9' on this server")
      },
    })
    await ctl.join(SEW, room1)
    expect(ctl.store.getState().notice?.text).toBe("Couldn't join Standup on sewlab: no room 'room-9' on this server")
  })

  it('joining while in a call leaves the current call first', async () => {
    let n = 0
    const { ctl, api, engines, scopeFor } = setup({ join: async (_s, roomId) => ({ ...answer(`call-${++n}`), roomId, roomName: roomId }) })
    await ctl.join(SEW, room1)
    await ctl.join(OTHER, room2)
    expect(api.leave).toHaveBeenCalledWith(scopeFor(SEW), 'call-1')
    expect(engines[0].calls).toContain('disconnect')
    expect(ctl.store.getState().call).toMatchObject({ hostKey: OTHER, callId: 'call-2', phase: 'connected' })
  })
})

describe('voice controller: during the call', () => {
  it('reconnecting, then back; an engine end with media_path_down ends the call with its reason', async () => {
    const { ctl, engines, toasts } = setup()
    await ctl.join(SEW, room1)
    engines[0].simulate('reconnecting')
    expect(ctl.store.getState().call?.phase).toBe('reconnecting')
    engines[0].simulate('connected')
    expect(ctl.store.getState().call?.phase).toBe('connected')
    engines[0].simulate('ended', 'media_path_down')
    await vi.waitFor(() => expect(ctl.store.getState().call).toBeNull())
    expect(toasts.at(-1)).toBe('The call ended: voice on sewlab is unavailable (its network lane is down).')
  })

  it("signed out of the call's server ends the call (§4.4); another server's sign-out does not", async () => {
    const { ctl, authFns, api } = setup()
    await ctl.join(SEW, room1)
    for (const fn of authFns) fn(OTHER, 'signin-required')
    expect(ctl.store.getState().call).not.toBeNull()
    for (const fn of authFns) fn(SEW, 'kicked')
    await vi.waitFor(() => expect(ctl.store.getState().call).toBeNull())
    expect(ctl.store.getState().notice?.text).toBe('You were signed out of sewlab.')
    // The server already ended the Sew session: no leave is sent.
    expect(api.leave).not.toHaveBeenCalled()
  })

  it('mute and deafen reach the engine and the server', async () => {
    const { ctl, engines, api, scopeFor } = setup()
    await ctl.join(SEW, room1)
    ctl.setMuted(true)
    expect(engines[0].muted).toBe(true)
    expect(api.selfState).toHaveBeenLastCalledWith(scopeFor(SEW), 'call-1', true, false)
    ctl.setMuted(false)
    ctl.setDeafened(true)
    expect(ctl.store.getState().call).toMatchObject({ deafened: true, muted: true })
    expect(api.selfState).toHaveBeenLastCalledWith(scopeFor(SEW), 'call-1', true, true)
  })

  it('a join answered after a leave is left at once, never kept', async () => {
    let release: (a: VoiceJoinAnswer) => void = () => {}
    const { ctl, api, scopeFor } = setup({
      join: () =>
        new Promise<VoiceJoinAnswer>((r) => {
          release = r
        }),
    })
    const p = ctl.join(SEW, room1)
    expect(ctl.store.getState().call?.phase).toBe('joining')
    await ctl.leave()
    release(answer('late-call'))
    await p
    expect(ctl.store.getState().call).toBeNull()
    expect(api.leave).toHaveBeenCalledWith(scopeFor(SEW), 'late-call')
  })
})

describe('voice controller: the seat check (no audio, so the list is the only signal)', () => {
  const withAda = (feedLive = true): VoiceRoomsList => ({
    ...list,
    feedLive,
    rooms: [{ ...room1, occupants: [{ user: 'Ada', name: null, role: 'admin', color: null, muted: false, deafened: false, speaking: false }] }, room2],
  })

  it('ends the call after two lists without this login, past the grace', async () => {
    const { ctl, api, clock, scopeFor } = setup()
    await ctl.join(SEW, room1)
    await vi.waitFor(() => expect(api.rooms).toHaveBeenCalled())
    await ctl.refreshRooms(SEW) // inside the grace: the feed may lag
    await ctl.refreshRooms(SEW)
    expect(ctl.store.getState().call).not.toBeNull()
    clock.now += 11_000
    await ctl.refreshRooms(SEW)
    expect(ctl.store.getState().call).not.toBeNull()
    await ctl.refreshRooms(SEW)
    expect(ctl.store.getState().call).toBeNull()
    expect(ctl.store.getState().notice?.text).toContain('sewlab dropped you from Standup')
    expect(api.leave).toHaveBeenCalledWith(scopeFor(SEW), 'call-1')
  })

  it('a list that shows this login resets it (names match in any case)', async () => {
    const { ctl, api, clock } = setup()
    await ctl.join(SEW, room1)
    clock.now += 11_000
    await ctl.refreshRooms(SEW)
    api.rooms.mockResolvedValueOnce(withAda())
    await ctl.refreshRooms(SEW)
    await ctl.refreshRooms(SEW)
    expect(ctl.store.getState().call).not.toBeNull()
  })

  it('a feed that is not live proves nothing: everyone looks gone', async () => {
    const { ctl, api, clock } = setup()
    api.rooms.mockResolvedValue({ ...list, feedLive: false })
    await ctl.join(SEW, room1)
    clock.now += 11_000
    await ctl.refreshRooms(SEW)
    await ctl.refreshRooms(SEW)
    await ctl.refreshRooms(SEW)
    expect(ctl.store.getState().call).not.toBeNull()
  })
})

describe('voice controller: voice_rooms_changed', () => {
  async function settled(ctl: ReturnType<typeof setup>['ctl']): Promise<void> {
    await ctl.refreshRooms()
    await vi.waitFor(() => expect(ctl.store.getState().rooms[SEW]?.loading).toBe(false))
  }

  it('the event refetches that server only, unless the list has that rev', async () => {
    const { ctl, api, scopeFor } = setup()
    await settled(ctl)
    api.rooms.mockClear()
    ctl.noteRoomsChanged(SEW, 1) // the list already has rev 1
    expect(api.rooms).not.toHaveBeenCalled()
    ctl.noteRoomsChanged(SEW, 2)
    await vi.waitFor(() => expect(api.rooms).toHaveBeenCalledTimes(1))
    expect(api.rooms).toHaveBeenCalledWith(scopeFor(SEW))
  })

  it('a lower rev still refetches: rev starts over when the daemon restarts', async () => {
    const { ctl, api } = setup()
    api.rooms.mockResolvedValue({ ...list, rev: 40 })
    await settled(ctl)
    api.rooms.mockClear()
    ctl.noteRoomsChanged(SEW, 1)
    await vi.waitFor(() => expect(api.rooms).toHaveBeenCalledTimes(1))
  })

  it('a burst during a read makes one more read, not one per event', async () => {
    const { ctl, api } = setup()
    await settled(ctl)
    api.rooms.mockClear()
    let release: (l: VoiceRoomsList) => void = () => {}
    api.rooms.mockImplementationOnce(() => new Promise<VoiceRoomsList>((r) => (release = r)))
    ctl.noteRoomsChanged(SEW, 2)
    ctl.noteRoomsChanged(SEW, 3)
    ctl.noteRoomsChanged(SEW, 4)
    expect(api.rooms).toHaveBeenCalledTimes(1)
    release({ ...list, rev: 2 })
    await vi.waitFor(() => expect(api.rooms).toHaveBeenCalledTimes(2))
  })

  it('from a server not listed yet: re-reads its boot status, not its rooms', async () => {
    const { ctl, api, check, entries } = setup()
    entries[SEW] = entry(SEW, [])
    ctl.recompute()
    api.rooms.mockClear()
    ctl.noteRoomsChanged(SEW, 2)
    expect(check).toHaveBeenCalledWith(SEW, { bootOnly: true })
    expect(api.rooms).not.toHaveBeenCalled()
  })

  it('subscribes to the window and each live server while started, and lets go', async () => {
    vi.useFakeTimers()
    try {
      const { ctl, api, entries, windowEvent, serverEvent } = setup()
      const stop = ctl.start()
      await vi.advanceTimersByTimeAsync(0)
      expect(windowEvent).toHaveLength(1)
      expect([...serverEvent.keys()].sort()).toEqual([OTHER, SEW].sort())
      api.rooms.mockClear()
      serverEvent.get(OTHER)?.(9)
      await vi.advanceTimersByTimeAsync(0)
      expect(api.rooms).toHaveBeenCalledTimes(1)
      // A server that stops hosting drops its subscription.
      entries[OTHER] = entry(OTHER, [])
      ctl.recompute()
      expect([...serverEvent.keys()]).toEqual([SEW])
      stop()
      expect(windowEvent).toHaveLength(0)
      expect(serverEvent.size).toBe(0)
    } finally {
      vi.useRealTimers()
    }
  })

  it('the sweep reads the lists too, so the headset knows who is in a room', async () => {
    const { ctl, api, check, scopeFor } = setup()
    await ctl.discover()
    expect(check).toHaveBeenCalledWith(SEW, { bootOnly: true })
    expect(api.rooms).toHaveBeenCalledWith(scopeFor(SEW))
    expect(ctl.store.getState().rooms[SEW].list?.rooms).toHaveLength(2)
  })
})

describe('voice controller: the list', () => {
  it('reads every live server with its own scope', async () => {
    const { ctl, api, scopeFor } = setup()
    await ctl.refreshRooms()
    expect(api.rooms).toHaveBeenCalledWith(scopeFor(SEW))
    expect(api.rooms).toHaveBeenCalledWith(scopeFor(OTHER))
    expect(ctl.store.getState().rooms[SEW].list?.rooms).toHaveLength(2)
  })

  it('a failing server keeps its error and never hides the others', async () => {
    const { ctl, api } = setup()
    api.rooms.mockImplementation(async (s: ServerScope) => {
      if (s.hostKey === OTHER) throw new Error('offline')
      return list
    })
    await ctl.refreshRooms()
    expect(ctl.store.getState().rooms[OTHER]).toMatchObject({ error: 'offline', list: null })
    expect(ctl.store.getState().rooms[SEW].list).not.toBeNull()
  })
})
