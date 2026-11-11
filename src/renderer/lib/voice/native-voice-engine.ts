// Voice rooms N2 (prd-k2-voice-rooms-v1 §14.3, §14.9; vs-live V44): the K2 app's
// native call engine, seen from the webview: `tauri-plugin-sew-voice`'s commands
// and events (`plugin:sew-voice|<command>`, `sew-voice://<event>`).
//
// The call lives in the app's Rust process, not in this window. Every window
// gets every event, any window may send a command, and a window opened mid-call
// catches up with `snapshot()`. No token ever crosses this bridge: `join` takes
// the saved host's id and the room's slug, and K2's Rust provider does the rest.
//
// The JSON is pinned by `native-voice-contract.json`, which src-tauri's
// `native_voice::tests` holds to the plugin's real types.

import type { VoiceDevices } from './voice-engine'

/** What this file needs from Tauri: injected, so tests run without it. */
export interface NativeVoiceBridge {
  invoke<T>(command: string, args?: Record<string, unknown>): Promise<T>
  /** Resolves to the unsubscribe. */
  listen(event: string, handler: (payload: unknown) => void): Promise<() => void>
}

export const PLUGIN_COMMAND_PREFIX = 'plugin:sew-voice|'
export const EVENT_PREFIX = 'sew-voice://'

/** What `set_device` takes for the OS default: it follows later changes too. */
export const DEFAULT_DEVICE = 'default'

/** Which server and room the call is on (Sew 0.7.2): absent while idle. After a
 *  moderator's move `room` is Sew's decimal channel id, even for a join by slug. */
export interface NativeCallPlace {
  target?: string
  room?: string
}

export type NativeCallState = NativeCallPlace &
  (
    | { state: 'idle' }
    | { state: 'connecting' }
    | { state: 'connected' }
    | { state: 'reconnecting'; attempt: number }
    /** Held (plugin 0.2.0+): the room is recorded and Sew lets the join in only
     *  with the person's answer to its notice. No mic, no media until then. */
    | { state: 'consent_required'; recordingId: string; noticeVersion: number }
    | { state: 'ended'; reason: string }
  )

/** The notice's slots K2's own notice shows (Sew's `ConsentPrompt`). */
export interface NativeConsentPrompt {
  /** Echoed in the answer: a changed notice is asked again. */
  noticeVersion: number
  /** Who started it (absent for a room that records by itself). */
  startedByName: string | null
  /** 0 = deleted once the transcript is final. */
  audioKeepDays: number
  /** The room records every call with two or more people. */
  required: boolean
  /** Apps whose servers can read the transcript. */
  appReaders: string[]
  /** Sew's own "agents in this channel can read the transcript". */
  agentReaders: boolean
  /** `join_unrecorded` or `not_join` (K2's own rooms are `not_join`). */
  declineMode: string
}

/** The `recording` event (plugin 0.2.0+): the call's active recording, or null
 *  once none is active. This device is recorded only after Sew says
 *  `granted`: no answer is never consent. */
export interface NativeRecording {
  recordingId: string
  /** `pending`, `granted`, `declined` or `withdrawn`. */
  myConsent: string
  /** The recorder (Sew Notes) is in the call's media now. */
  recorderPresent: boolean
  /** Sew 0.9.0: `manual`, `auto` or `required` (`manual` when absent). */
  trigger: string
  /** Sew 0.9.0: the room requires its recording (Stop is refused with 2+). */
  required: boolean
  /** Sew's member id of the person who started it. */
  startedBy: string | null
  /** The notice, while `myConsent` is `pending`. */
  prompt: NativeConsentPrompt | null
}

/** No names here: the media token and call keys carry none (the rooms list does). */
export interface NativeParticipant {
  memberId: string
  isSelf: boolean
  muted: boolean
  deafened: boolean
  /** A moderator muted them. */
  serverMuted: boolean
  serverDeafened: boolean
  /** Their role or a room rule took away speaking (Sew writes it only when true). */
  speakDenied: boolean
}

export interface NativeLevel {
  memberId: string
  /** 0 to 1. */
  level: number
}

export interface NativeDevice {
  id: string
  label: string
  isDefault: boolean
}

export interface NativeDevices extends VoiceDevices {
  inputs: NativeDevice[]
  outputs: NativeDevice[]
}

export interface NativeSnapshot {
  state: NativeCallState
  participants: NativeParticipant[]
  callId: string | null
  target: string | null
  room: string | null
}

export interface NativeEvents {
  onState(state: NativeCallState): void
  onParticipants(list: NativeParticipant[]): void
  onSpeaking(levels: NativeLevel[]): void
  onDevices(devices: NativeDevices): void
  onError(code: string, detail: unknown): void
  onRecording(recording: NativeRecording | null): void
}

/** A command's rejection: the plugin's `{code, detail}`. */
export class NativeVoiceError extends Error {
  readonly code: string
  readonly detail: unknown
  constructor(code: string, detail: unknown) {
    super(code)
    this.name = 'NativeVoiceError'
    this.code = code
    this.detail = detail
  }
}

function record(v: unknown): Record<string, unknown> | null {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : null
}

/** A shape this client doesn't know is a bug on one side: it throws, loudly. */
function badShape(what: string, v: unknown): Error {
  return new Error(`sew-voice: unexpected ${what}: ${JSON.stringify(v)}`)
}

export function parseCallState(v: unknown): NativeCallState {
  const r = record(v)
  const place: NativeCallPlace = {
    ...(typeof r?.target === 'string' ? { target: r.target } : {}),
    ...(typeof r?.room === 'string' ? { room: r.room } : {}),
  }
  switch (r?.state) {
    case 'idle':
    case 'connecting':
    case 'connected':
      return { state: r.state, ...place }
    case 'reconnecting':
      if (typeof r.attempt === 'number') return { state: 'reconnecting', attempt: r.attempt, ...place }
      break
    case 'consent_required':
      if (typeof r.recording_id === 'string' && typeof r.notice_version === 'number') {
        return { state: 'consent_required', recordingId: r.recording_id, noticeVersion: r.notice_version, ...place }
      }
      break
    case 'ended':
      if (typeof r.reason === 'string') return { state: 'ended', reason: r.reason, ...place }
      break
  }
  throw badShape('state', v)
}

export function parsePrompt(v: unknown): NativeConsentPrompt | null {
  const p = record(v)
  if (!p) return null
  if (typeof p.notice_version !== 'number') throw badShape('consent prompt', v)
  const started = record(p.started_by)
  return {
    noticeVersion: p.notice_version,
    startedByName: typeof started?.display_name === 'string' && started.display_name !== '' ? started.display_name : null,
    audioKeepDays: typeof p.audio_keep_days === 'number' ? p.audio_keep_days : 0,
    required: p.required === true,
    appReaders: Array.isArray(p.app_readers) ? p.app_readers.filter((x): x is string => typeof x === 'string') : [],
    agentReaders: p.agent_readers === true,
    declineMode: typeof p.decline_mode === 'string' ? p.decline_mode : 'join_unrecorded',
  }
}

export function parseRecording(v: unknown): NativeRecording | null {
  if (v === null) return null
  const r = record(v)
  if (!r || typeof r.recording_id !== 'string' || typeof r.my_consent !== 'string' || typeof r.recorder_present !== 'boolean') {
    throw badShape('recording', v)
  }
  return {
    recordingId: r.recording_id,
    myConsent: r.my_consent,
    recorderPresent: r.recorder_present,
    trigger: typeof r.trigger === 'string' ? r.trigger : 'manual',
    required: r.required === true,
    startedBy: typeof r.started_by === 'string' ? r.started_by : null,
    prompt: parsePrompt(r.consent_prompt),
  }
}

export function parseParticipants(v: unknown): NativeParticipant[] {
  if (!Array.isArray(v)) throw badShape('participants', v)
  return v.map((raw) => {
    const r = record(raw)
    if (!r || typeof r.member_id !== 'string') throw badShape('participant', raw)
    return {
      memberId: r.member_id,
      isSelf: r.is_self === true,
      muted: r.muted === true,
      deafened: r.deafened === true,
      serverMuted: r.server_muted === true,
      serverDeafened: r.server_deafened === true,
      speakDenied: r.speak_denied === true,
    }
  })
}

export function parseSpeaking(v: unknown): NativeLevel[] {
  if (!Array.isArray(v)) throw badShape('speaking', v)
  return v.map((raw) => {
    const r = record(raw)
    if (!r || typeof r.member_id !== 'string' || typeof r.level !== 'number') throw badShape('speaking entry', raw)
    return { memberId: r.member_id, level: r.level }
  })
}

function parseDeviceRows(v: unknown): NativeDevice[] {
  if (!Array.isArray(v)) throw badShape('device list', v)
  return v.map((raw) => {
    const r = record(raw)
    if (!r || typeof r.id !== 'string' || typeof r.name !== 'string') throw badShape('device', raw)
    return { id: r.id, label: r.name, isDefault: r.default === true }
  })
}

export function parseDevices(v: unknown): NativeDevices {
  const r = record(v)
  if (!r) throw badShape('devices', v)
  return { inputs: parseDeviceRows(r.inputs), outputs: parseDeviceRows(r.outputs), outputSelectable: true }
}

export function parseEngineError(v: unknown): { code: string; detail: unknown } {
  const r = record(v)
  if (!r || typeof r.code !== 'string') throw badShape('error', v)
  return { code: r.code, detail: r.detail ?? null }
}

/** A rejected command as a `NativeVoiceError`. Tauri rejects with the value the
 *  plugin returned. A plain string is Tauri's own: the command never ran (a
 *  missing permission, say), so it counts as `failed`. */
export function toNativeError(err: unknown): NativeVoiceError {
  if (err instanceof NativeVoiceError) return err
  const r = record(err)
  if (r && typeof r.code === 'string') return new NativeVoiceError(r.code, r.detail ?? null)
  return new NativeVoiceError('failed', typeof err === 'string' ? err : String(err))
}

export interface NativeStatus {
  available: boolean
}

export class NativeVoiceEngine {
  private unlisten: Array<() => void> = []

  constructor(private readonly bridge: NativeVoiceBridge) {}

  private async command<T>(name: string, args?: Record<string, unknown>): Promise<T> {
    try {
      return await this.bridge.invoke<T>(`${PLUGIN_COMMAND_PREFIX}${name}`, args)
    } catch (err) {
      throw toNativeError(err)
    }
  }

  /** Subscribe to the plugin's events, for this window's life. */
  async start(events: NativeEvents): Promise<void> {
    const on = async (name: string, handle: (payload: unknown) => void): Promise<void> => {
      this.unlisten.push(await this.bridge.listen(`${EVENT_PREFIX}${name}`, handle))
    }
    await Promise.all([
      on('state', (p) => events.onState(parseCallState(p))),
      on('participants', (p) => events.onParticipants(parseParticipants(p))),
      on('speaking', (p) => events.onSpeaking(parseSpeaking(p))),
      on('devices', (p) => events.onDevices(parseDevices(p))),
      on('errors', (p) => {
        const e = parseEngineError(p)
        events.onError(e.code, e.detail)
      }),
      on('recording', (p) => events.onRecording(parseRecording(p))),
    ])
  }

  stop(): void {
    for (const off of this.unlisten) off()
    this.unlisten = []
  }

  /** What a window opened mid-call needs. */
  async snapshot(): Promise<NativeSnapshot> {
    const raw = await this.command<unknown>('state')
    const r = record(raw)
    if (!r) throw badShape('state snapshot', raw)
    return {
      state: parseCallState(r),
      participants: parseParticipants(r.participants),
      callId: typeof r.call_id === 'string' ? r.call_id : null,
      target: typeof r.target === 'string' ? r.target : null,
      room: typeof r.room === 'string' ? r.room : null,
    }
  }

  /** Rejects with a `NativeVoiceError`: `already_in_call`, a provider's refusal… */
  async join(opts: { target: string; room: string; muted: boolean }): Promise<{ callId: string }> {
    const r = await this.command<{ call_id: string }>('join', { target: opts.target, room: opts.room, muted: opts.muted })
    if (typeof r?.call_id !== 'string') throw badShape('join answer', r)
    return { callId: r.call_id }
  }

  async leave(): Promise<void> {
    await this.command<null>('leave')
  }

  async mute(change: { muted?: boolean; deafened?: boolean }): Promise<void> {
    await this.command<null>('mute', change)
  }

  async devices(): Promise<NativeDevices> {
    return parseDevices(await this.command<unknown>('devices'))
  }

  async setDevice(change: { input?: string; output?: string }): Promise<void> {
    await this.command<null>('set_device', change)
  }

  /** The person's answer to the recording notice they saw (`noticeVersion`).
   *  A held join then joins; mid-call, Sew records them from here on. */
  async consent(granted: boolean, noticeVersion: number): Promise<void> {
    await this.command<null>('consent', { granted, notice_version: noticeVersion })
  }

  /** Record (Sew 0.9.0, permission set `sew-voice:recording`). */
  async startRecording(): Promise<void> {
    await this.command<null>('start_recording')
  }

  /** Stop the room's recording (Sew refuses `recording_required` with 2+ in a required room). */
  async stopRecording(): Promise<void> {
    await this.command<null>('stop_recording')
  }
}
