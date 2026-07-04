// F4 — completion chime gate + throttle. The chime is synthesized via Web
// Audio; node has no AudioContext, so a recording stub stands in — the
// assertions are on the GATING behavior (setting off → silent; chime-storm
// throttle → several unseen completions inside 3s chime once).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

let completionSoundEnabled = true
vi.mock('@/stores/settings', () => ({
  useSettingsStore: {
    getState: () => ({ completionSoundEnabled }),
  },
}))

import { playCompletionSound, __resetCompletionSoundThrottleForTests } from './completion-sound'

class FakeGainParam {
  setValueAtTime = vi.fn()
  linearRampToValueAtTime = vi.fn()
  exponentialRampToValueAtTime = vi.fn()
}

class FakeAudioContext {
  static instances: FakeAudioContext[] = []
  static oscillatorsStarted = 0
  state = 'running'
  currentTime = 0
  destination = {}
  constructor() {
    FakeAudioContext.instances.push(this)
  }
  resume = vi.fn(async () => undefined)
  createOscillator(): Record<string, unknown> {
    return {
      type: 'sine',
      frequency: { value: 0 },
      connect: vi.fn(),
      start: vi.fn(() => {
        FakeAudioContext.oscillatorsStarted++
      }),
      stop: vi.fn(),
    }
  }
  createGain(): Record<string, unknown> {
    return { gain: new FakeGainParam(), connect: vi.fn() }
  }
}

describe('F4 — playCompletionSound', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    vi.setSystemTime(1_000_000)
    completionSoundEnabled = true
    FakeAudioContext.instances = []
    FakeAudioContext.oscillatorsStarted = 0
    vi.stubGlobal('AudioContext', FakeAudioContext)
    __resetCompletionSoundThrottleForTests()
  })

  afterEach(() => {
    vi.unstubAllGlobals()
    vi.useRealTimers()
  })

  it('plays a two-note chime when enabled', () => {
    playCompletionSound()

    expect(FakeAudioContext.instances).toHaveLength(1)
    expect(FakeAudioContext.oscillatorsStarted).toBe(2)
  })

  it('is silent when the setting is off — never even touches audio', () => {
    completionSoundEnabled = false

    playCompletionSound()

    expect(FakeAudioContext.instances).toHaveLength(0)
    expect(FakeAudioContext.oscillatorsStarted).toBe(0)
  })

  it('throttles a chime storm — several completions inside 3s chime once', () => {
    playCompletionSound()
    playCompletionSound()
    vi.setSystemTime(1_000_000 + 2_000)
    playCompletionSound()

    expect(FakeAudioContext.oscillatorsStarted).toBe(2) // one chime = two notes

    vi.setSystemTime(1_000_000 + 3_500)
    playCompletionSound()

    expect(FakeAudioContext.oscillatorsStarted).toBe(4)
  })
})
