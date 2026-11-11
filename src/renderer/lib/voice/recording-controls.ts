// K2 0.46.11 (Sew 0.9.0): the call bar's Record and Stop, and K2's own
// recording notice for its voice rooms.
//
// Rosson 2026-11-10: in K2's own rooms people are told a recording is on and
// can't opt out. The notice has one acknowledgement ("OK": the person's grant,
// sent with the notice version they saw) and "Leave call". There is no "join
// without being recorded" and no "stop recording me": the daemon sets each K2
// room to "declining means not joining", and Sew drops anyone who doesn't
// answer within a minute. Apps' Spaces keep Sew's full per-person sheet; this
// is only for K2's own rooms.

import type { NativeConsentPrompt, NativeRecording } from './native-voice-engine'

/** The person's Sew permissions in the room (`POST /cli/voice/recording/controls`). */
export interface RecordingPerms {
  /** RECORD_CALLS in the channel's `my_perms` (shown, never decided on: the plugin decides). */
  recordCalls: boolean
  /** MANAGE_CHANNELS in the channel's `my_perms` (likewise). */
  manageChannels: boolean
  /** A Voice & Chat only login: it never records. */
  voiceChatOnly?: boolean
  /** The raw facts Sew 0.9.2's plugin takes (`set_recording_rights`), as
   *  the daemon read them: K2 adds no rule of its own. */
  rights?: RecordingRights
}

/** `set_recording_rights`' arguments (tauri-plugin-sew-voice 0.6.1, Sew 0.9.2). */
export interface RecordingRights {
  room: string
  my_perms: string
  dm: boolean
  recording_enabled?: boolean
  recording_dm_enabled?: boolean
  record_capped: boolean
  agent: boolean
}

export interface RecordingButtons {
  canRecord: boolean
  canStop: boolean
}

/**
 * Which of Record and Stop to show. Sew 0.9.2's plugin (0.6.1) computes them
 * from the rights K2 hands it (`set_recording_rights`) and says so on its
 * `recording_controls` event (`controls`); until that event comes, the
 * recording event's own `can_record` / `can_stop` (written only when true).
 * K2 adds no rule of its own: no word from the plugin shows neither. A Voice
 * & Chat only login never records either way.
 */
export function recordingButtons(opts: {
  perms: RecordingPerms | null
  recording: NativeRecording | null
  /** The plugin's `recording_controls`: null until it sends one. */
  controls?: RecordingButtons | null
}): RecordingButtons {
  if (opts.perms?.voiceChatOnly) return { canRecord: false, canStop: false }
  if (opts.controls) return opts.controls
  return { canRecord: opts.recording?.canRecord === true, canStop: opts.recording?.canStop === true }
}

/** K2's own line when an agent on this server may read its rooms' transcripts. */
export const AGENT_READERS_LINE = 'Agents on this server may read the transcript.'

export interface RecordingNoticeText {
  title: string
  lines: string[]
  ok: string
  leave: string
}

/** K2's notice for its own rooms: who started it, what is kept and how long,
 *  who reads it, and that staying means being recorded. */
export function recordingNoticeText(opts: {
  roomName: string
  prompt: NativeConsentPrompt
  /** Any agent on this server holds "Read call transcripts". */
  agentReaders: boolean
  /** Before joining (a held join) or in the call. */
  ask: 'join' | 'in_call'
}): RecordingNoticeText {
  const { prompt, roomName } = opts
  const lines: string[] = []
  if (prompt.startedByName) lines.push(`${prompt.startedByName} started recording this call.`)
  else if (prompt.required) lines.push('This room records every call with more than one person.')
  else lines.push('This room records its calls.')
  lines.push("Everyone's voice is recorded and transcribed. The transcript is kept in this room's chat.")
  lines.push(
    prompt.audioKeepDays === 0
      ? 'The audio is deleted once the transcript is final.'
      : `The audio is kept for ${prompt.audioKeepDays} ${prompt.audioKeepDays === 1 ? 'day' : 'days'}.`,
  )
  if (prompt.appReaders.length > 0) lines.push(`${prompt.appReaders.join(', ')} can read the transcript.`)
  if (opts.agentReaders || prompt.agentReaders) lines.push(AGENT_READERS_LINE)
  lines.push(opts.ask === 'join' ? 'Joining means being recorded.' : 'Staying in the call means being recorded.')
  return {
    title: opts.ask === 'join' ? `${roomName} is being recorded` : 'This call is being recorded',
    lines,
    ok: 'OK',
    leave: opts.ask === 'join' ? "Don't join" : 'Leave call',
  }
}

/** The badge's tooltip: what started it. */
export function recordingBadgeTitle(r: NativeRecording, startedByName: string | null): string {
  if (r.trigger === 'required') return 'Recording: this room records every call with more than one person'
  if (r.trigger === 'auto') return 'Recording: this room records its calls'
  return startedByName ? `Recording: started by ${startedByName}` : 'Recording'
}
