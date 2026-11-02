import { afterEach, describe, expect, it } from 'vitest'
import { makeVoiceEngine, SeatOnlyEngine, setVoiceEngineFactory, voiceEngineKind } from './voice-engine'
import { FakeVoiceEngine } from './fake-voice-engine'

afterEach(() => setVoiceEngineFactory(null))

const opts = { signalUrl: 'wss://v', mediaToken: 't', e2ee: null, relayOnly: true, inputDeviceId: null, outputDeviceId: null, startMuted: true }

describe('engine factory', () => {
  it('0.46.8 holds the seat only: connected, no devices, nothing to route', async () => {
    expect(voiceEngineKind()).toBe('seat')
    const e = makeVoiceEngine()
    expect(e).toBeInstanceOf(SeatOnlyEngine)
    const states: string[] = []
    await e.connect(opts, { onState: (s) => states.push(s), onParticipants: () => {} })
    expect(states).toEqual(['connected'])
    expect(await e.listDevices()).toEqual({ inputs: [], outputs: [], outputSelectable: false })
  })

  it('an installed engine is used, and is told relay-only', async () => {
    setVoiceEngineFactory(() => new FakeVoiceEngine())
    expect(voiceEngineKind()).toBe('native')
    const e = makeVoiceEngine() as FakeVoiceEngine
    const states: string[] = []
    await e.connect(opts, { onState: (s) => states.push(s), onParticipants: () => {} })
    expect(states).toEqual(['connecting', 'connected'])
    expect(e.relayOnly).toBe(true)
    expect(e.muted).toBe(true)
  })
})
