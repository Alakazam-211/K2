// A voice room's recordings and transcripts, read from the server that hosts
// the room (its own login, `scopeForHost`). Two daemon routes, both there
// since 0.46.11:
//
// - `GET /cli/voice/recordings?room=&after=&limit=`: the rooms' recordings
//   (id, room, when, what started it, the transcript's state; no text), and
//   `canRead`: whether this login may read the text.
// - `GET /cli/voice/transcript?id=&after=&limit=`: one recording's lines, a
//   page at a time (`next` is the cursor for `after`). Every read is logged.
//
// The owner's and Admins' own logins read; the bare owner token lists but
// never reads (`owner_token_cannot_read`); others get `transcripts_read_off`
// or `owner_or_admin_only`. A server without the routes (older than 0.46.11)
// answers 404, and one without Sew 0.9.0 a 409: the headset's list then shows
// no Recordings entry at all. The gate is asked once per open of the list,
// never in a loop (`probeRoomRecordings`).

import type { ServerScope } from '@/kessel/server-scope'
import { daemonCliGet } from '@/lib/daemon-cli'
import { httpCodeOf, httpStatusOf } from '@/lib/daemon-http-error'
import { formatCallDuration } from '@/lib/home-chat/calls'
import { withTranscriptFailure, type RecordingCardView, type TranscriptLine } from '@/lib/home-chat/recordings'
import { parseTranscriptFailure, type TranscriptFailure } from '@/lib/home-chat/recording-failure'

export const RECORDINGS_ROUTE = 'voice/recordings'
export const TRANSCRIPT_ROUTE = 'voice/transcript'

/** Sew's largest page for both routes. */
export const RECORDINGS_PAGE = 200
export const TRANSCRIPT_PAGE = 200
/** At most this many pages a read: the room filter runs after Sew's page, so
 *  a page can hold none of the room's recordings and still have a next. */
export const RECORDINGS_MAX_PAGES = 5
/** 40 pages of 200 lines: hours of talk. Past it the panel shows what came. */
export const TRANSCRIPT_MAX_PAGES = 40

// ── the gate ──────────────────────────────────────────────────────────

/** A settled answer is kept this long, so opening and closing the list a few
 *  times asks once. A server without the routes is remembered until reload. */
const GATE_TTL_MS = 60_000

type Gate = { shown: boolean; at: number; forever: boolean }
const gates = new Map<string, Gate>()
const inFlight = new Map<string, Promise<boolean>>()

/** For tests. */
export function resetRoomRecordingsGate(): void {
  gates.clear()
  inFlight.clear()
}

/** The answer of a server that doesn't know the route at all: no K2 code in
 *  the body (a daemon's own refusals always carry one), or 404/405. */
function routeUnknown(err: unknown): boolean {
  const status = httpStatusOf(err)
  if (status === 404 || status === 405) return httpCodeOf(err) === null || httpCodeOf(err) === 'not_found'
  return status === 400 && httpCodeOf(err) === null
}

/** May this login list the rooms' recordings on `scope`'s server? One
 *  request (`limit=1`), shared by every caller while it runs. Any error, any
 *  refusal and any answer that isn't a recordings page mean no. Never
 *  throws, never retries. */
export function probeRoomRecordings(scope: ServerScope, now: number = Date.now()): Promise<boolean> {
  const key = scope.connectionKey
  const kept = gates.get(key)
  if (kept && (kept.forever || now - kept.at < GATE_TTL_MS)) return Promise.resolve(kept.shown)
  const running = inFlight.get(key)
  if (running) return running
  const p = daemonCliGet<unknown>(scope, RECORDINGS_ROUTE, { limit: 1 }).then(
    (v) => {
      const shown = parseRoomRecordings(v) !== null
      gates.set(key, { shown, at: now, forever: false })
      return shown
    },
    (err: unknown) => {
      gates.set(key, { shown: false, at: now, forever: routeUnknown(err) })
      return false
    },
  )
  const settled = p.finally(() => inFlight.delete(key))
  inFlight.set(key, settled)
  return settled
}

// ── the list ──────────────────────────────────────────────────────────

/** One recording as the daemon lists it. */
export interface RoomRecording {
  id: string
  /** The room's slug (`room-1`). */
  room: string | null
  roomName: string | null
  /** Sew's `active`, `stopped`, … */
  state: string
  /** `manual` (someone pressed Record), `auto` or `required` (the server's rule). */
  trigger: string
  /** Sew's member id of whoever pressed Record. */
  startedBy: string | null
  /** Epoch ms, or null when Sew's time didn't parse. */
  startedAt: number | null
  stoppedAt: number | null
  /** `pending`, `running`, `done`, `partial` or `failed`; null from an older Sew. */
  transcriptState: string | null
  transcriptReady: boolean
  /** Why the transcript failed or has gaps (Sew 0.10.5's
   *  `MgmtRecordingSummary.transcript_failure`; absent from an older Sew). */
  transcriptFailure?: TranscriptFailure
}

export interface RoomRecordingsPage {
  recordings: RoomRecording[]
  next: string | null
  /** This login may read the text (an owner's or Admin's own login). */
  canRead: boolean
}

function str(v: unknown): string | null {
  if (typeof v === 'string' && v) return v
  if (typeof v === 'number' && Number.isFinite(v)) return String(v)
  return null
}

/** Sew's time: RFC 3339, or epoch seconds or ms. */
export function timeOf(v: unknown): number | null {
  if (typeof v === 'number' && Number.isFinite(v) && v > 0) return v < 1e12 ? v * 1000 : v
  if (typeof v !== 'string' || !v) return null
  if (/^\d+$/.test(v)) return timeOf(Number(v))
  const t = Date.parse(v)
  return Number.isFinite(t) ? t : null
}

function parseRecording(v: unknown): RoomRecording | null {
  const o = v as Record<string, unknown> | null
  const id = str(o?.id)
  if (!o || !id) return null
  const failure = parseTranscriptFailure(o.transcriptFailure)
  return {
    id,
    room: str(o.room),
    roomName: str(o.roomName),
    state: str(o.state) ?? '',
    trigger: str(o.trigger) ?? 'manual',
    startedBy: str(o.startedBy),
    startedAt: timeOf(o.startedAt),
    stoppedAt: timeOf(o.stoppedAt),
    transcriptState: str(o.transcriptState),
    transcriptReady: o.transcriptReady === true,
    ...(failure ? { transcriptFailure: failure } : {}),
  }
}

/** A recordings page, or null for anything else (an older server's shape). */
export function parseRoomRecordings(v: unknown): RoomRecordingsPage | null {
  const o = v as Record<string, unknown> | null
  if (!o || typeof o !== 'object' || !Array.isArray(o.recordings)) return null
  return {
    recordings: o.recordings.map(parseRecording).filter((r): r is RoomRecording => r !== null),
    next: str(o.next),
    canRead: o.canRead === true,
  }
}

/** Newest first; one without a time last. */
export function newestFirst(list: readonly RoomRecording[]): RoomRecording[] {
  return [...list].sort((a, b) => (b.startedAt ?? -Infinity) - (a.startedAt ?? -Infinity))
}

/** `room`'s recordings, newest first, at most `RECORDINGS_MAX_PAGES` pages.
 *  Throws the daemon's refusal. */
export async function fetchRoomRecordings(scope: ServerScope, room: string): Promise<RoomRecordingsPage> {
  const all: RoomRecording[] = []
  let canRead = false
  let after: string | null = null
  let next: string | null = null
  for (let i = 0; i < RECORDINGS_MAX_PAGES; i++) {
    const page = parseRoomRecordings(await daemonCliGet<unknown>(scope, RECORDINGS_ROUTE, { room, limit: RECORDINGS_PAGE, after: after ?? undefined }))
    if (!page) throw new Error('The server answered something that isn’t a recordings list.')
    all.push(...page.recordings)
    canRead = page.canRead
    next = page.next
    if (!next || next === after) break
    after = next
  }
  const seen = new Set<string>()
  const recordings = newestFirst(all.filter((r) => (seen.has(r.id) ? false : (seen.add(r.id), true))))
  return { recordings, next, canRead }
}

// ── the row ───────────────────────────────────────────────────────────

/** The transcript is there to read (Sew's `done` or `partial`, or an older
 *  Sew's `transcriptReady`). */
export function transcriptReadable(r: RoomRecording): boolean {
  if (r.transcriptState) return r.transcriptState === 'done' || r.transcriptState === 'partial'
  return r.transcriptReady
}

function stopped(r: RoomRecording): boolean {
  return r.stoppedAt !== null || (r.state !== '' && r.state !== 'active' && r.state !== 'recording')
}

export function recordingLengthMs(r: RoomRecording): number | null {
  return r.startedAt !== null && r.stoppedAt !== null && r.stoppedAt >= r.startedAt ? r.stoppedAt - r.startedAt : null
}

/** The DM's recording card, for a room's recording. `starter`: the shown
 *  name of who pressed Record, when known. */
export function roomRecordingCardView(r: RoomRecording, starter: string | null): RecordingCardView {
  const len = recordingLengthMs(r)
  const detail =
    r.trigger === 'auto' || r.trigger === 'required'
      ? 'Started by the server’s rule'
      : starter
        ? `Started by ${starter}`
        : 'Started with Record'
  const t = r.transcriptState
  const state: RecordingCardView['state'] = !stopped(r)
    ? 'live'
    : transcriptReadable(r)
      ? 'ready'
      : t === 'failed'
        ? 'failed'
        : 'transcribing'
  return withTranscriptFailure(
    {
      title: stopped(r) && len !== null ? `Recording · ${formatCallDuration(len)}` : 'Recording',
      detail,
      state,
      partial: t === 'partial',
    },
    r.transcriptFailure,
  )
}

/** "Nov 18, 2:41 PM". */
export function recordingWhen(ms: number | null): string {
  if (ms === null) return 'Time not known'
  const at = new Date(ms)
  return `${at.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}, ${at.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })}`
}

/** "Transcript · 12:04 · Nov 18, 2:41 PM". */
export function roomTranscriptHeading(r: RoomRecording): string {
  const parts = ['Transcript']
  const len = recordingLengthMs(r)
  if (len !== null) parts.push(formatCallDuration(len))
  if (r.startedAt !== null) parts.push(recordingWhen(r.startedAt))
  return parts.join(' · ')
}

// ── the transcript ────────────────────────────────────────────────────

export interface RoomTranscript {
  lines: TranscriptLine[]
  /** Speaker key → the name the daemon gave (`owner`, a login, an id). */
  names: Map<string, string>
  /** Stopped at TRANSCRIPT_MAX_PAGES with more to come. */
  truncated: boolean
}

/** Every line of recording `id`, oldest first, a page at a time until the
 *  daemon gives no next (or the same one twice), at most
 *  `TRANSCRIPT_MAX_PAGES` pages. Throws the daemon's refusal. */
export async function fetchRoomTranscript(scope: ServerScope, id: string): Promise<RoomTranscript> {
  const lines: TranscriptLine[] = []
  const names = new Map<string, string>()
  let after: string | null = null
  for (let i = 0; i < TRANSCRIPT_MAX_PAGES; i++) {
    const v = await daemonCliGet<{ lines?: unknown; next?: unknown }>(scope, TRANSCRIPT_ROUTE, { id, limit: TRANSCRIPT_PAGE, after: after ?? undefined })
    const page = Array.isArray(v?.lines) ? v.lines : []
    for (const raw of page) {
      const o = raw as Record<string, unknown> | null
      const lineId = str(o?.id)
      if (!o || !lineId) continue
      const name = str(o.speaker) ?? 'Someone'
      const key = str(o.speakerExternalId) ?? name
      names.set(key, name)
      lines.push({ id: lineId, speaker: key, text: typeof o.text === 'string' ? o.text : '', startMs: typeof o.startMs === 'number' ? o.startMs : null })
    }
    const next = str(v?.next)
    if (!next || next === after || page.length === 0) return { lines, names, truncated: false }
    after = next
  }
  return { lines, names, truncated: true }
}

/** What the transcript panel says when a read is refused, in plain words. */
export function transcriptRefusalLine(err: unknown): string {
  switch (httpCodeOf(err)) {
    case 'owner_token_cannot_read':
      return 'This computer’s owner key can’t read transcripts. Sign in to this server with your own login to read it.'
    case 'transcripts_read_off':
    case 'owner_or_admin_only':
      return 'Only this server’s owner and Admins can read this transcript.'
    case 'not_found':
      return 'This recording isn’t on this server any more.'
    case 'sew_feature_missing':
      return 'This server’s chat can’t hand out transcripts yet.'
    default: {
      const msg = err instanceof Error ? err.message : String(err)
      return `Couldn’t load the transcript (${msg}).`
    }
  }
}
