// The frame both agent-template sheets share (Create an agent, Save as
// template): the Kessel sheet's frame and step pattern on the shared
// DialogFrame, with the Settings dialogs' (AccessSheet) footer. One width,
// centred in the window; the step, title and its line pinned beside the ×;
// the body scrolls; the footer (an optional status line, then the buttons)
// stays in view. With `onSubmit` the body and footer are one form, so Enter
// answers it. Portalled to <body>.
//
// Each step's heading takes focus when the step changes (`focusKey`), so a
// screen reader hears where it is. Esc, the × and a press outside cancel,
// all three held while `busy` (with `heldReason` as the ×'s tooltip).
//
// Look: no new visual design (K2 Design polishes it).

import { useEffect, useId, useRef } from 'react'
import { createPortal } from 'react-dom'
import { DialogFrame, DialogScrim } from '@/components/ui'
import { DIALOG_MUTED } from '@/components/Settings/sections/AccessDialogs'

export function TemplateSheetFrame({
  testId,
  title,
  step,
  intro,
  focusKey,
  busy,
  heldReason,
  onCancel,
  onSubmit,
  status,
  footer,
  children,
}: {
  testId: string
  title: string
  /** "Step 2 of 4". */
  step?: { at: number; of: number }
  intro?: string
  /** A new value moves focus to the heading (each step, each state). */
  focusKey: string
  busy: boolean
  heldReason: string
  onCancel: () => void
  onSubmit?: () => void
  status?: React.ReactNode
  footer: React.ReactNode
  children: React.ReactNode
}): React.JSX.Element {
  const titleId = useId()
  const introId = useId()
  const heading = useRef<HTMLHeadingElement>(null)
  useEffect(() => {
    heading.current?.focus()
  }, [focusKey])
  const inner = (
    <>
      <div className="min-h-0 flex-1 overflow-y-auto px-5 pt-1 pb-1 space-y-4" data-testid={`${testId}-body`}>
        {children}
      </div>
      <div className="flex-shrink-0 flex items-center gap-3 px-5 pt-3 pb-5 border-t border-transparent" data-testid={`${testId}-footer`}>
        <div className="min-w-0 flex-1">{status}</div>
        <div className="flex flex-shrink-0 justify-end gap-2">{footer}</div>
      </div>
    </>
  )
  // Portalled to <body>, so the scrim covers the whole window wherever the
  // sheet opens from (Settings' panel included), as AccessSheet does.
  return createPortal(
    <>
      <DialogScrim />
      <DialogFrame
        onCancel={onCancel}
        cancelHeld={busy}
        cancelHeldReason={busy ? heldReason : undefined}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={intro ? introId : undefined}
        aria-busy={busy || undefined}
        data-testid={testId}
        className="w-[min(36rem,calc(100vw-2.5rem))] max-h-[calc(100vh-2.5rem)] flex flex-col"
      >
        <div className="flex-shrink-0 px-5 pt-5 pb-3 pr-10">
          {step ? (
            <p className="type-hint text-[var(--color-text-secondary)] mb-1" data-testid={`${testId}-step`}>
              Step {step.at} of {step.of}
            </p>
          ) : null}
          <h2
            ref={heading}
            id={titleId}
            tabIndex={-1}
            className="m-0 type-heading text-[var(--color-text-primary)] outline-none"
            data-testid={`${testId}-title`}
          >
            {title}
          </h2>
          {intro ? (
            <p id={introId} className={`${DIALOG_MUTED} mt-1`} data-testid={`${testId}-intro`}>
              {intro}
            </p>
          ) : null}
        </div>
        {onSubmit ? (
          <form
            className="flex min-h-0 flex-1 flex-col"
            noValidate
            onSubmit={(e) => {
              e.preventDefault()
              onSubmit()
            }}
          >
            {inner}
          </form>
        ) : (
          <div className="flex min-h-0 flex-1 flex-col">{inner}</div>
        )}
      </DialogFrame>
    </>,
    document.body,
  )
}

/** A labelled group of review lines (Files, Context, …). Hidden when empty
 *  only if the caller says so; the Review shows everything it's given. */
export function ReviewGroup({ title, testId, children }: { title: string; testId?: string; children: React.ReactNode }): React.JSX.Element {
  const id = useId()
  return (
    <section aria-labelledby={id} data-testid={testId}>
      <h3 id={id} className="type-row text-[var(--color-text-primary)] mb-1">
        {title}
      </h3>
      {children}
    </section>
  )
}

/** One review line: what, then its state quietly. */
export function ReviewLine({ what, state, note, testId }: { what: React.ReactNode; state?: string; note?: string; testId?: string }): React.JSX.Element {
  return (
    <li className="flex items-baseline justify-between gap-3 py-1" data-testid={testId}>
      <span className="min-w-0 type-hint text-[var(--color-text-primary)] font-mono [overflow-wrap:anywhere]">{what}</span>
      {state || note ? (
        <span className="flex-shrink-0 type-hint text-[var(--color-text-secondary)] text-right">
          {state}
          {note ? <span className="block text-[var(--color-text-muted)]">{note}</span> : null}
        </span>
      ) : null}
    </li>
  )
}
