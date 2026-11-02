import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  EVENT_PREFIX,
  NativeVoiceEngine,
  NativeVoiceError,
  PLUGIN_COMMAND_PREFIX,
  parseCallState,
  parseDevices,
  parseEngineError,
  parseParticipants,
  parseSpeaking,
  type NativeEvents,
  type NativeVoiceBridge,
} from './native-voice-engine'

const contract = JSON.parse(readFileSync(resolve(__dirname, 'native-voice-contract.json'), 'utf8'))

/** A plugin as the webview sees it: commands answer from `answers` or reject
 *  with what `rejects` holds; `fire` plays an event. */
function fakeBridge(answers: Record<string, unknown> = {}, rejects: Record<string, unknown> = {}) {
  const calls: Array<{ command: string; args: Record<string, unknown> | undefined }> = []
  const listeners = new Map<string, Array<(p: unknown) => void>>()
  const bridge: NativeVoiceBridge = {
    invoke: async <T,>(command: string, args?: Record<string, unknown>): Promise<T> => {
      calls.push({ command, args })
      if (command in rejects) throw rejects[command]
      return answers[command] as T
    },
    listen: async (event, handler) => {
      listeners.set(event, [...(listeners.get(event) ?? []), handler])
      return () => {
        listeners.set(event, (listeners.get(event) ?? []).filter((h) => h !== handler))
      }
    },
  }
  return {
    bridge,
    calls,
    listeners,
    fire: (event: string, payload: unknown) => {
      for (const h of listeners.get(event) ?? []) h(payload)
    },
  }
}

function events(): NativeEvents & { log: unknown[] } {
  const log: unknown[] = []
  return {
    log,
    onState: (s) => log.push(['state', s]),
    onParticipants: (p) => log.push(['participants', p]),
    onSpeaking: (l) => log.push(['speaking', l]),
    onDevices: (d) => log.push(['devices', d]),
    onError: (c, d) => log.push(['error', c, d]),
  }
}

describe("Sew's contract (native-voice-contract.json, held to the plugin's types in Rust)", () => {
  it('uses the names the plugin answers to', () => {
    expect(PLUGIN_COMMAND_PREFIX).toBe(`plugin:${contract.plugin}|`)
    expect(EVENT_PREFIX).toBe(contract.eventPrefix)
  })

  it('reads every state', () => {
    expect(parseCallState(contract.states.idle)).toEqual({ state: 'idle' })
    expect(parseCallState(contract.states.connecting)).toEqual({ state: 'connecting' })
    expect(parseCallState(contract.states.connected)).toEqual({ state: 'connected' })
    expect(parseCallState(contract.states.reconnecting)).toEqual({ state: 'reconnecting', attempt: 3 })
    expect(parseCallState(contract.states.ended)).toEqual({ state: 'ended', reason: 'channel_archived' })
  })

  it('reads participants (no names), speaking, devices and errors', () => {
    expect(parseParticipants(contract.participants)[0]).toEqual({
      memberId: '48831765147549699',
      isSelf: true,
      muted: true,
      deafened: false,
      serverMuted: false,
      serverDeafened: true,
    })
    expect(parseSpeaking(contract.speaking)).toEqual([{ memberId: '7', level: 0.25 }])
    const d = parseDevices(contract.devices)
    expect(d.inputs).toEqual([{ id: 'in:1', label: 'Mic', isDefault: true }])
    expect(d.outputs.map((o) => o.id)).toEqual(['out:1', 'out:2'])
    expect(d.outputSelectable).toBe(true)
    expect(parseEngineError(contract.errors.mic_denied)).toEqual({ code: 'mic_denied', detail: null })
    expect(parseEngineError(contract.errors.device_lost)).toEqual({ code: 'device_lost', detail: { audio: 'stream_lost' } })
  })

  it('sends exactly the argument shapes, with no token', async () => {
    const f = fakeBridge({ [`${PLUGIN_COMMAND_PREFIX}join`]: contract.joinResult })
    const e = new NativeVoiceEngine(f.bridge)
    expect(await e.join({ target: 'host-1', room: 'room-1', muted: true })).toEqual({ callId: 'r_x' })
    await e.mute({ muted: true })
    await e.setDevice({ output: 'out:2' })
    await e.leave()
    expect(f.calls).toEqual([
      { command: 'plugin:sew-voice|join', args: { target: 'host-1', room: 'room-1', muted: true } },
      { command: 'plugin:sew-voice|mute', args: contract.muteArgs },
      { command: 'plugin:sew-voice|set_device', args: contract.setDeviceArgs },
      { command: 'plugin:sew-voice|leave', args: undefined },
    ])
    const { deafened: _d, ...sent } = contract.joinArgs
    expect(f.calls[0].args).toEqual(sent)
  })

  it('a window opened mid-call reads the snapshot and K2 room record', async () => {
    const f = fakeBridge({
      [`${PLUGIN_COMMAND_PREFIX}state`]: contract.snapshots.live,
      voice_native_status: { available: true, call: { target: 'host-1', room: 'room-1' } },
    })
    expect(await new NativeVoiceEngine(f.bridge).snapshot()).toEqual({
      state: { state: 'reconnecting', attempt: 1 },
      participants: [],
      callId: 'r_x',
      target: 'host-1',
      room: 'room-1',
    })
    const idle = fakeBridge({ [`${PLUGIN_COMMAND_PREFIX}state`]: contract.snapshots.idle, voice_native_status: { available: true, call: null } })
    expect(await new NativeVoiceEngine(idle.bridge).snapshot()).toMatchObject({ state: { state: 'idle' }, callId: null, target: null, room: null })
  })
})

describe('events', () => {
  it('listens to the five events and stops on stop()', async () => {
    const f = fakeBridge()
    const e = new NativeVoiceEngine(f.bridge)
    const ev = events()
    await e.start(ev)
    expect([...f.listeners.keys()].sort()).toEqual(contract.events.map((n: string) => `${contract.eventPrefix}${n}`).sort())
    f.fire('sew-voice://state', contract.states.connected)
    f.fire('sew-voice://participants', contract.participants)
    f.fire('sew-voice://speaking', contract.speaking)
    f.fire('sew-voice://devices', contract.devices)
    f.fire('sew-voice://errors', contract.errors.device_lost)
    expect(ev.log.map((x) => (x as unknown[])[0])).toEqual(['state', 'participants', 'speaking', 'devices', 'error'])
    e.stop()
    f.fire('sew-voice://state', contract.states.idle)
    expect(ev.log).toHaveLength(5)
  })

  it('a payload outside the contract throws, it is never guessed at', () => {
    expect(() => parseCallState({ state: 'ringing' })).toThrow(/unexpected state/)
    expect(() => parseCallState({ state: 'ended' })).toThrow(/unexpected state/)
    expect(() => parseCallState({ state: 'reconnecting' })).toThrow(/unexpected state/)
    expect(() => parseParticipants({})).toThrow(/participants/)
    expect(() => parseParticipants([{ is_self: true }])).toThrow(/participant/)
    expect(() => parseSpeaking([{ member_id: '1' }])).toThrow(/speaking/)
    expect(() => parseDevices({ inputs: [] })).toThrow(/device list/)
    expect(() => parseEngineError({ detail: 1 })).toThrow(/error/)
  })
})

describe('rejections', () => {
  it("a command's {code, detail} becomes a NativeVoiceError", async () => {
    const f = fakeBridge({}, { [`${PLUGIN_COMMAND_PREFIX}join`]: contract.errors.refused })
    const err = await new NativeVoiceEngine(f.bridge).join({ target: 't', room: 'r', muted: false }).catch((e: unknown) => e)
    expect(err).toBeInstanceOf(NativeVoiceError)
    expect(err).toMatchObject({ code: 'voice_channel_full', detail: { limit: 25 } })
  })

  it("Tauri's own refusal (a string: the command never ran) is `failed` with its text", async () => {
    const f = fakeBridge({}, { [`${PLUGIN_COMMAND_PREFIX}leave`]: 'sew-voice.leave not allowed' })
    const err = await new NativeVoiceEngine(f.bridge).leave().catch((e: unknown) => e)
    expect(err).toMatchObject({ code: 'failed', detail: 'sew-voice.leave not allowed' })
  })

  it('a join answer with no call id fails loudly', async () => {
    const f = fakeBridge({ [`${PLUGIN_COMMAND_PREFIX}join`]: {} })
    await expect(new NativeVoiceEngine(f.bridge).join({ target: 't', room: 'r', muted: false })).rejects.toThrow(/join answer/)
  })
})
