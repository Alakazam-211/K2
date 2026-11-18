// A voice room's recordings and transcripts: the gate asks once and hides on
// anything but a recordings page, the list reads newest first across pages,
// and the transcript is read a page at a time until the daemon gives no
// next cursor, within a cap.

import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: (...a: unknown[]) => h.get(...a) }))

import { DaemonHttpError } from '@/lib/daemon-http-error'
import {
  TRANSCRIPT_MAX_PAGES,
  fetchRoomRecordings,
  fetchRoomTranscript,
  newestFirst,
  parseRoomRecordings,
  probeRoomRecordings,
  resetRoomRecordingsGate,
  roomRecordingCardView,
  roomTranscriptHeading,
  transcriptRefusalLine,
  type RoomRecording,
} from './room-recordings'

const scope = (k = 'zed'): ServerScope => ({ connectionKey: `host:${k}` }) as unknown as ServerScope

const rec = (over: Partial<RoomRecording> = {}): RoomRecording => ({
  id: '42',
  room: 'room-1',
  roomName: 'Standup',
  state: 'stopped',
  trigger: 'manual',
  startedBy: '7',
  startedAt: Date.parse('2026-11-18T14:41:00'),
  stoppedAt: Date.parse('2026-11-18T14:41:00') + 724_000,
  transcriptState: 'done',
  transcriptReady: true,
  ...over,
})

beforeEach(() => {
  h.get.mockReset()
  resetRoomRecordingsGate()
})

describe('the gate', () => {
  it('a recordings page shows it; one request, shared while it runs', async () => {
    h.get.mockResolvedValue({ ok: true, recordings: [], next: null, canRead: false })
    const [a, b] = await Promise.all([probeRoomRecordings(scope()), probeRoomRecordings(scope())])
    expect([a, b]).toEqual([true, true])
    expect(h.get).toHaveBeenCalledTimes(1)
    expect(h.get).toHaveBeenCalledWith(scope(), 'voice/recordings', { limit: 1 })
  })

  it('hides on 404, 400, a refusal, a network error, a thrown string and a 200 that isn’t a list; never throws', async () => {
    const answers = [
      () => Promise.reject(new DaemonHttpError('nope', 404, null)),
      () => Promise.reject(new DaemonHttpError('bad', 400, null)),
      () => Promise.reject(new DaemonHttpError('off', 403, 'transcripts_read_off')),
      () => Promise.reject(new DaemonHttpError('members', 403, 'owner_or_admin_only')),
      () => Promise.reject(new TypeError('Failed to fetch')),
      () => Promise.reject('odd'),
      () => Promise.resolve({ recordings: 'no' }),
      () => Promise.resolve(null),
    ]
    for (const [i, answer] of answers.entries()) {
      h.get.mockImplementationOnce(answer)
      await expect(probeRoomRecordings(scope(`s${i}`))).resolves.toBe(false)
    }
    expect(h.get).toHaveBeenCalledTimes(answers.length)
  })

  it('a server without the route is asked once until reload; other answers are kept a minute', async () => {
    h.get.mockRejectedValue(new DaemonHttpError('nope', 404, null))
    await probeRoomRecordings(scope('old'), 0)
    await probeRoomRecordings(scope('old'), 10 * 60_000)
    expect(h.get).toHaveBeenCalledTimes(1)

    h.get.mockReset()
    h.get.mockRejectedValue(new DaemonHttpError('off', 403, 'transcripts_read_off'))
    await probeRoomRecordings(scope('new'), 0)
    await probeRoomRecordings(scope('new'), 30_000)
    expect(h.get).toHaveBeenCalledTimes(1)
    await probeRoomRecordings(scope('new'), 61_000)
    expect(h.get).toHaveBeenCalledTimes(2)
  })
})

describe('the list', () => {
  it('parses the daemon’s rows; times from RFC 3339', () => {
    const page = parseRoomRecordings({
      ok: true,
      recordings: [{ id: 9, room: 'room-1', roomName: 'Standup', state: 'stopped', trigger: 'auto', startedBy: null, startedAt: '2026-11-18T14:41:00Z', stoppedAt: '2026-11-18T14:53:04Z', transcriptReady: true, transcriptState: 'done' }, { nope: 1 }],
      next: '77',
      canRead: true,
    })
    expect(page?.recordings).toHaveLength(1)
    expect(page?.recordings[0]).toMatchObject({ id: '9', trigger: 'auto', startedAt: Date.parse('2026-11-18T14:41:00Z') })
    expect(page?.next).toBe('77')
    expect(page?.canRead).toBe(true)
  })

  it('reads the room’s pages newest first, stopping at no next', async () => {
    h.get
      .mockResolvedValueOnce({ recordings: [{ id: '1', startedAt: '2026-11-17T10:00:00Z' }], next: '5', canRead: true })
      .mockResolvedValueOnce({ recordings: [], next: '9', canRead: true })
      .mockResolvedValueOnce({ recordings: [{ id: '2', startedAt: '2026-11-18T10:00:00Z' }], next: null, canRead: true })
    const page = await fetchRoomRecordings(scope(), 'room-1')
    expect(page.recordings.map((r) => r.id)).toEqual(['2', '1'])
    expect(h.get).toHaveBeenCalledTimes(3)
    expect(h.get.mock.calls.map((c) => c[2])).toEqual([
      { room: 'room-1', limit: 200, after: undefined },
      { room: 'room-1', limit: 200, after: '5' },
      { room: 'room-1', limit: 200, after: '9' },
    ])
  })

  it('stops on a repeated cursor and at five pages', async () => {
    h.get.mockResolvedValue({ recordings: [], next: '5', canRead: true })
    await fetchRoomRecordings(scope(), 'room-1')
    expect(h.get).toHaveBeenCalledTimes(2)
    h.get.mockReset()
    let n = 0
    h.get.mockImplementation(() => Promise.resolve({ recordings: [], next: String(++n), canRead: true }))
    await fetchRoomRecordings(scope(), 'room-1')
    expect(h.get).toHaveBeenCalledTimes(5)
  })

  it('newest first puts one without a time last', () => {
    expect(newestFirst([rec({ id: 'a', startedAt: null }), rec({ id: 'b', startedAt: 1 }), rec({ id: 'c', startedAt: 2 })]).map((r) => r.id)).toEqual(['c', 'b', 'a'])
  })

  it('each row reads as the DM’s card: Ready, Transcribing, failed, live; who started it', () => {
    expect(roomRecordingCardView(rec(), 'Ada')).toEqual({ title: 'Recording · 12:04', detail: 'Started by Ada', state: 'ready', partial: false })
    expect(roomRecordingCardView(rec({ transcriptState: 'running', transcriptReady: false }), null)).toMatchObject({ state: 'transcribing', detail: 'Started with Record' })
    expect(roomRecordingCardView(rec({ transcriptState: 'failed' }), null).state).toBe('failed')
    expect(roomRecordingCardView(rec({ transcriptState: 'partial' }), null)).toMatchObject({ state: 'ready', partial: true })
    expect(roomRecordingCardView(rec({ state: 'active', stoppedAt: null }), null)).toMatchObject({ state: 'live', title: 'Recording' })
    expect(roomRecordingCardView(rec({ trigger: 'required' }), 'Ada').detail).toBe('Started by the server’s rule')
    // An older Sew: only transcriptReady.
    expect(roomRecordingCardView(rec({ transcriptState: null, transcriptReady: true }), null).state).toBe('ready')
  })

  it("Sew 0.10.5's transcriptFailure: read when it's there, the row says why; without it, as before", () => {
    const page = parseRoomRecordings({
      recordings: [
        { id: 1, state: 'stopped', transcriptState: 'failed', transcriptFailure: { code: 'unsupported_language', message: '' } },
        { id: 2, state: 'stopped', transcriptState: 'failed' },
        { id: 3, state: 'stopped', transcriptState: 'failed', transcriptFailure: 'audio_gone' },
      ],
    })
    expect(page?.recordings[0].transcriptFailure).toEqual({ code: 'unsupported_language', message: '' })
    expect(page?.recordings[1]).not.toHaveProperty('transcriptFailure')
    expect(page?.recordings[2]).not.toHaveProperty('transcriptFailure')
    const failed = (f: { code: string; message: string }) => roomRecordingCardView(rec({ transcriptState: 'failed', transcriptFailure: f }), null)
    expect(failed({ code: 'audio_gone', message: '' }).failWords).toBe('the audio was removed before it could be transcribed.')
    expect(failed({ code: 'transcriber_error', message: '' }).failWords).toBe('it couldn’t be made from this recording.')
    expect(failed({ code: 'new_code', message: 'Sew’s words.' }).failWords).toBe('Sew’s words.')
    expect(failed({ code: 'new_code', message: '' })).not.toHaveProperty('failWords')
    const partial = roomRecordingCardView(rec({ transcriptState: 'partial', transcriptFailure: { code: 'audio_not_captured', message: '' } }), null)
    expect(partial).toMatchObject({ state: 'ready', partial: true, note: 'Part of the call wasn’t captured, so this transcript is incomplete.' })
    expect(partial).not.toHaveProperty('failWords')
  })

  it('the transcript heading: "Transcript · 12:04 · Nov 18, 2:41 PM"', () => {
    expect(roomTranscriptHeading(rec())).toMatch(/^Transcript · 12:04 · Nov 18, 2:41\sPM$/)
  })
})

describe('the transcript', () => {
  const line = (id: string, speaker = 'owner', ext = 'k2:owner') => ({ id, speaker, speakerExternalId: ext, startMs: Number(id) * 1000, endMs: null, text: `line ${id}` })

  it('pages with after= the cursor until no next; speaker names from the lines', async () => {
    h.get
      .mockResolvedValueOnce({ ok: true, lines: [line('1'), line('2', 'ana', 'k2:connect:u2')], next: '2' })
      .mockResolvedValueOnce({ ok: true, lines: [line('3')], next: null })
    const t = await fetchRoomTranscript(scope(), '42')
    expect(t.lines.map((l) => l.id)).toEqual(['1', '2', '3'])
    expect(t.lines[1]).toEqual({ id: '2', speaker: 'k2:connect:u2', text: 'line 2', startMs: 2000 })
    expect(t.names.get('k2:connect:u2')).toBe('ana')
    expect(t.names.get('k2:owner')).toBe('owner')
    expect(t.truncated).toBe(false)
    expect(h.get.mock.calls.map((c) => [c[1], c[2]])).toEqual([
      ['voice/transcript', { id: '42', limit: 200, after: undefined }],
      ['voice/transcript', { id: '42', limit: 200, after: '2' }],
    ])
  })

  it('stops on an empty page, a repeated cursor, and at the cap', async () => {
    h.get.mockResolvedValueOnce({ lines: [], next: '9' })
    await fetchRoomTranscript(scope(), '42')
    expect(h.get).toHaveBeenCalledTimes(1)

    h.get.mockReset()
    h.get.mockResolvedValue({ lines: [line('1')], next: '1' })
    await fetchRoomTranscript(scope(), '42')
    expect(h.get).toHaveBeenCalledTimes(2)

    h.get.mockReset()
    let n = 0
    h.get.mockImplementation(() => {
      n += 1
      return Promise.resolve({ lines: [line(String(n))], next: String(n) })
    })
    const t = await fetchRoomTranscript(scope(), '42')
    expect(h.get).toHaveBeenCalledTimes(TRANSCRIPT_MAX_PAGES)
    expect(t.truncated).toBe(true)
  })

  it('says each refusal in plain words', () => {
    expect(transcriptRefusalLine(new DaemonHttpError('x', 403, 'owner_token_cannot_read'))).toContain('Sign in to this server with your own login')
    expect(transcriptRefusalLine(new DaemonHttpError('x', 403, 'transcripts_read_off'))).toBe('Only this server’s owner and Admins can read this transcript.')
    expect(transcriptRefusalLine(new DaemonHttpError('x', 404, 'not_found'))).toBe('This recording isn’t on this server any more.')
    expect(transcriptRefusalLine(new Error('boom'))).toBe('Couldn’t load the transcript (boom).')
  })
})
