// A call's recording badge: one look in every place the call shows (the top
// bar's slim bar, the floating tile, the stage, full screen, the call's own
// window, the DM's strip and its header). A dot in the danger colour that
// breathes slowly (still under reduced motion: globals.css,
// `[data-recording-dot]`), then "Recording 03:12" in the place's own ink,
// never red text. The tooltip says who started it and when ("Alex started
// recording at 2:41 PM"), and how long the audio is kept when Sew says. It
// is driven by Sew's word that the call records, the same for everyone in
// it, never by who pressed Record.

import { useEffect, useState } from 'react'
import { convOfDmRoom } from '@/lib/home-chat/calls'
import { formatRecordingClock, recordingBadgeTitle } from '@/lib/voice/recording-controls'
import type { VoiceCall } from '@/stores/voice-core'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

/** The dot alone (the badge's, the DM header's chip, the recording card). */
export function RecordingDot({ still = false }: { still?: boolean }): React.JSX.Element {
  return (
    <span
      aria-hidden
      data-recording-dot={still ? undefined : ''}
      className="inline-block h-2 w-2 shrink-0 rounded-[var(--radius-badge)] bg-[var(--color-status-error)]"
    />
  )
}

/** The seconds since `startedAt`, ticking once a second; null without one. */
export function useRecordingClock(startedAt: number | null): string | null {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (startedAt === null) return
    setNow(Date.now())
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [startedAt])
  return startedAt === null ? null : formatRecordingClock(now - startedAt)
}

export interface RecordingBadgeProps {
  /** When it started (epoch ms); null: the clock is left out. */
  startedAt: number | null
  /** The tooltip ("Alex started recording at 2:41 PM"). */
  title: string
  testId: string
  /** Over video: the media label's chip, so it reads on any picture. */
  onMedia?: boolean
  /** `chip`: the DM header's short form, "Recording" with no clock. */
  short?: boolean
  className?: string
}

export function RecordingBadge({ startedAt, title, testId, onMedia = false, short = false, className = '' }: RecordingBadgeProps): React.JSX.Element {
  const clock = useRecordingClock(short ? null : startedAt)
  const look = onMedia
    ? 'rounded-[var(--radius-pill)] bg-[var(--color-media-label-bg)] px-1.5 leading-4 text-[var(--color-media-label-text)]'
    : 'text-[var(--color-text-primary)]'
  return (
    <span
      role="status"
      data-testid={testId}
      title={title}
      // Stable for screen readers (a status is read again on each change):
      // the clock ticks for the eye only. The tooltip's lines are read as
      // sentences.
      aria-label={title === 'Recording' ? 'Recording' : `Recording. ${title.split('\n').join('. ')}`}
      data-k2-chamfer={onMedia ? 'chip' : undefined}
      className={`type-hint flex shrink-0 items-center gap-1.5 whitespace-nowrap ${look} ${className}`}
      style={onMedia ? { ...noDrag, '--k2-ch-w': '0px' } : noDrag}
    >
      <RecordingDot />
      <span aria-hidden>
        Recording{clock ? <span className="tabular-nums"> {clock}</span> : null}
      </span>
    </span>
  )
}

/** A call's badge, from the voice store's call: nothing while Sew says no
 *  recording runs. */
export function CallRecordingBadge({ call, testId, onMedia }: { call: VoiceCall; testId: string; onMedia?: boolean }): React.JSX.Element | null {
  const r = call.recording
  if (!r) return null
  const starter = r.startedBy ? (call.participants.find((p) => p.memberId === r.startedBy)?.user ?? null) : null
  const startedAt = r.startedAt ?? null
  const title = recordingBadgeTitle(r, starter, convOfDmRoom(call.roomId) !== null, startedAt)
  return <RecordingBadge startedAt={startedAt} title={title} testId={testId} onMedia={onMedia} />
}
