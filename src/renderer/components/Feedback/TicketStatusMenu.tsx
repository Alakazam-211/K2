// The Tickets board's one status menu: the same rows, in the same order, from
// a card's status chip and from the open ticket's status line. The rows are
// every status the daemon keeps (TICKET_STATUSES), named as the list's
// sections are; a ticket whose workspace is gone gets only Resolved and
// Dismissed. Answered asks what was agreed first, because the daemon won't
// set it without an answer.
//
// The box floats from its trigger (the app's anchored menu), Frost in Glass
// as the app's other menus over content, cut in Terminal; focus goes into it
// and ↑ / ↓ move (ticket-menu.ts).

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { FLOAT_CLEAR, floatMaterial, MENU_LIST, MENU_ROW } from '@/components/ui/float-material'
import { Button, Input } from '@/components/ui'
import { CheckIcon } from '@/components/icons/K2Icons'
import { focusMenuStart, onMenuKeyDown, TICKET_MENU_BOX, TICKET_MENU_ROW } from './ticket-menu'
import { resolveFeedback, statusLabel, type FeedbackStatus, type TicketStatus } from './feedback-api'

/** The picked row's mark, the same in every ticket menu. */
export function MenuCheck(): React.JSX.Element {
  return <CheckIcon className="w-3 h-3 flex-shrink-0 text-[var(--color-accent-text)]" />
}

/** The Tickets board's floating menus are Frost in Glass, clear enough to
 *  show the list or report they belong to (the app's menu over its own
 *  content, FLOAT_CLEAR); Terminal keeps its opaque cut box. */
export const TICKET_FLOAT = { ...floatMaterial('menu', 'frost'), ...FLOAT_CLEAR }

/** What a status row says on hover. */
function statusTitle(s: TicketStatus): string | undefined {
  if (s === 'waiting') return 'Reopen: back to waiting on an answer'
  if (s === 'answered') return 'Say what was agreed, then mark it answered'
  return undefined
}

export function TicketStatusMenu({
  ticketId,
  status,
  currentAnswer,
  statuses,
  onChanged,
  onClose,
  setBox,
  style,
  placement,
  testId,
}: {
  ticketId: string
  status: FeedbackStatus
  currentAnswer: string | null
  statuses: readonly TicketStatus[]
  onChanged: () => void
  onClose: () => void
  /** The anchored menu's ref. */
  setBox: (el: HTMLDivElement | null) => void
  style: React.CSSProperties
  placement: string
  testId: string
}): React.JSX.Element {
  const [busy, setBusy] = useState(false)
  const [menuError, setMenuError] = useState<string | null>(null)
  const [answering, setAnswering] = useState(false)
  const [answer, setAnswer] = useState(currentAnswer ?? '')
  const box = useRef<HTMLDivElement | null>(null)
  const ref = useCallback(
    (el: HTMLDivElement | null) => {
      box.current = el
      setBox(el)
    },
    [setBox],
  )

  // Into the menu as it opens (the picked row, else the first).
  useEffect(() => {
    const frame = requestAnimationFrame(() => focusMenuStart(box.current))
    return () => cancelAnimationFrame(frame)
  }, [])
  // Capture-phase so Esc closes this menu before the board's Esc.
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      e.stopPropagation()
      onClose()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  const apply = async (next: TicketStatus, agreed?: string): Promise<void> => {
    if (busy) return
    setBusy(true)
    setMenuError(null)
    try {
      if (next === 'answered') await resolveFeedback(ticketId, next, agreed)
      else await resolveFeedback(ticketId, next)
      onChanged()
      onClose()
    } catch (e) {
      setMenuError(`The status didn’t change. ${e instanceof Error ? e.message : String(e)}`)
    } finally {
      setBusy(false)
    }
  }

  const choose = (next: TicketStatus): void => {
    if (next === status) {
      onClose()
      return
    }
    if (next === 'answered') {
      setAnswer(currentAnswer ?? '')
      setAnswering(true)
      return
    }
    void apply(next)
  }

  return (
    <div
      ref={ref}
      role="menu"
      aria-label="Change status"
      data-testid={testId}
      data-placement={placement}
      data-k2-chamfer="surface"
      style={style}
      onKeyDown={(e) => onMenuKeyDown(e, box.current, onClose)}
      // A click in the menu never reaches the card under the trigger.
      onClick={(e) => e.stopPropagation()}
      {...TICKET_FLOAT}
      {...MENU_LIST}
      className={`min-w-[180px] ${TICKET_MENU_BOX}`}
    >
      {statuses.map((s) => {
        const current = status === s
        return (
          <button
            key={s}
            type="button"
            role="menuitemradio"
            aria-checked={current}
            data-testid={`ticket-status-option-${s}`}
            disabled={busy}
            onClick={() => choose(s)}
            title={statusTitle(s)}
            data-k2-focus=""
            {...MENU_ROW}
            className={`${TICKET_MENU_ROW} ${
              current ? 'text-[var(--color-text-primary)] bg-[var(--color-wash-1)]' : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            <span className="flex-1">{statusLabel(s)}</span>
            {current && <MenuCheck />}
          </button>
        )
      })}
      {answering && (
        <div className="border-t border-[var(--color-border)] mt-0.5 px-3 py-2">
          <label className="block mb-1 type-hint text-[var(--color-text-secondary)]" htmlFor={`answered-${testId}-${ticketId}`}>
            What was agreed?
          </label>
          <Input
            id={`answered-${testId}-${ticketId}`}
            data-testid="ticket-answered-input"
            autoFocus
            value={answer}
            onChange={(e) => setAnswer(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && answer.trim()) {
                e.preventDefault()
                void apply('answered', answer.trim())
              }
            }}
            className="w-full"
          />
          <div className="mt-2 flex justify-end gap-2">
            <Button variant="ghost" onClick={() => setAnswering(false)}>
              Cancel
            </Button>
            <Button
              variant="accent"
              data-testid="ticket-answered-save"
              disabled={busy || answer.trim().length === 0}
              onClick={() => void apply('answered', answer.trim())}
            >
              Mark answered
            </Button>
          </div>
        </div>
      )}
      {menuError && (
        <div role="alert" className="px-3 py-1 type-hint text-[var(--color-status-error-text)] selectable-copy">
          {menuError}
        </div>
      )}
    </div>
  )
}
