// What the last action on a Settings page did, under the card it changed:
// a quiet confirmation ("sam can now watch this screen."), or what failed in
// plain words, the server's reason quietly under it, and the next step. One
// look for the Remote Desktop and Voice Rooms pages, matching the shared
// SaveNote / SaveError and SettingsLoadState.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import { SETTING_SERVER_REASON } from '../controls/SettingControls'

export type ActionNoteState =
  | { tone: 'ok'; text: string }
  | { tone: 'error'; text: string; reason: string | null; fromServer: boolean }
  | null

function reasonOf(err: unknown): string | null {
  const raw = err instanceof Error ? err.message : typeof err === 'string' ? err : ''
  const t = raw.trim().replace(/[.\s]+$/, '')
  return t || null
}

/** One note per card: `done(text)` after a write lands (it fades after a
 *  few seconds), `failed(text, err)` when it doesn't (it stays until the
 *  next action), `clear()` when the person starts something new. */
export function useActionNote(holdMs = 4000): {
  note: ActionNoteState
  done: (text: string) => void
  failed: (text: string, err: unknown) => void
  clear: () => void
} {
  const [note, setNote] = useState<ActionNoteState>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const stop = useCallback(() => {
    if (timer.current) clearTimeout(timer.current)
    timer.current = null
  }, [])
  useEffect(() => stop, [stop])
  const done = useCallback(
    (text: string) => {
      stop()
      setNote({ tone: 'ok', text })
      timer.current = setTimeout(() => setNote(null), holdMs)
    },
    [holdMs, stop],
  )
  const failed = useCallback(
    (text: string, err: unknown) => {
      stop()
      setNote({ tone: 'error', text, reason: reasonOf(err), fromServer: err instanceof DaemonHttpError })
    },
    [stop],
  )
  const clear = useCallback(() => {
    stop()
    setNote(null)
  }, [stop])
  return { note, done, failed, clear }
}

export function ActionNote({ note, testId }: { note: ActionNoteState; testId?: string }): React.JSX.Element | null {
  if (!note) return null
  if (note.tone === 'ok') {
    return (
      <p role="status" data-testid={testId} data-action-note="ok" className="inline-flex items-center gap-1 type-hint text-[var(--color-status-ok-soft)]">
        <svg aria-hidden="true" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" className="w-3 h-3 flex-shrink-0">
          <path d="M3.5 8.5 6.5 11.5 12.5 4.5" />
        </svg>
        {note.text}
      </p>
    )
  }
  return (
    <div role="alert" data-testid={testId} data-action-note="error">
      <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">{note.text}</p>
      {note.reason ? (
        <p className={SETTING_SERVER_REASON}>
          {note.fromServer ? 'The server said' : 'Details'}: {note.reason}.
        </p>
      ) : null}
    </div>
  )
}

/** A failed read's quiet second line for a SettingsLoadState: the server's
 *  own words, or, when the reply couldn't be read, what was wrong with it. */
export function readFailure(err: unknown): { detail: string | null; shape: string | null } {
  const reason = reasonOf(err)
  if (err instanceof DaemonHttpError) return { detail: reason, shape: null }
  return { detail: null, shape: reason }
}
