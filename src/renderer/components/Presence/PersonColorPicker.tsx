// "Colour" in the profile modal (0.46.0, Rosson 2026-10-17): the eight
// person hues plus Automatic (the name hash). Whoever is signed in picks
// their own. For the owner token or an owner-linked login that is the
// owner's colour, which the linked logins show too. The daemon stores and
// validates it (`POST /cli/people/color`) and re-broadcasts the presence
// roster, so every chip on every client repaints. An older daemon without
// the route shows the swatches disabled with "Needs a newer K2 server".

import React, { useEffect, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { CHAMFER_CLIP, PERSON_COLORS } from './PresenceChip'
import { fetchPersonColor, setPersonColor, type PersonColorView } from './person-color-api'

/** Plain names for the eight hues (tooltips and screen readers). */
export const PERSON_COLOR_NAMES: Record<string, string> = {
  '#2563eb': 'Blue',
  '#7c3aed': 'Violet',
  '#0d9488': 'Teal',
  '#db2777': 'Pink',
  '#4f46e5': 'Indigo',
  '#0891b2': 'Cyan',
  '#64748b': 'Slate',
  '#c026d3': 'Fuchsia',
}

/** The note under disabled swatches on a server without the route. */
export const COLOR_NEEDS_NEWER_SERVER = 'Needs a newer K2 server.'

type State =
  | { kind: 'loading' }
  | { kind: 'unsupported' }
  | { kind: 'error'; message: string }
  | { kind: 'ok'; view: PersonColorView }

function message(e: unknown): string {
  return e instanceof Error ? e.message : String(e)
}

export function PersonColorPicker({
  scope,
  onView,
}: {
  scope: ServerScope
  /** The current view (null: an older daemon without the route), so a
   *  parent can paint its own chip in the picked colour. */
  onView?: (view: PersonColorView | null) => void
}): React.JSX.Element | null {
  const [state, setState] = useState<State>({ kind: 'loading' })
  const [busy, setBusy] = useState(false)
  const [writeError, setWriteError] = useState<string | null>(null)

  useEffect(() => {
    if (!onView) return
    if (state.kind === 'ok') onView(state.view)
    else if (state.kind === 'unsupported') onView(null)
  }, [state, onView])

  useEffect(() => {
    let cancelled = false
    fetchPersonColor(scope).then(
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
  }, [scope])

  if (state.kind === 'loading') return <span className="text-[var(--color-text-muted)]">Loading…</span>
  if (state.kind === 'error') {
    return <span className="text-[var(--color-status-error-soft)]">Couldn’t load: {state.message}</span>
  }
  // An older server without the route: the swatches show, disabled, with
  // a short note (not hidden, so nobody wonders where the picker went).
  const locked = state.kind === 'unsupported'
  const view: PersonColorView =
    state.kind === 'ok' ? state.view : { palette: [...PERSON_COLORS], person: '', color: '', chosen: null, colors: {} }
  const disabled = busy || locked

  const pick = async (color: string | null): Promise<void> => {
    if (disabled || color === view.chosen) return
    setBusy(true)
    setWriteError(null)
    // Optimistic: mark the pick at once; roll back on a refusal.
    setState({ kind: 'ok', view: { ...view, chosen: color, color: color ?? view.color } })
    try {
      setState({ kind: 'ok', view: await setPersonColor(scope, color) })
    } catch (e) {
      setState({ kind: 'ok', view })
      setWriteError(message(e))
    } finally {
      setBusy(false)
    }
  }

  const ring = (on: boolean): React.CSSProperties => ({
    outline: on ? '2px solid var(--color-text-primary)' : '2px solid transparent',
    outlineOffset: 1,
  })

  return (
    <div data-testid="person-color-picker">
      <div className="flex flex-wrap items-center gap-1.5" role="group" aria-label="Your colour">
        <button
          type="button"
          data-testid="person-color-auto"
          aria-pressed={view.chosen === null}
          disabled={disabled}
          onClick={() => void pick(null)}
          title="Automatic: a colour from your name"
          className="h-5 px-1.5 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer disabled:cursor-default"
          style={{ ...ring(view.chosen === null && !locked), opacity: locked ? 0.45 : undefined }}
        >
          Automatic
        </button>
        {view.palette.map((c) => {
          const on = view.chosen === c
          const name = PERSON_COLOR_NAMES[c] ?? c
          return (
            <button
              key={c}
              type="button"
              data-testid={`person-color-${c}`}
              aria-label={name}
              aria-pressed={on}
              disabled={disabled}
              title={name}
              onClick={() => void pick(c)}
              className="h-5 w-5 cursor-pointer disabled:cursor-default"
              style={{ ...ring(on), opacity: locked ? 0.45 : undefined }}
            >
              <span aria-hidden className="block h-full w-full" style={{ background: c, clipPath: CHAMFER_CLIP }} />
            </button>
          )
        })}
      </div>
      {locked ? (
        <p data-testid="person-color-needs-newer" className="m-0 mt-1 text-[11px] text-[var(--color-text-muted)]">
          {COLOR_NEEDS_NEWER_SERVER}
        </p>
      ) : null}
      {view.person === 'owner' ? (
        <p className="m-0 mt-1 text-[11px] text-[var(--color-text-muted)]">Your linked logins show this colour too.</p>
      ) : null}
      {writeError ? (
        <p role="alert" className="m-0 mt-1 text-[11px] text-[var(--color-status-error-soft)]">
          {writeError}
        </p>
      ) : null}
    </div>
  )
}
