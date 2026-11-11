// Setup checklist (prd-setup-checklist-v1): the setup a person still has
// to do on the window's server, kept with the main navigation until it's
// done or hidden. The daemon decides every row and who sees it
// (`GET /cli/first-run/status`); this draws that answer. Each row opens the
// step itself, each row can be skipped, and the whole list can be hidden.
//
// Shown in the Agents sidebar and the Home sidebar; the collapsed rails get
// `SetupChecklistRailButton`, a count of the rows left that opens the nav.
// Nothing renders while the server's switch is off, or once it's finished.
// Hidden, it sits as a small "Get started" bubble in the same place; the
// bubble brings the list back (`POST first-run/show`, as `k2 setup show`).
// The bubble isn't the list: the sidebar's floor, its collapse button and
// the ⌘B guard (`isSetupChecklistShowing`) only hold for the full list.
//
// Look (K2 Design): the nav's own section language. The header is the
// shared `NavSectionLabel` with the count left and a quiet Hide; a thin
// accent bar shows how far along it is. Each row is a status mark (round in
// Glass from `--radius-status`, cut in Terminal like the other small
// controls), the step, and a quiet Skip. The current step (the first row
// still to do) leads: its title is set heavier and its mark is the accent;
// the other rows to do stay quiet. A row's line and its note share one muted
// voice, with one gap between title, line and note.
//
// Skip (or a skipped row's SKIPPED tag) sits on the title's line, its right
// edge on Hide's; the title keeps clear of it and the line and note under it
// use the row's full width. While the checklist shows, the sidebar is never
// narrower than SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH (260), where every title
// fits one line in every Style. Skip is always shown and comes after its row
// in keyboard order. A skipped row reads through its mark and its tag, never
// by fading. Done rows drop their line and go muted. Under the pointer the
// muted words (and the marks) step up to secondary, so they keep 4.5:1 on
// the hover wash: the wash is the row's own :hover and the step-up is
// `group-hover/row`, a named group on that same row (not any outer group),
// so whenever the wash shows, so does the step-up. Focus is an inset ring:
// nothing moves. Tokens only, so every Style and palette draws it.
//
// States: nothing until the first answer (the switch may be off, so there is
// no placeholder to flash); a failed read or gesture after that keeps the
// last list and says so quietly, with Try again; when the person finishes
// with the list open, "You're set up" shows for a moment, then nothing.

import { useEffect, useRef, useState } from 'react'
import { Button, Tag, cx } from '@/components/ui'
import { ChecklistIcon } from '@/components/icons/K2Icons'
import { NavSectionLabel } from '@/components/Sidebar/NavSection'
import { primaryScope } from '@/kessel/server-scope'
import { openFirstRunTarget } from '@/lib/first-run-openers'
import { useSidebarStore } from '@/stores/sidebar'
import {
  rowsLeft,
  showsCollapsed,
  useFirstRunStore,
  visibleRows,
  watchFirstRun,
  type FirstRunNote,
  type FirstRunRow,
  type FirstRunRowId,
  type FirstRunRowState,
} from '@/stores/first-run'

/** The contract's copy: what each step is for, never where or how it looks. */
export const SETUP_CHECKLIST_COPY: Record<FirstRunRowId, { label: string; line: string }> = {
  llm_sign_in: { label: 'Sign in to an AI tool', line: 'Claude, Codex or Grok. Your agents think with it.' },
  chief_of_staff: { label: 'Hire your Chief of Staff', line: 'Your lead agent. K5 hires it with you.' },
  first_agent: { label: 'Make your first agent', line: 'Each workspace is an agent with its own folder.' },
  tunnel: { label: 'Link your subdomain', line: 'Gives this server an address of its own at k2.dev, so your phone and other people can reach it.' },
  phone: { label: 'Connect your phone', line: 'Talk to your agents from your phone.' },
  own_sign_in: { label: 'Make your own sign-in', line: 'Sign in from your phone and your other devices.' },
}

export const NEEDS_TUNNEL_NOTE = 'Away from home, your phone needs your subdomain first.'
export const NEEDS_K5_NOTE = 'You’ll meet K5 first.'

/** Each note's copy (a note this client doesn't know never reaches here). */
export const SETUP_NOTE_COPY: Record<FirstRunNote, string> = {
  needs_tunnel: NEEDS_TUNNEL_NOTE,
  needs_k5: NEEDS_K5_NOTE,
}

export const CHECKLIST_TITLE = 'Setup checklist'

/** A row's state, for screen readers (the mark draws it) and, for a
 *  skipped row, on screen too. */
const STATE_TEXT: Record<FirstRunRowState, string> = { done: 'Done', todo: 'To do', skipped: 'Skipped' }

/** Said once, when the person finishes with the list open. */
export const SET_UP_TEXT = 'You’re set up'
/** The header's count once every drawn row is done or skipped. */
export const ALL_DONE_TEXT = 'All done'
/** A read or a gesture failed; the last list stays. */
export const CHECKLIST_ERROR_TEXT = 'Couldn’t update the checklist.'

/** The hidden list's bubble: its label, and its accessible name. */
export const GET_STARTED_TEXT = 'Get started'
export const GET_STARTED_LABEL = 'Get started: show the setup list'

/** How long "You're set up" stays before the list goes. */
const SET_UP_MS = 4000

/** The window's server's checklist rows, watched while mounted (null when
 *  nothing should show). `finished`: the daemon marked the list finished
 *  (with the switch on and the list not hidden). `collapsed`: the person
 *  hid it with steps left, so it sits as the "Get started" bubble. */
export function useSetupChecklistRows(): {
  rows: FirstRunRow[] | null
  error: string | null
  finished: boolean
  collapsed: boolean
} {
  const scope = primaryScope()
  useEffect(() => watchFirstRun(scope), [scope])
  const entry = useFirstRunStore((s) => s.entries[scope.id])
  const doc = entry?.doc
  return {
    rows: visibleRows(doc),
    error: entry?.error ?? null,
    finished: !!doc && doc.enabled && !doc.hidden && doc.finished,
    collapsed: showsCollapsed(doc),
  }
}

function leftText(n: number): string {
  return n === 1 ? '1 left' : `${n} left`
}

/** The quiet text action (Hide, Skip, Try again): no fill or edge, muted
 *  until hovered; the focus ring is the list rows' (`data-k2-focus`). */
const QUIET_ACTION_CLASS =
  'relative no-drag flex-shrink-0 px-1.5 py-0.5 type-hint text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer'

/** A row's status mark: hollow to do (the accent for the current step),
 *  filled with a check when done, a dash when skipped. Round in Glass
 *  (`--radius-status`), cut in Terminal like the radios and checkboxes
 *  (`data-k2-chamfer="chip"`, `--chamfer-chip`). */
function StepMark({ state, current = false, lift = false }: { state: FirstRunRowState; current?: boolean; lift?: boolean }): React.JSX.Element {
  const base = 'mt-[3px] w-3 h-3 flex-shrink-0 flex items-center justify-center rounded-[var(--radius-status)]'
  if (state === 'done') {
    return (
      <span
        aria-hidden="true"
        data-step-mark="done"
        data-k2-chamfer="chip"
        className={cx(base, 'border border-[var(--color-status-ok)] bg-[var(--color-status-ok)] text-[var(--color-bg)]')}
      >
        <svg width="8" height="8" viewBox="0 0 8 8" fill="none" stroke="currentColor" strokeWidth={1.6}>
          <path d="M1.5 4.2 3.2 5.8 6.5 2.4" />
        </svg>
      </span>
    )
  }
  if (state === 'skipped') {
    return (
      <span aria-hidden="true" data-step-mark="skipped" data-k2-chamfer="chip" className={cx(base, 'border border-[var(--color-border-strong)]')}>
        <span className={cx('w-1.5 h-px bg-[var(--color-text-muted)]', lift && 'group-hover/row:bg-[var(--color-text-secondary)]')} />
      </span>
    )
  }
  return (
    <span
      aria-hidden="true"
      data-step-mark="todo"
      data-k2-chamfer="chip"
      className={cx(
        base,
        'border-[1.5px] [--k2-ch-bw:1.5px]',
        current ? 'border-[var(--color-accent)]' : cx('border-[var(--color-text-muted)]', lift && 'group-hover/row:border-[var(--color-text-secondary)]'),
      )}
    />
  )
}

/** The current step: the first row still to do (null when none is). */
export function currentRowId(rows: readonly FirstRunRow[]): FirstRunRowId | null {
  return rows.find((r) => r.state === 'todo')?.id ?? null
}

function ChecklistRow({ row, current }: { row: FirstRunRow; current: boolean }): React.JSX.Element {
  const copy = SETUP_CHECKLIST_COPY[row.id]
  const todo = row.state === 'todo'
  const skipped = row.state === 'skipped'
  const opens = row.opens !== null
  const note = row.note !== null && todo ? SETUP_NOTE_COPY[row.note] : null
  // Muted words step up to secondary under the pointer, so they keep 4.5:1
  // on the hover wash.
  const muted = cx('text-[var(--color-text-muted)]', opens && 'group-hover/row:text-[var(--color-text-secondary)]')
  return (
    <li
      // The wash is this row's own :hover; the step-up is `group-hover/row`,
      // i.e. the same row's :hover, so the two always show together.
      className={cx('group/row relative', opens && 'hover:bg-[var(--color-wash-2)] transition-colors')}
      data-setup-row={row.id}
      data-state={row.state}
      data-current={current ? '' : undefined}
    >
      <button
        type="button"
        data-testid={`setup-row-${row.id}`}
        data-k2-focus=""
        aria-current={current ? 'step' : undefined}
        className="relative no-drag w-full min-w-0 flex items-start gap-1.5 pl-3 pr-2 py-1.5 text-left cursor-pointer disabled:cursor-default"
        disabled={!opens}
        onClick={() => void openFirstRunTarget(row.opens)}
      >
        <StepMark state={row.state} current={current} lift={opens} />
        <span className="min-w-0 flex-1">
          {/* The title line: the title, then Skip (drawn over the row's
              top right, so the title keeps clear of it) or the SKIPPED tag.
              The line and note under it use the full width. */}
          <span className="flex items-start gap-2">
            <span
              className={cx(
                'min-w-0 flex-1 type-row leading-snug',
                todo && 'pr-10',
                current
                  ? 'font-semibold text-[var(--color-text-primary)]'
                  : todo
                    ? 'text-[var(--color-text-secondary)]'
                    : muted,
              )}
              data-setup-title=""
            >
              {copy.label}
            </span>
            {skipped ? (
              <Tag
                tone="muted"
                aria-hidden="true"
                className="mr-1.5 mt-px group-hover/row:[--k2-tag-ink:var(--color-text-secondary)]!"
                data-testid={`setup-skipped-${row.id}`}
              >
                {STATE_TEXT.skipped}
              </Tag>
            ) : null}
          </span>
          {/* Balanced, so a hint never leaves one word alone on its last line. */}
          {todo && <span className={cx('block type-hint leading-snug text-balance mt-0.5', muted)}>{copy.line}</span>}
          {note ? (
            <span className={cx('block type-hint leading-snug text-balance mt-0.5', muted)} data-setup-note={row.note}>
              {note}
            </span>
          ) : null}
        </span>
        <span className="sr-only">{STATE_TEXT[row.state]}</span>
      </button>
      {todo && (
        <button
          type="button"
          data-k2-focus=""
          className={cx(
            'absolute right-2 top-1 no-drag px-1.5 py-0.5 type-hint leading-snug text-[var(--color-text-muted)] transition-colors cursor-pointer',
            'group-hover/row:text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)]!',
          )}
          aria-label={`Skip: ${copy.label}`}
          data-testid={`setup-skip-${row.id}`}
          onClick={() => void useFirstRunStore.getState().skip(row.id)}
        >
          Skip
        </button>
      )}
    </li>
  )
}

/** "You're set up" for a moment after the person finishes with the list
 *  open (the daemon's finish lands while it's shown), then nothing. */
function useSetUpMoment(rows: FirstRunRow[] | null, finished: boolean): boolean {
  const [moment, setMoment] = useState(false)
  const wasShown = useRef(false)
  useEffect(() => {
    if (rows) {
      wasShown.current = true
      return
    }
    if (finished && wasShown.current) setMoment(true)
    wasShown.current = false
  }, [rows, finished])
  useEffect(() => {
    if (!moment) return
    const t = setTimeout(() => setMoment(false), SET_UP_MS)
    return () => clearTimeout(t)
  }, [moment])
  return moment && !rows
}

/** The checklist, for a sidebar. Renders nothing when there's nothing to do. */
export function SetupChecklist({ className }: { className?: string }): React.JSX.Element | null {
  const { rows, error, finished, collapsed } = useSetupChecklistRows()
  const setUpMoment = useSetUpMoment(rows, finished)
  if (setUpMoment) {
    return (
      <section
        data-setup-checklist=""
        data-setup-finished=""
        role="status"
        className={cx('flex items-center gap-2.5 px-3 py-2 border-t border-[var(--color-border)] flex-shrink-0', className)}
      >
        <StepMark state="done" />
        <span className="type-row text-[var(--color-text-secondary)]">{SET_UP_TEXT}</span>
      </section>
    )
  }
  if (collapsed) return <GetStartedBubble error={error} className={className} />
  if (!rows) return null
  const left = rowsLeft(rows)
  const current = currentRowId(rows)
  const progress = rows.length === 0 ? 0 : (rows.length - left) / rows.length
  return (
    <section
      data-setup-checklist=""
      aria-labelledby="setup-checklist-title"
      className={cx('flex flex-col min-h-0 flex-shrink max-h-[45%] border-t border-[var(--color-border)]', className)}
    >
      <div className="flex items-center gap-1.5 pl-3 pr-2 pt-2 pb-1 flex-shrink-0">
        <h2 id="setup-checklist-title" className="m-0 flex flex-shrink-0 leading-none whitespace-nowrap">
          <NavSectionLabel>{CHECKLIST_TITLE}</NavSectionLabel>
        </h2>
        <span className="min-w-0 truncate text-[10px] tabular-nums text-[var(--color-text-muted)]" data-testid="setup-left">
          {left === 0 ? ALL_DONE_TEXT : leftText(left)}
        </span>
        <span className="flex-1" />
        <button
          type="button"
          data-k2-focus=""
          className={QUIET_ACTION_CLASS}
          aria-label="Hide the setup checklist"
          data-testid="setup-hide"
          onClick={() => void useFirstRunStore.getState().hide()}
        >
          Hide
        </button>
      </div>
      <div aria-hidden="true" className="mx-3 mb-1 h-0.5 flex-shrink-0 bg-[var(--color-border)] rounded-[var(--radius-pill)] overflow-hidden">
        <div
          data-setup-progress=""
          className="h-full bg-[var(--color-accent)] rounded-[var(--radius-pill)] transition-[width] duration-300"
          style={{ width: `${Math.round(progress * 100)}%` }}
        />
      </div>
      {error && (
        <div role="alert" className="flex items-center gap-2 pl-3 pr-2 py-0.5 flex-shrink-0" data-testid="setup-error">
          <span aria-hidden="true" className="w-1.5 h-1.5 flex-shrink-0 rounded-[var(--radius-status)] bg-[var(--color-status-warn)]" />
          <span className="flex-1 min-w-0 type-hint text-[var(--color-text-secondary)]" title={error}>
            {CHECKLIST_ERROR_TEXT}
          </span>
          <button
            type="button"
            data-k2-focus=""
            className={QUIET_ACTION_CLASS}
            data-testid="setup-retry"
            onClick={() => void useFirstRunStore.getState().load()}
          >
            Try again
          </button>
        </div>
      )}
      <ul className="m-0 p-0 pb-1 list-none min-h-0 overflow-y-auto">
        {rows.map((row) => (
          <ChecklistRow key={row.id} row={row} current={row.id === current} />
        ))}
      </ul>
    </section>
  )
}

/** The hidden list, as a small bubble where the list sits. Clicking it
 *  brings the list back (`POST first-run/show`); a failed show keeps the
 *  bubble and says so quietly.
 *
 *  Look (K2 Design): it reads as the theme's colour. The shared accent
 *  Button (the accent fill, the on-accent label and icon, the primary's
 *  hover and focus ring), in the theme's control shape: cut in Terminal,
 *  the field radius (a pill) in Glass. Compact, at the sidebar's content
 *  edge in the list's slot, so it sits in the nav's flow, not over it. */
function GetStartedBubble({ error, className }: { error: string | null; className?: string }): React.JSX.Element {
  return (
    <section
      data-setup-checklist-collapsed=""
      className={cx('flex flex-col gap-1 px-3 py-2 border-t border-[var(--color-border)] flex-shrink-0', className)}
    >
      <Button
        variant="accent"
        data-testid="setup-get-started"
        aria-label={GET_STARTED_LABEL}
        className="self-start flex-shrink-0 inline-flex items-center gap-1.5 font-medium"
        onClick={() => void useFirstRunStore.getState().show()}
      >
        <span aria-hidden="true" className="flex">
          <ChecklistIcon size={12} />
        </span>
        {GET_STARTED_TEXT}
      </Button>
      {error && (
        <p role="alert" className="m-0 type-hint text-[var(--color-text-secondary)]" title={error} data-testid="setup-error">
          {CHECKLIST_ERROR_TEXT}
        </p>
      )}
    </section>
  )
}

/** The collapsed rail's counterpart: the checklist icon with a badge of the
 *  rows left; opens the nav, where the checklist is. Nothing when none are
 *  left. */
export function SetupChecklistRailButton(): React.JSX.Element | null {
  const { rows } = useSetupChecklistRows()
  if (!rows) return null
  const left = rowsLeft(rows)
  if (left === 0) return null
  const label = `${CHECKLIST_TITLE}: ${leftText(left)}`
  return (
    <button
      type="button"
      data-material="frost"
      data-frost-shape="round"
      data-nav-control=""
      data-k2-focus=""
      data-testid="setup-rail"
      className="relative no-drag flex items-center justify-center w-8 h-8 flex-shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)] transition-colors"
      title={label}
      aria-label={label}
      onClick={() => useSidebarStore.getState().expand()}
    >
      <ChecklistIcon />
      <span
        aria-hidden="true"
        data-testid="setup-rail-count"
        className="absolute -top-0.5 -right-0.5 min-w-[14px] h-[14px] px-0.5 flex items-center justify-center text-[8px] font-bold tabular-nums text-[var(--color-on-accent)] bg-[var(--color-accent)] rounded-[var(--radius-badge)]"
      >
        {left}
      </span>
    </button>
  )
}
