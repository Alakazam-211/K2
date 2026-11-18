// @vitest-environment jsdom
// The recording badge's clock and tooltip: from Sew's start when it says
// (someone who joins late sees the real clock and the start time), no clock
// when the start isn't known, and a start a little in the future (clock
// skew) reads 00:00, never a negative time.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, render, screen } from '@testing-library/react'
import { RecordingBadge } from './RecordingBadge'
import { recordingBadgeTitle } from '@/lib/voice/recording-controls'
import { parseRecording } from '@/lib/voice/native-voice-engine'

const NOW = new Date(2026, 10, 18, 14, 44, 12).getTime()

beforeEach(() => {
  vi.useFakeTimers()
  vi.setSystemTime(NOW)
})
afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

const event = (started_at?: unknown) => ({
  recording_id: '77',
  my_consent: 'granted',
  recorder_present: true,
  trigger: 'manual',
  started_by: '7',
  ...(started_at === undefined ? {} : { started_at }),
})

function badgeFor(started_at?: unknown) {
  const r = parseRecording(event(started_at))!
  const startedAt = r.startedAt ?? null
  return render(<RecordingBadge startedAt={startedAt} title={recordingBadgeTitle(r, 'Alex', false, startedAt)} testId="badge" />)
}

describe('the recording badge', () => {
  it("a late joiner: the clock and the tooltip from Sew's start", () => {
    badgeFor(new Date(2026, 10, 18, 14, 41, 0).toISOString())
    const b = screen.getByTestId('badge')
    expect(b.textContent).toBe('Recording 03:12')
    expect(b.getAttribute('title')).toMatch(/^Alex started recording at 2:41\sPM$/)
    act(() => {
      vi.advanceTimersByTime(1_000)
    })
    expect(b.textContent).toBe('Recording 03:13')
  })

  it('without a start (an older Sew), or one that is not a date: no clock, and the tooltip leaves the time out', () => {
    badgeFor()
    expect(screen.getByTestId('badge').textContent).toBe('Recording')
    expect(screen.getByTestId('badge').getAttribute('title')).toBe('Alex started recording')
    cleanup()
    badgeFor('half past two')
    expect(screen.getByTestId('badge').textContent).toBe('Recording')
  })

  it('a start in the future (clock skew) reads 00:00 until it passes', () => {
    badgeFor(new Date(NOW + 4_000).toISOString())
    const b = screen.getByTestId('badge')
    expect(b.textContent).toBe('Recording 00:00')
    act(() => {
      vi.advanceTimersByTime(3_000)
    })
    expect(b.textContent).toBe('Recording 00:00')
    act(() => {
      vi.advanceTimersByTime(3_000)
    })
    expect(b.textContent).toBe('Recording 00:02')
  })
})
