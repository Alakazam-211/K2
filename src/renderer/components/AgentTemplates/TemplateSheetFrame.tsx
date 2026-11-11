// The frame both agent-template sheets share (Create an agent, Save as
// template): the Kessel sheet's frame and step pattern on the shared
// DialogFrame, with the Settings dialogs' (AccessSheet) pinned head and
// footer. One width, centred in the window, and one height across every step
// and state (as the Kessel sheet holds its height), so the buttons never
// move; only the body between the head and the footer scrolls, with
// AccessSheet's hairline on an edge that has more past it. The step, title
// and its line sit beside the ×; the footer is an optional status line, then
// the buttons. With `onSubmit` the body and footer are one form, so Enter
// answers it. Portalled to <body>.
//
// Each step's heading takes focus when the step changes (`focusKey`), so a
// screen reader hears where it is. Esc, the × and a press outside cancel,
// all three held while `busy` (with `heldReason` as the ×'s tooltip).
//
// Also the parts both reviews draw with: a card of label / value rows
// (`ReviewCard`, `ReviewRow`) with an optional Edit that goes back to a step,
// and a titled section (`ReviewSection`).

import { useEffect, useId, useRef } from 'react'
import { createPortal } from 'react-dom'
import { Button, DialogFrame, DialogScrim, Surface } from '@/components/ui'
import { DIALOG_MUTED, useScrollEdges } from '@/components/Settings/sections/AccessDialogs'

/** The sheet's one height: the pick step's list and the review hold here and
 *  scroll; shorter steps leave the footer where it is. */
export const TEMPLATE_SHEET_HEIGHT = 'h-[min(40rem,calc(100vh-2.5rem))]'

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
  const body = useScrollEdges()
  useEffect(() => {
    heading.current?.focus()
  }, [focusKey])
  // A new step starts at its top, and its edges are measured afresh.
  const { ref: bodyRef, measure } = body
  useEffect(() => {
    if (bodyRef.current) bodyRef.current.scrollTop = 0
    measure()
  }, [focusKey, bodyRef, measure])
  const edge = (on: boolean): string => (on ? 'border-[var(--color-border)]' : 'border-transparent')
  const inner = (
    <>
      <div
        ref={body.ref}
        onScroll={body.measure}
        // Focus that lands in the body brings the field's own block into view
        // (AccessSheet's rule), so nothing focused hides behind the footer.
        onFocus={(e) => {
          const box = (e.target as HTMLElement).parentElement
          const scroller = e.currentTarget
          if (box && box !== scroller && box.offsetHeight < scroller.clientHeight) box.scrollIntoView?.({ block: 'nearest' })
        }}
        className="min-h-0 flex-1 overflow-y-auto px-5 pt-1 pb-4"
        data-testid={`${testId}-body`}
      >
        <div className="space-y-4">{children}</div>
      </div>
      <div
        className={`flex-shrink-0 flex items-center gap-3 px-5 pt-3 pb-5 border-t ${edge(body.edges.bottom)}`}
        data-testid={`${testId}-footer`}
        data-scrolled-under={body.edges.bottom ? '' : undefined}
      >
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
        className={`w-[min(36rem,calc(100vw-2.5rem))] ${TEMPLATE_SHEET_HEIGHT} flex flex-col`}
      >
        <div className={`flex-shrink-0 px-5 pt-5 pb-4 pr-10 border-b ${edge(body.edges.top)}`} data-testid={`${testId}-head`}>
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

/** A titled part of a review (What K2 sets up, It will ask you, …). */
export function ReviewSection({
  title,
  hint,
  testId,
  children,
}: {
  title: string
  hint?: React.ReactNode
  testId?: string
  children: React.ReactNode
}): React.JSX.Element {
  const id = useId()
  return (
    <section aria-labelledby={id} data-testid={testId}>
      <h3 id={id} className="type-row text-[var(--color-text-primary)]">
        {title}
      </h3>
      {hint ? <p className={`${DIALOG_MUTED} mt-0.5`}>{hint}</p> : null}
      <div className="mt-2">{children}</div>
    </section>
  )
}

/** Review rows on one card: chamfered in Terminal, a tile in Glass, hairlines
 *  between rows (a Settings card group's look). */
export function ReviewCard({ children, testId }: { children: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <Surface chamfer material="tile" data-testid={testId}>
      <dl className="m-0 divide-y divide-[var(--color-border)]">{children}</dl>
    </Surface>
  )
}

/** One review row: what it is, its value (one line or a list), and an
 *  optional Edit that goes back to the step that set it. */
export function ReviewRow({
  label,
  children,
  onEdit,
  editLabel,
  testId,
}: {
  label: string
  children: React.ReactNode
  onEdit?: () => void
  /** The Edit button's name for a screen reader ("Edit the name"). */
  editLabel?: string
  testId?: string
}): React.JSX.Element {
  return (
    <div className="flex items-start gap-3 px-4 py-2.5" data-testid={testId}>
      <dt className="w-24 flex-shrink-0 pt-px type-hint text-[var(--color-text-secondary)]">{label}</dt>
      <dd className="m-0 min-w-0 flex-1 flex items-start justify-between gap-3">
        <div className="min-w-0 flex-1 type-hint text-[var(--color-text-primary)] [overflow-wrap:anywhere]">{children}</div>
        {onEdit ? (
          <Button variant="quiet" className="-my-1 flex-shrink-0" onClick={onEdit} aria-label={editLabel} data-review-edit={label}>
            Edit
          </Button>
        ) : null}
      </dd>
    </div>
  )
}

/** A list inside a review row: one item a line, with an optional quiet note
 *  on its right (a size, "Will add"). */
export function ReviewList({ items, testId }: { items: Array<{ key: string; what: React.ReactNode; note?: React.ReactNode; mono?: boolean; testId?: string }>; testId?: string }): React.JSX.Element {
  return (
    <ul className="m-0 p-0 list-none space-y-1" data-testid={testId}>
      {items.map((x) => (
        <li key={x.key} className="flex items-baseline justify-between gap-3" data-testid={x.testId}>
          <span className={`min-w-0 [overflow-wrap:anywhere] ${x.mono ? 'font-mono' : ''}`}>{x.what}</span>
          {x.note ? <span className="flex-shrink-0 text-right text-[var(--color-text-secondary)]">{x.note}</span> : null}
        </li>
      ))}
    </ul>
  )
}
