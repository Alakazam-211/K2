// Tickets board — the list's body: its sections of cards, or what stands in
// for them (loading, nothing to show, or the list failed to load).
//
// The cards are one listbox, the first step of the board's flow (pick a
// ticket, read its report, discuss it). One card is in the tab order (the
// open ticket, else the first); ↑ and ↓ move to the next card and open it
// straight into the report, Home and End jump to the ends, and Enter or
// Space opens the card in focus. Each section is a group named by its
// header.
//
// A failed load is never also an empty list: with nothing loaded the list
// says it couldn't load them, with Try again; with an earlier list loaded,
// that list stays, under a slim line with Try again.

import React, { useCallback, useId, useRef } from 'react'
import type { FeedbackListRow, TicketScope } from './feedback-api'
import { FeedbackCard, SectionHeader } from './TicketCard'
import { ProjectStateCard, StateRetry } from '@/components/Projects/StateCard'

export interface TicketSection {
  label: string
  rows: FeedbackListRow[]
}

/** The default no-tickets hint: what the page is for, and how tickets
 *  arrive (agents file them with the CLI). */
export const DEFAULT_TICKETS_EMPTY_HINT: React.ReactNode = (
  <>
    Agents ask you things here with <code className="font-mono">k2 tickets ask</code>. New tickets show up as they’re
    filed.
  </>
)

/** The card that holds the list's one tab stop: the open ticket when it's in
 *  the list, else the first card. */
export function ticketTabStopId(ordered: readonly FeedbackListRow[], selectedId: string | null): string | null {
  if (selectedId && ordered.some((r) => r.id === selectedId)) return selectedId
  return ordered[0]?.id ?? null
}

/** Where a key moves the list's focus from a card (null: the key isn't the
 *  list's, or there's nowhere to go). */
export function ticketListKeyTarget(
  ordered: readonly FeedbackListRow[],
  fromId: string,
  key: string,
): string | null {
  const at = ordered.findIndex((r) => r.id === fromId)
  if (at < 0) return null
  const to =
    key === 'ArrowDown' ? at + 1 : key === 'ArrowUp' ? at - 1 : key === 'Home' ? 0 : key === 'End' ? ordered.length - 1 : -1
  if (to < 0 || to >= ordered.length || to === at) return null
  return ordered[to].id
}

export function TicketListBody({
  rows,
  error,
  sections,
  ordered,
  selectedId,
  onSelect,
  nowSec,
  onMutated,
  onRetry,
  retrying,
  emptyList,
  scope,
  search,
  setScope,
  otherWaiting,
  emptyHint,
}: {
  /** All rows the board was given (`null` = still loading). */
  rows: FeedbackListRow[] | null
  error: string | null
  sections: TicketSection[]
  /** Every card in list order (the sections joined). */
  ordered: FeedbackListRow[]
  selectedId: string | null
  onSelect: (id: string) => void
  nowSec: number
  onMutated: () => void
  onRetry: () => void
  retrying: boolean
  /** The filters left nothing (and the list did load). */
  emptyList: boolean
  scope: TicketScope
  search: string
  setScope: (scope: TicketScope) => void
  otherWaiting: number
  emptyHint: React.ReactNode
}): React.JSX.Element {
  const listRef = useRef<HTMLDivElement>(null)
  const idBase = useId()
  const tabStop = ticketTabStopId(ordered, selectedId)

  const onKeyDown = useCallback(
    (e: React.KeyboardEvent<HTMLDivElement>): void => {
      const card = (e.target as HTMLElement).closest<HTMLElement>('[role="option"][data-ticket-id]')
      const id = card?.dataset.ticketId
      if (!card || !id || e.metaKey || e.ctrlKey || e.altKey) return
      if (e.key === 'Enter' || e.key === ' ') {
        e.preventDefault()
        onSelect(id)
        return
      }
      const next = ticketListKeyTarget(ordered, id, e.key)
      if (!next) {
        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') e.preventDefault()
        return
      }
      e.preventDefault()
      onSelect(next)
      Array.from(listRef.current?.querySelectorAll<HTMLElement>('[role="option"][data-ticket-id]') ?? [])
        .find((el) => el.dataset.ticketId === next)
        ?.focus()
    },
    [ordered, onSelect],
  )

  const failedEmpty = error !== null && (rows === null || rows.length === 0)

  return (
    <>
      {failedEmpty && (
        <div className="flex flex-1 items-center justify-center py-6">
          <ProjectStateCard
            kind="error"
            title="Couldn’t load tickets"
            detail={error}
            testId="ticket-list-error"
            action={<StateRetry onClick={onRetry} busy={retrying} />}
          >
            The server didn’t send the list. Your agents are fine.
          </ProjectStateCard>
        </div>
      )}
      {error !== null && !failedEmpty && (
        <div
          role="alert"
          data-testid="ticket-list-stale"
          className="mx-1 mt-2 flex items-center gap-2 type-hint text-[var(--color-status-error-text)]"
        >
          <span className="min-w-0 flex-1 truncate" title={error}>
            Couldn’t refresh the list. This is the last one loaded.
          </span>
          <button
            type="button"
            onClick={onRetry}
            disabled={retrying}
            className="k2-focus flex-shrink-0 text-[var(--color-accent-text)] hover:underline cursor-pointer disabled:opacity-50"
          >
            {retrying ? 'Trying again…' : 'Try again'}
          </button>
        </div>
      )}
      {rows === null && error === null && (
        <div className="px-1 py-8 text-center type-row text-[var(--color-text-muted)]">Loading tickets…</div>
      )}
      {emptyList && error === null && (
        <div data-testid="ticket-list-empty" className="flex flex-1 flex-col items-center justify-center text-center px-4">
          {rows !== null && rows.length > 0 && scope === 'waiting' && !search ? (
            <div data-testid="ticket-waiting-empty" className="flex flex-col items-center">
              <p className="type-name text-[var(--color-text-secondary)]">Nothing is waiting on you</p>
              <p className="type-hint text-[var(--color-text-muted)] mt-1">
                {otherWaiting > 0
                  ? `${otherWaiting} other waiting ${otherWaiting === 1 ? 'ticket is' : 'tickets are'} assigned to someone else or no one.`
                  : 'Tickets assigned to you that need an answer or a discussion show up here.'}
              </p>
              <button
                type="button"
                data-testid="ticket-waiting-empty-all"
                onClick={() => setScope('all')}
                className="k2-focus mt-1.5 type-hint text-[var(--color-accent-text)] hover:underline cursor-pointer"
              >
                Show all tickets
              </button>
            </div>
          ) : rows !== null && rows.length > 0 ? (
            <>
              <p className="type-name text-[var(--color-text-secondary)]">
                {scope === 'all' || search ? 'No tickets match your search' : 'Nothing assigned to you here'}
              </p>
              {scope !== 'all' && (
                <button
                  type="button"
                  onClick={() => setScope('all')}
                  className="k2-focus mt-1.5 type-hint text-[var(--color-accent-text)] hover:underline cursor-pointer"
                >
                  Show all tickets
                </button>
              )}
            </>
          ) : (
            <>
              <p className="type-name text-[var(--color-text-secondary)]">No tickets yet</p>
              <p data-testid="ticket-list-empty-hint" className="type-hint text-[var(--color-text-muted)] mt-1">
                {emptyHint}
              </p>
            </>
          )}
        </div>
      )}
      {ordered.length > 0 && (
        <div
          ref={listRef}
          role="listbox"
          aria-label="Tickets"
          data-ticket-listbox=""
          onKeyDown={onKeyDown}
        >
          {sections.map((section, i) =>
            section.rows.length > 0 ? (
              <div key={section.label} role="group" aria-labelledby={`${idBase}-s${i}`}>
                <SectionHeader id={`${idBase}-s${i}`} label={section.label} count={section.rows.length} />
                <div className="flex flex-col gap-1.5">
                  {section.rows.map((row) => (
                    <FeedbackCard
                      key={row.id}
                      row={row}
                      nowSec={nowSec}
                      selected={selectedId === row.id}
                      tabStop={tabStop === row.id}
                      onSelect={() => onSelect(row.id)}
                      onMutated={onMutated}
                    />
                  ))}
                </div>
              </div>
            ) : null,
          )}
        </div>
      )}
    </>
  )
}
