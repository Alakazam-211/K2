// A row's write, in words: Saving… while it is on its way, Saved for a
// moment after, or why it failed. A refusal on permission says who can
// change it and offers no retry; anything else says nothing changed and that
// trying again is safe, then the server's own reason quietly. Used by Org
// Chart and Grok Bot (the shared useSaveStatus / SaveError have no saving
// state and always offer a retry).

import React, { useEffect, useRef, useState } from 'react'
import { SaveNote, SETTING_SERVER_REASON } from '../controls/SettingControls'
import { daemonErrorText } from '@/lib/daemon-error'
import { httpCodeOf, httpStatusOf } from '@/lib/daemon-http-error'
import { PRODUCT_NAME } from '@/lib/brand'

export type RowSave = 'idle' | 'saving' | 'saved' | { error: string; denied: boolean; reached: boolean }

export const DENIED_LINE = 'Only the server’s owner or an admin can change these.'

/** Codes the daemon uses for "this login's role may not change it". */
const PERMISSION_CODES = new Set(['role_required', 'owner_only', 'admin_only', 'forbidden', 'not_allowed', 'unauthorized'])

/** A refusal on permission: 401, or 403 with a permission code (or none). */
export function isDenied(err: unknown): boolean {
  const status = httpStatusOf(err)
  const code = httpCodeOf(err)
  if (code && PERMISSION_CODES.has(code)) return true
  if (status === 401) return true
  return status === 403 && code === null
}

export function useRowSave(holdMs = 2000): {
  state: RowSave
  saving: () => void
  saved: () => void
  failed: (err: unknown) => void
  reset: () => void
} {
  const [state, setState] = useState<RowSave>('idle')
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)
  const clear = (): void => {
    if (timer.current) clearTimeout(timer.current)
    timer.current = null
  }
  useEffect(() => clear, [])
  return {
    state,
    saving: () => {
      clear()
      setState('saving')
    },
    saved: () => {
      clear()
      setState('saved')
      timer.current = setTimeout(() => setState('idle'), holdMs)
    },
    failed: (err) => {
      clear()
      setState({ error: daemonErrorText(err).trim(), denied: isDenied(err), reached: httpStatusOf(err) !== null })
    },
    reset: () => {
      clear()
      setState('idle')
    },
  }
}

/** "Saving…" or "Saved", quiet, beside the control. */
export function RowSaveNote({ state }: { state: RowSave }): React.JSX.Element | null {
  if (state === 'saving') {
    return (
      <span role="status" data-save-note="saving" className="type-hint text-[var(--color-text-secondary)] whitespace-nowrap">
        Saving…
      </span>
    )
  }
  return <SaveNote state={state === 'saved' ? 'saved' : 'idle'} />
}

/** Why the last write failed, under the row's hint. */
export function RowSaveError({ state, testId }: { state: RowSave; testId?: string }): React.JSX.Element | null {
  if (typeof state !== 'object') return null
  if (state.denied) {
    return (
      <p role="alert" data-save-error="denied" data-testid={testId} className="type-hint text-[var(--color-status-error-soft)] mt-1 leading-relaxed max-w-[75ch]">
        Couldn’t save. {DENIED_LINE}
      </p>
    )
  }
  const reason = state.error.replace(/[.\s]+$/, '')
  const lead = state.reached
    ? 'Couldn’t save this change. Nothing changed; try again.'
    : `Couldn’t save: ${PRODUCT_NAME} couldn’t reach this server. Nothing changed; try again.`
  return (
    <div role="alert" data-save-error="" data-testid={testId} className="mt-1">
      <p className="type-hint text-[var(--color-status-error-soft)] leading-relaxed max-w-[75ch]">{lead}</p>
      {reason && <p className={SETTING_SERVER_REASON}>The server said: {reason}.</p>}
    </div>
  )
}
