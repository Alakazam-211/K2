// K2's recording notice for its own voice rooms (K2 0.46.11, Sew 0.9.0).
//
// Rosson 2026-11-10: in K2's rooms people are told a recording is happening
// and can't opt out. The notice says who started it, what is kept and for how
// long, and (when an agent holds "Read call transcripts") that agents on this
// server may read the transcript. One acknowledgement, "OK" (the person's
// grant, with the notice version they saw), and the way out ("Leave call", or
// "Don't join" before joining). It has no default and can't be dismissed: no
// Escape, no backdrop click, no close button. No answer is never a yes; Sew
// drops a person who doesn't answer within a minute.

import { useEffect, useRef } from 'react'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { recordingNoticeText } from '@/lib/voice/recording-controls'
import type { VoiceCall } from '@/stores/voice-core'

export interface RecordingNoticeProps {
  call: VoiceCall
  /** Any agent on this server may read the rooms' transcripts. */
  agentReaders: boolean
  onAnswer: (answer: 'ok' | 'leave') => void
}

export default function RecordingNotice({ call, agentReaders, onAnswer }: RecordingNoticeProps): React.JSX.Element | null {
  const n = call.recordingNotice
  const first = useRef<HTMLDivElement>(null)
  const root = useRef<HTMLDivElement>(null)
  // A live Browser page is a native view drawn over the app: the notice must
  // claim it, or someone browsing would never see they are being recorded.
  useBrowserOverlay({ active: !!n, ref: root, fullscreen: true, kind: 'modal' })
  useEffect(() => {
    first.current?.focus()
  }, [n?.recordingId, n?.prompt.noticeVersion])
  if (!n) return null
  const text = recordingNoticeText({ roomName: call.roomName, prompt: n.prompt, agentReaders, ask: n.ask })
  return (
    <div
      ref={root}
      data-testid="recording-notice"
      className="fixed inset-0 z-[1000] flex items-center justify-center bg-[var(--color-scrim)]"
      // Never dismissed into an answer: a click outside does nothing.
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.preventDefault()
          e.stopPropagation()
        }
      }}
    >
      <div
        ref={first}
        tabIndex={-1}
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="recording-notice-title"
        className="w-[min(420px,calc(100vw-32px))] border border-[var(--color-border)] bg-[var(--color-bg-elevated)] p-4 text-[var(--color-text-primary)] outline-none"
      >
        <div className="mb-2 flex items-center gap-2">
          <span aria-hidden className="inline-block h-2.5 w-2.5 rounded-[var(--radius-pill)] bg-[var(--color-status-error)]" />
          <h2 id="recording-notice-title" className="text-sm font-semibold">
            {text.title}
          </h2>
        </div>
        <ul className="mb-4 space-y-1 text-xs text-[var(--color-text-secondary)]">
          {text.lines.map((l) => (
            <li key={l}>{l}</li>
          ))}
        </ul>
        <div className="flex justify-end gap-2">
          <button
            type="button"
            data-testid="recording-notice-leave"
            className="border border-[var(--color-border)] px-3 py-1 text-xs hover:bg-[var(--color-bg-surface)]"
            onClick={() => onAnswer('leave')}
          >
            {text.leave}
          </button>
          <button
            type="button"
            data-testid="recording-notice-ok"
            className="border border-[var(--color-text-primary)] bg-[var(--color-text-primary)] px-3 py-1 text-xs text-[var(--color-bg)]"
            onClick={() => onAnswer('ok')}
          >
            {text.ok}
          </button>
        </div>
      </div>
    </div>
  )
}
