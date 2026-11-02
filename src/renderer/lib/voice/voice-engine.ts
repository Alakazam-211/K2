// Voice rooms (prd-k2-voice-rooms-v1 §4.4, lane D): the media engine seam.
//
// 0.46.8 carries no call audio. `SeatOnlyEngine` holds the seat the server
// gave and starts no media, so join, leave, mute and the room list run on the
// signalling path alone. Native audio arrives in 0.46.9 through
// `sew-call-client`; wiring it is one `setVoiceEngineFactory` call. (The spike
// found stock WebKitGTK has no WebRTC, so calls won't run in the webview.)
//
// Relay-only (§6.6 M9/M10): when the box's media tier is tunnel, Sew's join
// answer says `media.relay_only: true`. A real engine must then use only the
// `turns:` server and no host or srflx candidates, so the box IP never leaks,
// even if the signalling hint is missing.

export type EngineState = 'connecting' | 'connected' | 'reconnecting' | 'ended'

export interface EngineParticipant {
  user: string
  speaking: boolean
  muted: boolean
}

export interface VoiceDevice {
  id: string
  label: string
}

export interface VoiceDevices {
  inputs: VoiceDevice[]
  outputs: VoiceDevice[]
  /** False where the engine can't route audio output. */
  outputSelectable: boolean
}

export interface EngineConnectOptions {
  signalUrl: string
  mediaToken: string
  /** `CallKeysInit` for MLS E2EE (opaque here). */
  e2ee: unknown
  /** §6.6 M10 (2). The engine must stay relay-only. */
  relayOnly: boolean
  inputDeviceId: string | null
  outputDeviceId: string | null
  startMuted: boolean
}

export interface EngineEvents {
  /** `reason` on `ended`: a Sew leave reason (`media_path_down`,
   *  `channel_archived`, `removed_by_moderator`…) or `network`. */
  onState(state: EngineState, reason?: string): void
  onParticipants(list: EngineParticipant[]): void
}

export interface VoiceEngine {
  /** Resolves once media is up; rejects with an `EngineError`. */
  connect(opts: EngineConnectOptions, events: EngineEvents): Promise<void>
  setMuted(muted: boolean): void
  setDeafened(deafened: boolean): void
  setInputDevice(id: string | null): Promise<void>
  setOutputDevice(id: string | null): Promise<void>
  listDevices(): Promise<VoiceDevices>
  /** Leave and release the mic. Idempotent. */
  disconnect(): Promise<void>
}

/** A refusal the shell explains (§4.6). */
export class EngineError extends Error {
  readonly code: 'mic_denied' | 'turn_unreachable' | 'failed'
  constructor(code: EngineError['code'], message: string) {
    super(message)
    this.name = 'EngineError'
    this.code = code
  }
}

/** 0.46.8: the seat only. Mute and deafen still reach the server through the
 *  store. */
export class SeatOnlyEngine implements VoiceEngine {
  async connect(_opts: EngineConnectOptions, events: EngineEvents): Promise<void> {
    events.onState('connected')
  }
  setMuted(): void {}
  setDeafened(): void {}
  async setInputDevice(): Promise<void> {}
  async setOutputDevice(): Promise<void> {}
  async listDevices(): Promise<VoiceDevices> {
    return { inputs: [], outputs: [], outputSelectable: false }
  }
  async disconnect(): Promise<void> {}
}

/** Which engine joins use: `seat` until native audio is installed. */
export type VoiceEngineKind = 'seat' | 'native'

let factory: (() => VoiceEngine) | null = null

/** Install the native engine (0.46.9), or null for the seat only. */
export function setVoiceEngineFactory(make: (() => VoiceEngine) | null): void {
  factory = make
}

export function voiceEngineKind(): VoiceEngineKind {
  return factory ? 'native' : 'seat'
}

/** A fresh engine for one call. */
export function makeVoiceEngine(): VoiceEngine {
  return factory ? factory() : new SeatOnlyEngine()
}
