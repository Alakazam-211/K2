// Recording a DM call (prd-home-call-recording-v1 S1, HR-10): the client
// side of what Sew posts after a recording. Sew posts a **recording card**
// (the record message, `Message.recording`) into the DM, and the transcript
// as replies under it, one per speaker turn (`thread_root_id`, and
// `Message.transcript` on each line). The DM draws the card as a line, like a
// call's own line; the lines never show as bubbles and never add to the DM's
// unread (the daemon keeps them out of its counts too). "Show transcript"
// reads them with `GET /cli/home-chat/replies` as the person: only the two
// people in the DM can. There is no audio playback in K2.

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliGet } from '@/lib/daemon-cli'
import { formatCallDuration } from './calls'
import { recordingBadgeTitle, recordingButtons } from '@/lib/voice/recording-controls'
import type { VoiceCall } from '@/stores/voice-core'

/** Sew's RecordingSummary on a recording card (`Message.recording`). */
export interface HomeChatRecordingSummary {
  recordingId: string
  /** Who pressed Record (a Sew member id); null when the server's rule started it. */
  startedBy: string | null
  auto: boolean
  stopped: boolean
  durationMs: number | null
  /** `pending`, `running`, `done`, `partial` or `failed`. */
  transcriptState: string
  turns: number
  /** `available`, `expired` or `purged`. */
  audioState: string
  /** How long the audio is kept, in days (0: deleted once the transcript is final). */
  keepDays: number | null
}

function idOf(v: unknown): string | null {
  if (typeof v === 'string' && v) return v
  if (typeof v === 'number' && Number.isFinite(v)) return String(v)
  return null
}

export function parseRecordingSummary(v: unknown): HomeChatRecordingSummary | null {
  const o = v as Record<string, unknown> | null
  const recordingId = idOf(o?.recording_id)
  if (!o || !recordingId) return null
  const t = (o.transcript ?? {}) as Record<string, unknown>
  const a = (o.audio ?? {}) as Record<string, unknown>
  return {
    recordingId,
    startedBy: idOf(o.started_by),
    auto: o.auto === true,
    stopped: typeof o.stopped_at === 'string' || typeof o.stop_reason === 'string',
    durationMs: typeof o.duration_ms === 'number' ? o.duration_ms : null,
    transcriptState: typeof t.state === 'string' ? t.state : 'pending',
    turns: typeof t.turns === 'number' ? t.turns : 0,
    audioState: typeof a.state === 'string' ? a.state : 'available',
    keepDays: typeof a.keep_days === 'number' ? a.keep_days : null,
  }
}

/** A recording's transcript line: a reply under the card, or a message that
 *  carries `transcript`. Off the DM's timeline (Show transcript reads them). */
export function transcriptRootOf(v: unknown): string | null {
  const o = v as Record<string, unknown> | null
  if (!o) return null
  const root = idOf(o.thread_root_id)
  if (root) return root
  if (o.transcript && typeof o.transcript === 'object') return idOf((o.transcript as Record<string, unknown>).recording_id) ?? 'transcript'
  return null
}

/** The recording card's state: `live` while it records, then `transcribing`
 *  until Sew's transcript is final, `ready` (Show transcript) or `failed`. */
export type RecordingCardState = 'live' | 'transcribing' | 'ready' | 'failed'

/** What the recording card shows: its title ("Recording · 12:04"), the
 *  line under it ("Started by Alex · audio kept 30 days") and its state.
 *  `starter` is the starter's shown name ("you" for the person). */
export interface RecordingCardView {
  title: string
  detail: string
  state: RecordingCardState
  /** The transcript has gaps (Sew's `partial`). */
  partial: boolean
}

export function recordingCardView(r: HomeChatRecordingSummary, starter: string | null): RecordingCardView {
  const detail: string[] = []
  if (starter) detail.push(`Started by ${starter}`)
  else if (r.auto) detail.push("Started by the server's rule")
  if (r.audioState !== 'available') detail.push('audio deleted')
  else if (r.keepDays === 0) detail.push('audio deleted once the transcript is final')
  else if (r.keepDays !== null) detail.push(`audio kept ${r.keepDays} ${r.keepDays === 1 ? 'day' : 'days'}`)
  if (detail.length > 0) detail[0] = detail[0].charAt(0).toUpperCase() + detail[0].slice(1)
  const t = r.transcriptState
  const state: RecordingCardState = !r.stopped ? 'live' : t === 'done' || t === 'partial' ? 'ready' : t === 'failed' ? 'failed' : 'transcribing'
  return {
    title: r.stopped ? `Recording · ${formatCallDuration(r.durationMs ?? 0)}` : 'Recording',
    detail: detail.join(' · '),
    state,
    partial: t === 'partial',
  }
}

/** The card as one line, for a screen reader or a copy: "Recording · 12:04
 *  · started by Alex · transcript ready · audio kept 30 days". */
export function recordingCardLine(r: HomeChatRecordingSummary, starter: string | null): string {
  const parts: string[] = [r.stopped ? `Recording · ${formatCallDuration(r.durationMs ?? 0)}` : 'Recording in progress']
  if (starter) parts.push(`started by ${starter}`)
  else if (r.auto) parts.push("started by the server's rule")
  const t = r.transcriptState
  parts.push(
    t === 'done'
      ? 'transcript ready'
      : t === 'partial'
        ? 'transcript ready (with gaps)'
        : t === 'failed'
          ? 'no transcript'
          : 'transcript coming',
  )
  if (r.audioState !== 'available') parts.push('audio deleted')
  else if (r.keepDays === 0) parts.push('audio deleted once the transcript is final')
  else if (r.keepDays !== null) parts.push(`audio kept ${r.keepDays} ${r.keepDays === 1 ? 'day' : 'days'}`)
  return parts.join(' · ')
}

/** Show transcript is there once a line can exist. */
export function canShowTranscript(r: HomeChatRecordingSummary): boolean {
  return r.turns > 0 || r.transcriptState === 'running' || r.transcriptState === 'done' || r.transcriptState === 'partial'
}

/** One transcript line as Show transcript draws it. */
export interface TranscriptLine {
  id: string
  /** The speaker (also the line's author): a Sew member id. */
  speaker: string
  text: string
  /** Ms from the recording's start, when Sew said. */
  startMs: number | null
}

export function parseTranscriptLine(v: unknown): TranscriptLine | null {
  const o = v as Record<string, unknown> | null
  const id = idOf(o?.id)
  if (!o || !id) return null
  const t = (o.transcript ?? {}) as Record<string, unknown>
  return {
    id,
    speaker: idOf(t.speaker_id) ?? idOf(o.author_id) ?? '',
    text: typeof o.body === 'string' ? o.body : '',
    startMs: typeof t.start_ms === 'number' ? t.start_ms : null,
  }
}

/** The transcript panel's heading: "Transcript · 12:04 · Nov 17, 2:41 PM"
 *  (how long it recorded, and when it started: the card's time). */
export function transcriptHeading(card: { recording?: HomeChatRecordingSummary | null; createdAt: number } | null): string {
  const parts = ['Transcript']
  const r = card?.recording
  if (r?.stopped && r.durationMs !== null) parts.push(formatCallDuration(r.durationMs))
  if (card && card.createdAt > 0) {
    const at = new Date(card.createdAt * 1000)
    parts.push(`${at.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}, ${at.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}`)
  }
  return parts.join(' · ')
}

/** A speaker turn as the transcript panel draws it: one person's lines in
 *  a row, under their name and the time into the recording. */
export interface TranscriptTurn {
  id: string
  speaker: string
  /** Ms from the recording's start of its first line, when Sew said. */
  startMs: number | null
  lines: TranscriptLine[]
}

/** The lines as turns: a speaker's lines in a row join one turn. */
export function transcriptTurns(lines: readonly TranscriptLine[]): TranscriptTurn[] {
  const turns: TranscriptTurn[] = []
  for (const l of lines) {
    const last = turns[turns.length - 1]
    if (last && last.speaker === l.speaker) last.lines.push(l)
    else turns.push({ id: l.id, speaker: l.speaker, startMs: l.startMs, lines: [l] })
  }
  return turns
}

/** The time into the recording a line was said ("4:12"), or null. */
export function transcriptTime(startMs: number | null): string | null {
  return startMs === null ? null : formatCallDuration(startMs)
}

/** The text pieces of `text`, with each case-blind match of `query` marked. */
export function transcriptMatches(text: string, query: string): { text: string; hit: boolean }[] {
  const q = query.trim().toLowerCase()
  if (!q) return [{ text, hit: false }]
  const out: { text: string; hit: boolean }[] = []
  const low = text.toLowerCase()
  let at = 0
  for (let i = low.indexOf(q); i !== -1; i = low.indexOf(q, at)) {
    if (i > at) out.push({ text: text.slice(at, i), hit: false })
    out.push({ text: text.slice(i, i + q.length), hit: true })
    at = i + q.length
  }
  if (at < text.length) out.push({ text: text.slice(at), hit: false })
  return out
}

/** "Copy transcript": a heading line, then each turn as "Alex (4:12): …". */
export function transcriptCopyText(heading: string, lines: readonly TranscriptLine[], nameOf: (memberId: string) => string): string {
  const body = transcriptTurns(lines).map((t) => {
    const time = transcriptTime(t.startMs)
    return `${nameOf(t.speaker)}${time ? ` (${time})` : ''}: ${t.lines.map((l) => l.text).join(' ')}`
  })
  return [heading, '', ...body].join('\n')
}

/** `GET /cli/home-chat/replies?conv=&id=`: a card's transcript lines, oldest
 *  first, as the person. */
export async function fetchHomeChatTranscript(scope: ServerScope, conv: string, cardId: string): Promise<TranscriptLine[]> {
  const v = await daemonCliGet<{ page?: unknown }>(scope, 'home-chat/replies', { conv, id: cardId })
  const page = v?.page as { items?: unknown } | unknown[] | undefined
  const items = Array.isArray(page) ? page : Array.isArray((page as { items?: unknown })?.items) ? ((page as { items: unknown[] }).items) : []
  const lines = items.map(parseTranscriptLine).filter((l): l is TranscriptLine => l !== null)
  // Sew ids are decimal strings of any length: compare them as numbers.
  return lines.sort((a, b) => (a.id.length !== b.id.length ? a.id.length - b.id.length : a.id < b.id ? -1 : a.id > b.id ? 1 : 0))
}

// ── The DM call strip ──────────────────────────────────────────────────

/** The call's recording as the DM strip shows it (§2): Record or Stop only
 *  when Sew says this person may, and the badge the whole time one runs (its
 *  tooltip says who started it, and when). */
export interface DmCallRecording {
  /** A recording runs: the badge, with this tooltip. */
  badge: string | null
  /** When it started (epoch ms), for the badge's clock; null: not known. */
  startedAt?: number | null
  canRecord: boolean
  canStop: boolean
}

/** The DM strip's recording (prd-home-call-recording-v1 §2, HR-9): the same
 *  rule as the voice rooms' call bar (`recordingButtons`: the plugin says,
 *  a Voice & Chat only login never), and the badge while one runs. */
export function dmCallRecording(c: Pick<VoiceCall, 'recording' | 'recordingPerms' | 'recordingControls' | 'participants'>): DmCallRecording {
  const r = c.recording
  const b = recordingButtons({ perms: c.recordingPerms, recording: r, controls: c.recordingControls })
  const starter = r?.startedBy ? (c.participants.find((p) => p.memberId === r.startedBy)?.user ?? null) : null
  if (!r) return { badge: null, canRecord: b.canRecord, canStop: b.canStop }
  const startedAt = r.startedAt ?? null
  return { badge: recordingBadgeTitle(r, starter, true, startedAt), startedAt, canRecord: b.canRecord, canStop: b.canStop }
}

/** The DM's recording that runs now, from its cards: the newest card Sew
 *  hasn't stopped. It is the same for both people and on every device, so
 *  the DM's header says so without the call open here. `createdAt` is the
 *  card's time in seconds (Sew posts the card as the recording starts). */
export function liveDmRecording<M extends { recording?: HomeChatRecordingSummary | null; createdAt: number }>(
  messages: readonly M[] | null,
): { card: M; startedAt: number } | null {
  if (!messages) return null
  for (let i = messages.length - 1; i >= 0; i--) {
    const m = messages[i]
    if (m.recording) return m.recording.stopped ? null : { card: m, startedAt: m.createdAt * 1000 }
  }
  return null
}
