// @vitest-environment jsdom
//
// K2's recording notice for its own rooms (0.46.11, Rosson 2026-11-10): who
// started it, what is kept, the agents line when the switch is on; OK or
// leave, no default, never dismissed into a yes (no Escape, no close button,
// focus on the words); the minute it waits, counted down; and the badge.

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { VoiceCall } from '@/stores/voice-core'
import RecordingNotice from './RecordingNotice'
import { CallRecordingBadge } from './RecordingBadge'

const call = (over: Partial<VoiceCall> = {}): VoiceCall => ({
  hostKey: 'sewlab.k2.dev',
  serverLabel: 'sewlab',
  roomId: 'room-1',
  roomName: 'Standup',
  callId: 'r_x',
  phase: 'connected',
  muted: false,
  deafened: false,
  speakDenied: false,
  serverMuted: false,
  micDenied: false,
  relayOnly: false,
  connectedAt: 1,
  participants: [{ user: 'alice', speaking: false, muted: false, memberId: '7' }],
  recording: { recordingId: '42', myConsent: 'pending', recorderPresent: true, trigger: 'manual', required: false, startedBy: '7', prompt: null },
  recordingNotice: {
    recordingId: '42',
    ask: 'in_call',
    prompt: { noticeVersion: 3141, startedByName: 'Alice', audioKeepDays: 30, required: false, appReaders: [], agentReaders: false, declineMode: 'not_join' },
  },
  recordingPerms: null,
  recordingControls: null,
  ...over,
})

afterEach(cleanup)

describe('RecordingNotice', () => {
  it('shows who started it and what is kept, with OK and Leave call, and no way to opt out', () => {
    const onAnswer = vi.fn()
    render(<RecordingNotice call={call()} agentReaders={false} onAnswer={onAnswer} />)
    expect(screen.getByRole('alertdialog').textContent).toContain('Alice started recording this call.')
    expect(screen.getByRole('alertdialog').textContent).toContain('The audio is kept for 30 days.')
    expect(screen.queryByText(/without being recorded|Stop recording me/i)).toBeNull()
    expect(screen.queryByText('Agents on this server may read the transcript.')).toBeNull()
    fireEvent.click(screen.getByTestId('recording-notice-ok'))
    expect(onAnswer).toHaveBeenLastCalledWith('ok')
    fireEvent.click(screen.getByTestId('recording-notice-leave'))
    expect(onAnswer).toHaveBeenLastCalledWith('leave')
    expect(screen.getByTestId('recording-notice-leave').textContent).toBe('Leave call')
  })

  it('names agents when an agent on this server may read the transcript', () => {
    render(<RecordingNotice call={call()} agentReaders onAnswer={vi.fn()} />)
    expect(screen.getByText('Agents on this server may read the transcript.')).toBeTruthy()
  })

  it('has no default and is never dismissed into an answer (Escape, a click outside)', () => {
    const onAnswer = vi.fn()
    render(<RecordingNotice call={call()} agentReaders={false} onAnswer={onAnswer} />)
    fireEvent.keyDown(screen.getByRole('alertdialog'), { key: 'Escape' })
    fireEvent.click(screen.getByTestId('recording-notice'))
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(onAnswer).not.toHaveBeenCalled()
    expect(screen.getByRole('alertdialog')).toBeTruthy()
    // Focus starts on the words, never on a button: a stray Enter answers nothing.
    expect(document.activeElement?.getAttribute('data-testid')).toBe('recording-notice-body')
    fireEvent.keyDown(document.activeElement!, { key: 'Enter' })
    expect(onAnswer).not.toHaveBeenCalled()
    // No close button of any kind: OK and the way out are the only buttons.
    expect(screen.queryByLabelText(/close/i)).toBeNull()
    expect(screen.getAllByRole('button').map((b) => b.getAttribute('data-testid'))).toEqual(['recording-notice-leave', 'recording-notice-ok'])
  })

  it('counts down the minute Sew waits, in a calm line', () => {
    vi.useFakeTimers()
    try {
      render(<RecordingNotice call={call()} agentReaders={false} onAnswer={vi.fn()} />)
      expect(screen.getByTestId('recording-notice-countdown').textContent).toBe("You'll leave the call in 1:00 if you don't answer")
      act(() => {
        vi.advanceTimersByTime(15_000)
      })
      expect(screen.getByTestId('recording-notice-countdown').textContent).toBe("You'll leave the call in 0:45 if you don't answer")
    } finally {
      vi.useRealTimers()
    }
  })

  it("before joining it says Don't join", () => {
    const c = call({ phase: 'joining' })
    c.recordingNotice = { ...c.recordingNotice!, ask: 'join' }
    render(<RecordingNotice call={c} agentReaders={false} onAnswer={vi.fn()} />)
    expect(screen.getByTestId('recording-notice-leave').textContent).toBe("Don't join")
    expect(screen.getByRole('alertdialog').textContent).toContain('Standup is being recorded')
  })

  it('the badge shows the whole time a recording is on: the dot, then the words in the normal ink with the clock', () => {
    const at = Date.now() - 192_000
    const rec = { ...call().recording!, startedAt: at }
    const { rerender } = render(<CallRecordingBadge call={call({ recording: rec, recordingNotice: null })} testId="call-topbar-recording" />)
    const badge = screen.getByTestId('call-topbar-recording')
    expect(badge.textContent).toBe('Recording 03:12')
    expect(badge.getAttribute('title')).toMatch(/^alice started recording at \d{1,2}:\d{2}\s[AP]M$/)
    expect(badge.className).toContain('--color-text-primary')
    expect(badge.className).not.toMatch(/status-error|danger/)
    // The dot pulses (globals.css holds it still under reduced motion).
    expect(badge.querySelector('[data-recording-dot]')).toBeTruthy()
    // Found running as this device joined: no clock, no time.
    rerender(<CallRecordingBadge call={call({ recordingNotice: null })} testId="call-topbar-recording" />)
    expect(screen.getByTestId('call-topbar-recording').textContent).toBe('Recording')
    expect(screen.getByTestId('call-topbar-recording').getAttribute('title')).toBe('alice started recording')
    rerender(<CallRecordingBadge call={call({ recording: null, recordingNotice: null })} testId="call-topbar-recording" />)
    expect(screen.queryByTestId('call-topbar-recording')).toBeNull()
  })

  it("a DM call's notice says this DM and never K2's agent line, even with an agent reader (HR-8)", () => {
    const onAnswer = vi.fn()
    render(<RecordingNotice call={call({ roomId: 'dm:900', roomName: 'Alex' })} agentReaders onAnswer={onAnswer} />)
    const text = screen.getByRole('alertdialog').textContent ?? ''
    expect(text).toContain('This call is being recorded')
    expect(text).toContain('The transcript is kept in this DM.')
    expect(text).not.toContain('Agents on this server may read the transcript.')
    expect(text).not.toMatch(/room/i)
    fireEvent.click(screen.getByTestId('recording-notice-ok'))
    expect(onAnswer).toHaveBeenLastCalledWith('ok')
    expect(screen.getByTestId('recording-notice-leave').textContent).toBe('Leave call')
  })
})
