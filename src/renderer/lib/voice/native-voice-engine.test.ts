import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'
import {
  EVENT_PREFIX,
  NativeVoiceEngine,
  NativeVoiceError,
  PLUGIN_COMMAND_PREFIX,
  RECORDING_CONTROLS_EVENT,
  parseCallState,
  parseDevices,
  parseEngineError,
  parseParticipants,
  parseRecording,
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
    onRecording: (r) => log.push(['recording', r]),
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
    expect(parseCallState(contract.states.consent_required)).toEqual({ state: 'consent_required', recordingId: '77', noticeVersion: 3141 })
  })

  it('reads the recording event, and null once none is active', () => {
    expect(parseRecording(contract.recording.pending)).toEqual({
      recordingId: '77',
      myConsent: 'pending',
      recorderPresent: true,
      trigger: 'manual',
      required: false,
      startedBy: null,
      prompt: null,
    })
    expect(parseRecording(contract.recording.granted)).toMatchObject({ recordingId: '77', myConsent: 'granted', recorderPresent: true, startedBy: '7' })
    // Plugin 0.6.1 writes can_record / can_stop only when true.
    expect(parseRecording(contract.recording.granted)).not.toHaveProperty('canRecord')
    expect(parseRecording({ ...contract.recording.granted, can_record: false, can_stop: true })).toMatchObject({ canRecord: false, canStop: true })
    expect(parseRecording(contract.recording.stopped)).toBeNull()
  })

  it('reads participants (no names), speaking, devices and errors', () => {
    expect(parseParticipants(contract.participants)[0]).toEqual({
      memberId: '48831765147549699',
      isSelf: true,
      muted: true,
      deafened: false,
      serverMuted: false,
      serverDeafened: true,
      speakDenied: false,
    })
    // Sew writes it only when true: missing reads as false.
    const sew = JSON.parse(readFileSync(resolve(__dirname, 'sew-native-voice-contract.json'), 'utf8'))
    expect(parseParticipants(sew.participants.joined)[0].speakDenied).toBe(false)
    expect(parseParticipants(sew.participants.self_speak_denied)[0]).toMatchObject({ speakDenied: true, muted: true, serverMuted: false })
    expect(parseParticipants(sew.participants.self_server_muted)[0]).toMatchObject({ speakDenied: false, serverMuted: true })
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
      [`${PLUGIN_COMMAND_PREFIX}state`]: { ...contract.snapshots.live, target: 'host-1', room: 'room-1' },
    })
    expect(await new NativeVoiceEngine(f.bridge).snapshot()).toEqual({
      state: { state: 'reconnecting', attempt: 1, target: 'host-1', room: 'room-1' },
      participants: [],
      callId: 'r_x',
      target: 'host-1',
      room: 'room-1',
    })
    const idle = fakeBridge({ [`${PLUGIN_COMMAND_PREFIX}state`]: contract.snapshots.idle })
    expect(await new NativeVoiceEngine(idle.bridge).snapshot()).toMatchObject({ state: { state: 'idle' }, callId: null, target: null, room: null })
  })
})

describe("Sew's 0.7.2 additions", () => {
  const sew = JSON.parse(readFileSync(resolve(__dirname, 'sew-native-voice-contract.json'), 'utf8'))

  it("reads Sew 0.7.3's held join and its recording payloads", () => {
    expect(parseCallState(sew.recording.held.state)).toEqual({
      state: 'consent_required',
      recordingId: '48831765147549720',
      noticeVersion: 3141,
      target: 'k2:acme',
      room: '48831765147549698',
    })
    expect(parseCallState(sew.state_snapshot.consent_required)).toMatchObject({ state: 'consent_required', noticeVersion: 3141 })
    expect(parseRecording(sew.recording.held.recording)).toEqual({
      recordingId: '48831765147549720',
      myConsent: 'pending',
      recorderPresent: false,
      trigger: 'manual',
      required: false,
      startedBy: '1001',
      prompt: { noticeVersion: 3141, startedByName: 'Alice', audioKeepDays: 30, required: false, appReaders: [], agentReaders: false, declineMode: 'join_unrecorded' },
    })
    expect(parseRecording(sew.recording.granted)?.myConsent).toBe('granted')
    expect(parseRecording(sew.recording.withdrawn)?.myConsent).toBe('withdrawn')
    expect(parseRecording(sew.recording.stopped)).toBeNull()
  })

  it('reads target and room on a state, and a move', () => {
    expect(parseCallState(sew.moved.payload)).toEqual({ state: 'connected', target: 'k2:acme', room: '48831765147549710' })
    expect(parseCallState(sew.ended.sew_reasons[0].payload)).toEqual({ state: 'ended', reason: 'perms_lost', target: 'k2:acme', room: '48831765147549698' })
  })
})

describe('events', () => {
  it('listens to every event and stops on stop()', async () => {
    const f = fakeBridge()
    const e = new NativeVoiceEngine(f.bridge)
    const ev = events()
    await e.start(ev)
    // Plus plugin 0.6.1's recording_controls (Sew's contract: recording.controls.rights).
    expect([...f.listeners.keys()].sort()).toEqual([...contract.events, 'recording_controls'].map((n: string) => `${contract.eventPrefix}${n}`).sort())
    f.fire('sew-voice://state', contract.states.connected)
    f.fire('sew-voice://participants', contract.participants)
    f.fire('sew-voice://speaking', contract.speaking)
    f.fire('sew-voice://devices', contract.devices)
    f.fire('sew-voice://errors', contract.errors.device_lost)
    f.fire('sew-voice://recording', contract.recording.granted)
    expect(ev.log.map((x) => (x as unknown[])[0])).toEqual(['state', 'participants', 'speaking', 'devices', 'error', 'recording'])
    e.stop()
    f.fire('sew-voice://state', contract.states.idle)
    expect(ev.log).toHaveLength(6)
  })

  it('a payload outside the contract throws, it is never guessed at', () => {
    expect(() => parseCallState({ state: 'ringing' })).toThrow(/unexpected state/)
    expect(() => parseCallState({ state: 'ended' })).toThrow(/unexpected state/)
    expect(() => parseCallState({ state: 'reconnecting' })).toThrow(/unexpected state/)
    expect(() => parseCallState({ state: 'consent_required', recording_id: '77' })).toThrow(/unexpected state/)
    expect(() => parseRecording({ recording_id: '77' })).toThrow(/recording/)
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

  it("sends the person's answer and Record/Stop as the plugin's commands (Sew 0.9.0)", async () => {
    const f = fakeBridge()
    const e = new NativeVoiceEngine(f.bridge)
    await e.consent(true, 3141)
    await e.startRecording()
    await e.stopRecording()
    const sew = JSON.parse(readFileSync(resolve(__dirname, 'sew-native-voice-contract.json'), 'utf8'))
    expect(f.calls).toEqual([
      { command: `${PLUGIN_COMMAND_PREFIX}consent`, args: sew.recording.held.consent.args },
      { command: `${PLUGIN_COMMAND_PREFIX}start_recording`, args: undefined },
      { command: `${PLUGIN_COMMAND_PREFIX}stop_recording`, args: undefined },
    ])
    expect(contract.recordingCommands).toEqual(['start_recording', 'stop_recording'])
  })

  it("set_recording_rights (plugin 0.6.1, Sew 0.9.2) sends Sew's contract args as given; any refusal throws", async () => {
    const sew = JSON.parse(readFileSync(resolve(__dirname, 'sew-native-voice-contract.json'), 'utf8'))
    const rights = sew.recording.controls.rights.args
    expect(sew.recording.controls.rights.command).toBe('set_recording_rights')
    expect(sew.recording.controls.rights.event).toBe(`${EVENT_PREFIX}${RECORDING_CONTROLS_EVENT}`)
    const ok = fakeBridge()
    await new NativeVoiceEngine(ok.bridge).setRecordingRights(rights)
    expect(ok.calls).toEqual([{ command: `${PLUGIN_COMMAND_PREFIX}set_recording_rights`, args: rights }])
    const missing = fakeBridge({}, { [`${PLUGIN_COMMAND_PREFIX}set_recording_rights`]: 'Command set_recording_rights not found' })
    await expect(new NativeVoiceEngine(missing.bridge).setRecordingRights(rights)).rejects.toBeInstanceOf(NativeVoiceError)
    const refused = fakeBridge({}, { [`${PLUGIN_COMMAND_PREFIX}set_recording_rights`]: { code: 'not_in_call', detail: null } })
    await expect(new NativeVoiceEngine(refused.bridge).setRecordingRights(rights)).rejects.toMatchObject({ code: 'not_in_call' })
  })

  it('a join answer with no call id fails loudly', async () => {
    const f = fakeBridge({ [`${PLUGIN_COMMAND_PREFIX}join`]: {} })
    await expect(new NativeVoiceEngine(f.bridge).join({ target: 't', room: 'r', muted: false })).rejects.toThrow(/join answer/)
  })
})
