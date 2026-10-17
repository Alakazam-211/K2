// F4 — completion chime gate + throttle. The chime is synthesized via Web
// Audio; node has no AudioContext, so a recording stub stands in — the
// assertions are on the GATING behavior (setting off → silent; chime-storm
// throttle → several unseen completions inside 3s chime once;
// per-workspace mute AND-gates the global toggle and is checked before
// the throttle).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

let completionSoundEnabled = true
let completionSound: unknown = 'chime'
let completionSoundVolume: unknown = 50
vi.mock('@/stores/settings', () => ({
  useSettingsStore: {
    getState: () => ({ completionSoundEnabled, completionSound, completionSoundVolume }),
  },
}))

// The project list of the room the pane is in (MS21/MS68) — passed in, never
// read from a global store.
let projects: { id: string; completionSoundEnabled?: number }[] = []

import {
  playCompletionSound,
  previewCompletionSound,
  __resetCompletionSoundThrottleForTests,
} from './completion-sound'
import {
  COMPLETION_SOUNDS,
  clampCompletionSoundVolume,
  completionSoundGain,
  normalizeCompletionSound,
} from './completion-sound-options'

class FakeParam {
  value = 0
  setValueAtTime = vi.fn()
  linearRampToValueAtTime = vi.fn()
  exponentialRampToValueAtTime = vi.fn()
}

interface FakeOsc {
  type: string
  frequency: FakeParam
  gain: FakeParam
}

class FakeAudioContext {
  static instances: FakeAudioContext[] = []
  static oscillatorsStarted = 0
  /** Every started note: its wave, pitch params and gain envelope. */
  static notes: FakeOsc[] = []
  state = 'running'
  currentTime = 0
  destination = {}
  constructor() {
    FakeAudioContext.instances.push(this)
  }
  resume = vi.fn(async () => undefined)
  createOscillator(): Record<string, unknown> {
    const osc = {
      type: 'sine',
      frequency: new FakeParam(),
      connect: vi.fn((g: { gain: FakeParam }) => {
        note.gain = g.gain
      }),
      start: vi.fn(() => {
        FakeAudioContext.oscillatorsStarted++
        FakeAudioContext.notes.push(note)
      }),
      stop: vi.fn(),
    }
    const note: FakeOsc = {
      get type() {
        return osc.type
      },
      frequency: osc.frequency,
      gain: new FakeParam(),
    }
    return osc
  }
  createGain(): Record<string, unknown> {
    return { gain: new FakeParam(), connect: vi.fn() }
  }
}

/** The peak gain each started note ramps up to. */
function peaks(): number[] {
  return FakeAudioContext.notes.map((n) => {
    const call = n.gain.linearRampToValueAtTime.mock.calls[0]
    if (!call) throw new Error('note never ramped its gain up')
    return call[0] as number
  })
}

describe('F4 — playCompletionSound', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(1_000_000)
    completionSoundEnabled = true
    completionSound = 'chime'
    completionSoundVolume = 50
    projects = []
    FakeAudioContext.instances = []
    FakeAudioContext.oscillatorsStarted = 0
    FakeAudioContext.notes = []
    vi.stubGlobal('AudioContext', FakeAudioContext)
    __resetCompletionSoundThrottleForTests()
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it('plays a two-note chime when enabled', () => {
    playCompletionSound(null, projects)

    expect(FakeAudioContext.instances).toHaveLength(1)
    expect(FakeAudioContext.oscillatorsStarted).toBe(2)
  })

  it('is silent when the setting is off — never even touches audio', () => {
    completionSoundEnabled = false

    playCompletionSound(null, projects)

    expect(FakeAudioContext.instances).toHaveLength(0)
    expect(FakeAudioContext.oscillatorsStarted).toBe(0)
  })

  it('is silent when global is on and the workspace is muted', () => {
    projects = [{ id: 'ws-a', completionSoundEnabled: 0 }]

    playCompletionSound('ws-a', projects)

    expect(FakeAudioContext.instances).toHaveLength(0)
    expect(FakeAudioContext.oscillatorsStarted).toBe(0)
  })

  it('plays when both the global toggle and the workspace flag are on', () => {
    projects = [{ id: 'ws-a', completionSoundEnabled: 1 }]

    playCompletionSound('ws-a', projects)

    expect(FakeAudioContext.instances).toHaveLength(1)
    expect(FakeAudioContext.oscillatorsStarted).toBe(2)
  })

  it('is silent when the global toggle is off even if the workspace is on', () => {
    completionSoundEnabled = false
    projects = [{ id: 'ws-a', completionSoundEnabled: 1 }]

    playCompletionSound('ws-a', projects)

    expect(FakeAudioContext.instances).toHaveLength(0)
    expect(FakeAudioContext.oscillatorsStarted).toBe(0)
  })

  it('treats a missing workspace field as ON', () => {
    projects = [{ id: 'ws-a' }]

    playCompletionSound('ws-a', projects)

    expect(FakeAudioContext.oscillatorsStarted).toBe(2)
  })

  it("reads the mute from the room's own list: the same id muted on another server does not silence it", () => {
    const serverA = [{ id: 'ws-a', completionSoundEnabled: 0 }]
    const serverB = [{ id: 'ws-a', completionSoundEnabled: 1 }]

    playCompletionSound('ws-a', serverB)
    expect(FakeAudioContext.oscillatorsStarted).toBe(2)

    vi.setSystemTime(1_010_000)
    playCompletionSound('ws-a', serverA)
    expect(FakeAudioContext.oscillatorsStarted).toBe(2)
  })

  it('does not consume the throttle when a muted workspace would have chimed', () => {
    projects = [
      { id: 'muted', completionSoundEnabled: 0 },
      { id: 'loud', completionSoundEnabled: 1 },
    ]

    playCompletionSound('muted', projects)
    playCompletionSound('loud', projects)

    expect(FakeAudioContext.oscillatorsStarted).toBe(2)
  })

  it('throttles a chime storm — several completions inside 3s chime once', () => {
    playCompletionSound(null, projects)
    playCompletionSound(null, projects)
    vi.setSystemTime(1_000_000 + 2_000)
    playCompletionSound(null, projects)

    expect(FakeAudioContext.oscillatorsStarted).toBe(2) // one chime = two notes

    vi.setSystemTime(1_000_000 + 3_500)
    playCompletionSound(null, projects)

    expect(FakeAudioContext.oscillatorsStarted).toBe(4)
  })
})

describe('0.46.1 — completion sound volume and choice', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(1_000_000)
    completionSoundEnabled = true
    completionSound = 'chime'
    completionSoundVolume = 50
    projects = []
    FakeAudioContext.instances = []
    FakeAudioContext.oscillatorsStarted = 0
    FakeAudioContext.notes = []
    vi.stubGlobal('AudioContext', FakeAudioContext)
    __resetCompletionSoundThrottleForTests()
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it('the defaults play the pre-0.46.1 chime at the pre-0.46.1 loudness', () => {
    playCompletionSound(null, projects)

    expect(FakeAudioContext.notes.map((n) => n.frequency.value)).toEqual([880, 1318.5])
    expect(FakeAudioContext.notes.map((n) => n.type)).toEqual(['sine', 'sine'])
    expect(peaks()).toEqual([0.12, 0.12])
    for (const n of FakeAudioContext.notes) {
      expect(n.gain.exponentialRampToValueAtTime.mock.calls[0][0]).toBe(0.0001)
    }
  })

  it('applies the volume: 100 is 3x, 25 is a third, 0 never touches audio', () => {
    completionSoundVolume = 100
    playCompletionSound(null, projects)
    expect(peaks()[0]).toBeCloseTo(0.36, 10)

    FakeAudioContext.notes = []
    vi.setSystemTime(1_010_000)
    completionSoundVolume = 25
    playCompletionSound(null, projects)
    expect(peaks()[0]).toBeCloseTo(0.04, 10)

    FakeAudioContext.notes = []
    FakeAudioContext.instances = []
    __resetCompletionSoundThrottleForTests()
    vi.setSystemTime(1_020_000)
    completionSoundVolume = 0
    playCompletionSound(null, projects)
    expect(FakeAudioContext.instances).toHaveLength(0)
    expect(FakeAudioContext.notes).toHaveLength(0)
  })

  it('keeps the decay target under the peak at the quietest volume', () => {
    completionSoundVolume = 1
    playCompletionSound(null, projects)
    for (const n of FakeAudioContext.notes) {
      const peak = n.gain.linearRampToValueAtTime.mock.calls[0][0] as number
      const floor = n.gain.exponentialRampToValueAtTime.mock.calls[0][0] as number
      expect(peak).toBeGreaterThan(0)
      expect(floor).toBeLessThan(peak)
      expect(floor).toBeGreaterThan(0)
    }
  })

  it.each([
    ['soft', [523.25, 659.25, 783.99], 'sine'],
    ['ding', [1046.5, 2888.3], 'sine'],
    ['tick', [2000], 'triangle'],
  ])('plays the chosen sound: %s', (id, freqs, wave) => {
    completionSound = id
    playCompletionSound(null, projects)

    expect(FakeAudioContext.notes.map((n) => n.frequency.value)).toEqual(freqs)
    expect(FakeAudioContext.notes.every((n) => n.type === wave)).toBe(true)
  })

  it('the pop glides its pitch down', () => {
    completionSound = 'pop'
    playCompletionSound(null, projects)

    expect(FakeAudioContext.notes).toHaveLength(1)
    const f = FakeAudioContext.notes[0].frequency
    expect(f.setValueAtTime.mock.calls[0][0]).toBe(620)
    expect(f.exponentialRampToValueAtTime.mock.calls[0][0]).toBe(180)
  })

  it('an unknown sound or a junk volume from the daemon plays the defaults', () => {
    completionSound = 'airhorn'
    completionSoundVolume = 'loud'
    playCompletionSound(null, projects)

    expect(FakeAudioContext.notes.map((n) => n.frequency.value)).toEqual([880, 1318.5])
    expect(peaks()).toEqual([0.12, 0.12])
  })

  it('Preview plays the given sound and volume even with the switch off and inside the throttle', () => {
    playCompletionSound(null, projects)
    expect(FakeAudioContext.notes).toHaveLength(2)
    completionSoundEnabled = false

    FakeAudioContext.notes = []
    previewCompletionSound('tick', 100)
    expect(FakeAudioContext.notes.map((n) => n.frequency.value)).toEqual([2000])
    expect(peaks()[0]).toBeCloseTo(0.24, 10)

    // Preview never consumes the chime throttle.
    completionSoundEnabled = true
    FakeAudioContext.notes = []
    __resetCompletionSoundThrottleForTests()
    previewCompletionSound('chime', 50)
    playCompletionSound(null, projects)
    expect(FakeAudioContext.notes).toHaveLength(4)
  })

  it('normalizes and clamps the settings values', () => {
    expect(COMPLETION_SOUNDS.map((s) => s.id)).toEqual(['chime', 'soft', 'pop', 'ding', 'tick'])
    expect(normalizeCompletionSound('ding')).toBe('ding')
    expect(normalizeCompletionSound('nope')).toBe('chime')
    expect(normalizeCompletionSound(undefined)).toBe('chime')
    expect(clampCompletionSoundVolume(150)).toBe(100)
    expect(clampCompletionSoundVolume(-3)).toBe(0)
    expect(clampCompletionSoundVolume(33.6)).toBe(34)
    expect(clampCompletionSoundVolume(undefined)).toBe(50)
    expect(clampCompletionSoundVolume(Number.NaN)).toBe(50)
    expect(completionSoundGain(0)).toBe(0)
    expect(completionSoundGain(50)).toBe(1)
    expect(completionSoundGain(100)).toBeCloseTo(3, 10)
  })
})
