// Settings → Admin Access and User Access: the dialog frame both pages use,
// its parts, and the one Reset password dialog they share, so the two
// can't drift. Each dialog is the shared Dialog (scrim + frame), portalled,
// one width, centred in the window; labelled full-width fields; the
// server's reason when a write fails; a done step that says what happened
// and what to do next. A password is shown once, in the open dialog only:
// closing drops it.

import React, { useCallback, useEffect, useId, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Button, Callout, DialogFrame, DialogScrim, Input, Radio, Surface } from '@/components/ui'
import { SETTING_ROW_LABEL } from '../controls/SettingControls'

export const DIALOG_LABEL = 'block type-row text-[var(--color-text-primary)] mb-1'
export const DIALOG_FIELD = 'w-full type-control'
export const DIALOG_MUTED = 'type-hint text-[var(--color-text-secondary)] leading-relaxed'

/**
 * A password someone else set is temporary (0.46.6 must-change): said
 * wherever Settings shows or confirms one, when the daemon says so.
 */
export const MUST_CHANGE_NOTE = 'It’s temporary: the next time they sign in, K2 asks them to choose their own.'

/** Which edges of a scroll box have content past them. */
function useScrollEdges(): {
  ref: React.RefObject<HTMLDivElement | null>
  edges: { top: boolean; bottom: boolean }
  measure: () => void
} {
  const ref = useRef<HTMLDivElement>(null)
  const [edges, setEdges] = useState({ top: false, bottom: false })
  const measure = useCallback(() => {
    const el = ref.current
    if (!el) return
    const top = el.scrollTop > 0
    const bottom = el.scrollTop + el.clientHeight < el.scrollHeight - 1
    setEdges((prev) => (prev.top === top && prev.bottom === bottom ? prev : { top, bottom }))
  }, [])
  useEffect(() => {
    const el = ref.current
    if (!el) return
    measure()
    if (typeof ResizeObserver === 'undefined') return
    // The window, or the form inside (an error, a section opening), resizing.
    const ro = new ResizeObserver(measure)
    ro.observe(el)
    if (el.firstElementChild) ro.observe(el.firstElementChild)
    return () => ro.disconnect()
  }, [measure])
  return { ref, edges, measure }
}

/** The shared Dialog with a title, a body that scrolls, and Esc to close
 *  (off while a write is in flight). The title and intro stay at the top
 *  and the `footer` (the dialog's buttons) at the bottom at any window
 *  height; only the body between them scrolls, with a hairline on an edge
 *  that has more past it. */
export function AccessSheet({
  title,
  intro,
  onClose,
  busy = false,
  pressOutside = true,
  testId,
  footer,
  children,
}: {
  title: string
  intro?: React.ReactNode
  onClose: () => void
  busy?: boolean
  /** False: a press outside does nothing (Esc and the × still close). */
  pressOutside?: boolean
  testId: string
  /** The buttons, kept in view under the scrolling body. A submit button
   *  here names its form with `form="<id>"`. */
  footer?: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  const titleId = useId()
  const body = useScrollEdges()
  // Esc, the × and a press outside close it (dialog-cancel leaves Esc to an
  // open menu inside first: App, Role); held while a write is in flight.
  const edge = (on: boolean): string => (on ? 'border-[var(--color-border)]' : 'border-transparent')
  // Portalled to <body>, so the scrim covers the whole window, not just the
  // Settings panel it opens from. Every dialog is one width and opens
  // centred in the window, whatever its height (graders read a top-anchored
  // short confirmation as sitting in a different place from a tall form).
  // A tall one scrolls between its title and its buttons.
  return createPortal(
    <>
      <DialogScrim />
      <DialogFrame
        onCancel={onClose}
        cancelHeld={busy}
        cancelHeldReason="Can’t close while this change saves"
        cancelOnPressOutside={pressOutside}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid={testId}
        className="w-[min(36rem,calc(100vw-2.5rem))] max-h-[calc(100vh-2.5rem)] flex flex-col"
      >
        {/* The title stays put beside the frame's ×; the body scrolls. */}
        <div className={`flex-shrink-0 px-5 pt-5 pb-3 pr-10 border-b ${edge(body.edges.top)}`} data-testid="dialog-head">
          <h3 id={titleId} className="type-heading text-[var(--color-text-primary)]">
            {title}
          </h3>
          {intro ? <p className={`${DIALOG_MUTED} mt-1`}>{intro}</p> : null}
        </div>
        <div
          ref={body.ref}
          onScroll={body.measure}
          className={`min-h-0 overflow-y-auto px-5 pt-1 ${footer ? 'pb-1' : 'pb-5'}`}
          data-testid="dialog-body"
        >
          <div className="space-y-4">{children}</div>
        </div>
        {footer ? (
          <div className={`flex-shrink-0 flex justify-end gap-2 px-5 pt-3 pb-5 border-t ${edge(body.edges.bottom)}`} data-testid="dialog-foot">
            {footer}
          </div>
        ) : null}
      </DialogFrame>
    </>,
    document.body,
  )
}

/** A labelled field with its hint or problem under it. */
export function Field({
  label,
  hint,
  problem,
  children,
  htmlFor,
}: {
  label: string
  hint?: React.ReactNode
  problem?: string | null
  children: React.ReactNode
  htmlFor: string
}): React.JSX.Element {
  return (
    <div>
      <label htmlFor={htmlFor} className={DIALOG_LABEL}>
        {label}
      </label>
      {children}
      {problem ? (
        <p className="type-hint text-[var(--color-status-error-soft)] mt-1" data-field-problem="">
          {problem}
        </p>
      ) : hint ? (
        <p className={`${DIALOG_MUTED} mt-1`}>{hint}</p>
      ) : null}
    </div>
  )
}

/** A failed write: what failed, what to do now, then the server's own
 *  reason, quietly. */
export function FailedNote({ what, detail, next }: { what: string; detail: string; next: string }): React.JSX.Element {
  // At the foot of a long form: bring it into view so the failure is seen.
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.scrollIntoView?.({ block: 'nearest' })
  }, [detail])
  return (
    <div ref={ref}>
      <Callout tone="error" role="alert" className="type-hint space-y-0.5" data-testid="dialog-error">
        <p className="font-medium">{what}</p>
        <p>{next}</p>
        <p>The server said: {detail.trim().replace(/[.\s]+$/, '')}.</p>
      </Callout>
    </div>
  )
}

/** A value to hand over (address, username, password) with Copy. */
export function CopyLine({ label, value, mono = true, testId }: { label: string; value: string; mono?: boolean; testId?: string }): React.JSX.Element {
  const [copy, setCopy] = useState<'idle' | 'done' | 'failed'>('idle')
  const doCopy = (): void => {
    const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
    if (!clip) return setCopy('failed')
    clip.writeText(value).then(
      () => setCopy('done'),
      () => setCopy('failed'),
    )
  }
  return (
    <div className="flex items-center justify-between gap-4 px-4 py-2.5">
      <div className="min-w-0">
        <p className="type-hint text-[var(--color-text-secondary)]">{label}</p>
        <p data-testid={testId} className={`type-row ${mono ? 'font-mono' : ''} break-all select-all text-[var(--color-text-primary)]`}>
          {value}
        </p>
        {copy === 'failed' ? <p className={DIALOG_MUTED}>Couldn’t copy. Select the text and copy it yourself.</p> : null}
      </div>
      <Button className="flex-shrink-0" aria-label={`Copy ${label.toLowerCase()}`} onClick={doCopy}>
        {copy === 'done' ? 'Copied' : 'Copy'}
      </Button>
    </div>
  )
}

/** Make one / Type one, and the typed field with Show. `rules` is the
 *  server's password rules in a sentence. */
export function PasswordChoice({
  name,
  makeHint,
  rules,
  mode,
  setMode,
  password,
  setPassword,
  problem,
}: {
  name: string
  /** What happens to a password K2 makes, in this dialog's words. */
  makeHint: string
  rules: string
  mode: 'make' | 'type'
  setMode: (m: 'make' | 'type') => void
  password: string
  setPassword: (v: string) => void
  problem: string | null
}): React.JSX.Element {
  const [shown, setShown] = useState(false)
  const id = useId()
  return (
    <fieldset className="space-y-2">
      <legend className={DIALOG_LABEL}>Password</legend>
      <div role="radiogroup" aria-label="How to set the password" className="space-y-2">
        <label className="flex items-start gap-2 cursor-pointer">
          <Radio name={name} className="mt-0.5" checked={mode === 'make'} onChange={() => setMode('make')} aria-label="Make one for me" />
          <span>
            <span className={SETTING_ROW_LABEL}>Make one for me</span>
            <span className={`block ${DIALOG_MUTED}`}>{makeHint}</span>
          </span>
        </label>
        <label className="flex items-start gap-2 cursor-pointer">
          <Radio name={name} className="mt-0.5" checked={mode === 'type'} onChange={() => setMode('type')} aria-label="Type one" />
          <span>
            <span className={SETTING_ROW_LABEL}>Type one</span>
            <span className={`block ${DIALOG_MUTED}`}>{rules}</span>
          </span>
        </label>
      </div>
      {mode === 'type' ? (
        <div className="pl-6">
          <label htmlFor={id} className="sr-only">
            Password
          </label>
          <div className="flex items-center gap-2">
            <Input
              id={id}
              type={shown ? 'text' : 'password'}
              autoComplete="new-password"
              autoFocus
              className={DIALOG_FIELD}
              aria-label="Password"
              aria-invalid={problem ? true : undefined}
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            <Button variant="quiet" className="flex-shrink-0" onClick={() => setShown((s) => !s)}>
              {shown ? 'Hide' : 'Show'}
            </Button>
          </div>
          {problem ? <p className="type-hint text-[var(--color-status-error-soft)] mt-1">{problem}</p> : null}
        </div>
      ) : null}
    </fieldset>
  )
}

/** What a reset gave back: the password to send (null when the server
 *  didn't return one), and whether it's temporary. */
export type ResetAnswer = { password: string | null; temporary: boolean }

/**
 * Reset someone's password, on either page: Make one for me (the default)
 * or Type one, submit, then the new password once with Copy and what to do
 * with it, then Done. A failure stays in the dialog with the server's
 * reason and keeps what was typed.
 */
export function PasswordResetDialog({
  who,
  intro,
  rules,
  problem,
  makeHint,
  signIn,
  extra,
  submit,
  onDone,
  onClose,
}: {
  who: string
  intro: React.ReactNode
  /** The server's password rules, in a sentence. */
  rules: string
  /** What's wrong with a typed password, or null. */
  problem: (password: string) => string | null
  makeHint: string
  /** Where they sign in with it, as a step: "They sign in to their Apps with it." */
  signIn: string
  /** Shown under the choice (a note, another way to reset). */
  extra?: React.ReactNode
  /** Sets it: `typed` null means make one. Throws with the server's reason. */
  submit: (typed: string | null) => Promise<ResetAnswer>
  /** The reset happened (re-read the list). */
  onDone: () => void
  onClose: () => void
}): React.JSX.Element {
  const [mode, setMode] = useState<'make' | 'type'>('make')
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState<ResetAnswer | null>(null)
  const formId = useId()
  const pwProblem = mode === 'type' && password ? problem(password) : null
  const ready = mode === 'make' || (!!password && !pwProblem)

  const run = async (): Promise<void> => {
    if (!ready || busy) return
    setBusy(true)
    setError(null)
    try {
      const answer = await submit(mode === 'make' ? null : password)
      setPassword('')
      setDone(answer)
      onDone()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  if (done) {
    return (
      <AccessSheet
        title={`New password for ${who}`}
        onClose={onClose}
        testId="reset-done-dialog"
        footer={
          <Button variant="accent" size="md" onClick={onClose} autoFocus>
            Done
          </Button>
        }
      >
        <p className={DIALOG_MUTED} data-testid="reset-password-msg">
          {done.temporary ? `Password reset for ${who}. ${MUST_CHANGE_NOTE}` : `Password reset for ${who}.`}
        </p>
        {done.password ? (
          <Surface chamfer material="tile">
            <CopyLine label="Password" value={done.password} testId="one-time-secret" />
          </Surface>
        ) : null}
        <div data-testid="reset-next-steps">
          <p className={DIALOG_LABEL}>Send it to {who}</p>
          <ol className={`${DIALOG_MUTED} list-decimal pl-5 space-y-0.5`}>
            <li>{done.password ? 'Copy the password' : 'Take the password you typed'} and send it to {who} somewhere only they can read it.</li>
            <li>{signIn}</li>
            {done.temporary ? <li>K2 then asks them to choose their own password.</li> : null}
          </ol>
        </div>
        {done.password ? <p className={DIALOG_MUTED}>K2 won’t show this password again: once you close this, it’s gone.</p> : null}
      </AccessSheet>
    )
  }

  return (
    <AccessSheet
      title={`Reset ${who}’s password`}
      intro={intro}
      onClose={onClose}
      busy={busy}
      testId={`reset-form-${who}`}
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button type="submit" form={formId} variant="accent" size="md" disabled={!ready || busy}>
            {busy ? 'Resetting…' : 'Reset password'}
          </Button>
        </>
      }
    >
      <form
        id={formId}
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          void run()
        }}
      >
        <PasswordChoice
          name="reset-password"
          makeHint={makeHint}
          rules={rules}
          mode={mode}
          setMode={setMode}
          password={password}
          setPassword={setPassword}
          problem={pwProblem}
        />
        {extra}
        {error ? <FailedNote what={`Couldn’t reset ${who}’s password.`} detail={error} next={mode === 'type' ? 'Their current password still works. Check what you typed, then try again.' : 'Their current password still works. Try again in a moment.'} /> : null}
      </form>
    </AccessSheet>
  )
}

/** A confirm-first question in the same frame and place as the page's
 *  other dialogs. Cancel has the focus, so Enter on a destructive question
 *  never confirms by accident. */
export function ConfirmSheet({
  title,
  message,
  confirmLabel,
  destructive = false,
  onConfirm,
  onCancel,
  testId,
}: {
  title: string
  message: React.ReactNode
  confirmLabel: string
  destructive?: boolean
  onConfirm: () => void
  onCancel: () => void
  testId: string
}): React.JSX.Element {
  return (
    <AccessSheet
      title={title}
      onClose={onCancel}
      testId={testId}
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onCancel} autoFocus>
            Cancel
          </Button>
          <Button variant={destructive ? 'danger-muted' : 'accent'} size="md" onClick={onConfirm}>
            {confirmLabel}
          </Button>
        </>
      }
    >
      <p className={DIALOG_MUTED}>{message}</p>
    </AccessSheet>
  )
}
