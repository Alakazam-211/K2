import { describe, expect, it } from 'vitest'
import { makeVoiceEngine, SeatOnlyEngine } from './voice-engine'

const opts = { signalUrl: 'wss://v', mediaToken: 't', e2ee: null, relayOnly: true, inputDeviceId: null, outputDeviceId: null, startMuted: true }

describe('the seat engine', () => {
  it('holds the seat only: connected, no devices, nothing to route', async () => {
    const e = makeVoiceEngine()
    expect(e).toBeInstanceOf(SeatOnlyEngine)
    const states: string[] = []
    await e.connect(opts, { onState: (s) => states.push(s), onParticipants: () => {} })
    expect(states).toEqual(['connected'])
    expect(await e.listDevices()).toEqual({ inputs: [], outputs: [], outputSelectable: false })
  })
})
