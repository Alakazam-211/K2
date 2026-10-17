// "Colour" in the profile modal (0.46.0, Rosson 2026-10-17: "give the user
// a hex colour option rather than the colours displayed"). Whoever is
// signed in picks their own:
//
//   - Automatic: the name hash over the Automatic palette (the default);
//   - a colour well (native `<input type="color">`) and a `#RRGGBB` field
//     (validated, case-insensitive, saved lowercase);
//   - a live preview chip with the person's initials, in the readable ink.
//
// For the owner token or an owner-linked login it is the owner's colour,
// which the linked logins show too. The daemon stores and validates it
// (`POST /cli/people/color`, any `#rrggbb`) and re-broadcasts the presence
// roster, so every chip on every client repaints. Saving is optimistic and
// rolls back on a refusal. An older daemon without the route shows the
// controls disabled with "Needs a newer K2 server".

import React, { useEffect, useRef, useState } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { PersonNames } from '@/lib/person-name'
import { PresenceChip, normalizeHexColor, type PresenceRole } from './PresenceChip'
import { fetchPersonColor, setPersonColor, type PersonColorView } from './person-color-api'

/** The note under the disabled controls on a server without the route. */
export const COLOR_NEEDS_NEWER_SERVER = 'Needs a newer K2 server.'
/** The hex field's message for anything that isn't `#RRGGBB`. */
export const HEX_FORMAT_HINT = 'Use #RRGGBB, for example #3a9e96.'

/** How long the colour well waits after the last drag step before saving. */
const WELL_SAVE_DELAY_MS = 400

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
  previewName = 'owner',
  previewRole = 'member',
  previewNames,
}: {
  scope: ServerScope
  /** The current view (null: an older daemon without the route), so a
   *  parent can paint its own chip in the picked colour, live. */
  onView?: (view: PersonColorView | null) => void
  /** Who the preview chip is (initials). */
  previewName?: string
  previewRole?: PresenceRole
  previewNames?: PersonNames | null
}): React.JSX.Element | null {
  const [state, setState] = useState<State>({ kind: 'loading' })
  const [busy, setBusy] = useState(false)
  const [writeError, setWriteError] = useState<string | null>(null)
  // What the well and the hex field show while editing (null: follow the view).
  const [draft, setDraft] = useState<string | null>(null)
  const [hexText, setHexText] = useState<string | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  useEffect(() => {
    if (!onView) return
    if (state.kind === 'ok') onView(draft ? { ...state.view, color: draft } : state.view)
    else if (state.kind === 'unsupported') onView(null)
  }, [state, draft, onView])

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

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current)
    },
    [],
  )

  if (state.kind === 'loading') return <span className="text-[11px] text-[var(--color-text-muted)]">Loading…</span>
  if (state.kind === 'error') {
    return <span className="text-[11px] text-[var(--color-status-error-soft)]">Couldn’t load: {state.message}</span>
  }
  const locked = state.kind === 'unsupported'
  const view: PersonColorView | null = state.kind === 'ok' ? state.view : null
  const effective = draft ?? view?.color ?? null
  const auto = view !== null && view.chosen === null && draft === null

  const save = async (color: string | null): Promise<void> => {
    if (!view || locked) return
    if (color === view.chosen) {
      setDraft(null)
      return
    }
    setBusy(true)
    setWriteError(null)
    // Optimistic: show the pick at once; roll back on a refusal.
    setState({ kind: 'ok', view: { ...view, chosen: color, color: color ?? view.color } })
    try {
      setState({ kind: 'ok', view: await setPersonColor(scope, color) })
    } catch (e) {
      setState({ kind: 'ok', view })
      setWriteError(message(e))
    } finally {
      setDraft(null)
      setHexText(null)
      setBusy(false)
    }
  }

  const onWell = (raw: string): void => {
    const hex = normalizeHexColor(raw)
    if (!hex) return
    setDraft(hex)
    setHexText(null)
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => void save(hex), WELL_SAVE_DELAY_MS)
  }

  const typed = hexText ?? (view?.chosen ?? '')
  const typedValid = hexText === null || normalizeHexColor(hexText) !== null
  const commitHex = (): void => {
    if (hexText === null) return
    const hex = normalizeHexColor(hexText)
    if (!hex) return
    void save(hex)
  }

  const field =
    'h-6 px-2 text-xs font-mono bg-[var(--color-bg-surface)] border text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] disabled:opacity-50'

  return (
    <div data-testid="person-color-picker" className="flex flex-col gap-1 min-w-0">
      <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Your colour">
        <PresenceChip
          name={previewName}
          role={previewRole}
          names={previewNames}
          size={24}
          color={effective}
          title=""
        />
        <button
          type="button"
          data-testid="person-color-auto"
          aria-pressed={auto}
          disabled={busy || locked}
          onClick={() => {
            if (timer.current) clearTimeout(timer.current)
            void save(null)
          }}
          title="Automatic: a colour from your name"
          className={`h-6 px-2 text-[11px] border cursor-pointer disabled:cursor-default disabled:opacity-50 rounded-[var(--radius-field)] ${
            auto
              ? 'border-[var(--color-text-primary)] text-[var(--color-text-primary)]'
              : 'border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
          }`}
        >
          Automatic
        </button>
        <input
          type="color"
          data-testid="person-color-well"
          aria-label="Pick a colour"
          disabled={busy || locked}
          value={effective ?? '#000000'}
          onChange={(e) => onWell(e.target.value)}
          className="h-6 w-8 p-0 border border-[var(--color-border)] bg-transparent cursor-pointer disabled:cursor-default disabled:opacity-50"
        />
        <input
          type="text"
          data-testid="person-color-hex"
          aria-label="Hex colour"
          aria-invalid={!typedValid}
          placeholder={auto && view ? view.color : '#RRGGBB'}
          spellCheck={false}
          autoCapitalize="none"
          maxLength={7}
          disabled={busy || locked}
          value={typed}
          onChange={(e) => setHexText(e.target.value)}
          onBlur={commitHex}
          onKeyDown={(e) => {
            if (e.key === 'Enter') commitHex()
            if (e.key === 'Escape') {
              e.stopPropagation()
              setHexText(null)
            }
          }}
          className={`${field} w-24 ${typedValid ? 'border-[var(--color-border)]' : 'border-[var(--color-status-error-soft)]'}`}
        />
      </div>
      {!typedValid ? (
        <p data-testid="person-color-hex-hint" className="m-0 text-[11px] text-[var(--color-status-error-soft)]">
          {HEX_FORMAT_HINT}
        </p>
      ) : null}
      {locked ? (
        <p data-testid="person-color-needs-newer" className="m-0 text-[11px] text-[var(--color-text-muted)]">
          {COLOR_NEEDS_NEWER_SERVER}
        </p>
      ) : null}
      {view?.person === 'owner' ? (
        <p className="m-0 text-[11px] text-[var(--color-text-muted)]">Your linked logins show this colour too.</p>
      ) : null}
      {writeError ? (
        <p role="alert" className="m-0 text-[11px] text-[var(--color-status-error-soft)]">
          {writeError}
        </p>
      ) : null}
    </div>
  )
}
