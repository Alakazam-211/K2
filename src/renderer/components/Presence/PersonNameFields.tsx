// First and last name editing (0.46.0, Rosson 2026-10-17). Optional for
// every person; the username stays the identity. Shared by the profile
// modal (your own names; the owner key edits the linked account's) and
// Admin Access (an Admin+ editing someone else's).
//
// `usePersonName` reads `GET /cli/people/name` and saves through
// `POST /cli/people/name` straight away: optimistic, rolled back with the
// server's error on a refusal (length cap 64, plain text, permissions).

import React, { useCallback, useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { PERSON_NAME_MAX } from '@/lib/person-name'
import { fetchPersonName, setPersonName, type PersonNameView } from './person-name-api'

export type PersonNameState =
  | { kind: 'loading' }
  | { kind: 'unsupported' }
  | { kind: 'error'; message: string }
  | { kind: 'ok'; view: PersonNameView }

export interface PersonNameApi {
  state: PersonNameState
  writeError: string | null
  busy: boolean
  /** Save one or both names (null or '' clears). */
  save: (update: { firstName?: string | null; lastName?: string | null }) => Promise<void>
}

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

/** `person` omitted = the caller. `reloadKey` re-reads (e.g. the owner
 *  picked a different account, whose names the owner now shows). */
export function usePersonName(scope: ServerScope, person?: string, reloadKey?: unknown): PersonNameApi {
  const [state, setState] = useState<PersonNameState>({ kind: 'loading' })
  const [writeError, setWriteError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const latest = useRef(state)
  latest.current = state

  useEffect(() => {
    let cancelled = false
    fetchPersonName(scope, person).then(
      (view) => {
        if (!cancelled) setState(view ? { kind: 'ok', view } : { kind: 'unsupported' })
      },
      (e: unknown) => {
        if (!cancelled) setState({ kind: 'error', message: message(e) })
      },
    )
    return () => {
      cancelled = true
    }
  }, [scope, person, reloadKey])

  const save = useCallback(
    async (update: { firstName?: string | null; lastName?: string | null }) => {
      const prev = latest.current
      if (prev.kind !== 'ok') return
      const clean = (v: string | null | undefined): string | null | undefined =>
        v === undefined ? undefined : v === null || v.trim() === '' ? null : v.trim()
      const firstName = clean(update.firstName)
      const lastName = clean(update.lastName)
      setBusy(true)
      setWriteError(null)
      setState({
        kind: 'ok',
        view: {
          ...prev.view,
          ...(firstName !== undefined ? { firstName } : {}),
          ...(lastName !== undefined ? { lastName } : {}),
        },
      })
      try {
        const body: { person?: string; firstName?: string | null; lastName?: string | null } = {}
        if (person) body.person = person
        if (firstName !== undefined) body.firstName = firstName
        if (lastName !== undefined) body.lastName = lastName
        setState({ kind: 'ok', view: await setPersonName(scope, body) })
      } catch (e) {
        setState(prev)
        setWriteError(message(e))
      } finally {
        setBusy(false)
      }
    },
    [scope, person],
  )

  return { state, writeError, busy, save }
}

/** One name input: type freely, save on blur or Enter (Escape reverts). */
export function PersonNameInput({
  label,
  value,
  onCommit,
  disabled = false,
  placeholder,
  testId,
  className,
}: {
  label: string
  value: string | null
  onCommit: (next: string) => void
  disabled?: boolean
  placeholder?: string
  testId?: string
  className?: string
}): React.JSX.Element {
  const [draft, setDraft] = useState(value ?? '')
  useEffect(() => setDraft(value ?? ''), [value])
  const commit = (): void => {
    const next = draft.trim()
    setDraft(next)
    if (next !== (value ?? '')) onCommit(next)
  }
  return (
    <input
      type="text"
      aria-label={label}
      data-testid={testId}
      value={draft}
      placeholder={placeholder ?? 'Optional'}
      maxLength={PERSON_NAME_MAX}
      disabled={disabled}
      autoComplete="off"
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === 'Enter') e.currentTarget.blur()
        if (e.key === 'Escape') {
          e.stopPropagation()
          setDraft(value ?? '')
        }
      }}
      className={`${className ?? 'w-full max-w-[240px]'} px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] disabled:opacity-50 no-drag`}
    />
  )
}
