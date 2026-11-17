// Recording a DM call (prd-home-call-recording-v1 §2, HR-10): the card's
// words, what counts as a transcript line, and that a page's lines never
// join the timeline.

import { describe, expect, it } from 'vitest'
import { parseHomeChatMessage, parseHomeChatPage } from './api'
import { canShowTranscript, parseRecordingSummary, parseTranscriptLine, recordingCardLine, transcriptRootOf } from './recordings'

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
  it('a reply under the card or a message with transcript is a line; a plain message is not', () => {
    expect(transcriptRootOf({ id: '51', thread_root_id: '50' })).toBe('50')
    expect(transcriptRootOf({ id: '51', transcript: { recording_id: '42' } })).toBe('42')
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

  it('a line names its speaker from transcript, else its author', () => {
    expect(parseTranscriptLine({ id: '51', author_id: '7', body: 'x', transcript: { speaker_id: '8', start_ms: 900 } })).toEqual({ id: '51', speaker: '8', text: 'x', startMs: 900 })
    expect(parseTranscriptLine({ id: '51', author_id: '7', body: 'x' })?.speaker).toBe('7')
  })
})
