// @vitest-environment jsdom
// A DM call's recording in the DM: the header chip, the recording card in
// each state (recording now, transcribing, ready, failed; never playback,
// download or delete), and the transcript panel (speaker turns, search with
// the matches marked, Copy transcript, Close).

import { afterEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { HomeChatRecordingCard, HomeChatRecordingChip, HomeChatTranscriptPanel, TRANSCRIPT_FAILED_LINE } from './HomeChatRecording'
import {
  liveDmRecording,
  liveDmRecordingTitle,
  parseRecordingSummary,
  recordingCardView,
  transcriptCopyText,
  transcriptHeading,
  transcriptMatches,
  transcriptTurns,
  type HomeChatRecordingSummary,
  type TranscriptLine,
} from '@/lib/home-chat/recordings'
import { parseHomeChatMessage } from '@/lib/home-chat/api'

afterEach(cleanup)

const summary = (over: Partial<HomeChatRecordingSummary> = {}): HomeChatRecordingSummary => ({
  recordingId: '42',
  startedBy: '7',
  auto: false,
  stopped: true,
  durationMs: 724_000,
  transcriptState: 'done',
  turns: 2,
  audioState: 'available',
  keepDays: 30,
  ...over,
})

describe('the recording card', () => {
  it('reads "Recording · 12:04", then "Started by Alex · audio kept 30 days"', () => {
    const v = recordingCardView(summary(), 'Alex')
    expect(v).toEqual({ title: 'Recording · 12:04', detail: 'Started by Alex · audio kept 30 days', state: 'ready', partial: false })
    expect(recordingCardView(summary({ keepDays: 0 }), 'you').detail).toBe('Started by you · audio deleted once the transcript is final')
    expect(recordingCardView(summary({ stopped: false, durationMs: null, transcriptState: 'pending' }), 'Alex')).toMatchObject({ title: 'Recording', state: 'live' })
    expect(recordingCardView(summary({ transcriptState: 'running' }), 'Alex').state).toBe('transcribing')
    expect(recordingCardView(summary({ transcriptState: 'pending' }), 'Alex').state).toBe('transcribing')
    expect(recordingCardView(summary({ transcriptState: 'failed' }), 'Alex').state).toBe('failed')
    expect(recordingCardView(summary({ transcriptState: 'partial' }), 'Alex')).toMatchObject({ state: 'ready', partial: true })
  })

  it('each state: transcribing with a spinner, ready with Show transcript, failed in muted words', () => {
    const show = vi.fn()
    const { rerender } = render(<HomeChatRecordingCard view={recordingCardView(summary({ transcriptState: 'running' }), 'Alex')} timeLabel="2m" />)
    const card = screen.getByTestId('home-chat-recording-card')
    expect(card.getAttribute('data-state')).toBe('transcribing')
    expect(screen.getByTestId('home-chat-recording-card-title').textContent).toBe('Recording · 12:04')
    expect(screen.getByTestId('home-chat-recording-card-detail').textContent).toBe('Started by Alex · audio kept 30 days')
    expect(card.textContent).toContain('Transcript in a minute or two')
    expect(card.querySelector('[data-k2-spinner]')).toBeTruthy()
    expect(screen.queryByTestId('home-chat-show-transcript')).toBeNull()

    rerender(<HomeChatRecordingCard view={recordingCardView(summary(), 'Alex')} timeLabel="2m" onShowTranscript={show} />)
    fireEvent.click(screen.getByTestId('home-chat-show-transcript'))
    expect(show).toHaveBeenCalledTimes(1)

    rerender(<HomeChatRecordingCard view={recordingCardView(summary({ transcriptState: 'failed' }), 'Alex')} timeLabel="2m" />)
    expect(screen.getByTestId('home-chat-recording-card-failed').textContent).toBe(TRANSCRIPT_FAILED_LINE)
    rerender(<HomeChatRecordingCard view={recordingCardView(summary({ transcriptState: 'failed' }), 'Alex')} timeLabel="2m" failReason="the recorder left early" />)
    expect(screen.getByTestId('home-chat-recording-card-failed').textContent).toBe('No transcript: the recorder left early')

    rerender(<HomeChatRecordingCard view={recordingCardView(summary({ stopped: false }), 'Alex')} timeLabel="now" />)
    expect(screen.getByTestId('home-chat-recording-card').getAttribute('data-state')).toBe('live')
    expect(screen.getByTestId('home-chat-recording-card').textContent).toContain('Recording now')
  })

  it('never offers playback, download or delete, in any state', () => {
    for (const t of ['pending', 'running', 'done', 'partial', 'failed']) {
      const { unmount } = render(<HomeChatRecordingCard view={recordingCardView(summary({ transcriptState: t }), 'Alex')} timeLabel="2m" onShowTranscript={() => {}} />)
      const text = screen.getByTestId('home-chat-recording-card').textContent ?? ''
      expect(text).not.toMatch(/play|download|delete/i)
      expect(document.querySelector('audio')).toBeNull()
      unmount()
    }
  })
})

describe("the DM header's chip", () => {
  it('says Recording, with who started it in the tooltip, and no clock', () => {
    render(<HomeChatRecordingChip title="Alex started recording at 2:41 PM" />)
    const chip = screen.getByTestId('home-chat-header-recording')
    expect(chip.textContent).toBe('Recording')
    expect(chip.getAttribute('title')).toBe('Alex started recording at 2:41 PM')
  })

  it('a running recording is the newest card Sew has not stopped, the same for both people', () => {
    const live = parseRecordingSummary({ recording_id: '42', started_by: '7' })
    const done = parseRecordingSummary({ recording_id: '41', started_by: '7', stopped_at: '2026-11-17T14:00:00Z', duration_ms: 1000 })
    expect(liveDmRecording([{ id: '1', createdAt: 100, recording: done }])).toBeNull()
    const got = liveDmRecording([
      { id: '1', createdAt: 100, recording: done },
      { id: '2', createdAt: 200, recording: live },
      { id: '3', createdAt: 300, recording: null },
    ])
    expect(got?.card.id).toBe('2')
    expect(got?.startedAt).toBe(200_000)
    expect(liveDmRecording(null)).toBeNull()
  })

  it("Sew's start time on the card (Sew 0.10.5) is the running recording's start; without it, the card's time", () => {
    const at = '2026-11-17T14:41:00Z'
    const live = parseRecordingSummary({ recording_id: '42', started_by: '7', started_at: at })
    expect(liveDmRecording([{ id: '2', createdAt: 200, recording: live }])?.startedAt).toBe(Date.parse(at))
    // Not a date: as if Sew never said.
    const bad = parseRecordingSummary({ recording_id: '42', started_by: '7', started_at: 'yesterday' })
    expect(bad).not.toHaveProperty('startedAt')
    expect(liveDmRecording([{ id: '2', createdAt: 200, recording: bad }])?.startedAt).toBe(200_000)
  })

  it("the header's tooltip from the card: who started it, and how long the audio is kept when the card says", () => {
    const at = new Date(2026, 10, 17, 14, 41).getTime()
    expect(liveDmRecordingTitle(summary({ stopped: false, keepDays: 30 }), 'Alex', at)).toMatch(/^Alex started recording at 2:41\sPM\nAudio kept 30 days$/)
    expect(liveDmRecordingTitle(summary({ stopped: false, keepDays: 0 }), 'Alex', at)).toMatch(/\nAudio deleted once the transcript is final$/)
    expect(liveDmRecordingTitle(summary({ stopped: false, auto: true, startedBy: null, keepDays: 1 }), null, at)).toMatch(/^This server started recording at 2:41\sPM: it records its calls\nAudio kept 1 day$/)
    expect(liveDmRecordingTitle(summary({ stopped: false, keepDays: null }), 'Alex', at)).toMatch(/^Alex started recording at 2:41\sPM$/)
    expect(liveDmRecordingTitle(undefined, null, at)).toMatch(/^Recording started at 2:41\sPM$/)
  })
})

describe("the card's reason (Sew 0.10.5's transcript_failure)", () => {
  const failed = (failure: unknown) => recordingCardView(parseRecordingSummary({ recording_id: '42', started_by: '7', stopped_at: '2026-11-17T14:00:00Z', duration_ms: 724_000, transcript: { state: 'failed' } }, failure)!, 'Alex')
  const failedLine = (failure: unknown): string => {
    const { unmount } = render(<HomeChatRecordingCard view={failed(failure)} timeLabel="2m" />)
    const text = screen.getByTestId('home-chat-recording-card-failed').textContent ?? ''
    unmount()
    return text
  }

  it("reads the record message's transcript_failure onto its card", () => {
    const m = parseHomeChatMessage({ id: '9', channel_id: '3', author_id: '1', created_at: '2026-11-17T14:00:00Z', recording: { recording_id: '42', stopped_at: '2026-11-17T14:00:00Z', transcript: { state: 'failed' } }, transcript_failure: { code: 'audio_gone', message: '' } })
    expect(m?.recording?.failure).toEqual({ code: 'audio_gone', message: '' })
    const old = parseHomeChatMessage({ id: '9', channel_id: '3', author_id: '1', created_at: '2026-11-17T14:00:00Z', recording: { recording_id: '42', transcript: { state: 'failed' } } })
    expect(old?.recording).not.toHaveProperty('failure')
  })

  it('each code in our words after "No transcript: "', () => {
    expect(failedLine({ code: 'audio_gone', message: 'x' })).toBe('No transcript: the audio was removed before it could be transcribed.')
    expect(failedLine({ code: 'unsupported_language', message: 'x' })).toBe('No transcript: this language isn’t supported yet.')
    expect(failedLine({ code: 'transcriber_error', message: 'x' })).toBe('No transcript: it couldn’t be made from this recording.')
  })

  it("an unknown code says Sew's message; an empty message or no field keeps the plain line", () => {
    expect(failedLine({ code: 'something_new', message: 'the transcriber was turned off.' })).toBe('No transcript: the transcriber was turned off.')
    expect(failedLine({ code: 'something_new', message: '' })).toBe(TRANSCRIPT_FAILED_LINE)
    expect(failedLine(undefined)).toBe(TRANSCRIPT_FAILED_LINE)
    expect(failed(undefined)).toEqual({ title: 'Recording · 12:04', detail: 'Started by Alex', state: 'failed', partial: false })
    // A partial-only code on a failed card isn't a reason.
    expect(failedLine({ code: 'audio_not_captured', message: 'x' })).toBe(TRANSCRIPT_FAILED_LINE)
  })

  it('audio_not_captured: a quiet note on the Ready card, and Show transcript still works', () => {
    const show = vi.fn()
    const raw = { recording_id: '42', started_by: '7', stopped_at: '2026-11-17T14:00:00Z', duration_ms: 724_000, transcript: { state: 'partial', turns: 3 } }
    const r = parseRecordingSummary(raw, { code: 'audio_not_captured', message: 'segments lost' })!
    render(<HomeChatRecordingCard view={recordingCardView(r, 'Alex')} timeLabel="2m" onShowTranscript={show} />)
    expect(screen.getByTestId('home-chat-recording-card').getAttribute('data-state')).toBe('ready')
    expect(screen.getByTestId('home-chat-recording-card-note').textContent).toBe('Part of the call wasn’t captured, so this transcript is incomplete.')
    expect(screen.queryByTestId('home-chat-recording-card-failed')).toBeNull()
    fireEvent.click(screen.getByTestId('home-chat-show-transcript'))
    expect(show).toHaveBeenCalledTimes(1)
    cleanup()
    // Without the field, the partial card keeps its line.
    render(<HomeChatRecordingCard view={recordingCardView(parseRecordingSummary(raw)!, 'Alex')} timeLabel="2m" onShowTranscript={show} />)
    expect(screen.getByTestId('home-chat-recording-card-note').textContent).toBe('Some parts couldn’t be transcribed.')
  })
})

const LINES: TranscriptLine[] = [
  { id: '1', speaker: '7', text: 'Shall we look at the roof plan?', startMs: 0 },
  { id: '2', speaker: '7', text: 'The north side first.', startMs: 4_000 },
  { id: '3', speaker: '1', text: 'Yes, the roof needs a second pass.', startMs: 252_000 },
]
const personOf = (id: string) => (id === '7' ? { name: 'Alex', username: 'alex' } : { name: 'You', username: 'me' })

describe('the transcript panel', () => {
  it('its heading, then speaker turns: one per run of a speaker, with the time into the call', () => {
    const at = new Date(2026, 10, 17, 14, 41).getTime() / 1000
    expect(transcriptHeading({ createdAt: at, recording: summary() })).toMatch(/^Transcript · 12:04 · Nov 17, 2:41\sPM$/)
    expect(transcriptTurns(LINES).map((t) => [t.speaker, t.lines.length])).toEqual([
      ['7', 2],
      ['1', 1],
    ])
    render(<HomeChatTranscriptPanel heading="Transcript · 12:04 · Nov 17, 2:41 PM" lines={LINES} state={null} personOf={personOf} onClose={vi.fn()} />)
    expect(screen.getByTestId('home-chat-transcript-heading').textContent).toBe('Transcript · 12:04 · Nov 17, 2:41 PM')
    const turns = screen.getAllByTestId('home-chat-transcript-turn')
    expect(turns).toHaveLength(2)
    expect(turns[0].textContent).toContain('Alex')
    expect(turns[0].textContent).toContain('0:00')
    expect(turns[1].textContent).toContain('4:12')
    expect(screen.getAllByTestId('home-chat-transcript-line').map((l) => l.textContent)).toEqual(LINES.map((l) => l.text))
    // Never chat bubbles: no reactions, no reply, no hover row.
    expect(document.querySelector('[data-testid="home-chat-hover-row"]')).toBeNull()
  })

  it('search marks every match, and says how many', () => {
    expect(transcriptMatches('The roof, the Roof', 'roof').filter((p) => p.hit).map((p) => p.text)).toEqual(['roof', 'Roof'])
    expect(transcriptMatches('nothing here', '')).toEqual([{ text: 'nothing here', hit: false }])
    render(<HomeChatTranscriptPanel heading="Transcript" lines={LINES} state={null} personOf={personOf} onClose={vi.fn()} />)
    fireEvent.change(screen.getByTestId('home-chat-transcript-search'), { target: { value: 'roof' } })
    const hits = document.querySelectorAll('mark[data-transcript-hit]')
    expect([...hits].map((h) => h.textContent)).toEqual(['roof', 'roof'])
    expect(screen.getByTestId('home-chat-transcript-hits').textContent).toBe('2 matches')
    fireEvent.change(screen.getByTestId('home-chat-transcript-search'), { target: { value: 'chimney' } })
    expect(screen.getByTestId('home-chat-transcript-hits').textContent).toBe('No matches')
  })

  it('Copy transcript copies each turn under its speaker; Close and Escape close it', async () => {
    const writeText = vi.fn(async () => {})
    Object.defineProperty(navigator, 'clipboard', { value: { writeText }, configurable: true })
    const close = vi.fn()
    render(<HomeChatTranscriptPanel heading="Transcript · 12:04" lines={LINES} state={null} personOf={personOf} onClose={close} />)
    await act(async () => {
      fireEvent.click(screen.getByTestId('home-chat-transcript-copy'))
    })
    const want = transcriptCopyText('Transcript · 12:04', LINES, (id) => personOf(id).name)
    expect(want).toBe(
      'Transcript · 12:04\n\nAlex (0:00): Shall we look at the roof plan? The north side first.\nYou (4:12): Yes, the roof needs a second pass.',
    )
    expect(writeText).toHaveBeenCalledWith(want)
    expect(screen.getByTestId('home-chat-transcript-copy').textContent).toBe('Copied')
    fireEvent.click(screen.getByTestId('home-chat-transcript-close'))
    expect(close).toHaveBeenCalledTimes(1)
    fireEvent.keyDown(screen.getByTestId('home-chat-transcript'), { key: 'Escape' })
    expect(close).toHaveBeenCalledTimes(2)
  })

  it('loading, an error and no lines yet say so, with no search', () => {
    const { rerender } = render(<HomeChatTranscriptPanel heading="Transcript" lines={null} state="Loading…" personOf={personOf} onClose={vi.fn()} />)
    expect(screen.getByTestId('home-chat-transcript-state').textContent).toBe('Loading…')
    expect(screen.queryByTestId('home-chat-transcript-search')).toBeNull()
    rerender(<HomeChatTranscriptPanel heading="Transcript" lines={[]} state={null} personOf={personOf} onClose={vi.fn()} />)
    expect(screen.getByTestId('home-chat-transcript-empty')).toBeTruthy()
  })
})
