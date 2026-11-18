// K2's recording notice for its own voice rooms (K2 0.46.11, Sew 0.9.0) and
// its DM calls.
//
// Rosson 2026-11-10: in K2's rooms people are told a recording is happening
// and can't opt out. The notice says who started it, what is kept and for how
// long, and (when an agent holds "Read call transcripts") that agents on this
// server may read the transcript. One acknowledgement, "OK" (the person's
// grant, with the notice version they saw), and the way out ("Leave call", or
// "Don't join" before joining). It has no default and can't be dismissed: no
// Escape, no backdrop click, no close button. No answer is never a yes; Sew
// drops a person who doesn't answer within a minute, and a calm line counts
// that minute down.
//
// The look: a centred modal sheet over everything, a live Browser page too.
// Frost in Glass, a raised panel with cut corners in Terminal. Focus starts
// on the words, never on a button, so a stray Enter answers nothing; OK
// (the primary) and the way out (a quiet danger button) are the same width.

import { useEffect, useRef, useState } from 'react'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { recordingNoticeText } from '@/lib/voice/recording-controls'
import { convOfDmRoom, formatCallDuration } from '@/lib/home-chat/calls'
import { Button } from '@/components/ui/Button'
import { Surface } from '@/components/ui/Surface'
import { floatMaterial } from '@/components/ui/float-material'
import type { VoiceCall } from '@/stores/voice-core'
import { RecordingDot } from './RecordingBadge'

/** How long Sew waits for an answer before it drops the person (ms). */
export const RECORDING_NOTICE_WAIT_MS = 60_000

/** The countdown's line: "You’ll leave the call in 0:45 if you don’t
 *  answer". Joining a room names it ("You won’t join Standup…"); `room`
 *  null (a DM call): "the call". */
export function noticeCountdownLine(ask: 'join' | 'in_call', leftMs: number, room: string | null = null): string {
  const left = formatCallDuration(Math.max(0, leftMs))
  return ask === 'join' ? `You won’t join ${room ?? 'the call'} if you don’t answer in ${left}` : `You’ll leave the call in ${left} if you don’t answer`
}

export interface RecordingNoticeProps {
  call: VoiceCall
  /** Any agent on this server may read the rooms' transcripts. */
  agentReaders: boolean
  onAnswer: (answer: 'ok' | 'leave') => void
}

export default function RecordingNotice({ call, agentReaders, onAnswer }: RecordingNoticeProps): React.JSX.Element | null {
  const n = call.recordingNotice
  const body = useRef<HTMLDivElement>(null)
  const root = useRef<HTMLDivElement>(null)
  // A live Browser page is a native view drawn over the app: the notice must
  // claim it, or someone browsing would never see they are being recorded.
  useBrowserOverlay({ active: !!n, ref: root, fullscreen: true, kind: 'modal' })
  const key = n ? `${n.recordingId}:${n.prompt.noticeVersion}` : null
  // The minute counts from when this notice (this recording, this version)
  // came up.
  const [shown, setShown] = useState<{ key: string | null; at: number }>(() => ({ key, at: Date.now() }))
  const [now, setNow] = useState(() => Date.now())
  if (shown.key !== key) setShown({ key, at: Date.now() })
  useEffect(() => {
    body.current?.focus()
    if (!key) return
    setNow(Date.now())
    const t = setInterval(() => setNow(Date.now()), 1000)
    return () => clearInterval(t)
  }, [key])
  // Escape never answers, wherever focus is (a key held from before the
  // sheet came up, or focus left on the page behind it).
  useEffect(() => {
    if (!key) return
    const stop = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      e.stopPropagation()
    }
    window.addEventListener('keydown', stop, true)
    return () => window.removeEventListener('keydown', stop, true)
  }, [key])
  if (!n) return null
  // A DM call's notice says "this DM" and carries no K2 agent line (HR-8).
  const dm = convOfDmRoom(call.roomId) !== null
  const text = recordingNoticeText({ roomName: call.roomName, prompt: n.prompt, agentReaders, ask: n.ask, dm })
  const left = Math.min(RECORDING_NOTICE_WAIT_MS, RECORDING_NOTICE_WAIT_MS - (now - shown.at))
  return (
    <div
      ref={root}
      data-testid="recording-notice"
      data-scrim=""
      className="no-drag fixed inset-0 z-[1000] flex items-center justify-center bg-[var(--color-scrim)] p-4"
      // Never dismissed into an answer: a click outside does nothing.
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
        }
      }}
    >
      <div data-k2-chamfer-float="" className="w-[min(420px,calc(var(--k2-vw)-32px))]">
        <Surface
          elevation={5}
          chamfer
          {...floatMaterial('dialog', 'frost')}
          role="alertdialog"
          aria-modal="true"
          aria-labelledby="recording-notice-title"
          aria-describedby="recording-notice-body"
          data-testid="recording-notice-sheet"
          className="flex flex-col gap-4 p-5 text-[var(--color-text-primary)]"
        >
          <div className="flex items-center gap-2">
            <RecordingDot />
            <h2 id="recording-notice-title" className="type-heading">
              {text.title}
            </h2>
          </div>
          <div
            ref={body}
            id="recording-notice-body"
            tabIndex={-1}
            data-testid="recording-notice-body"
            className="k2-focus flex flex-col gap-1.5 outline-none"
          >
            {text.lines.map((l) => (
              <p key={l} className="type-row text-[var(--color-text-secondary)]">
                {l}
              </p>
            ))}
          </div>
          <p data-testid="recording-notice-countdown" role="timer" aria-live="off" className="type-hint tabular-nums text-[var(--color-text-muted)]">
            {noticeCountdownLine(n.ask, left, dm ? null : call.roomName)}
          </p>
          <div className="grid grid-cols-2 gap-2">
            <Button variant="danger-quiet" size="md" className="min-h-8" data-testid="recording-notice-leave" onClick={() => onAnswer('leave')}>
              {text.leave}
            </Button>
            <Button variant="accent" size="md" className="min-h-8" data-testid="recording-notice-ok" onClick={() => onAnswer('ok')}>
              {text.ok}
            </Button>
          </div>
        </Surface>
      </div>
    </div>
  )
}
