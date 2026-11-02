// Voice rooms lane D: a test double for the media engine (no audio, no mic),
// with hooks for what the network or the server would do.

import {
  EngineError,
  type EngineConnectOptions,
  type EngineEvents,
  type EngineState,
  type VoiceDevices,
  type VoiceEngine,
} from './voice-engine'

export interface FakeEngineScript {
  /** Fail `connect` with this code (mic denied, TURN unreachable…). */
  failWith?: EngineError['code']
}

export class FakeVoiceEngine implements VoiceEngine {
  readonly calls: string[] = []
  muted = false
  deafened = false
  inputId: string | null = null
  outputId: string | null = null
  /** What the last `connect` was told. */
  relayOnly: boolean | null = null
  private events: EngineEvents | null = null
  private state: EngineState | 'idle' = 'idle'

  constructor(private readonly script: FakeEngineScript = {}) {}

  async connect(opts: EngineConnectOptions, events: EngineEvents): Promise<void> {
    this.calls.push(`connect ${opts.signalUrl} relay=${opts.relayOnly}`)
    this.events = events
    this.muted = opts.startMuted
    this.inputId = opts.inputDeviceId
    this.outputId = opts.outputDeviceId
    this.relayOnly = opts.relayOnly
    this.emit('connecting')
    await new Promise((r) => setTimeout(r, 0))
    if (this.script.failWith) {
      this.emit('ended', this.script.failWith)
      throw new EngineError(this.script.failWith, `fake engine: ${this.script.failWith}`)
    }
    if (this.state === 'ended') throw new EngineError('failed', 'fake engine: left while connecting')
    this.emit('connected')
    events.onParticipants([{ user: 'you', speaking: false, muted: this.muted }])
  }

  setMuted(muted: boolean): void {
    this.calls.push(`muted ${muted}`)
    this.muted = muted
  }

  setDeafened(deafened: boolean): void {
    this.calls.push(`deafened ${deafened}`)
    this.deafened = deafened
  }

  async setInputDevice(id: string | null): Promise<void> {
    this.inputId = id
  }

  async setOutputDevice(id: string | null): Promise<void> {
    this.outputId = id
  }

  async listDevices(): Promise<VoiceDevices> {
    return {
      inputs: [{ id: 'fake-mic', label: 'Fake microphone' }],
      outputs: [{ id: 'fake-out', label: 'Fake speakers' }],
      outputSelectable: true,
    }
  }

  async disconnect(): Promise<void> {
    this.calls.push('disconnect')
    if (this.state !== 'ended') this.emit('ended', 'left')
  }

  /** What the network or the server would do. */
  simulate(state: EngineState, reason?: string): void {
    this.emit(state, reason)
  }

  private emit(state: EngineState, reason?: string): void {
    this.state = state
    this.events?.onState(state, reason)
  }
}
