// Recording a DM call (prd-home-call-recording-v1 §2, HR-10): the card's
// words, what counts as a transcript line, and that a page's lines never
// join the timeline.

import { describe, expect, it, vi } from 'vitest'

const cli = vi.hoisted(() => ({ get: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: cli.get }))

import type { ServerScope } from '@/kessel/server-scope'
import { parseHomeChatMessage, parseHomeChatPage } from './api'
import { canShowTranscript, fetchDmRecordings, fetchHomeChatTranscript, parseRecordingSummary, parseTranscriptLine, recordingCardLine, transcriptRootOf } from './recordings'

const summary = (over: Record<string, unknown> = {}) =>
  parseRecordingSummary({
    recording_id: '42',
    started_by: '7',
    auto: false,
    stopped_at: '2026-11-14T10:12:04Z',
    stop_reason: 'stopped',
    duration_ms: 724_000,
    transcript: { state: 'done', turns: 3 },
    audio: { state: 'available', keep_days: 30 },
    ...over,
  })!

describe('the recording card', () => {
  it("reads as the PRD's line", () => {
    expect(recordingCardLine(summary(), 'Alex')).toBe('Recording · 12:04 · started by Alex · transcript ready · audio kept 30 days')
    expect(recordingCardLine(summary({ stopped_at: null, stop_reason: null, transcript: { state: 'running', turns: 1 } }), 'you')).toBe(
      'Recording in progress · started by you · transcript coming · audio kept 30 days',
    )
    expect(recordingCardLine(summary({ audio: { state: 'available', keep_days: 0 } }), 'Alex')).toContain('audio deleted once the transcript is final')
    expect(recordingCardLine(summary({ audio: { state: 'expired', keep_days: 30 } }), 'Alex')).toContain('audio deleted')
    expect(recordingCardLine(summary({ audio: { state: 'available', keep_days: 1 } }), null)).toContain('audio kept 1 day')
    expect(recordingCardLine(summary({ transcript: { state: 'failed', turns: 0 } }), 'Alex')).toContain('no transcript')
  })

  it('Show transcript once there can be lines', () => {
    expect(canShowTranscript(summary())).toBe(true)
    expect(canShowTranscript(summary({ transcript: { state: 'pending', turns: 0 } }))).toBe(false)
    expect(canShowTranscript(summary({ transcript: { state: 'failed', turns: 0 } }))).toBe(false)
  })

  it('a message with recording parses as a card, not a bubble line', () => {
    const m = parseHomeChatMessage({ id: '50', channel_id: '900', author_id: '7', body: '', recording: { recording_id: '42', started_by: '7' } })
    expect(m?.recording?.recordingId).toBe('42')
    expect(m?.transcriptOf).toBeNull()
  })
})

describe('transcript lines (HR-10)', () => {
  it('a message with a transcript object is a line; a plain message or thread reply is not', () => {
    expect(transcriptRootOf({ id: '51', thread_root_id: '50', transcript: { recording_id: '42' } })).toBe('50')
    expect(transcriptRootOf({ id: '51', transcript: { recording_id: '42' } })).toBe('42')
    // A person's ordinary thread reply (no `transcript`) is a message, not a line.
    expect(transcriptRootOf({ id: '54', thread_root_id: '50' })).toBeNull()
    expect(transcriptRootOf({ id: '54', thread_root_id: '50', transcript: null })).toBeNull()
    expect(transcriptRootOf({ id: '52', reference: { kind: 'reply', message_id: '50' } })).toBeNull()
    expect(transcriptRootOf({ id: '53' })).toBeNull()
  })

  it('a page never puts a line on the timeline', () => {
    const p = parseHomeChatPage({
      page: {
        items: [
          { id: '49', channel_id: '900', body: 'hi' },
          { id: '50', channel_id: '900', recording: { recording_id: '42' } },
          { id: '51', channel_id: '900', body: 'a line', thread_root_id: '50', transcript: { recording_id: '42' } },
        ],
      },
    })
    expect(p.messages.map((m) => m.id)).toEqual(['49', '50'])
  })

  it('a plain thread reply (thread_root_id, no transcript) stays on the timeline', () => {
    const p = parseHomeChatPage({
      page: {
        items: [
          { id: '49', channel_id: '900', body: 'hi' },
          { id: '55', channel_id: '900', body: 'a reply from the phone', thread_root_id: '49' },
        ],
      },
    })
    expect(p.messages.map((m) => m.id)).toEqual(['49', '55'])
    expect(p.messages[1].transcriptOf).toBeNull()
  })

  it('a line names its speaker from transcript, else its author', () => {
    expect(parseTranscriptLine({ id: '51', author_id: '7', body: 'x', transcript: { speaker_id: '8', start_ms: 900 } })).toEqual({ id: '51', speaker: '8', text: 'x', startMs: 900 })
    expect(parseTranscriptLine({ id: '51', author_id: '7', body: 'x' })?.speaker).toBe('7')
  })

  it('a message with transcript is a line (its root the card); a plain message or a plain thread reply is not', () => {
    expect(transcriptRootOf({ id: '51', thread_root_id: '50', transcript: { recording_id: '42' } })).toBe('50')
    expect(transcriptRootOf({ id: '51', transcript: { recording_id: '42' } })).toBe('42')
    // A person's reply in a Sew thread (from Sew's own app): no transcript, an ordinary message.
    expect(transcriptRootOf({ id: '51', thread_root_id: '50' })).toBeNull()
    expect(transcriptRootOf({ id: '52', reference: { kind: 'reply', message_id: '50' } })).toBeNull()
    expect(transcriptRootOf({ id: '53' })).toBeNull()
  })
  it('a plain thread reply in a page stays on the timeline', () => {
    const p = parseHomeChatPage({
      page: {
        items: [
          { id: '49', channel_id: '900', body: 'hi' },
          { id: '51', channel_id: '900', body: 'a reply from Sew', thread_root_id: '49' },
          { id: '52', channel_id: '900', body: 'a turn', thread_root_id: '50', transcript: { recording_id: '42' } },
        ],
      },
    })
    expect(p.messages.map((m) => m.id)).toEqual(['49', '51'])
    expect(p.messages[1].transcriptOf).toBeNull()
  })
  it("Show transcript reads only the card's turns, not a plain reply under it", async () => {
    cli.get.mockResolvedValueOnce({
      ok: true,
      page: {
        items: [
          { id: '53', channel_id: '900', author_id: '8', body: 'second', thread_root_id: '50', transcript: { speaker_id: '8', start_ms: 900 } },
          { id: '52', channel_id: '900', author_id: '7', body: 'nice call!', thread_root_id: '50' },
          { id: '51', channel_id: '900', author_id: '7', body: 'first', thread_root_id: '50', transcript: { speaker_id: '7', start_ms: 0 } },
        ],
      },
    })
    const lines = await fetchHomeChatTranscript({} as ServerScope, '900', '50')
    expect(cli.get).toHaveBeenCalledWith({}, 'home-chat/replies', { conv: '900', id: '50' })
    expect(lines.map((l) => l.id)).toEqual(['51', '53'])
  })
})

describe("a DM's recordings (Sew 0.11.3's client list, as the member)", () => {
  it('reads GET /cli/home-chat/recordings: newest first, each with its card, and the next page only while Sew says more', async () => {
    cli.get.mockResolvedValueOnce({
      ok: true,
      conv: '900',
      page: {
        items: [
          { id: '77', channel_id: '900', state: 'ready', started_by: '7', started_at: '2026-11-17T14:41:00.000Z', stopped_at: '2026-11-17T14:53:04.000Z', message_id: '50', audio_keep_days: 30 },
          { id: '76', channel_id: '900', state: 'recording', started_by: 8, message_id: 40, audio_keep_days: 0 },
          { channel_id: '900', state: 'ready' },
        ],
        has_more: true,
      },
    })
    const p = await fetchDmRecordings({} as ServerScope, '900')
    expect(cli.get).toHaveBeenLastCalledWith({}, 'home-chat/recordings', { conv: '900' })
    expect(p.items).toEqual([
      { recordingId: '77', cardId: '50', state: 'ready', startedBy: '7', startedAt: Date.parse('2026-11-17T14:41:00.000Z'), stoppedAt: Date.parse('2026-11-17T14:53:04.000Z'), audioKeepDays: 30 },
      { recordingId: '76', cardId: '40', state: 'recording', startedBy: '8', startedAt: null, stoppedAt: null, audioKeepDays: 0 },
    ])
    expect(p.before).toBe('76')
    cli.get.mockResolvedValueOnce({ ok: true, conv: '900', page: { items: [], has_more: false } })
    const next = await fetchDmRecordings({} as ServerScope, '900', '76')
    expect(cli.get).toHaveBeenLastCalledWith({}, 'home-chat/recordings', { conv: '900', before: '76' })
    expect(next).toEqual({ items: [], before: null })
  })
})
