// @vitest-environment jsdom
//
// K2's recording notice for its own rooms (0.46.11, Rosson 2026-11-10): who
// started it, what is kept, the agents line when the switch is on; OK or
// leave, no default, never dismissed into a yes; and the red badge.

import { cleanup, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, describe, expect, it, vi } from 'vitest'
import type { VoiceCall } from '@/stores/voice-core'
import RecordingNotice from './RecordingNotice'
import { RecordingBadge } from './CallControls'

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
    expect(onAnswer).not.toHaveBeenCalled()
    expect(screen.getByRole('alertdialog')).toBeTruthy()
    expect(document.activeElement?.getAttribute('role')).toBe('alertdialog')
  })

  it("before joining it says Don't join", () => {
    const c = call({ phase: 'joining' })
    c.recordingNotice = { ...c.recordingNotice!, ask: 'join' }
    render(<RecordingNotice call={c} agentReaders={false} onAnswer={vi.fn()} />)
    expect(screen.getByTestId('recording-notice-leave').textContent).toBe("Don't join")
    expect(screen.getByRole('alertdialog').textContent).toContain('Standup is being recorded')
  })

  it('the red badge shows the whole time a recording is on', () => {
    const { rerender } = render(<RecordingBadge call={call({ recordingNotice: null })} variant="topbar" />)
    expect(screen.getByTestId('call-topbar-recording').textContent).toBe('Recording')
    expect(screen.getByTestId('call-topbar-recording').getAttribute('title')).toBe('Recording: started by alice')
    rerender(<RecordingBadge call={call({ recording: null, recordingNotice: null })} variant="topbar" />)
    expect(screen.queryByTestId('call-topbar-recording')).toBeNull()
  })
})
